//! Register-based lowering skeleton.
//!
//! Phase 2 starts by turning validated stack-machine functions into an
//! inspectable register-oriented IR. This module is deliberately small: it
//! establishes the execution-side vocabulary and lowers straight-line functions
//! before broader control-flow/runtime semantics are added.

use alloc::{string::String, vec::Vec};

use crate::binary::instr::{DecodedInstr, Instr, decode_instr_sequence_with_offsets};
use crate::binary::module::Module;
use crate::error::{ByteOffset, DecodeContext, DecodeErrorKind};
use crate::types::{
    BlockType, CodeBody, DataMode, ElemIdx, ElementInit, ElementMode, ExportDesc, FuncIdx,
    FuncType, GlobalIdx, ImportDesc, LabelIdx, LocalDecl, LocalIdx, MemArg, MemIdx, MemType,
    Mutability, NumType, TableIdx, TableType, TypeIdx, ValType,
};
use crate::validate;
use crate::validate::error::{ValidationError, ValidationErrorKind};

/// A virtual register in lowered Baedeker IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reg(pub u32);

/// A typed value currently on the lowering stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegValue {
    pub reg: Reg,
    pub ty: ValType,
}

/// A validated module lowered into register IR.
#[derive(Debug, Clone, PartialEq)]
pub struct RegModule {
    pub funcs: Vec<RegFunc>,
    pub exports: Vec<RegExport>,
    /// Number of imported functions: `FuncIdx` values below this are not
    /// lowered and cannot be called by the interpreter yet.
    pub imported_func_count: u32,
    /// Defined memories (instantiated as zeroed linear memory).
    pub memories: Vec<MemType>,
    /// Defined globals, initialized in declaration order at instantiation.
    pub globals: Vec<RegGlobal>,
    /// Defined tables (instantiated as null-filled reference arrays).
    pub tables: Vec<TableType>,
    /// Element segments in index order; mode decides instantiation behavior.
    pub elements: Vec<RegElement>,
    /// All function types in the module's type section, for structural
    /// `call_indirect` type checks.
    pub types: Vec<FuncType>,
    /// Active data segments applied to memory at instantiation.
    pub data: Vec<RegDataSegment>,
    /// Number of imported memories (runtime access is not yet supported).
    pub imported_memory_count: u32,
    /// Number of imported globals (runtime access is not yet supported).
    pub imported_global_count: u32,
    /// Number of imported tables (runtime access is not yet supported).
    pub imported_table_count: u32,
}

/// An element segment in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegElement {
    pub mode: RegElementMode,
    /// Element values (funcref indices or nulls), in order.
    pub values: Vec<RegElemValue>,
}

/// Instantiation behavior of an element segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegElementMode {
    /// Written into the table at instantiation.
    Active {
        table: TableIdx,
        offset: Vec<RegConstInstr>,
    },
    /// Retained for `table.init` until dropped.
    Passive,
    /// Declarative: only declares functions for `ref.func`; never usable
    /// at runtime.
    Dropped,
}

/// One element value in a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegElemValue {
    FuncRef(FuncIdx),
    Null,
}

/// A defined global in lowered register IR.
#[derive(Debug, Clone, PartialEq)]
pub struct RegGlobal {
    pub ty: ValType,
    pub mutable: bool,
    /// Const initializer, evaluated at instantiation.
    pub init: Vec<RegConstInstr>,
}

/// An active data segment in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegDataSegment {
    pub memory: MemIdx,
    /// Const offset expression, evaluated at instantiation.
    pub offset: Vec<RegConstInstr>,
    pub bytes: Vec<u8>,
}

/// An instruction in a lowered constant expression (global initializers,
/// data segment offsets). Supports the const instrs plus the
/// extended-const integer arithmetic the validator accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegConstInstr {
    I32Const(i32),
    I64Const(i64),
    F32Const(u32),
    F64Const(u64),
    GlobalGet(GlobalIdx),
    I32Add,
    I32Sub,
    I32Mul,
    I64Add,
    I64Sub,
    I64Mul,
}

/// A function export in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegExport {
    pub name: String,
    pub func: FuncIdx,
}

/// A defined function lowered into register IR.
#[derive(Debug, Clone, PartialEq)]
pub struct RegFunc {
    pub idx: FuncIdx,
    pub type_idx: TypeIdx,
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
    /// All locals in function-index order: parameters first, then code-section locals.
    pub locals: Vec<ValType>,
    /// Type of each virtual register allocated while lowering this function.
    pub reg_types: Vec<ValType>,
    /// Basic blocks in execution order.
    pub blocks: Vec<RegBlock>,
}

/// A basic block in the lowered register IR.
#[derive(Debug, Clone, PartialEq)]
pub struct RegBlock {
    pub label: LabelIdx,
    pub instrs: Vec<RegInstr>,
    pub term: RegTerm,
}

/// A block terminator — how execution leaves this block.
#[derive(Debug, Clone, PartialEq)]
pub enum RegTerm {
    /// Fall through to the next block in sequence.
    Fallthrough,
    /// Branch to a target block, passing values.
    Br { target_block: u32, values: Vec<Reg> },
    /// Conditional branch: if cond is non-zero, branch; otherwise fall through.
    BrIf {
        cond: Reg,
        target_block: u32,
        values: Vec<Reg>,
    },
    /// Two-way fork: if cond is non-zero, go to then_block; else to else_block.
    IfFork {
        cond: Reg,
        then_block: u32,
        else_block: u32,
    },
    /// Multi-target dispatch (`br_table`): branch to `targets[i]` when the
    /// index value equals i, or to `default` when out of range.
    BrTable {
        index: Reg,
        targets: Vec<u32>,
        default: u32,
        values: Vec<Reg>,
    },
    /// Return from the function with values.
    Return { values: Vec<Reg> },
    /// Unconditional trap (`unreachable`).
    Trap,
}

impl RegBlock {
    pub fn new(label: LabelIdx, instrs: Vec<RegInstr>, term: RegTerm) -> Self {
        Self {
            label,
            instrs,
            term,
        }
    }
}

/// A lowered instruction with the source byte offset it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct RegInstr {
    pub offset: ByteOffset,
    pub op: RegOp,
}

/// Register-oriented operations.
#[derive(Debug, Clone, PartialEq)]
pub enum RegOp {
    LocalGet {
        dst: Reg,
        local: LocalIdx,
    },
    LocalSet {
        local: LocalIdx,
        value: Reg,
    },
    LocalTee {
        local: LocalIdx,
        value: Reg,
    },
    Drop {
        value: Reg,
    },
    I32Const {
        dst: Reg,
        value: i32,
    },
    I64Const {
        dst: Reg,
        value: i64,
    },
    F32Const {
        dst: Reg,
        value: f32,
    },
    F64Const {
        dst: Reg,
        value: f64,
    },
    Unary {
        op: UnaryOp,
        dst: Reg,
        value: Reg,
    },
    Binary {
        op: BinaryOp,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    /// Copy a register — used to deliver branch-carried values into the
    /// registers a continuation block expects (phi lowering via copies in
    /// predecessor blocks).
    Copy {
        dst: Reg,
        src: Reg,
    },
    /// Direct call (`call`): invoke `func` with `args`, writing each result
    /// register.
    Call {
        func: FuncIdx,
        args: Vec<Reg>,
        results: Vec<Reg>,
    },
    /// Conditional selection (`select`): dst = cond != 0 ? v1 : v2.
    Select {
        dst: Reg,
        v1: Reg,
        v2: Reg,
        cond: Reg,
    },
    /// Linear-memory load: dst = mem[effective(addr)..+width] per `op`.
    Load {
        op: LoadOp,
        dst: Reg,
        addr: Reg,
        memarg: MemArg,
    },
    /// Linear-memory store: mem[effective(addr)..+width] = value per `op`.
    Store {
        op: StoreOp,
        addr: Reg,
        value: Reg,
        memarg: MemArg,
    },
    /// Read a global.
    GlobalGet {
        dst: Reg,
        global: GlobalIdx,
    },
    /// Write a global.
    GlobalSet {
        global: GlobalIdx,
        value: Reg,
    },
    /// Current memory size in pages (`memory.size`).
    MemorySize {
        dst: Reg,
        memory: MemIdx,
    },
    /// Grow memory by `delta` pages (`memory.grow`): dst = previous size,
    /// or -1 on failure.
    MemoryGrow {
        dst: Reg,
        memory: MemIdx,
        delta: Reg,
    },
    /// Indirect call through a table (`call_indirect`).
    CallIndirect {
        type_idx: TypeIdx,
        table: TableIdx,
        index: Reg,
        args: Vec<Reg>,
        results: Vec<Reg>,
    },
    /// `table.get`: dst = table[index].
    TableGet {
        dst: Reg,
        table: TableIdx,
        index: Reg,
    },
    /// `table.set`: table[index] = value.
    TableSet {
        table: TableIdx,
        index: Reg,
        value: Reg,
    },
    /// `table.size`.
    TableSize {
        dst: Reg,
        table: TableIdx,
    },
    /// `table.grow`: grow by delta, filling with `value`; dst = previous
    /// size or -1 on failure.
    TableGrow {
        dst: Reg,
        table: TableIdx,
        value: Reg,
        delta: Reg,
    },
    /// `table.fill`: table[dst..dst+count] = value.
    TableFill {
        table: TableIdx,
        dst: Reg,
        value: Reg,
        count: Reg,
    },
    /// `table.copy`: dst_table[dst..] = src_table[src..] over count.
    TableCopy {
        dst_table: TableIdx,
        src_table: TableIdx,
        dst: Reg,
        src: Reg,
        count: Reg,
    },
    /// `table.init`: table[dst..] = elem[src..] over count.
    TableInit {
        table: TableIdx,
        elem: ElemIdx,
        dst: Reg,
        src: Reg,
        count: Reg,
    },
    /// `elem.drop`: drop the element segment's runtime storage.
    ElemDrop {
        elem: ElemIdx,
    },
    /// `ref.null`: produce a null reference.
    RefNull {
        dst: Reg,
    },
    /// `ref.func`: produce a function reference.
    RefFunc {
        dst: Reg,
        func: FuncIdx,
    },
    /// `ref.is_null`: dst = 1 when the reference is null, else 0.
    RefIsNull {
        dst: Reg,
        value: Reg,
    },
}

/// Linear-memory load operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOp {
    I32,
    I64,
    F32,
    F64,
    I32Load8S,
    I32Load8U,
    I32Load16S,
    I32Load16U,
    I64Load8S,
    I64Load8U,
    I64Load16S,
    I64Load16U,
    I64Load32S,
    I64Load32U,
}

impl LoadOp {
    /// Bytes read from memory.
    pub fn byte_width(self) -> usize {
        match self {
            LoadOp::I32 | LoadOp::F32 | LoadOp::I64Load32S | LoadOp::I64Load32U => 4,
            LoadOp::I64 | LoadOp::F64 => 8,
            LoadOp::I32Load8S | LoadOp::I32Load8U | LoadOp::I64Load8S | LoadOp::I64Load8U => 1,
            LoadOp::I32Load16S | LoadOp::I32Load16U | LoadOp::I64Load16S | LoadOp::I64Load16U => 2,
        }
    }

    /// Value type produced by the load.
    pub fn result_type(self) -> ValType {
        match self {
            LoadOp::I32
            | LoadOp::I32Load8S
            | LoadOp::I32Load8U
            | LoadOp::I32Load16S
            | LoadOp::I32Load16U => ValType::Num(NumType::I32),
            LoadOp::I64
            | LoadOp::I64Load8S
            | LoadOp::I64Load8U
            | LoadOp::I64Load16S
            | LoadOp::I64Load16U
            | LoadOp::I64Load32S
            | LoadOp::I64Load32U => ValType::Num(NumType::I64),
            LoadOp::F32 => ValType::Num(NumType::F32),
            LoadOp::F64 => ValType::Num(NumType::F64),
        }
    }

    /// Whether the loaded value is sign-extended to the result type.
    pub fn sign_extend(self) -> bool {
        matches!(
            self,
            LoadOp::I32Load8S
                | LoadOp::I32Load16S
                | LoadOp::I64Load8S
                | LoadOp::I64Load16S
                | LoadOp::I64Load32S
        )
    }
}

/// Linear-memory store operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    I32,
    I64,
    F32,
    F64,
    I32Store8,
    I32Store16,
    I64Store8,
    I64Store16,
    I64Store32,
}

impl StoreOp {
    /// Bytes written to memory.
    pub fn byte_width(self) -> usize {
        match self {
            StoreOp::I32 | StoreOp::F32 | StoreOp::I64Store32 => 4,
            StoreOp::I64 | StoreOp::F64 => 8,
            StoreOp::I32Store8 | StoreOp::I64Store8 => 1,
            StoreOp::I32Store16 | StoreOp::I64Store16 => 2,
        }
    }

    /// Value type consumed by the store.
    pub fn value_type(self) -> ValType {
        match self {
            StoreOp::I32 | StoreOp::I32Store8 | StoreOp::I32Store16 => ValType::Num(NumType::I32),
            StoreOp::I64 | StoreOp::I64Store8 | StoreOp::I64Store16 | StoreOp::I64Store32 => {
                ValType::Num(NumType::I64)
            }
            StoreOp::F32 => ValType::Num(NumType::F32),
            StoreOp::F64 => ValType::Num(NumType::F64),
        }
    }
}

/// Unary numeric operation lowered into register IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Eqz,
    I32WrapI64,
    I32Extend8S,
    I32Extend16S,
    I32TruncF32S,
    I32TruncF32U,
    I32TruncF64S,
    I32TruncF64U,
    F32ConvertI32S,
    F32ConvertI32U,
    F64ConvertI32S,
    F64ConvertI32U,
    F32Neg,
    F32Abs,
    F32Sqrt,
    F32Ceil,
    F32Floor,
    F32Trunc,
    F32Nearest,
    I64Clz,
    I64Ctz,
    I64Popcnt,
    I64Eqz,
    I64ExtendI32S,
    I64ExtendI32U,
    I64Extend8S,
    I64Extend16S,
    I64Extend32S,
    I64TruncF32S,
    I64TruncF32U,
    I64TruncF64S,
    I64TruncF64U,
    F32ConvertI64S,
    F32ConvertI64U,
    F64ConvertI64S,
    F64ConvertI64U,
    F64Neg,
    F64Abs,
    F64Sqrt,
    F64Ceil,
    F64Floor,
    F64Trunc,
    F64Nearest,
    F32DemoteF64,
    F64PromoteF32,
    I32ReinterpretF32,
    F32ReinterpretI32,
    I64ReinterpretF64,
    F64ReinterpretI64,
    I32TruncSatF32S,
    I32TruncSatF32U,
    I32TruncSatF64S,
    I32TruncSatF64U,
    I64TruncSatF32S,
    I64TruncSatF32U,
    I64TruncSatF64S,
    I64TruncSatF64U,
}

/// Binary numeric operation lowered into register IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrS,
    I64ShrU,
    I64Rotl,
    I64Rotr,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LtU,
    I64GtS,
    I64GtU,
    I64LeS,
    I64LeU,
    I64GeS,
    I64GeU,
    F32Add,
    F32Sub,
    F32Mul,
    F32Div,
    F32Min,
    F32Max,
    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    F64Min,
    F64Max,
    F32Eq,
    F32Ne,
    F32Lt,
    F32Gt,
    F32Le,
    F32Ge,
    F64Eq,
    F64Ne,
    F64Lt,
    F64Gt,
    F64Le,
    F64Ge,
}

impl UnaryOp {
    fn name(self) -> &'static str {
        match self {
            UnaryOp::I32Clz => "i32.clz",
            UnaryOp::I32Ctz => "i32.ctz",
            UnaryOp::I32Popcnt => "i32.popcnt",
            UnaryOp::I32Eqz => "i32.eqz",
            UnaryOp::I32WrapI64 => "i32.wrap_i64",
            UnaryOp::I32Extend8S => "i32.extend8_s",
            UnaryOp::I32Extend16S => "i32.extend16_s",
            UnaryOp::I32TruncF32S => "i32.trunc_f32_s",
            UnaryOp::I32TruncF32U => "i32.trunc_f32_u",
            UnaryOp::I32TruncF64S => "i32.trunc_f64_s",
            UnaryOp::I32TruncF64U => "i32.trunc_f64_u",
            UnaryOp::F32ConvertI32S => "f32.convert_i32_s",
            UnaryOp::F32ConvertI32U => "f32.convert_i32_u",
            UnaryOp::F64ConvertI32S => "f64.convert_i32_s",
            UnaryOp::F64ConvertI32U => "f64.convert_i32_u",
            UnaryOp::F32Neg => "f32.neg",
            UnaryOp::F32Abs => "f32.abs",
            UnaryOp::F32Sqrt => "f32.sqrt",
            UnaryOp::F32Ceil => "f32.ceil",
            UnaryOp::F32Floor => "f32.floor",
            UnaryOp::F32Trunc => "f32.trunc",
            UnaryOp::F32Nearest => "f32.nearest",
            UnaryOp::I64Clz => "i64.clz",
            UnaryOp::I64Ctz => "i64.ctz",
            UnaryOp::I64Popcnt => "i64.popcnt",
            UnaryOp::I64Eqz => "i64.eqz",
            UnaryOp::I64ExtendI32S => "i64.extend_i32_s",
            UnaryOp::I64ExtendI32U => "i64.extend_i32_u",
            UnaryOp::I64Extend8S => "i64.extend8_s",
            UnaryOp::I64Extend16S => "i64.extend16_s",
            UnaryOp::I64Extend32S => "i64.extend32_s",
            UnaryOp::I64TruncF32S => "i64.trunc_f32_s",
            UnaryOp::I64TruncF32U => "i64.trunc_f32_u",
            UnaryOp::I64TruncF64S => "i64.trunc_f64_s",
            UnaryOp::I64TruncF64U => "i64.trunc_f64_u",
            UnaryOp::F32ConvertI64S => "f32.convert_i64_s",
            UnaryOp::F32ConvertI64U => "f32.convert_i64_u",
            UnaryOp::F64ConvertI64S => "f64.convert_i64_s",
            UnaryOp::F64ConvertI64U => "f64.convert_i64_u",
            UnaryOp::F64Neg => "f64.neg",
            UnaryOp::F64Abs => "f64.abs",
            UnaryOp::F64Sqrt => "f64.sqrt",
            UnaryOp::F64Ceil => "f64.ceil",
            UnaryOp::F64Floor => "f64.floor",
            UnaryOp::F64Trunc => "f64.trunc",
            UnaryOp::F64Nearest => "f64.nearest",
            UnaryOp::F32DemoteF64 => "f32.demote_f64",
            UnaryOp::F64PromoteF32 => "f64.promote_f32",
            UnaryOp::I32ReinterpretF32 => "i32.reinterpret_f32",
            UnaryOp::F32ReinterpretI32 => "f32.reinterpret_i32",
            UnaryOp::I64ReinterpretF64 => "i64.reinterpret_f64",
            UnaryOp::F64ReinterpretI64 => "f64.reinterpret_i64",
            UnaryOp::I32TruncSatF32S => "i32.trunc_sat_f32_s",
            UnaryOp::I32TruncSatF32U => "i32.trunc_sat_f32_u",
            UnaryOp::I32TruncSatF64S => "i32.trunc_sat_f64_s",
            UnaryOp::I32TruncSatF64U => "i32.trunc_sat_f64_u",
            UnaryOp::I64TruncSatF32S => "i64.trunc_sat_f32_s",
            UnaryOp::I64TruncSatF32U => "i64.trunc_sat_f32_u",
            UnaryOp::I64TruncSatF64S => "i64.trunc_sat_f64_s",
            UnaryOp::I64TruncSatF64U => "i64.trunc_sat_f64_u",
        }
    }

    fn input_type(self) -> ValType {
        match self {
            UnaryOp::I32Clz
            | UnaryOp::I32Ctz
            | UnaryOp::I32Popcnt
            | UnaryOp::I32Eqz
            | UnaryOp::I32Extend8S
            | UnaryOp::I32Extend16S
            | UnaryOp::I64ExtendI32S
            | UnaryOp::I64ExtendI32U
            | UnaryOp::F32ConvertI32S
            | UnaryOp::F32ConvertI32U
            | UnaryOp::F64ConvertI32S
            | UnaryOp::F64ConvertI32U
            | UnaryOp::F32ReinterpretI32 => ValType::Num(NumType::I32),
            UnaryOp::I64Clz
            | UnaryOp::I64Ctz
            | UnaryOp::I64Popcnt
            | UnaryOp::I64Eqz
            | UnaryOp::I32WrapI64
            | UnaryOp::I64Extend8S
            | UnaryOp::I64Extend16S
            | UnaryOp::I64Extend32S
            | UnaryOp::F32ConvertI64S
            | UnaryOp::F32ConvertI64U
            | UnaryOp::F64ConvertI64S
            | UnaryOp::F64ConvertI64U
            | UnaryOp::F64ReinterpretI64 => ValType::Num(NumType::I64),
            UnaryOp::F32Neg
            | UnaryOp::F32Abs
            | UnaryOp::F32Sqrt
            | UnaryOp::F32Ceil
            | UnaryOp::F32Floor
            | UnaryOp::F32Trunc
            | UnaryOp::F32Nearest
            | UnaryOp::I32TruncF32S
            | UnaryOp::I32TruncF32U
            | UnaryOp::I64TruncF32S
            | UnaryOp::I64TruncF32U
            | UnaryOp::F64PromoteF32
            | UnaryOp::I32ReinterpretF32
            | UnaryOp::I32TruncSatF32S
            | UnaryOp::I32TruncSatF32U
            | UnaryOp::I64TruncSatF32S
            | UnaryOp::I64TruncSatF32U => ValType::Num(NumType::F32),
            UnaryOp::F64Neg
            | UnaryOp::F64Abs
            | UnaryOp::F64Sqrt
            | UnaryOp::F64Ceil
            | UnaryOp::F64Floor
            | UnaryOp::F64Trunc
            | UnaryOp::F64Nearest
            | UnaryOp::I32TruncF64S
            | UnaryOp::I32TruncF64U
            | UnaryOp::I64TruncF64S
            | UnaryOp::I64TruncF64U
            | UnaryOp::F32DemoteF64
            | UnaryOp::I64ReinterpretF64
            | UnaryOp::I32TruncSatF64S
            | UnaryOp::I32TruncSatF64U
            | UnaryOp::I64TruncSatF64S
            | UnaryOp::I64TruncSatF64U => ValType::Num(NumType::F64),
        }
    }

    fn result_type(self) -> ValType {
        match self {
            UnaryOp::I32Clz
            | UnaryOp::I32Ctz
            | UnaryOp::I32Popcnt
            | UnaryOp::I32Eqz
            | UnaryOp::I32WrapI64
            | UnaryOp::I32Extend8S
            | UnaryOp::I32Extend16S
            | UnaryOp::I32TruncF32S
            | UnaryOp::I32TruncF32U
            | UnaryOp::I32TruncF64S
            | UnaryOp::I32TruncF64U
            | UnaryOp::I32TruncSatF32S
            | UnaryOp::I32TruncSatF32U
            | UnaryOp::I32TruncSatF64S
            | UnaryOp::I32TruncSatF64U
            | UnaryOp::I32ReinterpretF32 => ValType::Num(NumType::I32),
            UnaryOp::I64Clz
            | UnaryOp::I64Ctz
            | UnaryOp::I64Popcnt
            | UnaryOp::I64ExtendI32S
            | UnaryOp::I64ExtendI32U
            | UnaryOp::I64Extend8S
            | UnaryOp::I64Extend16S
            | UnaryOp::I64Extend32S
            | UnaryOp::I64TruncF32S
            | UnaryOp::I64TruncF32U
            | UnaryOp::I64TruncF64S
            | UnaryOp::I64TruncF64U
            | UnaryOp::I64TruncSatF32S
            | UnaryOp::I64TruncSatF32U
            | UnaryOp::I64TruncSatF64S
            | UnaryOp::I64TruncSatF64U
            | UnaryOp::I64ReinterpretF64 => ValType::Num(NumType::I64),
            UnaryOp::F32Neg
            | UnaryOp::F32Abs
            | UnaryOp::F32Sqrt
            | UnaryOp::F32Ceil
            | UnaryOp::F32Floor
            | UnaryOp::F32Trunc
            | UnaryOp::F32Nearest
            | UnaryOp::F32ConvertI32S
            | UnaryOp::F32ConvertI32U
            | UnaryOp::F32ConvertI64S
            | UnaryOp::F32ConvertI64U
            | UnaryOp::F32DemoteF64
            | UnaryOp::F32ReinterpretI32 => ValType::Num(NumType::F32),
            UnaryOp::F64Neg
            | UnaryOp::F64Abs
            | UnaryOp::F64Sqrt
            | UnaryOp::F64Ceil
            | UnaryOp::F64Floor
            | UnaryOp::F64Trunc
            | UnaryOp::F64Nearest
            | UnaryOp::F64ConvertI32S
            | UnaryOp::F64ConvertI32U
            | UnaryOp::F64ConvertI64S
            | UnaryOp::F64ConvertI64U
            | UnaryOp::F64PromoteF32
            | UnaryOp::F64ReinterpretI64 => ValType::Num(NumType::F64),
            UnaryOp::I64Eqz => ValType::Num(NumType::I32),
        }
    }
}

impl BinaryOp {
    fn name(self) -> &'static str {
        match self {
            BinaryOp::I32Add => "i32.add",
            BinaryOp::I32Sub => "i32.sub",
            BinaryOp::I32Mul => "i32.mul",
            BinaryOp::I32DivS => "i32.div_s",
            BinaryOp::I32DivU => "i32.div_u",
            BinaryOp::I32RemS => "i32.rem_s",
            BinaryOp::I32RemU => "i32.rem_u",
            BinaryOp::I32And => "i32.and",
            BinaryOp::I32Or => "i32.or",
            BinaryOp::I32Xor => "i32.xor",
            BinaryOp::I32Shl => "i32.shl",
            BinaryOp::I32ShrS => "i32.shr_s",
            BinaryOp::I32ShrU => "i32.shr_u",
            BinaryOp::I32Rotl => "i32.rotl",
            BinaryOp::I32Rotr => "i32.rotr",
            BinaryOp::I32Eq => "i32.eq",
            BinaryOp::I32Ne => "i32.ne",
            BinaryOp::I32LtS => "i32.lt_s",
            BinaryOp::I32LtU => "i32.lt_u",
            BinaryOp::I32GtS => "i32.gt_s",
            BinaryOp::I32GtU => "i32.gt_u",
            BinaryOp::I32LeS => "i32.le_s",
            BinaryOp::I32LeU => "i32.le_u",
            BinaryOp::I32GeS => "i32.ge_s",
            BinaryOp::I32GeU => "i32.ge_u",
            BinaryOp::I64Add => "i64.add",
            BinaryOp::I64Sub => "i64.sub",
            BinaryOp::I64Mul => "i64.mul",
            BinaryOp::I64DivS => "i64.div_s",
            BinaryOp::I64DivU => "i64.div_u",
            BinaryOp::I64RemS => "i64.rem_s",
            BinaryOp::I64RemU => "i64.rem_u",
            BinaryOp::I64And => "i64.and",
            BinaryOp::I64Or => "i64.or",
            BinaryOp::I64Xor => "i64.xor",
            BinaryOp::I64Shl => "i64.shl",
            BinaryOp::I64ShrS => "i64.shr_s",
            BinaryOp::I64ShrU => "i64.shr_u",
            BinaryOp::I64Rotl => "i64.rotl",
            BinaryOp::I64Rotr => "i64.rotr",
            BinaryOp::I64Eq => "i64.eq",
            BinaryOp::I64Ne => "i64.ne",
            BinaryOp::I64LtS => "i64.lt_s",
            BinaryOp::I64LtU => "i64.lt_u",
            BinaryOp::I64GtS => "i64.gt_s",
            BinaryOp::I64GtU => "i64.gt_u",
            BinaryOp::I64LeS => "i64.le_s",
            BinaryOp::I64LeU => "i64.le_u",
            BinaryOp::I64GeS => "i64.ge_s",
            BinaryOp::I64GeU => "i64.ge_u",
            BinaryOp::F32Add => "f32.add",
            BinaryOp::F32Sub => "f32.sub",
            BinaryOp::F32Mul => "f32.mul",
            BinaryOp::F32Div => "f32.div",
            BinaryOp::F32Min => "f32.min",
            BinaryOp::F32Max => "f32.max",
            BinaryOp::F64Add => "f64.add",
            BinaryOp::F64Sub => "f64.sub",
            BinaryOp::F64Mul => "f64.mul",
            BinaryOp::F64Div => "f64.div",
            BinaryOp::F64Min => "f64.min",
            BinaryOp::F64Max => "f64.max",
            BinaryOp::F32Eq => "f32.eq",
            BinaryOp::F32Ne => "f32.ne",
            BinaryOp::F32Lt => "f32.lt",
            BinaryOp::F32Gt => "f32.gt",
            BinaryOp::F32Le => "f32.le",
            BinaryOp::F32Ge => "f32.ge",
            BinaryOp::F64Eq => "f64.eq",
            BinaryOp::F64Ne => "f64.ne",
            BinaryOp::F64Lt => "f64.lt",
            BinaryOp::F64Gt => "f64.gt",
            BinaryOp::F64Le => "f64.le",
            BinaryOp::F64Ge => "f64.ge",
        }
    }

    fn input_type(self) -> ValType {
        match self {
            BinaryOp::I32Add
            | BinaryOp::I32Sub
            | BinaryOp::I32Mul
            | BinaryOp::I32DivS
            | BinaryOp::I32DivU
            | BinaryOp::I32RemS
            | BinaryOp::I32RemU
            | BinaryOp::I32And
            | BinaryOp::I32Or
            | BinaryOp::I32Xor
            | BinaryOp::I32Shl
            | BinaryOp::I32ShrS
            | BinaryOp::I32ShrU
            | BinaryOp::I32Rotl
            | BinaryOp::I32Rotr
            | BinaryOp::I32Eq
            | BinaryOp::I32Ne
            | BinaryOp::I32LtS
            | BinaryOp::I32LtU
            | BinaryOp::I32GtS
            | BinaryOp::I32GtU
            | BinaryOp::I32LeS
            | BinaryOp::I32LeU
            | BinaryOp::I32GeS
            | BinaryOp::I32GeU => ValType::Num(NumType::I32),
            BinaryOp::I64Add
            | BinaryOp::I64Sub
            | BinaryOp::I64Mul
            | BinaryOp::I64DivS
            | BinaryOp::I64DivU
            | BinaryOp::I64RemS
            | BinaryOp::I64RemU
            | BinaryOp::I64And
            | BinaryOp::I64Or
            | BinaryOp::I64Xor
            | BinaryOp::I64Shl
            | BinaryOp::I64ShrS
            | BinaryOp::I64ShrU
            | BinaryOp::I64Rotl
            | BinaryOp::I64Rotr
            | BinaryOp::I64Eq
            | BinaryOp::I64Ne
            | BinaryOp::I64LtS
            | BinaryOp::I64LtU
            | BinaryOp::I64GtS
            | BinaryOp::I64GtU
            | BinaryOp::I64LeS
            | BinaryOp::I64LeU
            | BinaryOp::I64GeS
            | BinaryOp::I64GeU => ValType::Num(NumType::I64),
            BinaryOp::F32Add
            | BinaryOp::F32Sub
            | BinaryOp::F32Mul
            | BinaryOp::F32Div
            | BinaryOp::F32Min
            | BinaryOp::F32Max
            | BinaryOp::F32Eq
            | BinaryOp::F32Ne
            | BinaryOp::F32Lt
            | BinaryOp::F32Gt
            | BinaryOp::F32Le
            | BinaryOp::F32Ge => ValType::Num(NumType::F32),
            BinaryOp::F64Add
            | BinaryOp::F64Sub
            | BinaryOp::F64Mul
            | BinaryOp::F64Div
            | BinaryOp::F64Min
            | BinaryOp::F64Max
            | BinaryOp::F64Eq
            | BinaryOp::F64Ne
            | BinaryOp::F64Lt
            | BinaryOp::F64Gt
            | BinaryOp::F64Le
            | BinaryOp::F64Ge => ValType::Num(NumType::F64),
        }
    }

    fn result_type(self) -> ValType {
        match self {
            BinaryOp::I32Add
            | BinaryOp::I32Sub
            | BinaryOp::I32Mul
            | BinaryOp::I32DivS
            | BinaryOp::I32DivU
            | BinaryOp::I32RemS
            | BinaryOp::I32RemU
            | BinaryOp::I32And
            | BinaryOp::I32Or
            | BinaryOp::I32Xor
            | BinaryOp::I32Shl
            | BinaryOp::I32ShrS
            | BinaryOp::I32ShrU
            | BinaryOp::I32Rotl
            | BinaryOp::I32Rotr => ValType::Num(NumType::I32),
            BinaryOp::I64Add
            | BinaryOp::I64Sub
            | BinaryOp::I64Mul
            | BinaryOp::I64DivS
            | BinaryOp::I64DivU
            | BinaryOp::I64RemS
            | BinaryOp::I64RemU
            | BinaryOp::I64And
            | BinaryOp::I64Or
            | BinaryOp::I64Xor
            | BinaryOp::I64Shl
            | BinaryOp::I64ShrS
            | BinaryOp::I64ShrU
            | BinaryOp::I64Rotl
            | BinaryOp::I64Rotr => ValType::Num(NumType::I64),
            BinaryOp::I32Eq
            | BinaryOp::I32Ne
            | BinaryOp::I32LtS
            | BinaryOp::I32LtU
            | BinaryOp::I32GtS
            | BinaryOp::I32GtU
            | BinaryOp::I32LeS
            | BinaryOp::I32LeU
            | BinaryOp::I32GeS
            | BinaryOp::I32GeU
            | BinaryOp::I64Eq
            | BinaryOp::I64Ne
            | BinaryOp::I64LtS
            | BinaryOp::I64LtU
            | BinaryOp::I64GtS
            | BinaryOp::I64GtU
            | BinaryOp::I64LeS
            | BinaryOp::I64LeU
            | BinaryOp::I64GeS
            | BinaryOp::I64GeU => ValType::Num(NumType::I32),
            BinaryOp::F32Add
            | BinaryOp::F32Sub
            | BinaryOp::F32Mul
            | BinaryOp::F32Div
            | BinaryOp::F32Min
            | BinaryOp::F32Max => ValType::Num(NumType::F32),
            BinaryOp::F64Add
            | BinaryOp::F64Sub
            | BinaryOp::F64Mul
            | BinaryOp::F64Div
            | BinaryOp::F64Min
            | BinaryOp::F64Max => ValType::Num(NumType::F64),
            BinaryOp::F32Eq
            | BinaryOp::F32Ne
            | BinaryOp::F32Lt
            | BinaryOp::F32Gt
            | BinaryOp::F32Le
            | BinaryOp::F32Ge
            | BinaryOp::F64Eq
            | BinaryOp::F64Ne
            | BinaryOp::F64Lt
            | BinaryOp::F64Gt
            | BinaryOp::F64Le
            | BinaryOp::F64Ge => ValType::Num(NumType::I32),
        }
    }
}

/// Lowering error with byte offset and optional function context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerError {
    pub offset: ByteOffset,
    pub function: Option<FuncIdx>,
    pub kind: LowerErrorKind,
}

/// Specific lowering failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerErrorKind {
    Validation(ValidationErrorKind),
    Decode {
        context: DecodeContext,
        kind: DecodeErrorKind,
    },
    UnsupportedInstr {
        op: &'static str,
    },
    StackUnderflow {
        op: &'static str,
        expected: ValType,
    },
    TypeMismatch {
        op: &'static str,
        expected: ValType,
        found: ValType,
    },
    InvalidLabel {
        label: u32,
    },
    InvalidFunction {
        func: u32,
    },
    InvalidGlobal {
        global: u32,
    },
    InvalidTable {
        table: u32,
    },
    InvalidType {
        type_idx: u32,
    },
    UnexpectedElse,
    MissingFunctionEnd,
}

impl From<ValidationError> for LowerError {
    fn from(error: ValidationError) -> Self {
        Self {
            offset: error.offset,
            function: error.function,
            kind: LowerErrorKind::Validation(error.kind),
        }
    }
}

impl<'a> Module<'a> {
    /// Validate and lower this module into Baedeker register IR.
    pub fn lower(&self) -> Result<RegModule, LowerError> {
        lower_module(self)
    }
}

/// Validate and lower a decoded module into register IR.
pub fn lower_module(module: &Module<'_>) -> Result<RegModule, LowerError> {
    validate::validate_module(module)?;

    let imported_func_count = module.imported_function_count() as u32;
    let func_types = func_type_table(module);
    let global_types = global_type_table(module);
    let table_elem_types: Vec<crate::types::RefType> = module
        .imports()
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Table(table) => Some(table.elem),
            _ => None,
        })
        .chain(module.tables().iter().map(|table| table.elem))
        .collect();
    let tables = ModuleTables {
        func_types: &func_types,
        global_types: &global_types,
        types: module.types(),
        table_elem_types: &table_elem_types,
    };
    let mut funcs = Vec::new();

    for (defined_idx, (type_idx, code)) in module.functions().iter().zip(module.codes()).enumerate()
    {
        let func_idx = FuncIdx(imported_func_count + defined_idx as u32);
        let ty = &module.types()[type_idx.0 as usize];
        funcs.push(lower_function(func_idx, *type_idx, ty, code, &tables)?);
    }

    let imported_memory_count = module
        .imports()
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Mem(_)))
        .count() as u32;
    let imported_global_count = module
        .imports()
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Global(_)))
        .count() as u32;

    let memories = module.memories().to_vec();

    let mut globals = Vec::with_capacity(module.globals().len());
    for global in module.globals() {
        globals.push(RegGlobal {
            ty: global.global_type.val_type,
            mutable: global.global_type.mutability == Mutability::Var,
            init: lower_const_expr(global.init_expr, global.init_offset)?,
        });
    }

    let mut data = Vec::new();
    for segment in module.data() {
        if let DataMode::Active {
            memory,
            offset_expr,
            offset_offset,
        } = &segment.mode
        {
            data.push(RegDataSegment {
                memory: *memory,
                offset: lower_const_expr(offset_expr, *offset_offset)?,
                bytes: segment.init.to_vec(),
            });
        }
    }

    let imported_table_count = module
        .imports()
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Table(_)))
        .count() as u32;

    let tables = module.tables().to_vec();
    let types = module.types().to_vec();

    let mut elements = Vec::with_capacity(module.elements().len());
    for segment in module.elements() {
        let mut values = Vec::new();
        match &segment.init {
            ElementInit::FuncIndices(funcs) => {
                values.extend(funcs.iter().map(|func| RegElemValue::FuncRef(*func)));
            }
            ElementInit::Expressions(exprs) => {
                for expr in exprs {
                    values.push(lower_element_expr(expr)?);
                }
            }
        }
        let mode = match &segment.mode {
            ElementMode::Active {
                table,
                offset_expr,
                offset_offset,
            } => RegElementMode::Active {
                table: *table,
                offset: lower_const_expr(offset_expr, *offset_offset)?,
            },
            ElementMode::Passive => RegElementMode::Passive,
            ElementMode::Declarative => RegElementMode::Dropped,
        };
        elements.push(RegElement { mode, values });
    }

    let exports = module
        .exports()
        .iter()
        .filter_map(|export| match export.desc {
            ExportDesc::Func(func) => Some(RegExport {
                name: export.name.clone(),
                func,
            }),
            ExportDesc::Table(_) | ExportDesc::Mem(_) | ExportDesc::Global(_) => None,
        })
        .collect();

    Ok(RegModule {
        funcs,
        exports,
        imported_func_count,
        memories,
        globals,
        tables,
        elements,
        types,
        data,
        imported_memory_count,
        imported_global_count,
        imported_table_count,
    })
}

/// Lower one element-segment initializer expression to a funcref value.
/// Only `ref.func`/`ref.null` are supported at runtime (other const
/// expressions require host-provided imports).
fn lower_element_expr(expr: &crate::types::ElementExpr<'_>) -> Result<RegElemValue, LowerError> {
    let instrs =
        decode_instr_sequence_with_offsets(expr.expr, expr.offset).map_err(|error| LowerError {
            offset: error.offset,
            function: None,
            kind: LowerErrorKind::Decode {
                context: error.context,
                kind: error.kind,
            },
        })?;
    match instrs.as_slice() {
        [first, last] if matches!(last.instr, Instr::End) => match first.instr {
            Instr::RefFunc(func) => Ok(RegElemValue::FuncRef(func)),
            Instr::RefNull(_) => Ok(RegElemValue::Null),
            ref other => Err(LowerError {
                offset: first.offset,
                function: None,
                kind: LowerErrorKind::UnsupportedInstr {
                    op: instr_name(other),
                },
            }),
        },
        _ => Err(LowerError {
            offset: ByteOffset(expr.offset),
            function: None,
            kind: LowerErrorKind::UnsupportedInstr {
                op: "multi-instruction element expression",
            },
        }),
    }
}

/// Lower a constant expression (global initializer, data segment offset)
/// into owned form. Supports the const instructions plus the
/// extended-const integer arithmetic the validator accepts.
fn lower_const_expr(expr: &[u8], offset: usize) -> Result<Vec<RegConstInstr>, LowerError> {
    let instrs = decode_instr_sequence_with_offsets(expr, offset).map_err(|error| LowerError {
        offset: error.offset,
        function: None,
        kind: LowerErrorKind::Decode {
            context: error.context,
            kind: error.kind,
        },
    })?;

    let mut lowered = Vec::with_capacity(instrs.len());
    for decoded in instrs {
        let const_instr = match decoded.instr {
            Instr::I32Const(value) => RegConstInstr::I32Const(value),
            Instr::I64Const(value) => RegConstInstr::I64Const(value),
            Instr::F32Const(value) => RegConstInstr::F32Const(value.to_bits()),
            Instr::F64Const(value) => RegConstInstr::F64Const(value.to_bits()),
            Instr::GlobalGet(global) => RegConstInstr::GlobalGet(global),
            Instr::I32Add => RegConstInstr::I32Add,
            Instr::I32Sub => RegConstInstr::I32Sub,
            Instr::I32Mul => RegConstInstr::I32Mul,
            Instr::I64Add => RegConstInstr::I64Add,
            Instr::I64Sub => RegConstInstr::I64Sub,
            Instr::I64Mul => RegConstInstr::I64Mul,
            Instr::End => break,
            ref other => {
                return Err(LowerError {
                    offset: decoded.offset,
                    function: None,
                    kind: LowerErrorKind::UnsupportedInstr {
                        op: instr_name(other),
                    },
                });
            }
        };
        lowered.push(const_instr);
    }
    Ok(lowered)
}

/// Resolve the `FuncType` for every function in the index space (imported
/// first, then defined), so `call` lowering can type its operands.
fn func_type_table<'a, 'm>(module: &'a Module<'m>) -> Vec<&'a FuncType> {
    let mut table = Vec::new();
    for import in module.imports() {
        if let ImportDesc::Func(type_idx) = import.desc {
            table.push(&module.types()[type_idx.0 as usize]);
        }
    }
    for type_idx in module.functions() {
        table.push(&module.types()[type_idx.0 as usize]);
    }
    table
}

/// Resolve the value type of every global in the index space (imported
/// first, then defined), so `global.get`/`global.set` lowering can type
/// its operands.
fn global_type_table(module: &Module<'_>) -> Vec<ValType> {
    module
        .imports()
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Global(global) => Some(global.val_type),
            _ => None,
        })
        .chain(
            module
                .globals()
                .iter()
                .map(|global| global.global_type.val_type),
        )
        .collect()
}

fn lower_function(
    func_idx: FuncIdx,
    type_idx: TypeIdx,
    ty: &FuncType,
    code: &CodeBody<'_>,
    tables: &ModuleTables<'_>,
) -> Result<RegFunc, LowerError> {
    let instrs = code
        .instructions_with_offsets()
        .map_err(|error| LowerError {
            offset: error.offset,
            function: Some(func_idx),
            kind: LowerErrorKind::Decode {
                context: error.context,
                kind: error.kind,
            },
        })?;

    let locals = local_types(ty, code.locals.as_slice());
    let mut builder = FuncBuilder::new(func_idx, type_idx, ty, locals, tables);

    for decoded in instrs {
        if builder.lower_instr(decoded)? {
            return Ok(builder.finish());
        }
    }

    Err(LowerError {
        offset: ByteOffset(code.body_offset + code.body.len()),
        function: Some(func_idx),
        kind: LowerErrorKind::MissingFunctionEnd,
    })
}

fn local_types(ty: &FuncType, locals: &[LocalDecl]) -> Vec<ValType> {
    let local_count = locals
        .iter()
        .map(|local| local.count as usize)
        .sum::<usize>();
    let mut types = Vec::with_capacity(ty.params.len() + local_count);
    types.extend_from_slice(ty.params.as_slice());
    for local in locals {
        for _ in 0..local.count {
            types.push(local.val_type);
        }
    }
    types
}

struct FuncBuilder<'b> {
    func_idx: FuncIdx,
    type_idx: TypeIdx,
    params: Vec<ValType>,
    results: Vec<ValType>,
    locals: Vec<ValType>,
    stack: Vec<RegValue>,
    reg_types: Vec<ValType>,
    blocks: Vec<RegBlock>,
    current_instrs: Vec<RegInstr>,
    /// Shared module-level tables used while lowering function bodies.
    tables: &'b ModuleTables<'b>,
    /// Stack of active block/loop/if frames. Each entry records the label of
    /// the block that should follow the `end` of this control structure.
    label_stack: Vec<LabelFrame>,
    /// Branches whose target block index needs back-patching.
    pending_branches: Vec<PendingBranch>,
    /// Back-edge trampolines for conditional branches to loops with
    /// parameters, created at `finish`.
    pending_trampolines: Vec<TrampolineReq>,
}

/// A branch whose target block index needs back-patching once the target
/// frame's continuation block is known.
struct PendingBranch {
    /// The block containing the branch terminator.
    block: usize,
    /// Position in `label_stack` of the targeted frame.
    frame_pos: usize,
    /// Branch-carried values (copy sources for the continuation).
    values: Vec<Reg>,
    /// Which terminator slot to patch.
    slot: BranchSlot,
}

/// A loop back-edge target: the header block and the canonical parameter
/// registers branch values must be copied into.
struct LoopTarget {
    header: u32,
    param_regs: Vec<Reg>,
}

/// A request for a back-edge trampoline block: a conditional branch to a
/// loop with parameters cannot copy values into the loop's parameter
/// registers in its own block (the copies would clobber the registers on
/// the not-taken path), so the branch targets a trampoline that runs the
/// copies and then branches unconditionally.
struct TrampolineReq {
    /// The block containing the conditional branch terminator.
    block: usize,
    /// Which terminator slot jumps to the trampoline.
    slot: BranchSlot,
    /// Copy destinations (the loop's parameter registers).
    param_regs: Vec<Reg>,
    /// Copy sources (the branch's values).
    values: Vec<Reg>,
    /// The loop header block the trampoline branches to.
    header: u32,
    /// Byte offset of the originating branch instruction.
    offset: ByteOffset,
}

/// Which target slot of a branch terminator a pending branch patches.
#[derive(Debug, Clone, Copy)]
enum BranchSlot {
    /// The single target of `br` / `br_if` / an if-then exit.
    Single,
    /// The i-th target of `br_table` (`i == targets.len()` patches the
    /// default target).
    Table(usize),
}

struct LabelFrame {
    /// The label that `br` with this index targets.
    #[allow(dead_code)]
    label: LabelIdx,
    /// What kind of control structure this frame belongs to.
    kind: FrameKind,
    /// The result types expected at the `end` of this control structure.
    result_types: Vec<ValType>,
    /// The parameter types consumed at the start of this control structure.
    /// Branches to a `loop` label carry parameters (to the loop header);
    /// branches to any other label carry results (to the continuation).
    param_types: Vec<ValType>,
    /// The canonical registers holding the frame's parameters: the
    /// registers the body reads when it consumes params from the stack.
    /// Loop back-edges must deliver branch values into these registers.
    param_regs: Vec<Reg>,
    /// Operand stack height at frame entry (after consuming parameters).
    height: usize,
    /// Whether the code currently being lowered in this frame is
    /// unreachable (polymorphic stack, per the spec validation algorithm).
    unreachable: bool,
}

/// Module-level index-space tables shared by every function lowering:
/// function types, global types, the type section, and table element
/// types (all imported-first where an index space applies).
struct ModuleTables<'a> {
    func_types: &'a [&'a FuncType],
    global_types: &'a [ValType],
    types: &'a [FuncType],
    table_elem_types: &'a [crate::types::RefType],
}

/// The kind of control structure a label frame describes.
enum FrameKind {
    /// `block` (or the implicit function body frame).
    Block,
    /// `loop` — branches target the loop header block directly.
    Loop { header_block: u32 },
    /// `if` — `cond_block` is the IfFork block whose `else_block` field needs
    /// back-patching; `else_seen` records whether an `else` was lowered.
    If { cond_block: usize, else_seen: bool },
}

impl<'b> FuncBuilder<'b> {
    fn new(
        func_idx: FuncIdx,
        type_idx: TypeIdx,
        ty: &FuncType,
        locals: Vec<ValType>,
        tables: &'b ModuleTables<'b>,
    ) -> Self {
        // The function body itself is label 0, targeting a block that will
        // receive function-end returns (created on demand).
        Self {
            func_idx,
            type_idx,
            params: ty.params.clone(),
            results: ty.results.clone(),
            locals,
            stack: Vec::new(),
            reg_types: Vec::new(),
            blocks: Vec::new(),
            current_instrs: Vec::new(),
            label_stack: alloc::vec![LabelFrame {
                label: LabelIdx(0),
                kind: FrameKind::Block,
                result_types: ty.results.clone(),
                param_types: Vec::new(),
                param_regs: Vec::new(),
                height: 0,
                unreachable: false,
            }],
            pending_branches: Vec::new(),
            pending_trampolines: Vec::new(),
            tables,
        }
    }

    fn finish_block_with_label(&mut self, label: LabelIdx, term: RegTerm) {
        let instrs = core::mem::take(&mut self.current_instrs);
        self.blocks.push(RegBlock::new(label, instrs, term));
    }

    fn finish_block(&mut self, term: RegTerm) {
        let label = LabelIdx(self.blocks.len() as u32);
        self.finish_block_with_label(label, term);
    }

    fn finish(mut self) -> RegFunc {
        // If there are pending instructions without a terminator, add Fallthrough
        if !self.current_instrs.is_empty() || self.blocks.is_empty() {
            self.finish_block(RegTerm::Fallthrough);
        }
        // Create back-edge trampolines for conditional branches to loops
        // with parameters: each runs the parameter copies, then branches
        // unconditionally to the header. Trampolines sit at the end of the
        // block list so no fallthrough can reach them.
        for req in core::mem::take(&mut self.pending_trampolines) {
            let trampoline_idx = self.blocks.len() as u32;
            let instrs = req
                .param_regs
                .iter()
                .zip(req.values.iter())
                .filter(|(dst, src)| dst != src)
                .map(|(dst, src)| RegInstr {
                    offset: req.offset,
                    op: RegOp::Copy {
                        dst: *dst,
                        src: *src,
                    },
                })
                .collect();
            self.blocks.push(RegBlock::new(
                LabelIdx(trampoline_idx),
                instrs,
                RegTerm::Br {
                    target_block: req.header,
                    values: Vec::new(),
                },
            ));
            match (req.slot, &mut self.blocks[req.block].term) {
                (BranchSlot::Single, RegTerm::BrIf { target_block, .. }) => {
                    *target_block = trampoline_idx;
                }
                (
                    BranchSlot::Table(slot),
                    RegTerm::BrTable {
                        targets, default, ..
                    },
                ) => {
                    if slot < targets.len() {
                        targets[slot] = trampoline_idx;
                    } else {
                        *default = trampoline_idx;
                    }
                }
                (slot, term) => {
                    unreachable!("trampoline slot {slot:?} on non-branch terminator {term:?}")
                }
            }
        }
        RegFunc {
            idx: self.func_idx,
            type_idx: self.type_idx,
            params: self.params,
            results: self.results,
            locals: self.locals,
            reg_types: self.reg_types,
            blocks: self.blocks,
        }
    }
    /// Lower one decoded instruction. Returns `true` when the function body is complete.
    fn lower_instr(&mut self, decoded: DecodedInstr) -> Result<bool, LowerError> {
        let offset = decoded.offset;
        match decoded.instr {
            Instr::LocalGet(local) => {
                let ty = self.locals[local.0 as usize];
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(offset, RegOp::LocalGet { dst, local });
            }
            Instr::LocalSet(local) => {
                let expected = self.locals[local.0 as usize];
                let value = self.pop_expect(offset, "local.set", expected)?;
                self.emit(
                    offset,
                    RegOp::LocalSet {
                        local,
                        value: value.reg,
                    },
                );
            }
            Instr::LocalTee(local) => {
                let expected = self.locals[local.0 as usize];
                let value = self.pop_expect(offset, "local.tee", expected)?;
                self.stack.push(value);
                self.emit(
                    offset,
                    RegOp::LocalTee {
                        local,
                        value: value.reg,
                    },
                );
            }
            Instr::Drop => {
                let value = self.pop_any(offset, "drop")?;
                self.emit(offset, RegOp::Drop { value: value.reg });
            }
            Instr::Select => {
                let cond = self.pop_expect(offset, "select", ValType::Num(NumType::I32))?;
                // Untyped select: both operands must share the same numeric
                // type; the second operand's type is discovered from the
                // stack (the validator has already proven they match).
                let v2 = self.pop_any(offset, "select")?;
                let v1 = self.pop_expect(offset, "select", v2.ty)?;
                let dst = self.alloc_reg(v1.ty);
                self.stack.push(RegValue {
                    reg: dst,
                    ty: v1.ty,
                });
                self.emit(
                    offset,
                    RegOp::Select {
                        dst,
                        v1: v1.reg,
                        v2: v2.reg,
                        cond: cond.reg,
                    },
                );
            }
            Instr::SelectTyped(types) => {
                let cond = self.pop_expect(offset, "select", ValType::Num(NumType::I32))?;
                // The validator guarantees exactly one result type.
                let Some(&ty) = types.first() else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::UnsupportedInstr {
                            op: "select with empty type annotation",
                        },
                    });
                };
                let v2 = self.pop_expect(offset, "select", ty)?;
                let v1 = self.pop_expect(offset, "select", ty)?;
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(
                    offset,
                    RegOp::Select {
                        dst,
                        v1: v1.reg,
                        v2: v2.reg,
                        cond: cond.reg,
                    },
                );
            }
            Instr::I32Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::I32));
                self.stack.push(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::I32),
                });
                self.emit(offset, RegOp::I32Const { dst, value });
            }
            Instr::I64Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::I64));
                self.stack.push(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::I64),
                });
                self.emit(offset, RegOp::I64Const { dst, value });
            }
            Instr::F32Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::F32));
                self.stack.push(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::F32),
                });
                self.emit(offset, RegOp::F32Const { dst, value });
            }
            Instr::F64Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::F64));
                self.stack.push(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::F64),
                });
                self.emit(offset, RegOp::F64Const { dst, value });
            }
            Instr::Unreachable => {
                self.finish_block(RegTerm::Trap);
                self.set_unreachable();
            }
            Instr::Nop => {}
            Instr::Block(block_type) => {
                self.finish_block(RegTerm::Fallthrough);
                self.enter_frame(offset, "block", FrameKind::Block, block_type)?;
            }
            Instr::Loop(block_type) => {
                self.finish_block(RegTerm::Fallthrough);
                // The loop body starts a fresh block; branches to the loop
                // label jump back to it (back-edge), so its index is known
                // immediately and needs no back-patching.
                let header_block = self.blocks.len() as u32;
                self.enter_frame(offset, "loop", FrameKind::Loop { header_block }, block_type)?;
            }
            Instr::If(block_type) => {
                let cond = self.pop_expect(offset, "if", ValType::Num(NumType::I32))?;
                let cond_block = self.blocks.len();
                self.finish_block(RegTerm::IfFork {
                    cond: cond.reg,
                    then_block: (cond_block + 1) as u32,
                    // Back-patched at `else` (else-body start) or at `end`
                    // (no else: the continuation).
                    else_block: 0,
                });
                self.enter_frame(
                    offset,
                    "if",
                    FrameKind::If {
                        cond_block,
                        else_seen: false,
                    },
                    block_type,
                )?;
            }
            Instr::Else => {
                let frame_pos = self.label_stack.len() - 1;
                let (cond_block, result_types, frame_height) = match self.label_stack.last() {
                    Some(LabelFrame {
                        kind:
                            FrameKind::If {
                                cond_block,
                                else_seen: false,
                            },
                        result_types,
                        height,
                        ..
                    }) => (*cond_block, result_types.clone(), *height),
                    _ => {
                        return Err(LowerError {
                            offset,
                            function: Some(self.func_idx),
                            kind: LowerErrorKind::UnexpectedElse,
                        });
                    }
                };
                // Pop the then-body's results; they become the values of a
                // synthetic branch from the then-body exit to the
                // continuation, back-patched at `end` like any other branch.
                let mut values = Vec::with_capacity(result_types.len());
                for &expected in result_types.iter().rev() {
                    let found = self.pop_expect(offset, "else", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                let br_block_idx = self.blocks.len();
                self.pending_branches.push(PendingBranch {
                    block: br_block_idx,
                    frame_pos,
                    values: values.clone(),
                    slot: BranchSlot::Single,
                });
                self.finish_block(RegTerm::Br {
                    target_block: 0,
                    values,
                });
                // The else-body starts at the next block.
                let else_start = self.blocks.len() as u32;
                if let RegTerm::IfFork { else_block, .. } = &mut self.blocks[cond_block].term {
                    *else_block = else_start;
                }
                // Reset to the frame entry state for the else-body: the
                // else path starts with the frame's parameters again.
                self.stack.truncate(frame_height);
                let (param_regs, param_types) = {
                    let frame = self.label_stack.last().expect("if frame checked above");
                    (frame.param_regs.clone(), frame.param_types.clone())
                };
                for (&reg, &ty) in param_regs.iter().zip(param_types.iter()) {
                    self.stack.push(RegValue { reg, ty });
                }
                let frame = self.label_stack.last_mut().expect("if frame checked above");
                frame.unreachable = false;
                if let FrameKind::If { else_seen, .. } = &mut frame.kind {
                    *else_seen = true;
                }
            }
            Instr::End => {
                // The outermost end is the function end. The frame must
                // still be on the stack while results are popped (pops are
                // frame-aware), so handle it before popping.
                if self.label_stack.len() == 1 {
                    let values = self.pop_results(offset)?;
                    self.label_stack.pop();
                    self.finish_block(RegTerm::Return { values });
                    return Ok(true);
                }
                // Pop results matching this control frame's expected types.
                // The frame must still be on the stack while they are
                // popped: pops are frame-aware (entry height and
                // polymorphic-stack state).
                let result_types = self
                    .label_stack
                    .last()
                    .ok_or(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::MissingFunctionEnd,
                    })?
                    .result_types
                    .clone();
                let mut values = Vec::with_capacity(result_types.len());
                for &expected in result_types.iter().rev() {
                    let found = self.pop_expect(offset, "end", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                let frame = self
                    .label_stack
                    .pop()
                    .expect("frame presence checked above");
                // Reset the operand stack to the frame entry height, then push
                // the block's result values back onto the outer stack.
                self.stack.truncate(frame.height);
                for (&reg, &ty) in values.iter().zip(frame.result_types.iter()) {
                    self.stack.push(RegValue { reg, ty });
                }
                // Finish the body block.
                self.finish_block(RegTerm::Fallthrough);
                // The continuation block will be at self.blocks.len().
                // Back-patch all pending branches targeting this frame,
                // preserving each terminator's kind (Br vs BrIf), and append
                // copies delivering each branch's values into the registers
                // the continuation expects.
                let frame_pos = self.label_stack.len(); // position of the popped frame
                let continuation_idx = self.blocks.len() as u32;
                let mut remaining = Vec::with_capacity(self.pending_branches.len());
                for pending in core::mem::take(&mut self.pending_branches) {
                    if pending.frame_pos != frame_pos {
                        remaining.push(pending);
                        continue;
                    }
                    let term = &mut self.blocks[pending.block].term;
                    match pending.slot {
                        BranchSlot::Single => match term {
                            RegTerm::Br { target_block, .. }
                            | RegTerm::BrIf { target_block, .. } => {
                                *target_block = continuation_idx;
                            }
                            other => unreachable!(
                                "pending branch block has non-branch terminator: {other:?}"
                            ),
                        },
                        BranchSlot::Table(slot) => match term {
                            RegTerm::BrTable {
                                targets, default, ..
                            } => {
                                if slot < targets.len() {
                                    targets[slot] = continuation_idx;
                                } else {
                                    *default = continuation_idx;
                                }
                            }
                            other => unreachable!(
                                "pending branch block has non-branch terminator: {other:?}"
                            ),
                        },
                    }
                    // Copies from every targeted frame write disjoint
                    // register sets from the same sources, so appending
                    // per-frame copies to one block is sound.
                    for (dst, src) in values.iter().zip(pending.values.iter()) {
                        if dst != src {
                            self.blocks[pending.block].instrs.push(RegInstr {
                                offset,
                                op: RegOp::Copy {
                                    dst: *dst,
                                    src: *src,
                                },
                            });
                        }
                    }
                }
                self.pending_branches = remaining;
                // For an `if` without `else`, the IfFork's else edge targets
                // the continuation directly.
                if let FrameKind::If {
                    cond_block,
                    else_seen: false,
                } = frame.kind
                    && let RegTerm::IfFork { else_block, .. } = &mut self.blocks[cond_block].term
                {
                    *else_block = continuation_idx;
                }
                // Start a new continuation block (content will be filled by
                // subsequent instructions).
                self.finish_block(RegTerm::Fallthrough);
            }
            Instr::Br(label) => {
                let frame_pos = self.label_position(offset, label)?;
                // Branches to a loop label jump to the header carrying the
                // loop's parameters; all other branches jump to the frame's
                // continuation carrying its results.
                let (branch_types, loop_header) = self.branch_types_at(frame_pos);
                let mut values = Vec::with_capacity(branch_types.len());
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                if let Some(loop_target) = loop_header {
                    // Unconditional back-edge: deliver values into the
                    // loop's parameter registers inline (always taken).
                    self.emit_copies(offset, &loop_target.param_regs, &values);
                    self.finish_block(RegTerm::Br {
                        target_block: loop_target.header,
                        values,
                    });
                } else {
                    let br_block_idx = self.blocks.len();
                    self.pending_branches.push(PendingBranch {
                        block: br_block_idx,
                        frame_pos,
                        values: values.clone(),
                        slot: BranchSlot::Single,
                    });
                    self.finish_block(RegTerm::Br {
                        target_block: 0,
                        values,
                    });
                }
                self.set_unreachable();
            }
            Instr::BrIf(label) => {
                let cond = self.pop_expect(offset, "br_if", ValType::Num(NumType::I32))?;
                let frame_pos = self.label_position(offset, label)?;
                let (branch_types, loop_header) = self.branch_types_at(frame_pos);
                let mut values = Vec::with_capacity(branch_types.len());
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br_if", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                // The not-taken path keeps the branch values on the stack.
                for (&reg, &ty) in values.iter().zip(branch_types.iter()) {
                    self.stack.push(RegValue { reg, ty });
                }
                if let Some(loop_target) = loop_header {
                    // Conditional back-edge: a trampoline runs the parameter
                    // copies so the not-taken path keeps the loop's live
                    // parameter registers intact.
                    let br_block_idx = self.blocks.len();
                    self.pending_trampolines.push(TrampolineReq {
                        block: br_block_idx,
                        slot: BranchSlot::Single,
                        param_regs: loop_target.param_regs,
                        values: values.clone(),
                        header: loop_target.header,
                        offset,
                    });
                    self.finish_block(RegTerm::BrIf {
                        cond: cond.reg,
                        target_block: 0,
                        values,
                    });
                } else {
                    let br_block_idx = self.blocks.len();
                    self.pending_branches.push(PendingBranch {
                        block: br_block_idx,
                        frame_pos,
                        values: values.clone(),
                        slot: BranchSlot::Single,
                    });
                    self.finish_block(RegTerm::BrIf {
                        cond: cond.reg,
                        target_block: 0,
                        values,
                    });
                }
            }
            Instr::BrTable { targets, default } => {
                let index = self.pop_expect(offset, "br_table", ValType::Num(NumType::I32))?;
                // All targets must agree on branch arity and types (the
                // validator guarantees this); pop using the default target.
                let default_pos = self.label_position(offset, default)?;
                let (branch_types, _) = self.branch_types_at(default_pos);
                let mut values = Vec::with_capacity(branch_types.len());
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br_table", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                // Resolve targets: loop headers are known immediately; other
                // frames get a pending entry per slot for back-patching at
                // their `end`.
                let br_block_idx = self.blocks.len();
                let mut target_blocks = Vec::with_capacity(targets.len() + 1);
                for (slot, target) in targets.iter().chain(core::iter::once(&default)).enumerate() {
                    let frame_pos = self.label_position(offset, *target)?;
                    let (_, loop_header) = self.branch_types_at(frame_pos);
                    match loop_header {
                        Some(loop_target) => {
                            target_blocks.push(0);
                            self.pending_trampolines.push(TrampolineReq {
                                block: br_block_idx,
                                slot: BranchSlot::Table(slot),
                                param_regs: loop_target.param_regs,
                                values: values.clone(),
                                header: loop_target.header,
                                offset,
                            });
                        }
                        None => {
                            target_blocks.push(0);
                            self.pending_branches.push(PendingBranch {
                                block: br_block_idx,
                                frame_pos,
                                values: values.clone(),
                                slot: BranchSlot::Table(slot),
                            });
                        }
                    }
                }
                let default_block = target_blocks.pop().expect("default included above");
                self.finish_block(RegTerm::BrTable {
                    index: index.reg,
                    targets: target_blocks,
                    default: default_block,
                    values,
                });
                self.set_unreachable();
            }
            Instr::Return => {
                let values = self.pop_results(offset)?;
                self.finish_block(RegTerm::Return { values });
                // Code after `return` is unreachable, but lowering continues:
                // instructions up to the function's final `end` must still be
                // processed (under polymorphic stack discipline).
                self.set_unreachable();
            }
            Instr::Call(func) => {
                let Some(callee_ty) = self.tables.func_types.get(func.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidFunction { func: func.0 },
                    });
                };
                let mut args = Vec::with_capacity(callee_ty.params.len());
                for &expected in callee_ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "call", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                let mut results = Vec::with_capacity(callee_ty.results.len());
                for &ty in callee_ty.results.iter() {
                    let dst = self.alloc_reg(ty);
                    self.stack.push(RegValue { reg: dst, ty });
                    results.push(dst);
                }
                self.emit(
                    offset,
                    RegOp::Call {
                        func,
                        args,
                        results,
                    },
                );
            }
            Instr::GlobalGet(global) => {
                let ty = self.global_type(offset, global)?;
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(offset, RegOp::GlobalGet { dst, global });
            }
            Instr::GlobalSet(global) => {
                let ty = self.global_type(offset, global)?;
                let value = self.pop_expect(offset, "global.set", ty)?;
                self.emit(
                    offset,
                    RegOp::GlobalSet {
                        global,
                        value: value.reg,
                    },
                );
            }
            Instr::MemorySize(memory) => {
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(offset, RegOp::MemorySize { dst, memory });
            }
            Instr::MemoryGrow(memory) => {
                let delta = self.pop_expect(offset, "memory.grow", ValType::Num(NumType::I32))?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(
                    offset,
                    RegOp::MemoryGrow {
                        dst,
                        memory,
                        delta: delta.reg,
                    },
                );
            }
            Instr::CallIndirect {
                type_idx,
                table_idx,
            } => {
                let Some(ty) = self.tables.types.get(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                let index = self.pop_expect(offset, "call_indirect", ValType::Num(NumType::I32))?;
                let mut args = Vec::with_capacity(ty.params.len());
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "call_indirect", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                let mut results = Vec::with_capacity(ty.results.len());
                for &result_ty in ty.results.iter() {
                    let dst = self.alloc_reg(result_ty);
                    self.stack.push(RegValue {
                        reg: dst,
                        ty: result_ty,
                    });
                    results.push(dst);
                }
                self.emit(
                    offset,
                    RegOp::CallIndirect {
                        type_idx,
                        table: table_idx,
                        index: index.reg,
                        args,
                        results,
                    },
                );
            }
            Instr::TableGet(table) => {
                let index = self.pop_expect(offset, "table.get", ValType::Num(NumType::I32))?;
                let ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(
                    offset,
                    RegOp::TableGet {
                        dst,
                        table,
                        index: index.reg,
                    },
                );
            }
            Instr::TableSet(table) => {
                let ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let value = self.pop_expect(offset, "table.set", ty)?;
                let index = self.pop_expect(offset, "table.set", ValType::Num(NumType::I32))?;
                self.emit(
                    offset,
                    RegOp::TableSet {
                        table,
                        index: index.reg,
                        value: value.reg,
                    },
                );
            }
            Instr::TableSize(table) => {
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(offset, RegOp::TableSize { dst, table });
            }
            Instr::TableGrow(table) => {
                let delta = self.pop_expect(offset, "table.grow", ValType::Num(NumType::I32))?;
                let elem_ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let value = self.pop_expect(offset, "table.grow", elem_ty)?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(
                    offset,
                    RegOp::TableGrow {
                        dst,
                        table,
                        value: value.reg,
                        delta: delta.reg,
                    },
                );
            }
            Instr::TableFill(table) => {
                let count = self.pop_expect(offset, "table.fill", ValType::Num(NumType::I32))?;
                let elem_ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let value = self.pop_expect(offset, "table.fill", elem_ty)?;
                let dst_idx = self.pop_expect(offset, "table.fill", ValType::Num(NumType::I32))?;
                self.emit(
                    offset,
                    RegOp::TableFill {
                        table,
                        dst: dst_idx.reg,
                        value: value.reg,
                        count: count.reg,
                    },
                );
            }
            Instr::TableCopy { dst, src } => {
                let count = self.pop_expect(offset, "table.copy", ValType::Num(NumType::I32))?;
                let src_idx = self.pop_expect(offset, "table.copy", ValType::Num(NumType::I32))?;
                let dst_idx = self.pop_expect(offset, "table.copy", ValType::Num(NumType::I32))?;
                self.emit(
                    offset,
                    RegOp::TableCopy {
                        dst_table: dst,
                        src_table: src,
                        dst: dst_idx.reg,
                        src: src_idx.reg,
                        count: count.reg,
                    },
                );
            }
            Instr::TableInit {
                elem_idx,
                table_idx,
            } => {
                let count = self.pop_expect(offset, "table.init", ValType::Num(NumType::I32))?;
                let src = self.pop_expect(offset, "table.init", ValType::Num(NumType::I32))?;
                let dst = self.pop_expect(offset, "table.init", ValType::Num(NumType::I32))?;
                self.emit(
                    offset,
                    RegOp::TableInit {
                        table: table_idx,
                        elem: elem_idx,
                        dst: dst.reg,
                        src: src.reg,
                        count: count.reg,
                    },
                );
            }
            Instr::ElemDrop(elem) => {
                self.emit(offset, RegOp::ElemDrop { elem });
            }
            Instr::RefNull(ref_type) => {
                let ty = ValType::Ref(ref_type);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(offset, RegOp::RefNull { dst });
            }
            Instr::RefFunc(func) => {
                // Lowered as the nullable funcref type; the validator has
                // already proven the precise (non-null) type.
                let ty = ValType::Ref(crate::types::RefType::FuncRef);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(offset, RegOp::RefFunc { dst, func });
            }
            Instr::RefIsNull => {
                let value = self.pop_any(offset, "ref.is_null")?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty);
                self.stack.push(RegValue { reg: dst, ty });
                self.emit(
                    offset,
                    RegOp::RefIsNull {
                        dst,
                        value: value.reg,
                    },
                );
            }
            instr => {
                if let Some(op) = unary_op(&instr) {
                    self.lower_unary_op(offset, op)?;
                } else if let Some(op) = binary_op(&instr) {
                    self.lower_binary_op(offset, op)?;
                } else if let Some((op, memarg)) = load_op(&instr) {
                    self.lower_load(offset, op, memarg)?;
                } else if let Some((op, memarg)) = store_op(&instr) {
                    self.lower_store(offset, op, memarg)?;
                } else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::UnsupportedInstr {
                            op: instr_name(&instr),
                        },
                    });
                }
            }
        }

        Ok(false)
    }

    fn alloc_reg(&mut self, ty: ValType) -> Reg {
        let reg = Reg(self.reg_types.len() as u32);
        self.reg_types.push(ty);
        reg
    }

    fn emit(&mut self, offset: ByteOffset, op: RegOp) {
        self.current_instrs.push(RegInstr { offset, op });
    }

    fn pop_any(&mut self, offset: ByteOffset, op: &'static str) -> Result<RegValue, LowerError> {
        if self.at_frame_boundary() {
            if self.current_frame_unreachable() {
                // Polymorphic stack: synthesize an undefined register. The
                // surrounding code is unreachable, so the register is never
                // read at runtime.
                let ty = ValType::Num(NumType::I32);
                let reg = self.alloc_reg(ty);
                return Ok(RegValue { reg, ty });
            }
            return Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::StackUnderflow {
                    op,
                    expected: ValType::Num(NumType::I32),
                },
            });
        }
        Ok(self.stack.pop().expect("stack height checked"))
    }

    /// Whether the operand stack is exactly at the current frame's entry
    /// height — pops below this point are frame-boundary pops.
    fn at_frame_boundary(&self) -> bool {
        let frame = self
            .label_stack
            .last()
            .expect("function frame is always present");
        self.stack.len() == frame.height
    }

    fn current_frame_unreachable(&self) -> bool {
        self.label_stack
            .last()
            .expect("function frame is always present")
            .unreachable
    }

    /// Resolve a branch label to a position in `label_stack`, or fail on an
    /// out-of-range label.
    fn label_position(&self, offset: ByteOffset, label: LabelIdx) -> Result<usize, LowerError> {
        let label_idx = label.0 as usize;
        if label_idx >= self.label_stack.len() {
            return Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidLabel { label: label.0 },
            });
        }
        Ok(self.label_stack.len() - 1 - label_idx)
    }

    /// Enter a new control frame: consume the block type's parameters from
    /// the operand stack (recording their canonical registers), push the
    /// frame with its base height below the params, then push the params
    /// back as the frame's initial working stack.
    fn enter_frame(
        &mut self,
        offset: ByteOffset,
        op: &'static str,
        kind: FrameKind,
        block_type: BlockType,
    ) -> Result<(), LowerError> {
        let (param_types, result_types) = self.block_type_sig(offset, block_type)?;
        let mut param_regs = Vec::with_capacity(param_types.len());
        for &expected in param_types.iter().rev() {
            let found = self.pop_expect(offset, op, expected)?;
            param_regs.push(found.reg);
        }
        param_regs.reverse();
        let height = self.stack.len();
        for (&reg, &ty) in param_regs.iter().zip(param_types.iter()) {
            self.stack.push(RegValue { reg, ty });
        }
        self.label_stack.push(LabelFrame {
            label: LabelIdx(self.label_stack.len() as u32),
            kind,
            result_types,
            param_types,
            param_regs,
            height,
            unreachable: false,
        });
        Ok(())
    }

    /// Resolve a block type to its (params, results) signature.
    fn block_type_sig(
        &self,
        offset: ByteOffset,
        block_type: BlockType,
    ) -> Result<(Vec<ValType>, Vec<ValType>), LowerError> {
        match block_type {
            BlockType::Empty => Ok((Vec::new(), Vec::new())),
            BlockType::Val(ty) => Ok((Vec::new(), alloc::vec![ty])),
            BlockType::TypeIdx(idx) => {
                let ty = self.tables.types.get(idx as usize).ok_or(LowerError {
                    offset,
                    function: Some(self.func_idx),
                    kind: LowerErrorKind::InvalidType { type_idx: idx },
                })?;
                Ok((ty.params.clone(), ty.results.clone()))
            }
        }
    }

    /// Emit copy instructions delivering `srcs` into `dsts` (used for
    /// unconditional loop back-edges; conditional branches use trampolines).
    fn emit_copies(&mut self, offset: ByteOffset, dsts: &[Reg], srcs: &[Reg]) {
        for (&dst, &src) in dsts.iter().zip(srcs.iter()) {
            if dst != src {
                self.emit(offset, RegOp::Copy { dst, src });
            }
        }
    }

    /// The value types a branch to the frame at `frame_pos` must carry, and
    /// the loop target details when the frame is a loop.
    fn branch_types_at(&self, frame_pos: usize) -> (Vec<ValType>, Option<LoopTarget>) {
        let frame = &self.label_stack[frame_pos];
        match frame.kind {
            FrameKind::Loop { header_block } => (
                frame.param_types.clone(),
                Some(LoopTarget {
                    header: header_block,
                    param_regs: frame.param_regs.clone(),
                }),
            ),
            _ => (frame.result_types.clone(), None),
        }
    }

    /// Mark the current control frame unreachable: the stack is truncated to
    /// the frame's entry height and further pops become polymorphic.
    fn set_unreachable(&mut self) {
        let frame = self
            .label_stack
            .last_mut()
            .expect("function frame is always present");
        self.stack.truncate(frame.height);
        frame.unreachable = true;
    }

    fn lower_binary_op(&mut self, offset: ByteOffset, op: BinaryOp) -> Result<(), LowerError> {
        let input = op.input_type();
        let output = op.result_type();
        let rhs = self.pop_expect(offset, op.name(), input)?;
        let lhs = self.pop_expect(offset, op.name(), input)?;
        let dst = self.alloc_reg(output);
        self.stack.push(RegValue {
            reg: dst,
            ty: output,
        });
        self.emit(
            offset,
            RegOp::Binary {
                op,
                dst,
                lhs: lhs.reg,
                rhs: rhs.reg,
            },
        );
        Ok(())
    }

    fn global_type(&self, offset: ByteOffset, global: GlobalIdx) -> Result<ValType, LowerError> {
        self.tables
            .global_types
            .get(global.0 as usize)
            .copied()
            .ok_or(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidGlobal { global: global.0 },
            })
    }

    fn table_elem_type(
        &self,
        offset: ByteOffset,
        table: TableIdx,
    ) -> Result<crate::types::RefType, LowerError> {
        self.tables
            .table_elem_types
            .get(table.0 as usize)
            .copied()
            .ok_or(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidTable { table: table.0 },
            })
    }

    fn lower_load(
        &mut self,
        offset: ByteOffset,
        op: LoadOp,
        memarg: MemArg,
    ) -> Result<(), LowerError> {
        let addr = self.pop_expect(offset, "load", ValType::Num(NumType::I32))?;
        let ty = op.result_type();
        let dst = self.alloc_reg(ty);
        self.stack.push(RegValue { reg: dst, ty });
        self.emit(
            offset,
            RegOp::Load {
                op,
                dst,
                addr: addr.reg,
                memarg,
            },
        );
        Ok(())
    }

    fn lower_store(
        &mut self,
        offset: ByteOffset,
        op: StoreOp,
        memarg: MemArg,
    ) -> Result<(), LowerError> {
        let value = self.pop_expect(offset, "store", op.value_type())?;
        let addr = self.pop_expect(offset, "store", ValType::Num(NumType::I32))?;
        self.emit(
            offset,
            RegOp::Store {
                op,
                addr: addr.reg,
                value: value.reg,
                memarg,
            },
        );
        Ok(())
    }

    fn lower_unary_op(&mut self, offset: ByteOffset, op: UnaryOp) -> Result<(), LowerError> {
        let input = op.input_type();
        let output = op.result_type();
        let value = self.pop_expect(offset, op.name(), input)?;
        let dst = self.alloc_reg(output);
        self.stack.push(RegValue {
            reg: dst,
            ty: output,
        });
        self.emit(
            offset,
            RegOp::Unary {
                op,
                dst,
                value: value.reg,
            },
        );
        Ok(())
    }

    fn pop_expect(
        &mut self,
        offset: ByteOffset,
        op: &'static str,
        expected: ValType,
    ) -> Result<RegValue, LowerError> {
        if self.at_frame_boundary() {
            if self.current_frame_unreachable() {
                // Polymorphic stack: synthesize an undefined register of the
                // expected type.
                let reg = self.alloc_reg(expected);
                return Ok(RegValue { reg, ty: expected });
            }
            return Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::StackUnderflow { op, expected },
            });
        }

        let found = self.stack.pop().expect("stack height checked");

        if found.ty != expected {
            return Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::TypeMismatch {
                    op,
                    expected,
                    found: found.ty,
                },
            });
        }

        Ok(found)
    }

    fn pop_results(&mut self, offset: ByteOffset) -> Result<Vec<Reg>, LowerError> {
        let results = self.results.clone();
        let mut values = Vec::with_capacity(results.len());
        for &expected in results.iter().rev() {
            let found = self.pop_expect(offset, "function end", expected)?;
            values.push(found.reg);
        }
        values.reverse();
        Ok(values)
    }
}

fn load_op(instr: &Instr) -> Option<(LoadOp, MemArg)> {
    let (op, memarg) = match *instr {
        Instr::I32Load(memarg) => (LoadOp::I32, memarg),
        Instr::I64Load(memarg) => (LoadOp::I64, memarg),
        Instr::F32Load(memarg) => (LoadOp::F32, memarg),
        Instr::F64Load(memarg) => (LoadOp::F64, memarg),
        Instr::I32Load8S(memarg) => (LoadOp::I32Load8S, memarg),
        Instr::I32Load8U(memarg) => (LoadOp::I32Load8U, memarg),
        Instr::I32Load16S(memarg) => (LoadOp::I32Load16S, memarg),
        Instr::I32Load16U(memarg) => (LoadOp::I32Load16U, memarg),
        Instr::I64Load8S(memarg) => (LoadOp::I64Load8S, memarg),
        Instr::I64Load8U(memarg) => (LoadOp::I64Load8U, memarg),
        Instr::I64Load16S(memarg) => (LoadOp::I64Load16S, memarg),
        Instr::I64Load16U(memarg) => (LoadOp::I64Load16U, memarg),
        Instr::I64Load32S(memarg) => (LoadOp::I64Load32S, memarg),
        Instr::I64Load32U(memarg) => (LoadOp::I64Load32U, memarg),
        _ => return None,
    };
    Some((op, memarg))
}

fn store_op(instr: &Instr) -> Option<(StoreOp, MemArg)> {
    let (op, memarg) = match *instr {
        Instr::I32Store(memarg) => (StoreOp::I32, memarg),
        Instr::I64Store(memarg) => (StoreOp::I64, memarg),
        Instr::F32Store(memarg) => (StoreOp::F32, memarg),
        Instr::F64Store(memarg) => (StoreOp::F64, memarg),
        Instr::I32Store8(memarg) => (StoreOp::I32Store8, memarg),
        Instr::I32Store16(memarg) => (StoreOp::I32Store16, memarg),
        Instr::I64Store8(memarg) => (StoreOp::I64Store8, memarg),
        Instr::I64Store16(memarg) => (StoreOp::I64Store16, memarg),
        Instr::I64Store32(memarg) => (StoreOp::I64Store32, memarg),
        _ => return None,
    };
    Some((op, memarg))
}

fn unary_op(instr: &Instr) -> Option<UnaryOp> {
    match instr {
        Instr::I32Clz => Some(UnaryOp::I32Clz),
        Instr::I32Ctz => Some(UnaryOp::I32Ctz),
        Instr::I32Popcnt => Some(UnaryOp::I32Popcnt),
        Instr::I32Eqz => Some(UnaryOp::I32Eqz),
        Instr::I32WrapI64 => Some(UnaryOp::I32WrapI64),
        Instr::I32Extend8S => Some(UnaryOp::I32Extend8S),
        Instr::I32Extend16S => Some(UnaryOp::I32Extend16S),
        Instr::I32TruncF32S => Some(UnaryOp::I32TruncF32S),
        Instr::I32TruncF32U => Some(UnaryOp::I32TruncF32U),
        Instr::I32TruncF64S => Some(UnaryOp::I32TruncF64S),
        Instr::I32TruncF64U => Some(UnaryOp::I32TruncF64U),
        Instr::F32ConvertI32S => Some(UnaryOp::F32ConvertI32S),
        Instr::F32ConvertI32U => Some(UnaryOp::F32ConvertI32U),
        Instr::F64ConvertI32S => Some(UnaryOp::F64ConvertI32S),
        Instr::F64ConvertI32U => Some(UnaryOp::F64ConvertI32U),
        Instr::F32Neg => Some(UnaryOp::F32Neg),
        Instr::F32Abs => Some(UnaryOp::F32Abs),
        Instr::F32Sqrt => Some(UnaryOp::F32Sqrt),
        Instr::F32Ceil => Some(UnaryOp::F32Ceil),
        Instr::F32Floor => Some(UnaryOp::F32Floor),
        Instr::F32Trunc => Some(UnaryOp::F32Trunc),
        Instr::F32Nearest => Some(UnaryOp::F32Nearest),
        Instr::I64Clz => Some(UnaryOp::I64Clz),
        Instr::I64Ctz => Some(UnaryOp::I64Ctz),
        Instr::I64Popcnt => Some(UnaryOp::I64Popcnt),
        Instr::I64Eqz => Some(UnaryOp::I64Eqz),
        Instr::I64ExtendI32S => Some(UnaryOp::I64ExtendI32S),
        Instr::I64ExtendI32U => Some(UnaryOp::I64ExtendI32U),
        Instr::I64Extend8S => Some(UnaryOp::I64Extend8S),
        Instr::I64Extend16S => Some(UnaryOp::I64Extend16S),
        Instr::I64Extend32S => Some(UnaryOp::I64Extend32S),
        Instr::I64TruncF32S => Some(UnaryOp::I64TruncF32S),
        Instr::I64TruncF32U => Some(UnaryOp::I64TruncF32U),
        Instr::I64TruncF64S => Some(UnaryOp::I64TruncF64S),
        Instr::I64TruncF64U => Some(UnaryOp::I64TruncF64U),
        Instr::F32ConvertI64S => Some(UnaryOp::F32ConvertI64S),
        Instr::F32ConvertI64U => Some(UnaryOp::F32ConvertI64U),
        Instr::F64ConvertI64S => Some(UnaryOp::F64ConvertI64S),
        Instr::F64ConvertI64U => Some(UnaryOp::F64ConvertI64U),
        Instr::F64Neg => Some(UnaryOp::F64Neg),
        Instr::F64Abs => Some(UnaryOp::F64Abs),
        Instr::F64Sqrt => Some(UnaryOp::F64Sqrt),
        Instr::F64Ceil => Some(UnaryOp::F64Ceil),
        Instr::F64Floor => Some(UnaryOp::F64Floor),
        Instr::F64Trunc => Some(UnaryOp::F64Trunc),
        Instr::F64Nearest => Some(UnaryOp::F64Nearest),
        Instr::F32DemoteF64 => Some(UnaryOp::F32DemoteF64),
        Instr::F64PromoteF32 => Some(UnaryOp::F64PromoteF32),
        Instr::I32ReinterpretF32 => Some(UnaryOp::I32ReinterpretF32),
        Instr::F32ReinterpretI32 => Some(UnaryOp::F32ReinterpretI32),
        Instr::I64ReinterpretF64 => Some(UnaryOp::I64ReinterpretF64),
        Instr::F64ReinterpretI64 => Some(UnaryOp::F64ReinterpretI64),
        Instr::I32TruncSatF32S => Some(UnaryOp::I32TruncSatF32S),
        Instr::I32TruncSatF32U => Some(UnaryOp::I32TruncSatF32U),
        Instr::I32TruncSatF64S => Some(UnaryOp::I32TruncSatF64S),
        Instr::I32TruncSatF64U => Some(UnaryOp::I32TruncSatF64U),
        Instr::I64TruncSatF32S => Some(UnaryOp::I64TruncSatF32S),
        Instr::I64TruncSatF32U => Some(UnaryOp::I64TruncSatF32U),
        Instr::I64TruncSatF64S => Some(UnaryOp::I64TruncSatF64S),
        Instr::I64TruncSatF64U => Some(UnaryOp::I64TruncSatF64U),
        _ => None,
    }
}

fn binary_op(instr: &Instr) -> Option<BinaryOp> {
    match instr {
        Instr::I32Add => Some(BinaryOp::I32Add),
        Instr::I32Sub => Some(BinaryOp::I32Sub),
        Instr::I32Mul => Some(BinaryOp::I32Mul),
        Instr::I32DivS => Some(BinaryOp::I32DivS),
        Instr::I32DivU => Some(BinaryOp::I32DivU),
        Instr::I32RemS => Some(BinaryOp::I32RemS),
        Instr::I32RemU => Some(BinaryOp::I32RemU),
        Instr::I32And => Some(BinaryOp::I32And),
        Instr::I32Or => Some(BinaryOp::I32Or),
        Instr::I32Xor => Some(BinaryOp::I32Xor),
        Instr::I32Shl => Some(BinaryOp::I32Shl),
        Instr::I32ShrS => Some(BinaryOp::I32ShrS),
        Instr::I32ShrU => Some(BinaryOp::I32ShrU),
        Instr::I32Rotl => Some(BinaryOp::I32Rotl),
        Instr::I32Rotr => Some(BinaryOp::I32Rotr),
        Instr::I32Eq => Some(BinaryOp::I32Eq),
        Instr::I32Ne => Some(BinaryOp::I32Ne),
        Instr::I32LtS => Some(BinaryOp::I32LtS),
        Instr::I32LtU => Some(BinaryOp::I32LtU),
        Instr::I32GtS => Some(BinaryOp::I32GtS),
        Instr::I32GtU => Some(BinaryOp::I32GtU),
        Instr::I32LeS => Some(BinaryOp::I32LeS),
        Instr::I32LeU => Some(BinaryOp::I32LeU),
        Instr::I32GeS => Some(BinaryOp::I32GeS),
        Instr::I32GeU => Some(BinaryOp::I32GeU),
        Instr::I64Add => Some(BinaryOp::I64Add),
        Instr::I64Sub => Some(BinaryOp::I64Sub),
        Instr::I64Mul => Some(BinaryOp::I64Mul),
        Instr::I64DivS => Some(BinaryOp::I64DivS),
        Instr::I64DivU => Some(BinaryOp::I64DivU),
        Instr::I64RemS => Some(BinaryOp::I64RemS),
        Instr::I64RemU => Some(BinaryOp::I64RemU),
        Instr::I64And => Some(BinaryOp::I64And),
        Instr::I64Or => Some(BinaryOp::I64Or),
        Instr::I64Xor => Some(BinaryOp::I64Xor),
        Instr::I64Shl => Some(BinaryOp::I64Shl),
        Instr::I64ShrS => Some(BinaryOp::I64ShrS),
        Instr::I64ShrU => Some(BinaryOp::I64ShrU),
        Instr::I64Rotl => Some(BinaryOp::I64Rotl),
        Instr::I64Rotr => Some(BinaryOp::I64Rotr),
        Instr::I64Eq => Some(BinaryOp::I64Eq),
        Instr::I64Ne => Some(BinaryOp::I64Ne),
        Instr::I64LtS => Some(BinaryOp::I64LtS),
        Instr::I64LtU => Some(BinaryOp::I64LtU),
        Instr::I64GtS => Some(BinaryOp::I64GtS),
        Instr::I64GtU => Some(BinaryOp::I64GtU),
        Instr::I64LeS => Some(BinaryOp::I64LeS),
        Instr::I64LeU => Some(BinaryOp::I64LeU),
        Instr::I64GeS => Some(BinaryOp::I64GeS),
        Instr::I64GeU => Some(BinaryOp::I64GeU),
        Instr::F32Add => Some(BinaryOp::F32Add),
        Instr::F32Sub => Some(BinaryOp::F32Sub),
        Instr::F32Mul => Some(BinaryOp::F32Mul),
        Instr::F32Div => Some(BinaryOp::F32Div),
        Instr::F32Min => Some(BinaryOp::F32Min),
        Instr::F32Max => Some(BinaryOp::F32Max),
        Instr::F64Add => Some(BinaryOp::F64Add),
        Instr::F64Sub => Some(BinaryOp::F64Sub),
        Instr::F64Mul => Some(BinaryOp::F64Mul),
        Instr::F64Div => Some(BinaryOp::F64Div),
        Instr::F64Min => Some(BinaryOp::F64Min),
        Instr::F64Max => Some(BinaryOp::F64Max),
        Instr::F32Eq => Some(BinaryOp::F32Eq),
        Instr::F32Ne => Some(BinaryOp::F32Ne),
        Instr::F32Lt => Some(BinaryOp::F32Lt),
        Instr::F32Gt => Some(BinaryOp::F32Gt),
        Instr::F32Le => Some(BinaryOp::F32Le),
        Instr::F32Ge => Some(BinaryOp::F32Ge),
        Instr::F64Eq => Some(BinaryOp::F64Eq),
        Instr::F64Ne => Some(BinaryOp::F64Ne),
        Instr::F64Lt => Some(BinaryOp::F64Lt),
        Instr::F64Gt => Some(BinaryOp::F64Gt),
        Instr::F64Le => Some(BinaryOp::F64Le),
        Instr::F64Ge => Some(BinaryOp::F64Ge),
        _ => None,
    }
}

fn instr_name(instr: &Instr) -> &'static str {
    match instr {
        Instr::Unreachable => "unreachable",
        Instr::Nop => "nop",
        Instr::Block(_) => "block",
        Instr::Loop(_) => "loop",
        Instr::If(_) => "if",
        Instr::Else => "else",
        Instr::Br(_) => "br",
        Instr::BrIf(_) => "br_if",
        Instr::BrTable { .. } => "br_table",
        Instr::Return => "return",
        Instr::Call(_) => "call",
        Instr::CallIndirect { .. } => "call_indirect",
        Instr::LocalSet(_) => "local.set",
        Instr::LocalTee(_) => "local.tee",
        Instr::GlobalGet(_) => "global.get",
        Instr::GlobalSet(_) => "global.set",
        Instr::TableGet(_) => "table.get",
        Instr::TableSet(_) => "table.set",
        Instr::I64Add => "i64.add",
        _ => "instruction",
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::binary::module::Module;

    #[test]
    fn lower_simple_add_fixture_to_register_ir() {
        let bytes = baedeker_testdata::fixture_bytes("add");
        let module = Module::decode(&bytes).unwrap();
        let reg_module = module.lower().unwrap();

        assert_eq!(reg_module.funcs.len(), 1);
        let func = &reg_module.funcs[0];
        assert_eq!(
            func.params,
            vec![ValType::Num(NumType::I32), ValType::Num(NumType::I32)]
        );
        assert_eq!(func.results, vec![ValType::Num(NumType::I32)]);
        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I32); 3]);
        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::LocalGet {
                    dst: Reg(0),
                    local: LocalIdx(1),
                },
                &RegOp::LocalGet {
                    dst: Reg(1),
                    local: LocalIdx(0),
                },
                &RegOp::Binary {
                    op: BinaryOp::I32Add,
                    dst: Reg(2),
                    lhs: Reg(0),
                    rhs: Reg(1),
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn lower_block_and_return() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: [] -> []
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x07, 0x01, 0x05, 0x00, // code body header
            0x02, 0x40, 0x0b, 0x0b, // block end end
        ];
        let module = Module::decode(&bytes).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];
        assert_eq!(func.blocks.len(), 4);
        assert!(matches!(func.blocks[0].term, RegTerm::Fallthrough));
        assert!(matches!(
            func.blocks.last().unwrap().term,
            RegTerm::Return { .. }
        ));
    }

    #[test]
    fn execute_lowered_add_fixture() {
        let bytes = baedeker_testdata::fixture_bytes("add");
        let module = Module::decode(&bytes).unwrap();
        let reg_module = module.lower().unwrap();

        let result = crate::runtime::execute_func(
            &reg_module.funcs[0],
            &[
                crate::runtime::Value::I32(20),
                crate::runtime::Value::I32(22),
            ],
        )
        .unwrap();

        assert_eq!(result, vec![crate::runtime::Value::I32(42)]);
    }

    #[test]
    fn lower_local_set_temp_storage() {
        let module = Module::decode(local_set_temp_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I32); 4]);
        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::I32Const {
                    dst: Reg(0),
                    value: 40,
                },
                &RegOp::LocalSet {
                    local: LocalIdx(0),
                    value: Reg(0),
                },
                &RegOp::LocalGet {
                    dst: Reg(1),
                    local: LocalIdx(0),
                },
                &RegOp::I32Const {
                    dst: Reg(2),
                    value: 2,
                },
                &RegOp::Binary {
                    op: BinaryOp::I32Add,
                    dst: Reg(3),
                    lhs: Reg(1),
                    rhs: Reg(2),
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn execute_local_set_temp_storage() {
        let module = Module::decode(local_set_temp_module()).unwrap();
        let reg_module = module.lower().unwrap();

        let result = crate::runtime::execute_func(&reg_module.funcs[0], &[]).unwrap();

        assert_eq!(result, vec![crate::runtime::Value::I32(42)]);
    }

    #[test]
    fn lower_local_tee_keeps_value_on_stack() {
        let module = Module::decode(local_tee_stack_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I32); 3]);
        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::I32Const {
                    dst: Reg(0),
                    value: 40,
                },
                &RegOp::LocalTee {
                    local: LocalIdx(0),
                    value: Reg(0),
                },
                &RegOp::I32Const {
                    dst: Reg(1),
                    value: 2,
                },
                &RegOp::Binary {
                    op: BinaryOp::I32Add,
                    dst: Reg(2),
                    lhs: Reg(0),
                    rhs: Reg(1),
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn execute_local_tee_stack_value() {
        let module = Module::decode(local_tee_stack_module()).unwrap();
        let reg_module = module.lower().unwrap();

        let result = crate::runtime::execute_func(&reg_module.funcs[0], &[]).unwrap();

        assert_eq!(result, vec![crate::runtime::Value::I32(42)]);
    }

    fn local_set_temp_module() -> &'static [u8] {
        &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type: [] -> [i32]
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x0f, 0x01, 0x0d, 0x01, 0x01, 0x7f, // one i32 local
            0x41, 0x28, // i32.const 40
            0x21, 0x00, // local.set 0
            0x20, 0x00, // local.get 0
            0x41, 0x02, // i32.const 2
            0x6a, // i32.add
            0x0b, // end
        ]
    }

    fn local_tee_stack_module() -> &'static [u8] {
        &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type: [] -> [i32]
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x0d, 0x01, 0x0b, 0x01, 0x01, 0x7f, // one i32 local
            0x41, 0x28, // i32.const 40
            0x22, 0x00, // local.tee 0
            0x41, 0x02, // i32.const 2
            0x6a, // i32.add
            0x0b, // end
        ]
    }

    #[test]
    fn lower_i32_sub_and_mul_cohort() {
        let module = Module::decode(i32_sub_mul_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I32); 5]);
        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::I32Const {
                    dst: Reg(0),
                    value: 50,
                },
                &RegOp::I32Const {
                    dst: Reg(1),
                    value: 8,
                },
                &RegOp::Binary {
                    op: BinaryOp::I32Sub,
                    dst: Reg(2),
                    lhs: Reg(0),
                    rhs: Reg(1),
                },
                &RegOp::I32Const {
                    dst: Reg(3),
                    value: 3,
                },
                &RegOp::Binary {
                    op: BinaryOp::I32Mul,
                    dst: Reg(4),
                    lhs: Reg(2),
                    rhs: Reg(3),
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn execute_i32_sub_and_mul_cohort() {
        let module = Module::decode(i32_sub_mul_module()).unwrap();
        let reg_module = module.lower().unwrap();

        let result = crate::runtime::execute_func(&reg_module.funcs[0], &[]).unwrap();

        assert_eq!(result, vec![crate::runtime::Value::I32(126)]);
    }

    #[test]
    fn lower_i64_add_cohort() {
        let module = Module::decode(i64_add_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I64); 3]);
        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::I64Const {
                    dst: Reg(0),
                    value: 20,
                },
                &RegOp::I64Const {
                    dst: Reg(1),
                    value: 22,
                },
                &RegOp::Binary {
                    op: BinaryOp::I64Add,
                    dst: Reg(2),
                    lhs: Reg(0),
                    rhs: Reg(1),
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn execute_i64_add_cohort() {
        let module = Module::decode(i64_add_module()).unwrap();
        let reg_module = module.lower().unwrap();

        let result = crate::runtime::execute_func(&reg_module.funcs[0], &[]).unwrap();

        assert_eq!(result, vec![crate::runtime::Value::I64(42)]);
    }

    fn i32_sub_mul_module() -> &'static [u8] {
        &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type: [] -> [i32]
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x0c, 0x01, 0x0a, 0x00, // one body, no locals
            0x41, 0x32, // i32.const 50
            0x41, 0x08, // i32.const 8
            0x6b, // i32.sub
            0x41, 0x03, // i32.const 3
            0x6c, // i32.mul
            0x0b, // end
        ]
    }

    fn i64_add_module() -> &'static [u8] {
        &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7e, // type: [] -> [i64]
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x09, 0x01, 0x07, 0x00, // one body, no locals
            0x42, 0x14, // i64.const 20
            0x42, 0x16, // i64.const 22
            0x7c, // i64.add
            0x0b, // end
        ]
    }

    #[test]
    fn lower_i32_eqz_cohort() {
        let module = Module::decode(i32_eqz_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I32); 2]);
        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::LocalGet {
                    dst: Reg(0),
                    local: LocalIdx(0),
                },
                &RegOp::Unary {
                    op: UnaryOp::I32Eqz,
                    dst: Reg(1),
                    value: Reg(0),
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn execute_i32_eqz_cohort() {
        let module = Module::decode(i32_eqz_module()).unwrap();
        let reg_module = module.lower().unwrap();

        let zero =
            crate::runtime::execute_func(&reg_module.funcs[0], &[crate::runtime::Value::I32(0)])
                .unwrap();
        let nonzero =
            crate::runtime::execute_func(&reg_module.funcs[0], &[crate::runtime::Value::I32(7)])
                .unwrap();

        assert_eq!(zero, vec![crate::runtime::Value::I32(1)]);
        assert_eq!(nonzero, vec![crate::runtime::Value::I32(0)]);
    }

    fn i32_eqz_module() -> &'static [u8] {
        &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x06, 0x01, 0x60, 0x01, 0x7f, 0x01, 0x7f, // type: [i32] -> [i32]
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x07, 0x01, 0x05, 0x00, // one body, no locals
            0x20, 0x00, // local.get 0
            0x45, // i32.eqz
            0x0b, // end
        ]
    }

    #[test]
    fn lower_explicit_return_instruction() {
        let module = Module::decode(explicit_return_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(
            func.blocks[0]
                .instrs
                .iter()
                .map(|instr| &instr.op)
                .collect::<Vec<_>>(),
            vec![
                &RegOp::I32Const {
                    dst: Reg(0),
                    value: 42,
                },
                // Return checked via blocks[0].term
            ]
        );
    }

    #[test]
    fn execute_explicit_return_instruction() {
        let module = Module::decode(explicit_return_module()).unwrap();
        let reg_module = module.lower().unwrap();

        let result = crate::runtime::execute_func(&reg_module.funcs[0], &[]).unwrap();

        assert_eq!(result, vec![crate::runtime::Value::I32(42)]);
    }

    fn explicit_return_module() -> &'static [u8] {
        &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type: [] -> [i32]
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x07, 0x01, 0x05, 0x00, // one body, no locals
            0x41, 0x2a, // i32.const 42
            0x0f, // return
            0x0b, // end
        ]
    }

    /// Lower a WAT module for control-flow shape tests.
    fn lower_wat(source: &str) -> RegModule {
        let buf = wast::parser::ParseBuffer::new(source).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        let module = Module::decode(&bytes).unwrap();
        module.lower().unwrap()
    }

    fn run_wat(source: &str, args: &[crate::runtime::Value]) -> Vec<crate::runtime::Value> {
        let reg_module = lower_wat(source);
        crate::runtime::execute_func(&reg_module.funcs[0], args).unwrap()
    }

    #[test]
    fn lower_if_else_shape() {
        let reg_module = lower_wat(
            "(module (func (param i32) (result i32)
               local.get 0
               if (result i32)
                 i32.const 1
               else
                 i32.const 2
               end))",
        );
        let func = &reg_module.funcs[0];

        // Block 0 must end in an IfFork whose then/else targets were
        // back-patched to real block indices.
        let RegTerm::IfFork {
            then_block,
            else_block,
            ..
        } = func.blocks[0].term
        else {
            panic!("expected IfFork, got {:?}", func.blocks[0].term);
        };
        assert_eq!(then_block, 1);
        assert_ne!(else_block, 0);
        // The then-body must exit via a branch over the else-body.
        assert!(matches!(
            func.blocks[then_block as usize].term,
            RegTerm::Br { .. }
        ));
    }

    #[test]
    fn execute_if_else_paths() {
        let source = "(module (func (param i32) (result i32)
            local.get 0
            if (result i32)
              i32.const 1
            else
              i32.const 2
            end))";
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(1)]),
            vec![crate::runtime::Value::I32(1)]
        );
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(0)]),
            vec![crate::runtime::Value::I32(2)]
        );
    }

    #[test]
    fn lower_if_without_else_patches_else_to_continuation() {
        let reg_module = lower_wat(
            "(module (func (param i32) (result i32)
               local.get 0
               if
                 i32.const 42
                 drop
               end
               i32.const 7))",
        );
        let func = &reg_module.funcs[0];
        let RegTerm::IfFork { else_block, .. } = func.blocks[0].term else {
            panic!("expected IfFork, got {:?}", func.blocks[0].term);
        };
        // With no else, the else edge must reach the continuation whose
        // fallthrough chain leads to the final Return.
        let mut idx = else_block;
        loop {
            match func.blocks[idx as usize].term {
                RegTerm::Fallthrough => idx += 1,
                RegTerm::Return { .. } => break,
                ref other => panic!("unexpected terminator on else path: {other:?}"),
            }
        }
    }

    #[test]
    fn lower_loop_back_edge_targets_header() {
        let reg_module = lower_wat(
            "(module (func (param i32) (result i32)
               (local i32)
               block
                 loop
                   local.get 0
                   i32.eqz
                   br_if 1
                   local.get 0
                   i32.const 1
                   i32.sub
                   local.set 0
                   br 0
                 end
               end
               local.get 1))",
        );
        let func = &reg_module.funcs[0];
        // The loop body contains a `br 0` back-edge: a Br terminator whose
        // target is an earlier (header) block.
        let back_edge = func
            .blocks
            .iter()
            .enumerate()
            .find_map(|(idx, block)| match block.term {
                RegTerm::Br { target_block, .. } if (target_block as usize) < idx => {
                    Some(target_block)
                }
                _ => None,
            })
            .expect("expected a loop back-edge Br");
        // The header block is where the pre-loop block falls through to
        // (block 0 = pre-block, block 1 = pre-loop, block 2 = loop header).
        assert_eq!(back_edge, 2);
    }

    #[test]
    fn execute_loop_sum() {
        let source = "(module (func (param i32) (result i32)
            (local i32)
            block
              loop
                local.get 0
                i32.eqz
                br_if 1
                local.get 1
                local.get 0
                i32.add
                local.set 1
                local.get 0
                i32.const 1
                i32.sub
                local.set 0
                br 0
              end
            end
            local.get 1))";
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(5)]),
            vec![crate::runtime::Value::I32(15)]
        );
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(0)]),
            vec![crate::runtime::Value::I32(0)]
        );
    }

    #[test]
    fn lower_br_value_appends_copy_to_branch_block() {
        let reg_module = lower_wat(
            "(module (func (result i32)
               block (result i32)
                 i32.const 1
                 br 0
                 i32.const 2
               end))",
        );
        let func = &reg_module.funcs[0];
        // The block containing `br 0` must deliver its value into the
        // continuation's expected register via a Copy before branching.
        let br_block = func
            .blocks
            .iter()
            .find(|block| matches!(block.term, RegTerm::Br { .. }))
            .expect("expected a Br block");
        assert!(
            br_block
                .instrs
                .iter()
                .any(|instr| matches!(instr.op, RegOp::Copy { .. })),
            "expected a Copy instruction in the branch block: {br_block:?}"
        );
    }

    #[test]
    fn execute_br_value_delivers_branch_site_value() {
        let source = "(module (func (result i32)
            block (result i32)
              i32.const 1
              br 0
              i32.const 2
            end))";
        assert_eq!(run_wat(source, &[]), vec![crate::runtime::Value::I32(1)]);
    }

    #[test]
    fn execute_return_does_not_stop_lowering() {
        // Regression: `return` used to halt lowering, dropping the code
        // after the block's `end`.
        let source = "(module (func (param i32) (result i32)
            block
              local.get 0
              br_if 0
              i32.const 10
              return
            end
            i32.const 20))";
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(0)]),
            vec![crate::runtime::Value::I32(10)]
        );
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(1)]),
            vec![crate::runtime::Value::I32(20)]
        );
    }

    #[test]
    fn execute_unreachable_traps() {
        let reg_module = lower_wat("(module (func (result i32) unreachable))");
        let error = crate::runtime::execute_func(&reg_module.funcs[0], &[]).unwrap_err();
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(crate::runtime::RuntimeTrap::Unreachable)
        );
    }

    /// Execute an exported function with full module context (required for
    /// `call` instructions).
    fn run_wat_export(
        source: &str,
        name: &str,
        args: &[crate::runtime::Value],
    ) -> Result<Vec<crate::runtime::Value>, crate::runtime::RuntimeError> {
        let reg_module = lower_wat(source);
        let mut store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        crate::runtime::execute_export(&reg_module, &mut store, name, args)
    }

    #[test]
    fn lower_call_shape() {
        let reg_module = lower_wat(
            "(module
               (func $add (param i32 i32) (result i32)
                 local.get 0 local.get 1 i32.add)
               (func (export \"main\") (param i32 i32) (result i32)
                 local.get 0 local.get 1 call $add))",
        );
        let func = &reg_module.funcs[1];
        let call = func.blocks[0]
            .instrs
            .iter()
            .find_map(|instr| match &instr.op {
                RegOp::Call {
                    func,
                    args,
                    results,
                } => Some((func, args, results)),
                _ => None,
            })
            .expect("expected a Call op");
        assert_eq!(*call.0, FuncIdx(0));
        assert_eq!(call.1.as_slice(), &[Reg(0), Reg(1)]);
        assert_eq!(call.2.as_slice(), &[Reg(2)]);
    }

    #[test]
    fn execute_call_recursion_and_multi_result() {
        let fac = run_wat_export(
            "(module
               (func $fac (param i32) (result i32)
                 local.get 0
                 i32.const 2
                 i32.lt_s
                 if (result i32)
                   i32.const 1
                 else
                   local.get 0
                   local.get 0
                   i32.const 1
                   i32.sub
                   call $fac
                   i32.mul
                 end)
               (func (export \"fac\") (param i32) (result i32)
                 local.get 0
                 call $fac))",
            "fac",
            &[crate::runtime::Value::I32(5)],
        );
        assert_eq!(fac, Ok(vec![crate::runtime::Value::I32(120)]));

        let rem = run_wat_export(
            "(module
               (func $divmod (param i32 i32) (result i32 i32)
                 local.get 0 local.get 1 i32.div_u
                 local.get 0 local.get 1 i32.rem_u)
               (func (export \"rem\") (param i32 i32) (result i32)
                 (local i32)
                 local.get 0 local.get 1 call $divmod
                 local.set 2
                 drop
                 local.get 2))",
            "rem",
            &[
                crate::runtime::Value::I32(17),
                crate::runtime::Value::I32(5),
            ],
        );
        assert_eq!(rem, Ok(vec![crate::runtime::Value::I32(2)]));
    }

    #[test]
    fn execute_call_exhaustion_traps() {
        let error = run_wat_export(
            "(module (func $boom (export \"boom\") call $boom))",
            "boom",
            &[],
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(crate::runtime::RuntimeTrap::CallStackExhausted)
        );
    }

    #[test]
    fn execute_select_variants() {
        let source = "(module (func (param i32 i32 i32) (result i32)
            local.get 0
            local.get 1
            local.get 2
            select))";
        assert_eq!(
            run_wat(
                source,
                &[
                    crate::runtime::Value::I32(10),
                    crate::runtime::Value::I32(20),
                    crate::runtime::Value::I32(1),
                ],
            ),
            vec![crate::runtime::Value::I32(10)]
        );
        assert_eq!(
            run_wat(
                source,
                &[
                    crate::runtime::Value::I32(10),
                    crate::runtime::Value::I32(20),
                    crate::runtime::Value::I32(0),
                ],
            ),
            vec![crate::runtime::Value::I32(20)]
        );
    }

    #[test]
    fn lower_select_shape() {
        let reg_module = lower_wat(
            "(module (func (param i32 i32 i32) (result i32)
               local.get 0
               local.get 1
               local.get 2
               select))",
        );
        let func = &reg_module.funcs[0];
        assert!(func.blocks[0].instrs.iter().any(|instr| matches!(
            instr.op,
            RegOp::Select {
                dst: Reg(3),
                v1: Reg(0),
                v2: Reg(1),
                cond: Reg(2),
            }
        )));
    }

    #[test]
    fn lower_br_table_shape_and_patching() {
        let reg_module = lower_wat(
            "(module (func (param i32) (result i32)
               block (result i32)
                 block (result i32)
                   i32.const 0
                   local.get 0
                   br_table 0 1
                 end
                 i32.const 10
                 i32.add
                 br 0
               end))",
        );
        let func = &reg_module.funcs[0];
        let br_table_block = func
            .blocks
            .iter()
            .find(|block| matches!(block.term, RegTerm::BrTable { .. }))
            .expect("expected a BrTable block");
        let RegTerm::BrTable {
            targets, default, ..
        } = &br_table_block.term
        else {
            unreachable!()
        };
        // Both slots back-patched to real continuation blocks.
        assert_eq!(targets.len(), 1);
        assert_ne!(targets[0], 0);
        assert_ne!(*default, 0);
        assert_ne!(targets[0], *default);
        // The block carries copies delivering the branch value.
        assert!(
            br_table_block
                .instrs
                .iter()
                .any(|instr| matches!(instr.op, RegOp::Copy { .. }))
        );
    }

    #[test]
    fn execute_br_table_dispatch() {
        let source = "(module (func (param i32) (result i32)
            block (result i32)
              block (result i32)
                i32.const 0
                local.get 0
                br_table 0 1
              end
              i32.const 10
              i32.add
              br 0
            end))";
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(0)]),
            vec![crate::runtime::Value::I32(10)]
        );
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(1)]),
            vec![crate::runtime::Value::I32(0)]
        );
        // Out-of-range and negative indices take the default.
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(9)]),
            vec![crate::runtime::Value::I32(0)]
        );
        assert_eq!(
            run_wat(source, &[crate::runtime::Value::I32(-1)]),
            vec![crate::runtime::Value::I32(0)]
        );
    }

    #[test]
    fn lower_load_store_shape() {
        let reg_module = lower_wat(
            "(module
               (memory 1)
               (func (export \"f\") (param i32) (result i32)
                 local.get 0
                 local.get 0
                 i32.load offset=4
                 i32.store
                 i32.const 0))",
        );
        let func = &reg_module.funcs[0];
        let ops: Vec<&RegOp> = func.blocks[0]
            .instrs
            .iter()
            .map(|instr| &instr.op)
            .collect();
        assert!(matches!(
            ops[2],
            RegOp::Load {
                op: LoadOp::I32,
                memarg: MemArg { offset: 4, .. },
                ..
            }
        ));
        assert!(matches!(
            ops[3],
            RegOp::Store {
                op: StoreOp::I32,
                ..
            }
        ));
    }

    #[test]
    fn execute_memory_roundtrip_and_oob_trap() {
        let source = "(module
            (memory 1)
            (func (export \"roundtrip\") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              i32.store
              local.get 0
              i32.load)
            (func (export \"load\") (param i32) (result i32)
              local.get 0
              i32.load))";
        assert_eq!(
            run_wat_export(
                source,
                "roundtrip",
                &[
                    crate::runtime::Value::I32(8),
                    crate::runtime::Value::I32(-3)
                ],
            ),
            Ok(vec![crate::runtime::Value::I32(-3)])
        );
        // One page: address 65533 + 4-byte load is out of bounds.
        let error = run_wat_export(source, "load", &[crate::runtime::Value::I32(65533)])
            .expect_err("expected OOB trap");
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(
                crate::runtime::RuntimeTrap::OutOfBoundsMemoryAccess
            )
        );
    }

    #[test]
    fn execute_globals_and_data_segments() {
        let source = "(module
            (memory 1)
            (global $g (mut i32) (i32.const 10))
            (data (i32.const 4) \"\\2a\\00\\00\\00\")
            (func (export \"bump\") (result i32)
              global.get $g
              i32.const 1
              i32.add
              global.set $g
              global.get $g)
            (func (export \"load4\") (result i32)
              i32.const 4
              i32.load))";
        // Data segment wrote 42 at address 4 during instantiation.
        assert_eq!(
            run_wat_export(source, "load4", &[]),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
        assert_eq!(
            run_wat_export(source, "bump", &[]),
            Ok(vec![crate::runtime::Value::I32(11)])
        );
    }

    #[test]
    fn lower_call_indirect_shape() {
        let reg_module = lower_wat(
            "(module
               (type $t (func (param i32) (result i32)))
               (table 1 funcref)
               (func (export \"apply\") (param i32 i32) (result i32)
                 local.get 1
                 local.get 0
                 call_indirect (type $t)))",
        );
        let func = &reg_module.funcs[0];
        let call = func.blocks[0]
            .instrs
            .iter()
            .find_map(|instr| match &instr.op {
                RegOp::CallIndirect {
                    type_idx,
                    table,
                    args,
                    results,
                    ..
                } => Some((type_idx, table, args, results)),
                _ => None,
            })
            .expect("expected a CallIndirect op");
        assert_eq!(*call.0, TypeIdx(0));
        assert_eq!(*call.1, TableIdx(0));
        assert_eq!(call.2.as_slice(), &[Reg(0)]);
        assert_eq!(call.3.as_slice(), &[Reg(2)]);
    }

    #[test]
    fn execute_call_indirect_and_traps() {
        let source = "(module
            (type $t (func (result i32)))
            (table 2 funcref)
            (func $f (type $t) i32.const 42)
            (elem (i32.const 0) $f)
            (func (export \"go\") (param i32) (result i32)
              local.get 0
              call_indirect (type $t)))";
        assert_eq!(
            run_wat_export(source, "go", &[crate::runtime::Value::I32(0)]),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
        // Null slot.
        let error = run_wat_export(source, "go", &[crate::runtime::Value::I32(1)])
            .expect_err("expected uninitialized element trap");
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(
                crate::runtime::RuntimeTrap::UninitializedElement
            )
        );
        // Out of bounds.
        let error = run_wat_export(source, "go", &[crate::runtime::Value::I32(7)])
            .expect_err("expected undefined element trap");
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(crate::runtime::RuntimeTrap::UndefinedElement)
        );
    }

    #[test]
    fn execute_table_grow_fill_copy() {
        let source = "(module
            (type $t (func (result i32)))
            (table 1 funcref)
            (func $f (type $t) i32.const 9)
            (elem declare func $f)
            (func (export \"go\") (param i32) (result i32)
              local.get 0
              call_indirect (type $t))
            (func (export \"setup\") (result i32)
              ref.func $f
              i32.const 2
              table.grow
              drop
              i32.const 1
              ref.func $f
              i32.const 2
              table.fill
              i32.const 1
              i32.const 2
              i32.const 1
              table.copy
              table.size))";
        // After setup: [null, f, f] (grow 2, fill 2 from 1, copy [1..2] to 2).
        let reg_module = lower_wat(source);
        let mut store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        assert_eq!(
            crate::runtime::execute_export(&reg_module, &mut store, "setup", &[]),
            Ok(vec![crate::runtime::Value::I32(3)])
        );
        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &mut store,
                "go",
                &[crate::runtime::Value::I32(2)],
            ),
            Ok(vec![crate::runtime::Value::I32(9)])
        );
    }

    #[test]
    fn lower_loop_with_params_consumes_them_into_frame() {
        let reg_module = lower_wat(
            "(module (func (param i32 i32) (result i32)
               local.get 0
               local.get 1
               block (param i32 i32) (result i32)
                 i32.add
               end))",
        );
        let func = &reg_module.funcs[0];
        // The block body must be able to pop both params (they live in the
        // frame, not below it): i32.add lowers without underflow, and the
        // function returns the sum.
        assert_eq!(
            crate::runtime::execute_func(
                &func.clone(),
                &[
                    crate::runtime::Value::I32(30),
                    crate::runtime::Value::I32(12)
                ],
            ),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn lower_br_if_to_loop_with_params_uses_trampoline() {
        let reg_module = lower_wat(
            "(module (func (param i32) (result i32)
               (local $n i32)
               i32.const 0
               local.get 0
               loop (param i32 i32) (result i32)
                 local.set $n
                 local.get $n
                 i32.add
                 local.get $n
                 i32.const 1
                 i32.sub
                 local.tee $n
                 local.get $n
                 i32.const 1
                 i32.ge_s
                 br_if 0
                 drop
               end))",
        );
        let func = &reg_module.funcs[0];
        // The conditional back-edge must target a trampoline (not the loop
        // header directly); the trampoline carries the param copies.
        let trampoline = func.blocks.iter().find(|block| {
            block
                .instrs
                .iter()
                .any(|instr| matches!(instr.op, RegOp::Copy { .. }))
                && matches!(block.term, RegTerm::Br { .. })
        });
        assert!(
            trampoline.is_some(),
            "expected a trampoline block with param copies"
        );
        // The BrIf terminator must point at that trampoline.
        let br_if = func
            .blocks
            .iter()
            .find_map(|block| match &block.term {
                RegTerm::BrIf { target_block, .. } => Some(*target_block),
                _ => None,
            })
            .expect("expected a BrIf terminator");
        let trampoline_idx = func
            .blocks
            .iter()
            .position(|block| core::ptr::eq(block, trampoline.unwrap()))
            .unwrap() as u32;
        assert_eq!(br_if, trampoline_idx);
    }

    #[test]
    fn lower_else_without_if_is_an_error() {
        // (func i32.const 1 else end) — else outside an if frame.
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: [] -> []
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x05, 0x01, 0x03, 0x00, // one body, no locals
            0x05, // else
            0x0b, // end
        ];
        let module = Module::decode(&bytes).unwrap();
        // The validator rejects it; if it reaches lowering, lowering must
        // reject it too.
        let error = module.lower().unwrap_err();
        assert!(matches!(
            error.kind,
            LowerErrorKind::Validation(_) | LowerErrorKind::UnexpectedElse
        ));
    }
}
