// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Register-based lowering skeleton.
//!
//! Phase 2 starts by turning validated stack-machine functions into an
//! inspectable register-oriented IR. This module is deliberately small: it
//! establishes the execution-side vocabulary and lowers straight-line functions
//! before broader control-flow/runtime semantics are added.

use alloc::{string::String, vec::Vec};

use crate::binary::instr::{
    DecodedInstr, Instr, TryTableCatch, TryTableCatchKind, decode_instr_sequence_with_offsets,
};
use crate::binary::module::Module;
use crate::error::{ByteOffset, DecodeContext, DecodeErrorKind};
use crate::types::{
    BlockType, CodeBody, DataMode, ElemIdx, ElementInit, ElementMode, ExportDesc, FuncIdx,
    FuncType, GlobalIdx, HeapType, ImportDesc, LabelIdx, LocalDecl, LocalIdx, MemArg, MemIdx,
    MemType, Mutability, NumType, RefType, SubType, TableIdx, TableType, TagIdx, TypeIdx, ValType,
};
use crate::validate;
use crate::validate::error::{ValidationError, ValidationErrorKind};

/// A virtual register in lowered Baedeker IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Reg(pub u32);

/// A typed value currently on the lowering stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegValue {
    pub reg: Reg,
    pub ty: ValType,
}

/// A validated module lowered into register IR.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegModule {
    pub funcs: Vec<RegFunc>,
    pub exports: Vec<RegExport>,
    /// Number of imported functions: `FuncIdx` values below this are not
    /// lowered and cannot be called by the interpreter yet.
    pub imported_func_count: u32,
    /// Function import declarations in index order (for host-function
    /// registration and lazy import resolution).
    pub imported_funcs: Vec<RegImport>,
    /// Memory import declarations in index order.
    pub imported_memories: Vec<RegMemoryImport>,
    /// Global import declarations in index order.
    pub imported_globals: Vec<RegGlobalImport>,
    /// Table import declarations in index order.
    pub imported_tables: Vec<RegTableImport>,
    /// The start function, if the module declares one.
    pub start: Option<FuncIdx>,
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
    /// The module's full type section: kind-preserving (struct/array/func)
    /// definitions used for cross-module structural comparison of concrete
    /// reference types at link time. `types` is the function-only view of
    /// this section.
    pub type_defs: Vec<SubType>,
    /// All data segments in index order; mode decides instantiation behavior.
    pub data: Vec<RegDataSegment>,
    /// Number of imported memories (runtime access is not yet supported).
    pub imported_memory_count: u32,
    /// Number of imported globals (runtime access is not yet supported).
    pub imported_global_count: u32,
    /// Number of imported tables (runtime access is not yet supported).
    pub imported_table_count: u32,
    /// The module's defined-tag type indices, mirroring `Module.tags`.
    /// Imported tag types are resolved at instantiation (U4).
    pub tags: Vec<TypeIdx>,
    /// Tag import declarations in index order.
    pub imported_tags: Vec<ImportedTag>,
    /// Extents of `rec` type-group entries (start, len) mirroring the
    /// decoded module's type section — the provenance source for
    /// defined-tag link compatibility.
    pub rec_groups: Vec<(u32, u32)>,
}

/// An element segment in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegElement {
    pub mode: RegElementMode,
    /// Element values (funcref indices or nulls), in order.
    pub values: Vec<RegElemValue>,
}

/// Instantiation behavior of an element segment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RegElemValue {
    FuncRef(FuncIdx),
    /// A `global.get` of a funcref global, resolved at instantiation.
    GlobalGet(GlobalIdx),
    Null,
    /// Any other const expression permitted by `is_const` (GC allocation
    /// forms, conversions, multi-instruction expressions). Evaluated against
    /// the heap at instantiation, with the same allocation discipline as a
    /// global initializer.
    Expr(Vec<RegConstInstr>),
}

/// A defined global in lowered register IR.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegGlobal {
    pub ty: ValType,
    pub mutable: bool,
    /// Const initializer, evaluated at instantiation.
    pub init: Vec<RegConstInstr>,
}

/// A data segment in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegDataSegment {
    pub mode: RegDataMode,
    pub bytes: Vec<u8>,
}

/// Instantiation behavior of a data segment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RegDataMode {
    /// Written into memory at instantiation, then dropped.
    Active {
        memory: MemIdx,
        offset: Vec<RegConstInstr>,
    },
    /// Retained for `memory.init` until dropped.
    Passive,
}

/// An instruction in a lowered constant expression (global initializers,
/// data segment offsets). Supports the const instrs plus the
/// extended-const integer arithmetic the validator accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RegConstInstr {
    I32Const(i32),
    I64Const(i64),
    F32Const(u32),
    F64Const(u64),
    V128Const([u8; 16]),
    GlobalGet(GlobalIdx),
    /// `ref.null` with the funcref shorthand — evaluates to a null
    /// funcref.
    RefNull,
    RefFunc(FuncIdx),
    I32Add,
    I32Sub,
    I32Mul,
    I64Add,
    I64Sub,
    I64Mul,
    /// `ref.null rt` for non-funcref reference types — carries the type
    /// so the evaluator constructs the correctly-typed null (externref,
    /// exnref, typed refs).
    RefNullOf(RefType),
    /// `ref.null ht` with a GC abstract heap type (`any`, `eq`, `i31`,
    /// `struct`, `array`, `none`, `nofunc`, `noextern`, `noexn`). The
    /// evaluator constructs the correctly-typed null reference.
    RefNullGC(HeapType),
    /// `ref.i31`: box the `i32` on the stack into a 31-bit immediate.
    RefI31,
    /// `struct.new $x` (`default = false`) or `struct.new_default $x`
    /// (`default = true`). The explicit form pops one value per field; the
    /// default form pops nothing and fills every field with its default.
    /// Both forms are const-eligible (`valid.ml:1029-1040` wildcards the
    /// `initop`).
    StructNew {
        type_idx: TypeIdx,
        default: bool,
    },
    /// `array.new $x` (`default = false`) or `array.new_default $x`
    /// (`default = true`). Both forms pop the length; the explicit form also
    /// pops the fill value. Both are const-eligible (pin wildcard).
    ArrayNew {
        type_idx: TypeIdx,
        default: bool,
    },
    /// `array.new_fixed $x $n`: pops `count` element values, first operand
    /// lowest index.
    ArrayNewFixed {
        type_idx: TypeIdx,
        count: u32,
    },
    /// `any.convert_extern` / `extern.convert_any`: an immediate value-level
    /// re-tag, never an allocation.
    ExternConvert {
        direction: ExternConvert,
    },
}

/// Direction of the extern/any conversion const forms (`externtype`-level
/// re-tags, `eval.ml:929-939`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExternConvert {
    /// `extern.convert_any`: any-side reference → extern-side reference.
    AnyToExtern,
    /// `any.convert_extern`: extern-side reference → any-side reference.
    ExternToAny,
}

/// A function import declaration in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegImport {
    pub module: String,
    pub name: String,
    pub ty: FuncType,
    /// The type index the import was declared with. Import matching compares
    /// the provider's function DEftype against this declaration's deftype
    /// (`match_externtype` → `match_deftype`), so canonical identity of the
    /// whole type entry (rec-group shape, finality, supertypes) participates —
    /// a params/results-only comparison would conflate `(rec (type (func))
    /// (type (struct)))` with `(rec (type (struct)) (type (func)))`.
    pub type_idx: TypeIdx,
}

/// A memory import declaration in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegMemoryImport {
    pub module: String,
    pub name: String,
    pub ty: MemType,
}

/// A global import declaration in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegGlobalImport {
    pub module: String,
    pub name: String,
    pub ty: crate::types::GlobalType,
}

/// A table import declaration in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegTableImport {
    pub module: String,
    pub name: String,
    pub ty: TableType,
}

/// A tag import declared by the lowered module (state-import style).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImportedTag {
    pub module: String,
    pub name: String,
    /// The tag's function type (params; results are empty by validation).
    pub ty: FuncType,
    /// The type index the import was declared with. Retained so link-time
    /// comparison can recompute the tag's OWN referenced function type's
    /// complete closed form (finality + supertypes + comptype) from the
    /// serialized type section exactly as the parameter fix does — a
    /// property the params-only `FuncType` cannot express.
    pub type_idx: TypeIdx,
    /// Rec-group provenance of the tag's type: the group's member types
    /// (structural signature) and the tag type's position in the group.
    /// `None` for bare (non-rec) types. Two tag types are link-compatible
    /// only when both are bare, or both come from structurally identical
    /// groups at the same position (iso-recursive equality, flattened).
    pub rec: Option<(Vec<SubType>, u32)>,
}

/// A function export in lowered register IR.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegExport {
    pub name: String,
    pub desc: RegExportDesc,
}

/// What kind of item an export refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RegExportDesc {
    Func(FuncIdx),
    Table(TableIdx),
    Mem(MemIdx),
    Global(GlobalIdx),
    /// Exported exception tag (placeholder; lowering lands with U4).
    Tag(TagIdx),
}

/// A defined function lowered into register IR.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// Static protected regions produced by lowering `try_table` bodies.
    pub regions: Vec<ProtectedRegion>,
}

/// A basic block in the lowered register IR.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegBlock {
    pub label: LabelIdx,
    pub instrs: Vec<RegInstr>,
    pub term: RegTerm,
}

/// A block terminator — how execution leaves this block.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// `br_on_null`: branch when the reference value is null.
    BrIfNull {
        value: Reg,
        target_block: u32,
        values: Vec<Reg>,
    },
    /// `br_on_non_null`: branch when the reference value is non-null,
    /// forwarding it (as the last branch value) to the target.
    BrIfNonNull {
        value: Reg,
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
    /// Tail call (`return_call`): replace the current frame with the callee
    /// at the same stack depth. Terminates the block like `Return`.
    TailCall { func: FuncIdx, args: Vec<Reg> },
    /// Tail call through a table (`return_call_indirect`).
    TailCallIndirect {
        type_idx: TypeIdx,
        table: TableIdx,
        index: Reg,
        args: Vec<Reg>,
    },
    /// Tail call through a function reference (`return_call_ref`).
    TailCallRef {
        type_idx: TypeIdx,
        func: Reg,
        args: Vec<Reg>,
    },
    /// Unconditional trap (`unreachable`).
    Trap,
    /// `throw` — raise the module-local tag with the payload registers.
    Throw { tag: TagIdx, values: Vec<Reg> },
    /// `throw_ref` — re-raise the exception held in the exnref register.
    ThrowRef { exn: Reg },
}

/// A static protected region produced by lowering one `try_table`.
/// `first_block..end_block` (exclusive) is the contiguous block range of
/// the try body; control outside that range is not protected by this
/// region even if it dynamically executed inside the try earlier.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProtectedRegion {
    pub first_block: u32,
    pub end_block: u32,
    pub catches: Vec<RegCatch>,
}

/// One catch clause, lowered. On a match the runtime writes the payload
/// values (plus the exnref, for `_ref` clauses, as the LAST value) into
/// `dst_regs` and enters `target_block`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegCatch {
    /// `None` = catch_all. A module-local tag index (same module as the
    /// region's function).
    pub tag: Option<TagIdx>,
    pub target_block: u32,
    /// Registers the landing writes: tag params in order, then the
    /// exnref register for `catch_ref`/`catch_all_ref`.
    pub dst_regs: Vec<Reg>,
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegInstr {
    pub offset: ByteOffset,
    pub op: RegOp,
}

/// Register-oriented operations.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// v128.loadN_lane: read N bytes into `lane` of the operand vector.
    V128LoadLane {
        width: u8,
        dst: Reg,
        vec: Reg,
        addr: Reg,
        memarg: MemArg,
        lane: u8,
    },
    /// v128.storeN_lane: write `lane`'s low N bytes to memory.
    V128StoreLane {
        width: u8,
        addr: Reg,
        vec: Reg,
        memarg: MemArg,
        lane: u8,
    },
    /// v128 constant.
    V128Const {
        dst: Reg,
        value: [u8; 16],
    },
    /// Broadcast a scalar into every lane.
    V128Splat {
        dst: Reg,
        shape: LaneShape,
        src: Reg,
    },
    /// Extract one lane as a scalar.
    V128ExtractLane {
        dst: Reg,
        shape: LaneShape,
        src: Reg,
        lane: u8,
        signed: bool,
    },
    /// Replace one lane of a vector with a scalar.
    V128ReplaceLane {
        dst: Reg,
        shape: LaneShape,
        vec: Reg,
        scalar: Reg,
        lane: u8,
    },
    /// Lane-wise binary operation (add/sub/mul/div and bitwise and/or/xor).
    V128Binary {
        shape: LaneShape,
        kind: V128BinaryKind,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    /// Bitwise not over all 128 bits.
    V128Not {
        dst: Reg,
        src: Reg,
    },
    /// Lane-wise integer unary (abs/neg/popcnt).
    V128Unary {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
        kind: V128UnaryKind,
    },
    /// Widen the low (`high == false`) or high half of `src`'s lanes to
    /// double width; dst shape is implied by `src_shape`.
    V128Extend {
        dst: Reg,
        src: Reg,
        src_shape: LaneShape,
        high: bool,
        signed: bool,
    },
    /// Extended multiply: widen both operands' selected half, then
    /// multiply lane-wise in the wider type.
    V128ExtMul {
        dst: Reg,
        a: Reg,
        b: Reg,
        src_shape: LaneShape,
        high: bool,
        signed: bool,
    },
    /// Pairwise saturating-free sum of adjacent lanes, doubling width.
    V128ExtAddPairwise {
        dst: Reg,
        src: Reg,
        src_shape: LaneShape,
        signed: bool,
    },
    /// Saturating-narrow `a` into the dst low half and `b` into the dst
    /// high half, halving width. `_u` clamps negative lanes to 0.
    V128Narrow {
        dst: Reg,
        a: Reg,
        b: Reg,
        dst_shape: LaneShape,
        signed: bool,
    },
    /// i32x4.dot_i16x8_s: signed dot of adjacent i16 pairs, wrapping.
    V128Dot {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// Per-bit select: mask bit 1 → a, 0 → b.
    V128Bitselect {
        dst: Reg,
        a: Reg,
        b: Reg,
        mask: Reg,
    },
    /// i32 1 if any bit in the vector is nonzero, else 0.
    V128AnyTrue {
        dst: Reg,
        src: Reg,
    },
    /// i32 1 if every lane (per `shape`) is nonzero, else 0.
    V128AllTrue {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
    },
    /// i32 with bit i = MSB of lane i (per `shape`).
    V128Bitmask {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
    },
    /// Lane-wise compare; dst lanes are all-ones/all-zeros at the
    /// operand lane width.
    V128Compare {
        dst: Reg,
        a: Reg,
        b: Reg,
        shape: LaneShape,
        kind: V128CompareKind,
    },
    /// Lane-wise shift by an i32 scalar `amount`, taken modulo the lane
    /// bit width (spec semantics; e.g. i8x16.shl by 9 == by 1).
    V128Shift {
        dst: Reg,
        src: Reg,
        amount: Reg,
        shape: LaneShape,
        kind: V128ShiftKind,
    },
    /// Lane-wise float unary (abs/neg/sqrt/ceil/floor/trunc/nearest).
    V128FloatUnary {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
        kind: V128FloatUnaryKind,
    },
    /// Shape-changing conversion per `kind`.
    V128Convert {
        dst: Reg,
        src: Reg,
        kind: V128ConvertKind,
    },
    /// result[i] = b[i] < 16 ? a[b[i]] : 0.
    V128Swizzle {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// result[i] = lanes[i] < 16 ? a[lanes[i]] : b[lanes[i] - 16].
    V128Shuffle {
        dst: Reg,
        a: Reg,
        b: Reg,
        lanes: [u8; 16],
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
    /// `memory.init`: mem[dst..] = data_segment[src..] over count bytes.
    MemoryInit {
        memory: MemIdx,
        data: crate::types::DataIdx,
        dst: Reg,
        src: Reg,
        count: Reg,
    },
    /// `data.drop`: drop the data segment's runtime storage.
    DataDrop {
        data: crate::types::DataIdx,
    },
    /// `memory.copy`: dst_mem[dst..] = src_mem[src..] over count bytes.
    MemoryCopy {
        dst_memory: MemIdx,
        src_memory: MemIdx,
        dst: Reg,
        src: Reg,
        count: Reg,
    },
    /// `memory.fill`: mem[dst..dst+count] = value (low byte).
    MemoryFill {
        memory: MemIdx,
        dst: Reg,
        value: Reg,
        count: Reg,
    },
    /// Indirect call through a table (`call_indirect`).
    CallIndirect {
        type_idx: TypeIdx,
        table: TableIdx,
        index: Reg,
        args: Vec<Reg>,
        results: Vec<Reg>,
    },
    /// Direct call through a function reference (`call_ref`).
    CallRef {
        type_idx: TypeIdx,
        func: Reg,
        args: Vec<Reg>,
        results: Vec<Reg>,
    },
    /// `table.get`: dst = `table[index]`.
    TableGet {
        dst: Reg,
        table: TableIdx,
        index: Reg,
    },
    /// `table.set`: `table[index]` = value.
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
    /// `ref.null`: produce a null reference of the given reference type.
    RefNull {
        dst: Reg,
        ref_type: RefType,
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
    /// `ref.as_non_null`: trap on null, otherwise copy the reference.
    RefAsNonNull {
        dst: Reg,
        value: Reg,
    },
    /// `struct.new $x`: allocate a struct from `fields` (declaration order).
    StructNew {
        dst: Reg,
        type_idx: TypeIdx,
        fields: Vec<Reg>,
    },
    /// `struct.new_default $x`: allocate a struct with default field values.
    StructNewDefault {
        dst: Reg,
        type_idx: TypeIdx,
    },
    /// `struct.get{,s,u} $x $i`.
    StructGet {
        dst: Reg,
        type_idx: TypeIdx,
        field: u32,
        ext: GcExt,
        object: Reg,
    },
    /// `struct.set $x $i`.
    StructSet {
        type_idx: TypeIdx,
        field: u32,
        object: Reg,
        value: Reg,
    },
    /// `array.new $x`: `len` copies of `value`.
    ArrayNew {
        dst: Reg,
        type_idx: TypeIdx,
        len: Reg,
        value: Reg,
    },
    /// `array.new_default $x`.
    ArrayNewDefault {
        dst: Reg,
        type_idx: TypeIdx,
        len: Reg,
    },
    /// `array.new_fixed $x $n`.
    ArrayNewFixed {
        dst: Reg,
        type_idx: TypeIdx,
        values: Vec<Reg>,
    },
    /// `array.new_data $x $y`.
    ArrayNewData {
        dst: Reg,
        type_idx: TypeIdx,
        data: crate::types::DataIdx,
        src: Reg,
        len: Reg,
    },
    /// `array.new_elem $x $y`.
    ArrayNewElem {
        dst: Reg,
        type_idx: TypeIdx,
        elem: ElemIdx,
        src: Reg,
        len: Reg,
    },
    /// `array.get{,s,u} $x`.
    ArrayGet {
        dst: Reg,
        type_idx: TypeIdx,
        array: Reg,
        index: Reg,
        ext: GcExt,
    },
    /// `array.set $x`.
    ArraySet {
        type_idx: TypeIdx,
        array: Reg,
        index: Reg,
        value: Reg,
    },
    /// `array.len`.
    ArrayLen {
        dst: Reg,
        array: Reg,
    },
    /// `array.fill $x`.
    ArrayFill {
        type_idx: TypeIdx,
        array: Reg,
        index: Reg,
        value: Reg,
        len: Reg,
    },
    /// `array.copy $x $y`.
    ArrayCopy {
        dst_type: TypeIdx,
        src_type: TypeIdx,
        dst: Reg,
        dst_off: Reg,
        src: Reg,
        src_off: Reg,
        len: Reg,
    },
    /// `array.init_data $x $y`.
    ArrayInitData {
        type_idx: TypeIdx,
        data: crate::types::DataIdx,
        array: Reg,
        dst_off: Reg,
        src_off: Reg,
        len: Reg,
    },
    /// `array.init_elem $x $y`.
    ArrayInitElem {
        type_idx: TypeIdx,
        elem: ElemIdx,
        array: Reg,
        dst_off: Reg,
        src_off: Reg,
        len: Reg,
    },
    /// `ref.i31`: box an i32 into a 31-bit immediate.
    RefI31 {
        dst: Reg,
        value: Reg,
    },
    /// `i31.get_s`/`i31.get_u`.
    I31Get {
        dst: Reg,
        value: Reg,
        signed: bool,
    },
    /// `ref.test (ref null? ht)`: dst = 1 when `value` matches the target.
    RefTest {
        dst: Reg,
        value: Reg,
        nullable: bool,
        ht: HeapType,
    },
    /// `ref.cast (ref null? ht)`: dst = `value` when it matches, else trap.
    RefCast {
        dst: Reg,
        value: Reg,
        nullable: bool,
        ht: HeapType,
    },
    /// `ref.eq`: dst = 1 when the two eq references are equal.
    RefEq {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    /// `any.convert_extern`: re-tag an extern reference as an any reference.
    AnyConvertExtern {
        dst: Reg,
        value: Reg,
    },
    /// `extern.convert_any`: re-tag an any reference as an extern reference.
    ExternConvertAny {
        dst: Reg,
        value: Reg,
    },
}

/// The packed-field extension marker carried in lowered GC `get` ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GcExt {
    /// Unpacked read (`struct.get` / `array.get`).
    None,
    /// Sign-extended packed read.
    S,
    /// Zero-extended packed read.
    U,
}

/// SIMD lane shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LaneShape {
    I8x16,
    I16x8,
    I32x4,
    I64x2,
    F32x4,
    F64x2,
}

impl LaneShape {
    /// Bytes per lane.
    pub fn lane_width(self) -> usize {
        match self {
            LaneShape::I8x16 => 1,
            LaneShape::I16x8 => 2,
            LaneShape::I32x4 | LaneShape::F32x4 => 4,
            LaneShape::I64x2 | LaneShape::F64x2 => 8,
        }
    }

    /// Scalar type of one lane.
    pub fn scalar_type(self) -> ValType {
        match self {
            LaneShape::I8x16 | LaneShape::I16x8 | LaneShape::I32x4 => ValType::Num(NumType::I32),
            LaneShape::I64x2 => ValType::Num(NumType::I64),
            LaneShape::F32x4 => ValType::Num(NumType::F32),
            LaneShape::F64x2 => ValType::Num(NumType::F64),
        }
    }
}

/// Lane-wise binary operation kinds. Bitwise kinds ignore lane shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum V128BinaryKind {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Xor,
    Andnot,
    AddSatS,
    AddSatU,
    SubSatS,
    SubSatU,
    MinS,
    MinU,
    MaxS,
    MaxU,
    AvgrU,
    Q15MulrSatS,
    FMin,
    FMax,
    PMin,
    PMax,
}

/// Integer lane-wise unary kinds (`f32x4`/`f64x2` abs/neg are separate
/// float ops and are NOT part of this unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum V128UnaryKind {
    Abs,
    Neg,
    Popcnt,
}

/// Lane-wise float unary kinds. `nearest` is round-half-to-EVEN (NOT
/// `f64::round`). abs/neg are bit operations on the sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum V128FloatUnaryKind {
    Abs,
    Neg,
    Sqrt,
    Ceil,
    Floor,
    Trunc,
    Nearest,
}

/// Shape-changing lane conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum V128ConvertKind {
    TruncSatF32x4S,
    TruncSatF32x4U,
    ConvertI32x4S,
    ConvertI32x4U,
    TruncSatF64x2SZero,
    TruncSatF64x2UZero,
    DemoteF64x2Zero,
    PromoteLowF32x4,
    ConvertLowI32x4S,
    ConvertLowI32x4U,
}

/// Lane-wise compare kinds. Integer shapes use the s/u variants; float
/// shapes use the plain variants. Result lanes are all-ones/all-zeros at
/// the OPERAND lane width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum V128CompareKind {
    Eq,
    Ne,
    LtS,
    LtU,
    GtS,
    GtU,
    LeS,
    LeU,
    GeS,
    GeU,
    Lt,
    Gt,
    Le,
    Ge,
}

/// Lane-wise shift kinds (`shr_s` is arithmetic on the signed lane
/// view; `shr_u` is logical on the unsigned view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum V128ShiftKind {
    Shl,
    ShrS,
    ShrU,
}

/// Linear-memory load operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    V128,
    V128Load8x8S,
    V128Load8x8U,
    V128Load16x4S,
    V128Load16x4U,
    V128Load32x2S,
    V128Load32x2U,
    V128Load8Splat,
    V128Load16Splat,
    V128Load32Splat,
    V128Load64Splat,
    V128Load32Zero,
    V128Load64Zero,
}

impl LoadOp {
    /// Bytes read from memory.
    pub fn byte_width(self) -> usize {
        match self {
            LoadOp::V128 => 16,
            LoadOp::V128Load8x8S | LoadOp::V128Load8x8U => 8,
            LoadOp::V128Load16x4S | LoadOp::V128Load16x4U => 8,
            LoadOp::V128Load32x2S | LoadOp::V128Load32x2U => 8,
            LoadOp::V128Load8Splat => 1,
            LoadOp::V128Load16Splat => 2,
            LoadOp::V128Load32Splat | LoadOp::V128Load32Zero => 4,
            LoadOp::V128Load64Splat | LoadOp::V128Load64Zero => 8,
            LoadOp::I32 | LoadOp::F32 | LoadOp::I64Load32S | LoadOp::I64Load32U => 4,
            LoadOp::I64 | LoadOp::F64 => 8,
            LoadOp::I32Load8S | LoadOp::I32Load8U | LoadOp::I64Load8S | LoadOp::I64Load8U => 1,
            LoadOp::I32Load16S | LoadOp::I32Load16U | LoadOp::I64Load16S | LoadOp::I64Load16U => 2,
        }
    }

    /// Value type produced by the load.
    pub fn result_type(self) -> ValType {
        match self {
            LoadOp::V128
            | LoadOp::V128Load8x8S
            | LoadOp::V128Load8x8U
            | LoadOp::V128Load16x4S
            | LoadOp::V128Load16x4U
            | LoadOp::V128Load32x2S
            | LoadOp::V128Load32x2U
            | LoadOp::V128Load8Splat
            | LoadOp::V128Load16Splat
            | LoadOp::V128Load32Splat
            | LoadOp::V128Load64Splat
            | LoadOp::V128Load32Zero
            | LoadOp::V128Load64Zero => ValType::Vec(crate::types::VecType::V128),
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    V128,
}

impl StoreOp {
    /// Bytes written to memory.
    pub fn byte_width(self) -> usize {
        match self {
            StoreOp::V128 => 16,
            StoreOp::I32 | StoreOp::F32 | StoreOp::I64Store32 => 4,
            StoreOp::I64 | StoreOp::F64 => 8,
            StoreOp::I32Store8 | StoreOp::I64Store8 => 1,
            StoreOp::I32Store16 | StoreOp::I64Store16 => 2,
        }
    }

    /// Value type consumed by the store.
    pub fn value_type(self) -> ValType {
        match self {
            StoreOp::V128 => ValType::Vec(crate::types::VecType::V128),
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    F32Copysign,
    F64Copysign,
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
            BinaryOp::F32Copysign => "f32.copysign",
            BinaryOp::F64Copysign => "f64.copysign",
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
            | BinaryOp::F32Copysign
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
            | BinaryOp::F64Copysign
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
            | BinaryOp::F32Copysign
            | BinaryOp::F32Min
            | BinaryOp::F32Max => ValType::Num(NumType::F32),
            BinaryOp::F64Add
            | BinaryOp::F64Sub
            | BinaryOp::F64Mul
            | BinaryOp::F64Div
            | BinaryOp::F64Copysign
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

impl LowerError {
    /// Map a decode failure to a lowering error.
    ///
    /// A refused allocation while decoding is not a malformed-input error: it
    /// surfaces as the shared [`ValidationErrorKind::ResourceLimitExceeded`]
    /// refusal so the whole validate + lower path reports one resource-limit
    /// idiom (resource discipline, never an allocation abort).
    fn from_decode(error: crate::error::DecodeError, function: Option<FuncIdx>) -> Self {
        let kind = match error.kind {
            DecodeErrorKind::ResourceLimitExceeded => {
                LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded {
                    what: "decoded instructions",
                })
            }
            kind => LowerErrorKind::Decode {
                context: error.context,
                kind,
            },
        };
        Self {
            offset: error.offset,
            function,
            kind,
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
    let func_types = func_type_table(module)?;
    let global_types = global_type_table(module)?;
    let tag_types = tag_type_table(module)?;
    let mut table_elem_types: Vec<crate::types::RefType> = vec_with_capacity(
        module.imports().len() + module.tables().len(),
        ByteOffset(0),
        None,
        "table element types",
    )?;
    for import in module.imports() {
        if let ImportDesc::Table(table) = &import.desc {
            push_fallible(
                &mut table_elem_types,
                table.elem,
                ByteOffset(0),
                None,
                "table element types",
            )?;
        }
    }
    for table in module.tables() {
        push_fallible(
            &mut table_elem_types,
            table.elem,
            ByteOffset(0),
            None,
            "table element types",
        )?;
    }
    let mut memory_addr_types: Vec<NumType> = vec_with_capacity(
        module.imports().len() + module.memories().len(),
        ByteOffset(0),
        None,
        "memory address types",
    )?;
    for import in module.imports() {
        if let ImportDesc::Mem(ty) = &import.desc {
            push_fallible(
                &mut memory_addr_types,
                addr_num_type(&ty.limits),
                ByteOffset(0),
                None,
                "memory address types",
            )?;
        }
    }
    for ty in module.memories() {
        push_fallible(
            &mut memory_addr_types,
            addr_num_type(&ty.limits),
            ByteOffset(0),
            None,
            "memory address types",
        )?;
    }
    let mut table_addr_types: Vec<NumType> = vec_with_capacity(
        module.imports().len() + module.tables().len(),
        ByteOffset(0),
        None,
        "table address types",
    )?;
    for import in module.imports() {
        if let ImportDesc::Table(ty) = &import.desc {
            push_fallible(
                &mut table_addr_types,
                addr_num_type(&ty.limits),
                ByteOffset(0),
                None,
                "table address types",
            )?;
        }
    }
    for ty in module.tables() {
        push_fallible(
            &mut table_addr_types,
            addr_num_type(&ty.limits),
            ByteOffset(0),
            None,
            "table address types",
        )?;
    }
    let tables = ModuleTables {
        func_types: &func_types,
        global_types: &global_types,
        types: module.types(),
        table_elem_types: &table_elem_types,
        memory_addr_types: &memory_addr_types,
        table_addr_types: &table_addr_types,
        tag_types: &tag_types,
    };
    let mut funcs = Vec::new();

    for (defined_idx, (type_idx, code)) in module.functions().iter().zip(module.codes()).enumerate()
    {
        let func_idx = FuncIdx(imported_func_count + defined_idx as u32);
        let ty = module.func_type(*type_idx).ok_or(LowerError {
            offset: ByteOffset(0),
            function: None,
            kind: LowerErrorKind::InvalidType {
                type_idx: type_idx.0,
            },
        })?;
        let reg_func = lower_function(func_idx, *type_idx, ty, code, &tables)?;
        if funcs.try_reserve(1).is_err() {
            return Err(LowerError {
                offset: ByteOffset(0),
                function: None,
                kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded {
                    what: "lowered functions",
                }),
            });
        }
        funcs.push(reg_func);
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

    let memories = copy_slice_fallible(module.memories(), "memory types")?;

    let mut globals = vec_with_capacity(
        module.globals().len(),
        ByteOffset(0),
        None,
        "global definitions",
    )?;
    for global in module.globals() {
        push_fallible(
            &mut globals,
            RegGlobal {
                ty: global.global_type.val_type,
                mutable: global.global_type.mutability == Mutability::Var,
                init: lower_const_expr(global.init_expr, global.init_offset)?,
            },
            ByteOffset(0),
            None,
            "global definitions",
        )?;
    }

    let mut data = Vec::new();
    for segment in module.data() {
        let mode = match &segment.mode {
            DataMode::Active {
                memory,
                offset_expr,
                offset_offset,
            } => RegDataMode::Active {
                memory: *memory,
                offset: lower_const_expr(offset_expr, *offset_offset)?,
            },
            DataMode::Passive => RegDataMode::Passive,
        };
        push_fallible(
            &mut data,
            RegDataSegment {
                mode,
                bytes: copy_slice_fallible(segment.init, "data segment bytes")?,
            },
            ByteOffset(0),
            None,
            "data segments",
        )?;
    }

    let imported_table_count = module
        .imports()
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Table(_)))
        .count() as u32;

    let imported_funcs = collect_imports(module, |import| match import.desc {
        ImportDesc::Func(type_idx) => match module.func_type(type_idx) {
            Some(ty) => Ok(Some(RegImport {
                module: clone_string_fallible(&import.module, "import module name")?,
                name: clone_string_fallible(&import.name, "import name")?,
                ty: clone_func_type_fallible(ty)?,
                type_idx,
            })),
            // Validation rejects unknown type indices, so this arm is
            // defensive only: preserve the historical skip-the-import
            // behaviour rather than inventing a new error.
            None => Ok(None),
        },
        _ => Ok(None),
    })?;

    let imported_memories = collect_imports(module, |import| match import.desc {
        ImportDesc::Mem(ty) => Ok(Some(RegMemoryImport {
            module: clone_string_fallible(&import.module, "import module name")?,
            name: clone_string_fallible(&import.name, "import name")?,
            ty,
        })),
        _ => Ok(None),
    })?;

    let imported_globals = collect_imports(module, |import| match import.desc {
        ImportDesc::Global(ty) => Ok(Some(RegGlobalImport {
            module: clone_string_fallible(&import.module, "import module name")?,
            name: clone_string_fallible(&import.name, "import name")?,
            ty,
        })),
        _ => Ok(None),
    })?;

    let imported_tables = collect_imports(module, |import| match &import.desc {
        ImportDesc::Table(ty) => Ok(Some(RegTableImport {
            module: clone_string_fallible(&import.module, "import module name")?,
            name: clone_string_fallible(&import.name, "import name")?,
            ty: clone_table_type_fallible(ty)?,
        })),
        _ => Ok(None),
    })?;

    let imported_tags = collect_imports(module, |import| match import.desc {
        ImportDesc::Tag(type_idx) => match module.func_type(type_idx) {
            Some(ty) => Ok(Some(ImportedTag {
                module: clone_string_fallible(&import.module, "import module name")?,
                name: clone_string_fallible(&import.name, "import name")?,
                ty: clone_func_type_fallible(ty)?,
                type_idx,
                rec: rec_provenance(module, type_idx)?,
            })),
            None => Ok(None),
        },
        _ => Ok(None),
    })?;

    let start = module.start();

    let tables = copy_slice_fallible(module.tables(), "table types")?;
    let type_defs = reg_type_table(module)?;
    let types = func_type_view(&type_defs)?;

    let mut elements = vec_with_capacity(
        module.elements().len(),
        ByteOffset(0),
        None,
        "element segments",
    )?;
    for segment in module.elements() {
        let mut values = Vec::new();
        match &segment.init {
            ElementInit::FuncIndices(funcs) => {
                values.try_reserve(funcs.len()).map_err(|_| LowerError {
                    offset: ByteOffset(0),
                    function: None,
                    kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded {
                        what: "element initializer values",
                    }),
                })?;
                values.extend(funcs.iter().map(|func| RegElemValue::FuncRef(*func)));
            }
            ElementInit::Expressions(exprs) => {
                for expr in exprs {
                    push_fallible(
                        &mut values,
                        lower_element_expr(expr)?,
                        ByteOffset(0),
                        None,
                        "element initializer values",
                    )?;
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
        push_fallible(
            &mut elements,
            RegElement { mode, values },
            ByteOffset(0),
            None,
            "element segments",
        )?;
    }

    let mut exports = vec_with_capacity(
        module.exports().len(),
        ByteOffset(0),
        None,
        "module exports",
    )?;
    for export in module.exports() {
        push_fallible(
            &mut exports,
            RegExport {
                name: clone_string_fallible(&export.name, "export name")?,
                desc: match export.desc {
                    ExportDesc::Func(idx) => RegExportDesc::Func(idx),
                    ExportDesc::Table(idx) => RegExportDesc::Table(idx),
                    ExportDesc::Mem(idx) => RegExportDesc::Mem(idx),
                    ExportDesc::Global(idx) => RegExportDesc::Global(idx),
                    ExportDesc::Tag(idx) => RegExportDesc::Tag(idx),
                },
            },
            ByteOffset(0),
            None,
            "module exports",
        )?;
    }

    Ok(RegModule {
        funcs,
        exports,
        imported_func_count,
        imported_funcs,
        imported_memories,
        imported_globals,
        imported_tables,
        start,
        memories,
        globals,
        tables,
        elements,
        types,
        type_defs,
        data,
        imported_memory_count,
        imported_global_count,
        imported_table_count,
        tags: copy_slice_fallible(&module.tags, "tag type indices")?,
        imported_tags,
        rec_groups: copy_slice_fallible(&module.rec_groups, "rec groups")?,
    })
}

/// Lower one element-segment initializer expression to a funcref value.
/// Only `ref.func`/`ref.null` are supported at runtime (other const
/// expressions require host-provided imports).
fn lower_element_expr(expr: &crate::types::ElementExpr<'_>) -> Result<RegElemValue, LowerError> {
    let instrs = decode_instr_sequence_with_offsets(expr.expr, expr.offset)
        .map_err(|error| LowerError::from_decode(error, None))?;
    match instrs.as_slice() {
        [first, last] if matches!(last.instr, Instr::End) => match first.instr {
            Instr::RefFunc(func) => Ok(RegElemValue::FuncRef(func)),
            Instr::RefNull(RefType::FuncRef) => Ok(RegElemValue::Null),
            Instr::GlobalGet(global) => Ok(RegElemValue::GlobalGet(global)),
            _ => {
                // Any other const-eligible expression (GC allocation forms,
                // non-funcref nulls, conversions) is evaluated at
                // instantiation against the heap.
                Ok(RegElemValue::Expr(lower_const_expr(
                    expr.expr,
                    expr.offset,
                )?))
            }
        },
        _ => {
            // Multi-instruction const expressions (extended-const
            // arithmetic, nested GC construction) are evaluated at
            // instantiation against the heap.
            Ok(RegElemValue::Expr(lower_const_expr(
                expr.expr,
                expr.offset,
            )?))
        }
    }
}

/// Lower a constant expression (global initializer, data segment offset)
/// into owned form. Supports the const instructions plus the
/// extended-const integer arithmetic the validator accepts.
pub(crate) fn lower_const_expr(
    expr: &[u8],
    offset: usize,
) -> Result<Vec<RegConstInstr>, LowerError> {
    let instrs = decode_instr_sequence_with_offsets(expr, offset)
        .map_err(|error| LowerError::from_decode(error, None))?;

    let mut lowered = vec_with_capacity(
        instrs.len(),
        ByteOffset(offset),
        None,
        "constant instructions",
    )?;
    for decoded in instrs {
        let const_instr = match decoded.instr {
            Instr::I32Const(value) => RegConstInstr::I32Const(value),
            Instr::I64Const(value) => RegConstInstr::I64Const(value),
            Instr::F32Const(value) => RegConstInstr::F32Const(value.to_bits()),
            Instr::F64Const(value) => RegConstInstr::F64Const(value.to_bits()),
            Instr::V128Const(bytes) => RegConstInstr::V128Const(bytes),
            Instr::GlobalGet(global) => RegConstInstr::GlobalGet(global),
            Instr::RefNull(ref_type) => match ref_type {
                RefType::FuncRef => RegConstInstr::RefNull,
                RefType::ExternRef => RegConstInstr::RefNullOf(ref_type),
                RefType::Typed { heap, .. } => match heap {
                    HeapType::Func | HeapType::Extern | HeapType::Exn | HeapType::Type(_) => {
                        RegConstInstr::RefNullOf(ref_type)
                    }
                    gc => RegConstInstr::RefNullGC(gc),
                },
            },
            Instr::RefFunc(func) => RegConstInstr::RefFunc(func),
            Instr::RefI31 => RegConstInstr::RefI31,
            Instr::StructNew(type_idx) => RegConstInstr::StructNew {
                type_idx,
                default: false,
            },
            Instr::StructNewDefault(type_idx) => RegConstInstr::StructNew {
                type_idx,
                default: true,
            },
            Instr::ArrayNew(type_idx) => RegConstInstr::ArrayNew {
                type_idx,
                default: false,
            },
            Instr::ArrayNewDefault(type_idx) => RegConstInstr::ArrayNew {
                type_idx,
                default: true,
            },
            Instr::ArrayNewFixed { type_idx, count } => {
                RegConstInstr::ArrayNewFixed { type_idx, count }
            }
            Instr::AnyConvertExtern => RegConstInstr::ExternConvert {
                direction: ExternConvert::ExternToAny,
            },
            Instr::ExternConvertAny => RegConstInstr::ExternConvert {
                direction: ExternConvert::AnyToExtern,
            },
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
fn func_type_table<'a, 'm>(module: &'a Module<'m>) -> Result<Vec<&'a FuncType>, LowerError> {
    let mut table = vec_with_capacity(
        module.imports().len() + module.functions().len(),
        ByteOffset(0),
        None,
        "function type table",
    )?;
    for import in module.imports() {
        if let ImportDesc::Func(type_idx) = import.desc
            && let Some(func) = module.func_type(type_idx)
        {
            push_fallible(&mut table, func, ByteOffset(0), None, "function type table")?;
        }
    }
    for type_idx in module.functions() {
        if let Some(func) = module.func_type(*type_idx) {
            push_fallible(&mut table, func, ByteOffset(0), None, "function type table")?;
        }
    }
    Ok(table)
}

/// The module's full, kind-preserving type section. Used as the source of
/// truth for cross-module structural comparison of concrete reference
/// types; `func_type_view` derives the function-only view from it. The outer
/// vector is reserved fallibly (it scales with the type-section length);
/// per-element `SubType` cloning is bounded by the type's own arity.
fn reg_type_table(module: &Module<'_>) -> Result<Vec<SubType>, LowerError> {
    let types = module.types();
    let mut out = vec_with_capacity(types.len(), ByteOffset(0), None, "register type table")?;
    for ty in types {
        push_fallible(
            &mut out,
            clone_subtype_fallible(ty)?,
            ByteOffset(0),
            None,
            "register type table",
        )?;
    }
    Ok(out)
}

/// Fallibly deep-clone a subtype: its supertype index list and every
/// module-scaled inner vector are reserved before they are filled, so a
/// refused allocation is a clean error rather than a process abort.
fn clone_subtype_fallible(ty: &SubType) -> Result<SubType, LowerError> {
    let mut supertypes = vec_with_capacity(
        ty.supertypes.len(),
        ByteOffset(0),
        None,
        "subtype supertypes",
    )?;
    supertypes.extend_from_slice(&ty.supertypes);
    let comp = match &ty.comp {
        crate::types::CompType::Func(func) => {
            crate::types::CompType::Func(clone_func_type_fallible(func)?)
        }
        crate::types::CompType::Struct(fields) => {
            let mut out =
                vec_with_capacity(fields.len(), ByteOffset(0), None, "struct field types")?;
            out.extend_from_slice(fields);
            crate::types::CompType::Struct(out)
        }
        crate::types::CompType::Array(field) => crate::types::CompType::Array(*field),
    };
    Ok(SubType {
        is_final: ty.is_final,
        supertypes,
        comp,
    })
}

/// Fallibly clone a table type: its initializer expression (module-controlled
/// bytes, present when the table carries the `0x40` init marker) is reserved
/// before it is filled, so a refused allocation is a clean error rather than a
/// process abort.
fn clone_table_type_fallible(
    ty: &crate::types::TableType,
) -> Result<crate::types::TableType, LowerError> {
    let init = match &ty.init {
        Some(expr) => Some(copy_slice_fallible(expr, "table initializer expression")?),
        None => None,
    };
    Ok(crate::types::TableType {
        elem: ty.elem,
        limits: ty.limits,
        init,
    })
}

/// Fallibly deep-clone a function type's parameter and result vectors.
fn clone_func_type_fallible(ty: &FuncType) -> Result<FuncType, LowerError> {
    Ok(FuncType {
        params: copy_slice_fallible(&ty.params, "function parameter types")?,
        results: copy_slice_fallible(&ty.results, "function result types")?,
    })
}

/// Function types in type-index order, for the register IR's structural
/// `call_indirect` checks. Non-function entries are placeholders: validation
/// ensures every `call_indirect`/`ref.func` type index points at a function
/// type, so the placeholder is never observed for a valid module.
fn func_type_view(type_defs: &[SubType]) -> Result<Vec<FuncType>, LowerError> {
    let mut out = vec_with_capacity(type_defs.len(), ByteOffset(0), None, "function type view")?;
    for sub in type_defs {
        let func = match sub.as_func() {
            Some(func) => clone_func_type_fallible(func)?,
            None => FuncType {
                params: Vec::new(),
                results: Vec::new(),
            },
        };
        push_fallible(&mut out, func, ByteOffset(0), None, "function type view")?;
    }
    Ok(out)
}

/// Resolve the value type of every global in the index space (imported
/// first, then defined), so `global.get`/`global.set` lowering can type
/// its operands.
fn global_type_table(module: &Module<'_>) -> Result<Vec<ValType>, LowerError> {
    let mut out = vec_with_capacity(
        module.imports().len() + module.globals().len(),
        ByteOffset(0),
        None,
        "global type table",
    )?;
    for import in module.imports() {
        if let ImportDesc::Global(global) = import.desc {
            push_fallible(
                &mut out,
                global.val_type,
                ByteOffset(0),
                None,
                "global type table",
            )?;
        }
    }
    for global in module.globals() {
        push_fallible(
            &mut out,
            global.global_type.val_type,
            ByteOffset(0),
            None,
            "global type table",
        )?;
    }
    Ok(out)
}

/// Resolve the type index of every tag in the index space (imported first,
/// then defined). The index-space table lets lowering type `throw` payloads
/// Rec-group provenance of a type index, for tag link compatibility:
/// `Some((member types, position))` when the index falls inside a `rec`
/// group entry, `None` for bare types. The member copy is fallible.
fn rec_provenance(
    module: &Module<'_>,
    type_idx: TypeIdx,
) -> Result<Option<(Vec<SubType>, u32)>, LowerError> {
    let idx = type_idx.0;
    let Some(&(start, len)) = module
        .rec_groups
        .iter()
        .find(|&&(start, len)| idx >= start && idx < start + len)
    else {
        return Ok(None);
    };
    let types = module.types();
    let members = copy_slice_fallible(
        &types[start as usize..(start + len) as usize],
        "rec group members",
    )?;
    Ok(Some((members, idx - start)))
}

/// and `try_table` catch landings without re-deriving import order per call.
fn tag_type_table(module: &Module<'_>) -> Result<Vec<TypeIdx>, LowerError> {
    let mut out = vec_with_capacity(
        module.imports().len() + module.tags.len(),
        ByteOffset(0),
        None,
        "tag type table",
    )?;
    for import in module.imports() {
        if let ImportDesc::Tag(type_idx) = import.desc {
            push_fallible(&mut out, type_idx, ByteOffset(0), None, "tag type table")?;
        }
    }
    for type_idx in module.tags.iter().copied() {
        push_fallible(&mut out, type_idx, ByteOffset(0), None, "tag type table")?;
    }
    Ok(out)
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
        .map_err(|error| LowerError::from_decode(error, Some(func_idx)))?;

    let locals = local_types(func_idx, code.body_offset, ty, code.locals.as_slice())?;
    let mut builder = FuncBuilder::new(func_idx, type_idx, ty, locals, tables)?;

    for decoded in instrs {
        if builder.lower_instr(decoded)? {
            return builder.finish();
        }
    }

    Err(LowerError {
        offset: ByteOffset(code.body_offset + code.body.len()),
        function: Some(func_idx),
        kind: LowerErrorKind::MissingFunctionEnd,
    })
}

fn local_types(
    func_idx: FuncIdx,
    offset: usize,
    ty: &FuncType,
    locals: &[LocalDecl],
) -> Result<Vec<ValType>, LowerError> {
    let local_count = locals
        .iter()
        .map(|local| local.count as usize)
        .sum::<usize>();
    let mut types = fallible_copy_slice(
        func_idx,
        ByteOffset(offset),
        "function parameter types",
        &ty.params,
    )?;
    if types.try_reserve(local_count).is_err() {
        return Err(LowerError {
            offset: ByteOffset(offset),
            function: Some(func_idx),
            kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded {
                what: "function locals",
            }),
        });
    }
    for local in locals {
        for _ in 0..local.count {
            types.push(local.val_type);
        }
    }
    Ok(types)
}

/// Fallibly copy `src` into a fresh `Vec` (try_reserve-then-fill), anchored at
/// `func_idx`. Mirrors [`FuncBuilder::fallible_copy`] for the pre-builder
/// function-setup path.
fn fallible_copy_slice<T: Clone>(
    func_idx: FuncIdx,
    offset: ByteOffset,
    what: &'static str,
    src: &[T],
) -> Result<Vec<T>, LowerError> {
    let mut out = Vec::new();
    out.try_reserve(src.len()).map_err(|_| LowerError {
        offset,
        function: Some(func_idx),
        kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded { what }),
    })?;
    out.extend_from_slice(src);
    Ok(out)
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
    /// Offset of the instruction currently being lowered, used to anchor
    /// resource-limit errors raised by bookkeeping helpers that lack an
    /// offset parameter.
    cur_offset: ByteOffset,
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
    /// Static protected regions produced by `try_table` lowering.
    regions: Vec<ProtectedRegion>,
    /// Catch landings whose target block index (and dst_regs) need
    /// back-patching once the targeted frame's end is reached.
    pending_catches: Vec<PendingCatch>,
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

/// A catch landing whose target block index and `dst_regs` are only known
/// once the targeted (non-loop) frame's `end` is lowered.
struct PendingCatch {
    region_idx: usize,
    clause_idx: usize,
    /// Position in `label_stack` of the targeted frame at try_table
    /// time (the clause labels resolve in the OUTER context).
    frame_pos: usize,
    /// The value types the landing writes (tag params [+ exnref]).
    value_types: Vec<ValType>,
}

/// Which conditional reference branch is being lowered.
enum RefBranchKind {
    /// `br_on_null`: taken when the reference is null.
    OnNull,
    /// `br_on_non_null`: taken when the reference is non-null (the ref is
    /// forwarded as the last branch value).
    OnNonNull,
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

/// Coarse reference compatibility for lowering's operand checks.
///
/// Lowering tracks types only for register allocation; the validator has
/// already proven precise ref types (nullability, concrete type indices,
/// subtyping). Here we only need to keep function references and external
/// references from crossing kinds.
fn ref_compatible(types: &[crate::types::SubType], found: ValType, expected: ValType) -> bool {
    fn kind(types: &[crate::types::SubType], ty: &crate::types::RefType) -> u8 {
        match ty {
            crate::types::RefType::FuncRef => 0,
            crate::types::RefType::ExternRef => 1,
            crate::types::RefType::Typed { heap, .. } => match heap {
                crate::types::HeapType::Func => 0,
                crate::types::HeapType::Extern => 1,
                crate::types::HeapType::Exn => 2,
                crate::types::HeapType::NoFunc => 0,
                crate::types::HeapType::NoExtern => 1,
                crate::types::HeapType::NoExn => 2,
                crate::types::HeapType::Type(idx) => {
                    // A concrete type's hierarchy side comes from the type
                    // section: struct/array are any-side, functions func-side.
                    match types.get(idx.0 as usize).map(|sub| &sub.comp) {
                        Some(crate::types::CompType::Struct(_))
                        | Some(crate::types::CompType::Array(_)) => 3,
                        _ => 0,
                    }
                }
                crate::types::HeapType::Any
                | crate::types::HeapType::None
                | crate::types::HeapType::Eq
                | crate::types::HeapType::I31
                | crate::types::HeapType::Struct
                | crate::types::HeapType::Array => 3,
            },
        }
    }
    match (found, expected) {
        (ValType::Ref(found), ValType::Ref(expected)) => {
            kind(types, &found) == kind(types, &expected)
        }
        _ => false,
    }
}

/// function types, global types, the type section, and table element
/// types (all imported-first where an index space applies).
/// Module-level index-space tables shared by every function lowering:
/// function types, global types, the type section, and table element
/// types (all imported-first where an index space applies).
struct ModuleTables<'a> {
    func_types: &'a [&'a FuncType],
    global_types: &'a [ValType],
    types: &'a [crate::types::SubType],
    table_elem_types: &'a [crate::types::RefType],
    /// Address index type (i64 vs i32) of every memory, imported first.
    memory_addr_types: &'a [NumType],
    /// Address index type (i64 vs i32) of every table, imported first.
    table_addr_types: &'a [NumType],
    /// Type index of every tag in the index space (imported first), so
    /// `throw`/`try_table` lowering can type tag payloads.
    tag_types: &'a [TypeIdx],
}

impl ModuleTables<'_> {
    /// The function signature at `idx`, if that type index is a func type.
    fn func_type(&self, idx: usize) -> Option<&FuncType> {
        self.types.get(idx).and_then(crate::types::SubType::as_func)
    }
}

/// The numeric type used to address a memory/table from its declared limits.
fn addr_num_type(limits: &crate::types::Limits) -> NumType {
    if limits.is64 {
        NumType::I64
    } else {
        NumType::I32
    }
}

/// The narrower of two address types (`min at1 at2` in the reference): i32
/// unless both operands are i64.
fn min_num_type(lhs: NumType, rhs: NumType) -> NumType {
    if matches!(lhs, NumType::I64) && matches!(rhs, NumType::I64) {
        NumType::I64
    } else {
        NumType::I32
    }
}

/// The non-null concrete reference type `(ref $idx)`.
fn gc_ref_type(type_idx: TypeIdx) -> ValType {
    ValType::Ref(crate::types::RefType::concrete(false, type_idx))
}

/// The nullable concrete reference type `(ref null $idx)`.
fn gc_ref_type_nullable(type_idx: TypeIdx) -> ValType {
    ValType::Ref(crate::types::RefType::concrete(true, type_idx))
}

/// `diff_reftype` (`valid/valid.ml:234`).
fn lower_diff_reftype(
    src: crate::types::RefType,
    tgt: crate::types::RefType,
) -> crate::types::RefType {
    if tgt.is_nullable() {
        src.as_non_null()
    } else {
        src
    }
}

/// The unpacked value type of a storage type (packed i8/i16 → i32).
fn gc_unpacked(storage: crate::types::StorageType) -> ValType {
    match storage {
        crate::types::StorageType::Val(val) => val,
        crate::types::StorageType::I8 | crate::types::StorageType::I16 => {
            ValType::Num(NumType::I32)
        }
    }
}

/// The kind of control structure a label frame describes.
enum FrameKind {
    /// `block` (or the implicit function body frame).
    Block,
    /// `try_table` — the body is an ordinary block region; the region
    /// index records which `ProtectedRegion` its `end` bounds.
    TryTable { region_idx: usize },
    /// `loop` — branches target the loop header block directly.
    Loop { header_block: u32 },
    /// `if` — `cond_block` is the IfFork block whose `else_block` field needs
    /// back-patching; `else_seen` records whether an `else` was lowered.
    If { cond_block: usize, else_seen: bool },
}

/// Fallibly reserve room for `len` elements in a fresh `Vec`.
///
/// Resource discipline: `Vec::with_capacity` aborts on allocator refusal, so
/// every module-scaled preallocation on the lowering path goes through this
/// helper and yields a clean [`ValidationErrorKind::ResourceLimitExceeded`].
fn vec_with_capacity<T>(
    len: usize,
    offset: ByteOffset,
    function: Option<FuncIdx>,
    what: &'static str,
) -> Result<Vec<T>, LowerError> {
    let mut values = Vec::new();
    values.try_reserve(len).map_err(|_| LowerError {
        offset,
        function,
        kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded { what }),
    })?;
    Ok(values)
}

/// Collect the imports selected by `select`, reserving the output vector
/// fallibly up front (one entry per import is an upper bound).
///
/// The selector itself is fallible: every import item carries a cloned
/// module/name pair and a cloned type entry, and those allocations are
/// reserved with `try_reserve` before they are filled (resource discipline —
/// a refused allocation is a clean error, never a process abort).
fn collect_imports<T>(
    module: &Module<'_>,
    select: impl Fn(&crate::types::Import) -> Result<Option<T>, LowerError>,
) -> Result<Vec<T>, LowerError> {
    let mut out = vec_with_capacity(
        module.imports().len(),
        ByteOffset(0),
        None,
        "module imports",
    )?;
    for import in module.imports() {
        if let Some(item) = select(import)? {
            push_fallible(&mut out, item, ByteOffset(0), None, "module imports")?;
        }
    }
    Ok(out)
}

/// Fallibly copy a module-level slice anchored at offset 0.
fn copy_slice_fallible<T: Clone>(src: &[T], what: &'static str) -> Result<Vec<T>, LowerError> {
    let mut out = vec_with_capacity(src.len(), ByteOffset(0), None, what)?;
    out.extend_from_slice(src);
    Ok(out)
}

/// Fallibly clone a module-controlled string (a plain `String::clone` is an
/// infallible allocation that aborts on allocator refusal).
fn clone_string_fallible(value: &str, what: &'static str) -> Result<String, LowerError> {
    let mut out = String::new();
    out.try_reserve(value.len()).map_err(|_| LowerError {
        offset: ByteOffset(0),
        function: None,
        kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded { what }),
    })?;
    out.push_str(value);
    Ok(out)
}

/// Push `value` onto `values`, growing the backing store fallibly in
/// geometric chunks before the push.
///
/// Resource discipline: a refused growth is a clean
/// [`ValidationErrorKind::ResourceLimitExceeded`], never an allocation abort
/// (the U3 declining-allocator sweep aborts on any infallible `Vec` growth).
/// Growth stays amortized O(1) because the chunk grows with the length.
fn push_fallible<T>(
    values: &mut Vec<T>,
    value: T,
    offset: ByteOffset,
    function: Option<FuncIdx>,
    what: &'static str,
) -> Result<(), LowerError> {
    if values.len() == values.capacity() {
        values
            .try_reserve(values.len().max(8))
            .map_err(|_| LowerError {
                offset,
                function,
                kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded {
                    what,
                }),
            })?;
    }
    values.push(value);
    Ok(())
}

impl<'b> FuncBuilder<'b> {
    fn new(
        func_idx: FuncIdx,
        type_idx: TypeIdx,
        ty: &FuncType,
        locals: Vec<ValType>,
        tables: &'b ModuleTables<'b>,
    ) -> Result<Self, LowerError> {
        // The function body itself is label 0, targeting a block that will
        // receive function-end returns (created on demand).
        let params = fallible_copy_slice(
            func_idx,
            ByteOffset(0),
            "function parameter types",
            &ty.params,
        )?;
        let results = fallible_copy_slice(
            func_idx,
            ByteOffset(0),
            "function result types",
            &ty.results,
        )?;
        let frame_results = fallible_copy_slice(
            func_idx,
            ByteOffset(0),
            "function frame result types",
            &ty.results,
        )?;
        let mut label_stack = Vec::new();
        if label_stack.try_reserve(1).is_err() {
            return Err(LowerError {
                offset: ByteOffset(0),
                function: Some(func_idx),
                kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded {
                    what: "function label stack",
                }),
            });
        }
        label_stack.push(LabelFrame {
            label: LabelIdx(0),
            kind: FrameKind::Block,
            result_types: frame_results,
            param_types: Vec::new(),
            param_regs: Vec::new(),
            height: 0,
            unreachable: false,
        });
        Ok(Self {
            func_idx,
            type_idx,
            params,
            results,
            locals,
            stack: Vec::new(),
            reg_types: Vec::new(),
            blocks: Vec::new(),
            current_instrs: Vec::new(),
            cur_offset: ByteOffset(0),
            label_stack,
            pending_branches: Vec::new(),
            pending_trampolines: Vec::new(),
            regions: Vec::new(),
            pending_catches: Vec::new(),
            tables,
        })
    }

    fn finish_block_with_label(
        &mut self,
        label: LabelIdx,
        term: RegTerm,
    ) -> Result<(), LowerError> {
        let instrs = core::mem::take(&mut self.current_instrs);
        if self.blocks.try_reserve(1).is_err() {
            return Err(self.resource_error(self.cur_offset, "control blocks"));
        }
        push_fallible(
            &mut self.blocks,
            RegBlock::new(label, instrs, term),
            self.cur_offset,
            Some(self.func_idx),
            "control blocks",
        )?;
        Ok(())
    }

    fn finish_block(&mut self, term: RegTerm) -> Result<(), LowerError> {
        let label = LabelIdx(self.blocks.len() as u32);
        self.finish_block_with_label(label, term)
    }

    /// Finish a conditional reference branch (`br_on_null` /
    /// `br_on_non_null`), mirroring the `br_if` discipline: loop back-edges
    /// get a copy trampoline; other targets are back-patched at frame `end`.
    fn finish_cond_ref_branch(
        &mut self,
        offset: ByteOffset,
        frame_pos: usize,
        loop_header: Option<LoopTarget>,
        values: Vec<Reg>,
        value_reg: Reg,
        kind: RefBranchKind,
    ) -> Result<(), LowerError> {
        let make_term = |target_block: u32, values: Vec<Reg>| match kind {
            RefBranchKind::OnNull => RegTerm::BrIfNull {
                value: value_reg,
                target_block,
                values,
            },
            RefBranchKind::OnNonNull => RegTerm::BrIfNonNull {
                value: value_reg,
                target_block,
                values,
            },
        };
        let br_block_idx = self.blocks.len();
        if let Some(loop_target) = loop_header {
            let entry = TrampolineReq {
                block: br_block_idx,
                slot: BranchSlot::Single,
                param_regs: loop_target.param_regs,
                values: self.fallible_copy(offset, "branch values", &values)?,
                header: loop_target.header,
                offset,
            };
            push_fallible(
                &mut self.pending_trampolines,
                entry,
                self.cur_offset,
                Some(self.func_idx),
                "pending trampolines",
            )?;
            self.finish_block(make_term(0, values))?;
        } else {
            let entry = PendingBranch {
                block: br_block_idx,
                frame_pos,
                values: self.fallible_copy(offset, "branch values", &values)?,
                slot: BranchSlot::Single,
            };
            push_fallible(
                &mut self.pending_branches,
                entry,
                self.cur_offset,
                Some(self.func_idx),
                "pending branches",
            )?;
            self.finish_block(make_term(0, values))?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<RegFunc, LowerError> {
        // If there are pending instructions without a terminator, add Fallthrough
        if !self.current_instrs.is_empty() || self.blocks.is_empty() {
            self.finish_block(RegTerm::Fallthrough)?;
        }
        // Create back-edge trampolines for conditional branches to loops
        // with parameters: each runs the parameter copies, then branches
        // unconditionally to the header. Trampolines sit at the end of the
        // block list so no fallthrough can reach them.
        for req in core::mem::take(&mut self.pending_trampolines) {
            let trampoline_idx = self.blocks.len() as u32;
            let mut instrs: Vec<RegInstr> = Vec::new();
            instrs
                .try_reserve(req.param_regs.len())
                .map_err(|_| self.resource_error(self.cur_offset, "loop trampoline copies"))?;
            for (dst, src) in req.param_regs.iter().zip(req.values.iter()) {
                if dst != src {
                    instrs.push(RegInstr {
                        offset: req.offset,
                        op: RegOp::Copy {
                            dst: *dst,
                            src: *src,
                        },
                    });
                }
            }
            if self.blocks.try_reserve(1).is_err() {
                return Err(self.resource_error(self.cur_offset, "loop trampoline blocks"));
            }
            push_fallible(
                &mut self.blocks,
                RegBlock::new(
                    LabelIdx(trampoline_idx),
                    instrs,
                    RegTerm::Br {
                        target_block: req.header,
                        values: Vec::new(),
                    },
                ),
                self.cur_offset,
                Some(self.func_idx),
                "control blocks",
            )?;
            match (req.slot, &mut self.blocks[req.block].term) {
                (
                    BranchSlot::Single,
                    RegTerm::BrIf { target_block, .. }
                    | RegTerm::BrIfNull { target_block, .. }
                    | RegTerm::BrIfNonNull { target_block, .. },
                ) => {
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
        #[cfg(debug_assertions)]
        {
            for region in &self.regions {
                debug_assert!(
                    region.first_block < region.end_block,
                    "try_table region has empty/invalid block range: {region:?}"
                );
            }
            for (i, a) in self.regions.iter().enumerate() {
                for b in self.regions.iter().skip(i + 1) {
                    let disjoint = a.end_block <= b.first_block || b.end_block <= a.first_block;
                    let a_contains_b = a.first_block <= b.first_block && b.end_block <= a.end_block;
                    let b_contains_a = b.first_block <= a.first_block && a.end_block <= b.end_block;
                    debug_assert!(
                        disjoint || a_contains_b || b_contains_a,
                        "try_table regions must be properly nested: {a:?} vs {b:?}"
                    );
                }
            }
        }
        Ok(RegFunc {
            idx: self.func_idx,
            type_idx: self.type_idx,
            params: self.params,
            results: self.results,
            locals: self.locals,
            reg_types: self.reg_types,
            blocks: self.blocks,
            regions: self.regions,
        })
    }
    /// Lower one decoded instruction. Returns `true` when the function body is complete.
    fn lower_instr(&mut self, decoded: DecodedInstr) -> Result<bool, LowerError> {
        let offset = decoded.offset;
        self.cur_offset = offset;
        match decoded.instr {
            Instr::LocalGet(local) => {
                let ty = self.locals[local.0 as usize];
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::LocalGet { dst, local })?;
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
                )?;
            }
            Instr::LocalTee(local) => {
                let expected = self.locals[local.0 as usize];
                let value = self.pop_expect(offset, "local.tee", expected)?;
                self.push_stack(value)?;
                self.emit(
                    offset,
                    RegOp::LocalTee {
                        local,
                        value: value.reg,
                    },
                )?;
            }
            Instr::Drop => {
                let value = self.pop_any(offset, "drop")?;
                self.emit(offset, RegOp::Drop { value: value.reg })?;
            }
            Instr::Select => {
                let cond = self.pop_expect(offset, "select", ValType::Num(NumType::I32))?;
                // Untyped select: both operands must share the same numeric
                // type; the second operand's type is discovered from the
                // stack (the validator has already proven they match).
                let v2 = self.pop_any(offset, "select")?;
                let v1 = self.pop_expect(offset, "select", v2.ty)?;
                let dst = self.alloc_reg(v1.ty)?;
                self.push_stack(RegValue {
                    reg: dst,
                    ty: v1.ty,
                })?;
                self.emit(
                    offset,
                    RegOp::Select {
                        dst,
                        v1: v1.reg,
                        v2: v2.reg,
                        cond: cond.reg,
                    },
                )?;
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
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::Select {
                        dst,
                        v1: v1.reg,
                        v2: v2.reg,
                        cond: cond.reg,
                    },
                )?;
            }
            Instr::I32Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::I32))?;
                self.push_stack(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::I32),
                })?;
                self.emit(offset, RegOp::I32Const { dst, value })?;
            }
            Instr::I64Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::I64))?;
                self.push_stack(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::I64),
                })?;
                self.emit(offset, RegOp::I64Const { dst, value })?;
            }
            Instr::F32Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::F32))?;
                self.push_stack(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::F32),
                })?;
                self.emit(offset, RegOp::F32Const { dst, value })?;
            }
            Instr::F64Const(value) => {
                let dst = self.alloc_reg(ValType::Num(NumType::F64))?;
                self.push_stack(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::F64),
                })?;
                self.emit(offset, RegOp::F64Const { dst, value })?;
            }
            Instr::Unreachable => {
                self.finish_block(RegTerm::Trap)?;
                self.set_unreachable();
            }
            Instr::Nop => {}
            Instr::Throw(tag) => {
                let params = self.tag_params(offset, tag)?;
                let mut values =
                    self.temp_vec(params.len(), self.cur_offset, "lowering temporary")?;
                for &expected in params.iter().rev() {
                    let found = self.pop_expect(offset, "throw", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                self.finish_block(RegTerm::Throw { tag, values })?;
                self.set_unreachable();
            }
            Instr::ThrowRef => {
                let found = self.pop_expect(
                    offset,
                    "throw_ref",
                    ValType::Ref(RefType::from_parts(true, HeapType::Exn)),
                )?;
                self.finish_block(RegTerm::ThrowRef { exn: found.reg })?;
                self.set_unreachable();
            }
            Instr::Block(block_type) => {
                self.finish_block(RegTerm::Fallthrough)?;
                self.enter_frame(offset, "block", FrameKind::Block, block_type)?;
            }
            Instr::Loop(block_type) => {
                self.finish_block(RegTerm::Fallthrough)?;
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
                })?;
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
            Instr::TryTable(try_table) => {
                // The body is a fresh block region: its first block is the
                // next one to be created (after the tip block is finished).
                self.finish_block(RegTerm::Fallthrough)?;
                let first_block = self.blocks.len() as u32;
                let region_idx = self.regions.len();
                let region = ProtectedRegion {
                    first_block,
                    end_block: 0,
                    catches: self.temp_vec(
                        try_table.catches.len(),
                        self.cur_offset,
                        "catch landing registers",
                    )?,
                };
                push_fallible(
                    &mut self.regions,
                    region,
                    self.cur_offset,
                    Some(self.func_idx),
                    "catch regions",
                )?;
                // Catch labels resolve in the OUTER context (the try_table's
                // own frame is not yet pushed).
                for (clause_idx, catch) in try_table.catches.iter().enumerate() {
                    let frame_pos = self.label_position(offset, LabelIdx(catch.label))?;
                    let value_types = self.catch_value_types(offset, catch)?;
                    // A loop label's branch convention delivers into the
                    // loop's parameter registers and targets its header,
                    // both known now (the loop frame is already on the stack).
                    let clause = match self.branch_types_at_fallible(offset, frame_pos)?.1 {
                        Some(loop_target) => RegCatch {
                            tag: catch.tag,
                            target_block: loop_target.header,
                            dst_regs: loop_target.param_regs,
                        },
                        None => {
                            push_fallible(
                                &mut self.pending_catches,
                                PendingCatch {
                                    region_idx,
                                    clause_idx,
                                    frame_pos,
                                    value_types,
                                },
                                self.cur_offset,
                                Some(self.func_idx),
                                "pending catches",
                            )?;
                            RegCatch {
                                tag: catch.tag,
                                target_block: 0,
                                dst_regs: Vec::new(),
                            }
                        }
                    };
                    self.regions[region_idx].catches.push(clause);
                }
                self.enter_frame(
                    offset,
                    "try_table",
                    FrameKind::TryTable { region_idx },
                    try_table.block_type,
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
                    }) => (
                        *cond_block,
                        self.fallible_copy(offset, "else result types", result_types)?,
                        *height,
                    ),
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
                let mut values =
                    self.temp_vec(result_types.len(), self.cur_offset, "lowering temporary")?;
                for &expected in result_types.iter().rev() {
                    let found = self.pop_expect(offset, "else", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                let br_block_idx = self.blocks.len();
                let entry = PendingBranch {
                    block: br_block_idx,
                    frame_pos,
                    values: self.fallible_copy(offset, "branch values", &values)?,
                    slot: BranchSlot::Single,
                };
                push_fallible(
                    &mut self.pending_branches,
                    entry,
                    self.cur_offset,
                    Some(self.func_idx),
                    "pending branches",
                )?;
                self.finish_block(RegTerm::Br {
                    target_block: 0,
                    values,
                })?;
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
                    (
                        self.fallible_copy(offset, "else parameter registers", &frame.param_regs)?,
                        self.fallible_copy(offset, "else parameter types", &frame.param_types)?,
                    )
                };
                for (&reg, &ty) in param_regs.iter().zip(param_types.iter()) {
                    self.push_stack(RegValue { reg, ty })?;
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
                    // Finish the body block, then patch branches targeting
                    // the function frame (`br 0`) to a continuation block
                    // that returns — delivering their values into the
                    // return registers via copies, like any other frame.
                    self.finish_block(RegTerm::Fallthrough)?;
                    let continuation_idx = self.blocks.len() as u32;
                    let func_idx = self.func_idx;
                    for pending in core::mem::take(&mut self.pending_branches) {
                        match &mut self.blocks[pending.block].term {
                            RegTerm::Br { target_block, .. }
                            | RegTerm::BrIf { target_block, .. }
                            | RegTerm::BrIfNull { target_block, .. }
                            | RegTerm::BrIfNonNull { target_block, .. } => {
                                *target_block = continuation_idx;
                            }
                            RegTerm::BrTable {
                                targets, default, ..
                            } => {
                                if let BranchSlot::Table(slot) = pending.slot {
                                    if slot < targets.len() {
                                        targets[slot] = continuation_idx;
                                    } else {
                                        *default = continuation_idx;
                                    }
                                }
                            }
                            other => unreachable!(
                                "pending branch block has non-branch terminator: {other:?}"
                            ),
                        }
                        for (dst, src) in values.iter().zip(pending.values.iter()) {
                            if dst != src {
                                push_fallible(
                                    &mut self.blocks[pending.block].instrs,
                                    RegInstr {
                                        offset,
                                        op: RegOp::Copy {
                                            dst: *dst,
                                            src: *src,
                                        },
                                    },
                                    offset,
                                    Some(func_idx),
                                    "block branch copies",
                                )?;
                            }
                        }
                    }
                    // Catch clauses targeting the function frame land on the
                    // return continuation with the function's result regs.
                    // Process the matching clauses in place and then compact
                    // the vector in place: a replacement `Vec` sized by
                    // `pending_catches.len()` was an infallible,
                    // module-scaled allocation that could abort on allocator
                    // refusal.
                    {
                        let regions = &mut self.regions;
                        for pending in &self.pending_catches {
                            if pending.frame_pos != 0 {
                                continue;
                            }
                            debug_assert_eq!(
                                pending.value_types.len(),
                                values.len(),
                                "catch landing arity must match the function result registers"
                            );
                            let dst_regs = fallible_copy_slice(
                                func_idx,
                                offset,
                                "catch landing registers",
                                &values,
                            )?;
                            let clause =
                                &mut regions[pending.region_idx].catches[pending.clause_idx];
                            clause.target_block = continuation_idx;
                            clause.dst_regs = dst_regs;
                        }
                    }
                    self.pending_catches
                        .retain(|pending| pending.frame_pos != 0);
                    self.finish_block(RegTerm::Return { values })?;
                    return Ok(true);
                }
                // Pop results matching this control frame's expected types.
                // The frame must still be on the stack while they are
                // popped: pops are frame-aware (entry height and
                // polymorphic-stack state). Move the frame's result types out
                // (no ownership copy — the previous `result_types.clone()`
                // scaled with result arity and terminated on allocator
                // refusal); the frame is popped immediately below, so nothing
                // else reads them.
                if self.label_stack.last().is_none() {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::MissingFunctionEnd,
                    });
                }
                let result_types = core::mem::take(
                    &mut self
                        .label_stack
                        .last_mut()
                        .expect("frame presence checked above")
                        .result_types,
                );
                let mut values: Vec<Reg> = Vec::new();
                values
                    .try_reserve(result_types.len())
                    .map_err(|_| self.resource_error(offset, "block result registers"))?;
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
                if self.stack.try_reserve(result_types.len()).is_err() {
                    return Err(self.resource_error(offset, "block result operands"));
                }
                for (&reg, &ty) in values.iter().zip(result_types.iter()) {
                    self.push_stack(RegValue { reg, ty })?;
                }
                // Finish the body block.
                self.finish_block(RegTerm::Fallthrough)?;
                // The continuation block will be at self.blocks.len().
                // For an `if` without `else` that takes parameters, the
                // IfFork's else edge is the identity: deliver the params into
                // the continuation's registers via a trampoline inserted
                // before the continuation (the else path never computed the
                // result registers). Branches target the continuation after
                // the trampoline; the else edge targets the trampoline.
                let frame_pos = self.label_stack.len(); // position of the popped frame
                let mut else_trampoline: Option<(usize, u32, usize)> = None;
                if let FrameKind::If {
                    cond_block,
                    else_seen: false,
                } = frame.kind
                    && !frame.param_regs.is_empty()
                {
                    // The then-body must branch over the trampoline (it
                    // would otherwise fall through it).
                    let body_end_idx = self.blocks.len() - 1;
                    let trampoline_idx = self.blocks.len() as u32;
                    let mut instrs: Vec<RegInstr> = self.temp_vec(
                        core::cmp::min(values.len(), frame.param_regs.len()),
                        offset,
                        "if trampoline copies",
                    )?;
                    for (dst, src) in values.iter().zip(frame.param_regs.iter()) {
                        if dst != src {
                            push_fallible(
                                &mut instrs,
                                RegInstr {
                                    offset,
                                    op: RegOp::Copy {
                                        dst: *dst,
                                        src: *src,
                                    },
                                },
                                offset,
                                Some(self.func_idx),
                                "if trampoline copies",
                            )?;
                        }
                    }
                    push_fallible(
                        &mut self.blocks,
                        RegBlock::new(LabelIdx(trampoline_idx), instrs, RegTerm::Fallthrough),
                        self.cur_offset,
                        Some(self.func_idx),
                        "control blocks",
                    )?;
                    else_trampoline = Some((cond_block, trampoline_idx, body_end_idx));
                }
                // Back-patch all pending branches targeting this frame,
                // preserving each terminator's kind (Br vs BrIf), and append
                // copies delivering each branch's values into the registers
                // the continuation expects.
                let continuation_idx = self.blocks.len() as u32;
                let func_idx = self.func_idx;
                // Process the branches targeting this frame in place and then
                // compact the vector in place, dropping the processed
                // branches. A replacement `Vec` sized by
                // `pending_branches.len()` was an infallible, module-scaled
                // allocation that could abort on allocator refusal.
                {
                    let blocks = &mut self.blocks;
                    for pending in &self.pending_branches {
                        if pending.frame_pos != frame_pos {
                            continue;
                        }
                        let term = &mut blocks[pending.block].term;
                        match pending.slot {
                            BranchSlot::Single => match term {
                                RegTerm::Br { target_block, .. }
                                | RegTerm::BrIf { target_block, .. }
                                | RegTerm::BrIfNull { target_block, .. }
                                | RegTerm::BrIfNonNull { target_block, .. } => {
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
                                push_fallible(
                                    &mut blocks[pending.block].instrs,
                                    RegInstr {
                                        offset,
                                        op: RegOp::Copy {
                                            dst: *dst,
                                            src: *src,
                                        },
                                    },
                                    offset,
                                    Some(func_idx),
                                    "block branch copies",
                                )?;
                            }
                        }
                    }
                }
                self.pending_branches
                    .retain(|pending| pending.frame_pos != frame_pos);
                // A `try_table` frame's region ends at the continuation
                // (exclusive): its body's blocks are exactly those from
                // `first_block` up to the continuation.
                if let FrameKind::TryTable { region_idx } = frame.kind {
                    self.regions[region_idx].end_block = continuation_idx;
                }
                // Back-patch catch clauses targeting this frame: the landing
                // writes directly into the continuation's result registers
                // (there is no branching block, so no copies are emitted).
                // Process the catch clauses targeting this frame in place and
                // then compact the vector in place: a replacement `Vec` sized
                // by `pending_catches.len()` was an infallible, module-scaled
                // allocation that could abort on allocator refusal.
                {
                    let regions = &mut self.regions;
                    for pending in &self.pending_catches {
                        if pending.frame_pos != frame_pos {
                            continue;
                        }
                        debug_assert_eq!(
                            pending.value_types.len(),
                            values.len(),
                            "catch landing arity must match the target frame's registers"
                        );
                        let dst_regs = fallible_copy_slice(
                            func_idx,
                            offset,
                            "catch landing registers",
                            &values,
                        )?;
                        let clause = &mut regions[pending.region_idx].catches[pending.clause_idx];
                        clause.target_block = continuation_idx;
                        clause.dst_regs = dst_regs;
                    }
                }
                self.pending_catches
                    .retain(|pending| pending.frame_pos != frame_pos);
                // For an `if` without `else`, patch the IfFork's else edge to
                // the identity trampoline (when params exist) and retarget
                // the then-body's end to branch over it to the continuation.
                if let FrameKind::If {
                    cond_block,
                    else_seen: false,
                } = frame.kind
                {
                    if let RegTerm::IfFork { else_block, .. } = &mut self.blocks[cond_block].term {
                        *else_block = match else_trampoline {
                            Some((_, trampoline_idx, _)) => trampoline_idx,
                            None => continuation_idx,
                        };
                    }
                    if let Some((_, _, body_end_idx)) = else_trampoline {
                        self.blocks[body_end_idx].term = RegTerm::Br {
                            target_block: continuation_idx,
                            values: Vec::new(),
                        };
                    }
                }
                // Start a new continuation block (content will be filled by
                // subsequent instructions).
                self.finish_block(RegTerm::Fallthrough)?;
            }
            Instr::Br(label) => {
                let frame_pos = self.label_position(offset, label)?;
                // Branches to a loop label jump to the header carrying the
                // loop's parameters; all other branches jump to the frame's
                // continuation carrying its results.
                let (branch_types, loop_header) =
                    self.branch_types_at_fallible(offset, frame_pos)?;
                let mut values =
                    self.temp_vec(branch_types.len(), self.cur_offset, "lowering temporary")?;
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                if let Some(loop_target) = loop_header {
                    // Unconditional back-edge: deliver values into the
                    // loop's parameter registers inline (always taken).
                    self.emit_copies(offset, &loop_target.param_regs, &values)?;
                    self.finish_block(RegTerm::Br {
                        target_block: loop_target.header,
                        values,
                    })?;
                } else {
                    let br_block_idx = self.blocks.len();
                    let entry = PendingBranch {
                        block: br_block_idx,
                        frame_pos,
                        values: self.fallible_copy(offset, "branch values", &values)?,
                        slot: BranchSlot::Single,
                    };
                    push_fallible(
                        &mut self.pending_branches,
                        entry,
                        self.cur_offset,
                        Some(self.func_idx),
                        "pending branches",
                    )?;
                    self.finish_block(RegTerm::Br {
                        target_block: 0,
                        values,
                    })?;
                }
                self.set_unreachable();
            }
            Instr::BrIf(label) => {
                let cond = self.pop_expect(offset, "br_if", ValType::Num(NumType::I32))?;
                let frame_pos = self.label_position(offset, label)?;
                let (branch_types, loop_header) =
                    self.branch_types_at_fallible(offset, frame_pos)?;
                let mut values =
                    self.temp_vec(branch_types.len(), self.cur_offset, "lowering temporary")?;
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br_if", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                // The not-taken path keeps the branch values on the stack.
                for (&reg, &ty) in values.iter().zip(branch_types.iter()) {
                    self.push_stack(RegValue { reg, ty })?;
                }
                if let Some(loop_target) = loop_header {
                    // Conditional back-edge: a trampoline runs the parameter
                    // copies so the not-taken path keeps the loop's live
                    // parameter registers intact.
                    let br_block_idx = self.blocks.len();
                    let entry = TrampolineReq {
                        block: br_block_idx,
                        slot: BranchSlot::Single,
                        param_regs: loop_target.param_regs,
                        values: self.fallible_copy(offset, "branch values", &values)?,
                        header: loop_target.header,
                        offset,
                    };
                    push_fallible(
                        &mut self.pending_trampolines,
                        entry,
                        self.cur_offset,
                        Some(self.func_idx),
                        "pending trampolines",
                    )?;
                    self.finish_block(RegTerm::BrIf {
                        cond: cond.reg,
                        target_block: 0,
                        values,
                    })?;
                } else {
                    let br_block_idx = self.blocks.len();
                    let entry = PendingBranch {
                        block: br_block_idx,
                        frame_pos,
                        values: self.fallible_copy(offset, "branch values", &values)?,
                        slot: BranchSlot::Single,
                    };
                    push_fallible(
                        &mut self.pending_branches,
                        entry,
                        self.cur_offset,
                        Some(self.func_idx),
                        "pending branches",
                    )?;
                    self.finish_block(RegTerm::BrIf {
                        cond: cond.reg,
                        target_block: 0,
                        values,
                    })?;
                }
            }
            Instr::BrTable { targets, default } => {
                let index = self.pop_expect(offset, "br_table", ValType::Num(NumType::I32))?;
                // All targets must agree on branch arity and types (the
                // validator guarantees this); pop using the default target.
                let default_pos = self.label_position(offset, default)?;
                let (branch_types, _) = self.branch_types_at_fallible(offset, default_pos)?;
                let mut values =
                    self.temp_vec(branch_types.len(), self.cur_offset, "lowering temporary")?;
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br_table", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                // Resolve targets: loop headers are known immediately; other
                // frames get a pending entry per slot for back-patching at
                // their `end`.
                let br_block_idx = self.blocks.len();
                let mut target_blocks =
                    self.temp_vec(targets.len() + 1, self.cur_offset, "lowering temporary")?;
                for (slot, target) in targets.iter().chain(core::iter::once(&default)).enumerate() {
                    let frame_pos = self.label_position(offset, *target)?;
                    let (_, loop_header) = self.branch_types_at_fallible(offset, frame_pos)?;
                    match loop_header {
                        Some(loop_target) => {
                            target_blocks.push(0);
                            let entry = TrampolineReq {
                                block: br_block_idx,
                                slot: BranchSlot::Table(slot),
                                param_regs: loop_target.param_regs,
                                values: self.fallible_copy(offset, "branch values", &values)?,
                                header: loop_target.header,
                                offset,
                            };
                            push_fallible(
                                &mut self.pending_trampolines,
                                entry,
                                self.cur_offset,
                                Some(self.func_idx),
                                "pending trampolines",
                            )?;
                        }
                        None => {
                            target_blocks.push(0);
                            let entry = PendingBranch {
                                block: br_block_idx,
                                frame_pos,
                                values: self.fallible_copy(offset, "branch values", &values)?,
                                slot: BranchSlot::Table(slot),
                            };
                            push_fallible(
                                &mut self.pending_branches,
                                entry,
                                self.cur_offset,
                                Some(self.func_idx),
                                "pending branches",
                            )?;
                        }
                    }
                }
                let default_block = target_blocks.pop().expect("default included above");
                self.finish_block(RegTerm::BrTable {
                    index: index.reg,
                    targets: target_blocks,
                    default: default_block,
                    values,
                })?;
                self.set_unreachable();
            }
            Instr::BrOnNull(label) => {
                let value = self.pop_any(offset, "br_on_null")?;
                let frame_pos = self.label_position(offset, label)?;
                let (branch_types, loop_header) =
                    self.branch_types_at_fallible(offset, frame_pos)?;
                let mut values =
                    self.temp_vec(branch_types.len(), self.cur_offset, "lowering temporary")?;
                for &expected in branch_types.iter().rev() {
                    let found = self.pop_expect(offset, "br_on_null", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                // Not-taken path: branch values stay, plus the ref (non-null
                // on this path by definition).
                for (&reg, &ty) in values.iter().zip(branch_types.iter()) {
                    self.push_stack(RegValue { reg, ty })?;
                }
                self.push_stack(value)?;
                self.finish_cond_ref_branch(
                    offset,
                    frame_pos,
                    loop_header,
                    values,
                    value.reg,
                    RefBranchKind::OnNull,
                )?;
            }
            Instr::BrOnNonNull(label) => {
                let value = self.pop_any(offset, "br_on_non_null")?;
                let frame_pos = self.label_position(offset, label)?;
                let (branch_types, loop_header) =
                    self.branch_types_at_fallible(offset, frame_pos)?;
                // The label's last value is the forwarded non-null ref; only
                // the leading types come from the stack.
                let leading_types = match branch_types.split_last() {
                    Some((_, leading)) => leading,
                    None => &[][..],
                };
                let mut values =
                    self.temp_vec(branch_types.len(), self.cur_offset, "lowering temporary")?;
                for &expected in leading_types.iter().rev() {
                    let found = self.pop_expect(offset, "br_on_non_null", expected)?;
                    values.push(found.reg);
                }
                values.reverse();
                // Not-taken path: leading values stay; the ref is consumed.
                for (&reg, &ty) in values.iter().zip(leading_types.iter()) {
                    self.push_stack(RegValue { reg, ty })?;
                }
                // Taken path forwards the ref as the last branch value.
                values.push(value.reg);
                self.finish_cond_ref_branch(
                    offset,
                    frame_pos,
                    loop_header,
                    values,
                    value.reg,
                    RefBranchKind::OnNonNull,
                )?;
            }
            Instr::Return => {
                let values = self.pop_results(offset)?;
                self.finish_block(RegTerm::Return { values })?;
                // Code after `return` is unreachable, but lowering continues:
                // instructions up to the function's final `end` must still be
                // processed (under polymorphic stack discipline).
                self.set_unreachable();
            }
            Instr::ReturnCall(func) => {
                let Some(callee_ty) = self.tables.func_types.get(func.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidFunction { func: func.0 },
                    });
                };
                let mut args = self.temp_vec(
                    callee_ty.params.len(),
                    self.cur_offset,
                    "lowering temporary",
                )?;
                for &expected in callee_ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "return_call", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                self.finish_block(RegTerm::TailCall { func, args })?;
                self.set_unreachable();
            }
            Instr::ReturnCallIndirect {
                type_idx,
                table_idx,
            } => {
                let Some(ty) = self.tables.func_type(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                let index = self.pop_expect(
                    offset,
                    "return_call_indirect",
                    ValType::Num(self.table_num_type(table_idx)),
                )?;
                let mut args =
                    self.temp_vec(ty.params.len(), self.cur_offset, "lowering temporary")?;
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "return_call_indirect", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                self.finish_block(RegTerm::TailCallIndirect {
                    type_idx,
                    table: table_idx,
                    index: index.reg,
                    args,
                })?;
                self.set_unreachable();
            }
            Instr::ReturnCallRef(type_idx) => {
                let Some(ty) = self.tables.func_type(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                // Stack order: [args..., funcref] — the reference is on top.
                let func = self.pop_any(offset, "return_call_ref")?;
                let mut args =
                    self.temp_vec(ty.params.len(), self.cur_offset, "lowering temporary")?;
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "return_call_ref", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                self.finish_block(RegTerm::TailCallRef {
                    type_idx,
                    func: func.reg,
                    args,
                })?;
                self.set_unreachable();
            }
            Instr::CallRef(type_idx) => {
                let Some(ty) = self.tables.func_type(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                // Stack order: [args..., funcref] — the reference is on top.
                let func = self.pop_any(offset, "call_ref")?;
                let mut args =
                    self.temp_vec(ty.params.len(), self.cur_offset, "lowering temporary")?;
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "call_ref", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                let mut results =
                    self.temp_vec(ty.results.len(), self.cur_offset, "lowering temporary")?;
                for &ty in ty.results.iter() {
                    let dst = self.alloc_reg(ty)?;
                    self.push_stack(RegValue { reg: dst, ty })?;
                    results.push(dst);
                }
                self.emit(
                    offset,
                    RegOp::CallRef {
                        type_idx,
                        func: func.reg,
                        args,
                        results,
                    },
                )?;
            }
            Instr::Call(func) => {
                let Some(callee_ty) = self.tables.func_types.get(func.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidFunction { func: func.0 },
                    });
                };
                let mut args = self.temp_vec(
                    callee_ty.params.len(),
                    self.cur_offset,
                    "lowering temporary",
                )?;
                for &expected in callee_ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "call", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                let mut results = self.temp_vec(
                    callee_ty.results.len(),
                    self.cur_offset,
                    "lowering temporary",
                )?;
                for &ty in callee_ty.results.iter() {
                    let dst = self.alloc_reg(ty)?;
                    self.push_stack(RegValue { reg: dst, ty })?;
                    results.push(dst);
                }
                self.emit(
                    offset,
                    RegOp::Call {
                        func,
                        args,
                        results,
                    },
                )?;
            }
            Instr::GlobalGet(global) => {
                let ty = self.global_type(offset, global)?;
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::GlobalGet { dst, global })?;
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
                )?;
            }
            Instr::MemorySize(memory) => {
                let ty = ValType::Num(self.memory_num_type(memory));
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::MemorySize { dst, memory })?;
            }
            Instr::MemoryGrow(memory) => {
                let addr_ty = ValType::Num(self.memory_num_type(memory));
                let delta = self.pop_expect(offset, "memory.grow", addr_ty)?;
                let ty = addr_ty;
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::MemoryGrow {
                        dst,
                        memory,
                        delta: delta.reg,
                    },
                )?;
            }
            Instr::MemoryInit(data_idx, mem_idx) => {
                let addr_ty = ValType::Num(self.memory_num_type(mem_idx));
                let count = self.pop_expect(offset, "memory.init", ValType::Num(NumType::I32))?;
                let src = self.pop_expect(offset, "memory.init", ValType::Num(NumType::I32))?;
                let dst = self.pop_expect(offset, "memory.init", addr_ty)?;
                self.emit(
                    offset,
                    RegOp::MemoryInit {
                        memory: mem_idx,
                        data: data_idx,
                        dst: dst.reg,
                        src: src.reg,
                        count: count.reg,
                    },
                )?;
            }
            Instr::DataDrop(data_idx) => {
                self.emit(offset, RegOp::DataDrop { data: data_idx })?;
            }
            Instr::MemoryCopy { dst, src } => {
                let dst_ty = self.memory_num_type(dst);
                let src_ty = self.memory_num_type(src);
                let count_ty = min_num_type(dst_ty, src_ty);
                let count = self.pop_expect(offset, "memory.copy", ValType::Num(count_ty))?;
                let src_idx = self.pop_expect(offset, "memory.copy", ValType::Num(src_ty))?;
                let dst_idx = self.pop_expect(offset, "memory.copy", ValType::Num(dst_ty))?;
                self.emit(
                    offset,
                    RegOp::MemoryCopy {
                        dst_memory: dst,
                        src_memory: src,
                        dst: dst_idx.reg,
                        src: src_idx.reg,
                        count: count.reg,
                    },
                )?;
            }
            Instr::MemoryFill(memory) => {
                let addr_ty = ValType::Num(self.memory_num_type(memory));
                let count = self.pop_expect(offset, "memory.fill", addr_ty)?;
                let value = self.pop_expect(offset, "memory.fill", ValType::Num(NumType::I32))?;
                let dst = self.pop_expect(offset, "memory.fill", addr_ty)?;
                self.emit(
                    offset,
                    RegOp::MemoryFill {
                        memory,
                        dst: dst.reg,
                        value: value.reg,
                        count: count.reg,
                    },
                )?;
            }
            Instr::CallIndirect {
                type_idx,
                table_idx,
            } => {
                let Some(ty) = self.tables.func_type(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                let index = self.pop_expect(
                    offset,
                    "call_indirect",
                    ValType::Num(self.table_num_type(table_idx)),
                )?;
                let mut args =
                    self.temp_vec(ty.params.len(), self.cur_offset, "lowering temporary")?;
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "call_indirect", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                let mut results =
                    self.temp_vec(ty.results.len(), self.cur_offset, "lowering temporary")?;
                for &result_ty in ty.results.iter() {
                    let dst = self.alloc_reg(result_ty)?;
                    self.push_stack(RegValue {
                        reg: dst,
                        ty: result_ty,
                    })?;
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
                )?;
            }
            Instr::TableGet(table) => {
                let index = self.pop_expect(
                    offset,
                    "table.get",
                    ValType::Num(self.table_num_type(table)),
                )?;
                let ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::TableGet {
                        dst,
                        table,
                        index: index.reg,
                    },
                )?;
            }
            Instr::TableSet(table) => {
                let ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let value = self.pop_expect(offset, "table.set", ty)?;
                let index = self.pop_expect(
                    offset,
                    "table.set",
                    ValType::Num(self.table_num_type(table)),
                )?;
                self.emit(
                    offset,
                    RegOp::TableSet {
                        table,
                        index: index.reg,
                        value: value.reg,
                    },
                )?;
            }
            Instr::TableSize(table) => {
                let ty = ValType::Num(self.table_num_type(table));
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::TableSize { dst, table })?;
            }
            Instr::TableGrow(table) => {
                let addr_ty = ValType::Num(self.table_num_type(table));
                let delta = self.pop_expect(offset, "table.grow", addr_ty)?;
                let elem_ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let value = self.pop_expect(offset, "table.grow", elem_ty)?;
                let ty = addr_ty;
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::TableGrow {
                        dst,
                        table,
                        value: value.reg,
                        delta: delta.reg,
                    },
                )?;
            }
            Instr::TableFill(table) => {
                let addr_ty = ValType::Num(self.table_num_type(table));
                let count = self.pop_expect(offset, "table.fill", addr_ty)?;
                let elem_ty = ValType::Ref(self.table_elem_type(offset, table)?);
                let value = self.pop_expect(offset, "table.fill", elem_ty)?;
                let dst_idx = self.pop_expect(offset, "table.fill", addr_ty)?;
                self.emit(
                    offset,
                    RegOp::TableFill {
                        table,
                        dst: dst_idx.reg,
                        value: value.reg,
                        count: count.reg,
                    },
                )?;
            }
            Instr::TableCopy { dst, src } => {
                let dst_ty = self.table_num_type(dst);
                let src_ty = self.table_num_type(src);
                let count_ty = min_num_type(dst_ty, src_ty);
                let count = self.pop_expect(offset, "table.copy", ValType::Num(count_ty))?;
                let src_idx = self.pop_expect(offset, "table.copy", ValType::Num(src_ty))?;
                let dst_idx = self.pop_expect(offset, "table.copy", ValType::Num(dst_ty))?;
                self.emit(
                    offset,
                    RegOp::TableCopy {
                        dst_table: dst,
                        src_table: src,
                        dst: dst_idx.reg,
                        src: src_idx.reg,
                        count: count.reg,
                    },
                )?;
            }
            Instr::TableInit {
                elem_idx,
                table_idx,
            } => {
                let addr_ty = ValType::Num(self.table_num_type(table_idx));
                let count = self.pop_expect(offset, "table.init", ValType::Num(NumType::I32))?;
                let src = self.pop_expect(offset, "table.init", ValType::Num(NumType::I32))?;
                let dst = self.pop_expect(offset, "table.init", addr_ty)?;
                self.emit(
                    offset,
                    RegOp::TableInit {
                        table: table_idx,
                        elem: elem_idx,
                        dst: dst.reg,
                        src: src.reg,
                        count: count.reg,
                    },
                )?;
            }
            Instr::ElemDrop(elem) => {
                self.emit(offset, RegOp::ElemDrop { elem })?;
            }
            Instr::RefNull(ref_type) => {
                let ty = ValType::Ref(ref_type);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::RefNull { dst, ref_type })?;
            }
            Instr::RefFunc(func) => {
                // Lowered as the nullable funcref type; the validator has
                // already proven the precise (non-null) type.
                let ty = ValType::Ref(crate::types::RefType::FuncRef);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::RefFunc { dst, func })?;
            }
            Instr::RefIsNull => {
                let value = self.pop_any(offset, "ref.is_null")?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::RefIsNull {
                        dst,
                        value: value.reg,
                    },
                )?;
            }
            Instr::RefAsNonNull => {
                let value = self.pop_any(offset, "ref.as_non_null")?;
                // In unreachable code the synthesized operand has a default
                // type; the result must still be a reference for downstream
                // consumers.
                let ty = match value.ty {
                    ValType::Ref(_) => value.ty,
                    _ => ValType::Ref(crate::types::RefType::FuncRef),
                };
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::RefAsNonNull {
                        dst,
                        value: value.reg,
                    },
                )?;
            }
            Instr::StructNew(type_idx) => {
                let field_types = self.gc_struct_field_types(offset, type_idx)?;
                let mut fields =
                    self.temp_vec(field_types.len(), self.cur_offset, "lowering temporary")?;
                for &field_ty in field_types.iter().rev() {
                    fields.push(self.pop_expect(offset, "struct.new", field_ty)?.reg);
                }
                fields.reverse();
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::StructNew {
                        dst,
                        type_idx,
                        fields,
                    },
                )?;
            }
            Instr::StructNewDefault(type_idx) => {
                let _ = self.gc_struct_field_types(offset, type_idx)?;
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::StructNewDefault { dst, type_idx })?;
            }
            Instr::StructGet { type_idx, field } => {
                self.lower_gc_field_get(offset, type_idx, field, GcExt::None)?;
            }
            Instr::StructGetS { type_idx, field } => {
                self.lower_gc_field_get(offset, type_idx, field, GcExt::S)?;
            }
            Instr::StructGetU { type_idx, field } => {
                self.lower_gc_field_get(offset, type_idx, field, GcExt::U)?;
            }
            Instr::StructSet { type_idx, field } => {
                let field_ty = self.gc_struct_field(offset, type_idx, field)?;
                let value = self.pop_expect(offset, "struct.set", field_ty)?;
                let object =
                    self.pop_expect(offset, "struct.set", gc_ref_type_nullable(type_idx))?;
                self.emit(
                    offset,
                    RegOp::StructSet {
                        type_idx,
                        field,
                        object: object.reg,
                        value: value.reg,
                    },
                )?;
            }
            Instr::ArrayNew(type_idx) => {
                let field_ty = self.gc_array_field(offset, type_idx)?;
                let len = self.pop_expect(offset, "array.new", ValType::Num(NumType::I32))?;
                let value = self.pop_expect(offset, "array.new", field_ty)?;
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ArrayNew {
                        dst,
                        type_idx,
                        len: len.reg,
                        value: value.reg,
                    },
                )?;
            }
            Instr::ArrayNewDefault(type_idx) => {
                let _ = self.gc_array_field(offset, type_idx)?;
                let len =
                    self.pop_expect(offset, "array.new_default", ValType::Num(NumType::I32))?;
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ArrayNewDefault {
                        dst,
                        type_idx,
                        len: len.reg,
                    },
                )?;
            }
            Instr::ArrayNewFixed { type_idx, count } => {
                let field_ty = self.gc_array_field(offset, type_idx)?;
                // Mirror the validator's polymorphic-stack rule: in an
                // unreachable frame at the frame boundary, `count` operands
                // are conceptually present without existing. Do NOT
                // materialize `count` registers (count is a u32 LEB, up to
                // u32::MAX) — a huge count in dead code must lower without
                // allocating. The emitted term never executes because the
                // surrounding code is unreachable.
                let mut values = Vec::new();
                for _ in 0..count {
                    if self.at_frame_boundary() && self.current_frame_unreachable() {
                        break;
                    }
                    let field = self.pop_expect(offset, "array.new_fixed", field_ty)?;
                    push_fallible(
                        &mut values,
                        field.reg,
                        offset,
                        Some(self.func_idx),
                        "array.new_fixed operands",
                    )?;
                }
                values.reverse();
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ArrayNewFixed {
                        dst,
                        type_idx,
                        values,
                    },
                )?;
            }
            Instr::ArrayNewData { type_idx, data } => {
                let _ = self.gc_array_field(offset, type_idx)?;
                let len = self.pop_expect(offset, "array.new_data", ValType::Num(NumType::I32))?;
                let src = self.pop_expect(offset, "array.new_data", ValType::Num(NumType::I32))?;
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ArrayNewData {
                        dst,
                        type_idx,
                        data,
                        src: src.reg,
                        len: len.reg,
                    },
                )?;
            }
            Instr::ArrayNewElem { type_idx, elem } => {
                let _ = self.gc_array_field(offset, type_idx)?;
                let len = self.pop_expect(offset, "array.new_elem", ValType::Num(NumType::I32))?;
                let src = self.pop_expect(offset, "array.new_elem", ValType::Num(NumType::I32))?;
                let ty = gc_ref_type(type_idx);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ArrayNewElem {
                        dst,
                        type_idx,
                        elem,
                        src: src.reg,
                        len: len.reg,
                    },
                )?;
            }
            Instr::ArrayGet(type_idx) => {
                self.lower_gc_array_get(offset, type_idx, GcExt::None)?;
            }
            Instr::ArrayGetS(type_idx) => {
                self.lower_gc_array_get(offset, type_idx, GcExt::S)?;
            }
            Instr::ArrayGetU(type_idx) => {
                self.lower_gc_array_get(offset, type_idx, GcExt::U)?;
            }
            Instr::ArraySet(type_idx) => {
                let field_ty = self.gc_array_field(offset, type_idx)?;
                let value = self.pop_expect(offset, "array.set", field_ty)?;
                let index = self.pop_expect(offset, "array.set", ValType::Num(NumType::I32))?;
                let array = self.pop_expect(offset, "array.set", gc_ref_type_nullable(type_idx))?;
                self.emit(
                    offset,
                    RegOp::ArraySet {
                        type_idx,
                        array: array.reg,
                        index: index.reg,
                        value: value.reg,
                    },
                )?;
            }
            Instr::ArrayLen => {
                let array = self.pop_any(offset, "array.len")?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ArrayLen {
                        dst,
                        array: array.reg,
                    },
                )?;
            }
            Instr::ArrayFill(type_idx) => {
                let field_ty = self.gc_array_field(offset, type_idx)?;
                let len = self.pop_expect(offset, "array.fill", ValType::Num(NumType::I32))?;
                let value = self.pop_expect(offset, "array.fill", field_ty)?;
                let index = self.pop_expect(offset, "array.fill", ValType::Num(NumType::I32))?;
                let array =
                    self.pop_expect(offset, "array.fill", gc_ref_type_nullable(type_idx))?;
                self.emit(
                    offset,
                    RegOp::ArrayFill {
                        type_idx,
                        array: array.reg,
                        index: index.reg,
                        value: value.reg,
                        len: len.reg,
                    },
                )?;
            }
            Instr::ArrayCopy { dst, src } => {
                // dst_type and src_type must share storage; the validator
                // proved it. Lowering only needs the reference types.
                let dst_field = self.gc_array_field(offset, dst)?;
                let _ = self.gc_array_field(offset, src)?;
                let len = self.pop_expect(offset, "array.copy", ValType::Num(NumType::I32))?;
                let src_off = self.pop_expect(offset, "array.copy", ValType::Num(NumType::I32))?;
                let src_ref = self.pop_expect(offset, "array.copy", gc_ref_type_nullable(src))?;
                let dst_off = self.pop_expect(offset, "array.copy", ValType::Num(NumType::I32))?;
                let dst_ref = self.pop_expect(offset, "array.copy", gc_ref_type_nullable(dst))?;
                let _ = dst_field;
                self.emit(
                    offset,
                    RegOp::ArrayCopy {
                        dst_type: dst,
                        src_type: src,
                        dst: dst_ref.reg,
                        dst_off: dst_off.reg,
                        src: src_ref.reg,
                        src_off: src_off.reg,
                        len: len.reg,
                    },
                )?;
            }
            Instr::ArrayInitData { type_idx, data } => {
                let _ = self.gc_array_field(offset, type_idx)?;
                let len = self.pop_expect(offset, "array.init_data", ValType::Num(NumType::I32))?;
                let src_off =
                    self.pop_expect(offset, "array.init_data", ValType::Num(NumType::I32))?;
                let dst_off =
                    self.pop_expect(offset, "array.init_data", ValType::Num(NumType::I32))?;
                let array =
                    self.pop_expect(offset, "array.init_data", gc_ref_type_nullable(type_idx))?;
                self.emit(
                    offset,
                    RegOp::ArrayInitData {
                        type_idx,
                        data,
                        array: array.reg,
                        dst_off: dst_off.reg,
                        src_off: src_off.reg,
                        len: len.reg,
                    },
                )?;
            }
            Instr::ArrayInitElem { type_idx, elem } => {
                let _ = self.gc_array_field(offset, type_idx)?;
                let len = self.pop_expect(offset, "array.init_elem", ValType::Num(NumType::I32))?;
                let src_off =
                    self.pop_expect(offset, "array.init_elem", ValType::Num(NumType::I32))?;
                let dst_off =
                    self.pop_expect(offset, "array.init_elem", ValType::Num(NumType::I32))?;
                let array =
                    self.pop_expect(offset, "array.init_elem", gc_ref_type_nullable(type_idx))?;
                self.emit(
                    offset,
                    RegOp::ArrayInitElem {
                        type_idx,
                        elem,
                        array: array.reg,
                        dst_off: dst_off.reg,
                        src_off: src_off.reg,
                        len: len.reg,
                    },
                )?;
            }
            Instr::RefI31 => {
                let value = self.pop_expect(offset, "ref.i31", ValType::Num(NumType::I32))?;
                let ty = ValType::Ref(crate::types::RefType::from_parts(false, HeapType::I31));
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::RefI31 {
                        dst,
                        value: value.reg,
                    },
                )?;
            }
            Instr::I31GetS => {
                let value = self.pop_expect(
                    offset,
                    "i31.get_s",
                    ValType::Ref(crate::types::RefType::from_parts(true, HeapType::I31)),
                )?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::I31Get {
                        dst,
                        value: value.reg,
                        signed: true,
                    },
                )?;
            }
            Instr::I31GetU => {
                let value = self.pop_expect(
                    offset,
                    "i31.get_u",
                    ValType::Ref(crate::types::RefType::from_parts(true, HeapType::I31)),
                )?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::I31Get {
                        dst,
                        value: value.reg,
                        signed: false,
                    },
                )?;
            }
            Instr::RefTest { nullable, ht } => {
                let operand = self.ref_operand_type(ht);
                let value = self.pop_expect(offset, "ref.test", operand)?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::RefTest {
                        dst,
                        value: value.reg,
                        nullable,
                        ht,
                    },
                )?;
            }
            Instr::RefCast { nullable, ht } => {
                let operand = self.ref_operand_type(ht);
                let value = self.pop_expect(offset, "ref.cast", operand)?;
                let ty = ValType::Ref(crate::types::RefType::from_parts(nullable, ht));
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::RefCast {
                        dst,
                        value: value.reg,
                        nullable,
                        ht,
                    },
                )?;
            }
            Instr::RefEq => {
                let eq = ValType::Ref(crate::types::RefType::from_parts(true, HeapType::Eq));
                let rhs = self.pop_expect(offset, "ref.eq", eq)?;
                let lhs = self.pop_expect(offset, "ref.eq", eq)?;
                let ty = ValType::Num(NumType::I32);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::RefEq {
                        dst,
                        lhs: lhs.reg,
                        rhs: rhs.reg,
                    },
                )?;
            }
            Instr::BrOnCast {
                fail,
                src,
                tgt,
                label,
            } => {
                self.lower_br_on_cast(offset, fail, src, tgt, label)?;
            }
            Instr::AnyConvertExtern => {
                let operand =
                    ValType::Ref(crate::types::RefType::from_parts(true, HeapType::Extern));
                let value = self.pop_expect(offset, "any.convert_extern", operand)?;
                let nullable = match value.ty {
                    ValType::Ref(ref_type) => ref_type.is_nullable(),
                    _ => true,
                };
                let ty = ValType::Ref(crate::types::RefType::from_parts(nullable, HeapType::Any));
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::AnyConvertExtern {
                        dst,
                        value: value.reg,
                    },
                )?;
            }
            Instr::ExternConvertAny => {
                let operand = ValType::Ref(crate::types::RefType::from_parts(true, HeapType::Any));
                let value = self.pop_expect(offset, "extern.convert_any", operand)?;
                let nullable = match value.ty {
                    ValType::Ref(ref_type) => ref_type.is_nullable(),
                    _ => true,
                };
                let ty = ValType::Ref(crate::types::RefType::from_parts(
                    nullable,
                    HeapType::Extern,
                ));
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(
                    offset,
                    RegOp::ExternConvertAny {
                        dst,
                        value: value.reg,
                    },
                )?;
            }
            Instr::V128Const(value) => {
                let ty = ValType::Vec(crate::types::VecType::V128);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::V128Const { dst, value })?;
            }
            Instr::I8x16Splat => self.lower_splat(offset, LaneShape::I8x16)?,
            Instr::I16x8Splat => self.lower_splat(offset, LaneShape::I16x8)?,
            Instr::I32x4Splat => self.lower_splat(offset, LaneShape::I32x4)?,
            Instr::I64x2Splat => self.lower_splat(offset, LaneShape::I64x2)?,
            Instr::F32x4Splat => self.lower_splat(offset, LaneShape::F32x4)?,
            Instr::F64x2Splat => self.lower_splat(offset, LaneShape::F64x2)?,
            Instr::I32x4ExtractLane(lane) => {
                self.lower_extract_lane(offset, LaneShape::I32x4, lane, true)?
            }
            Instr::F32x4ExtractLane(lane) => {
                self.lower_extract_lane(offset, LaneShape::F32x4, lane, true)?
            }
            Instr::I32x4ReplaceLane(lane) => {
                self.lower_replace_lane(offset, LaneShape::I32x4, lane)?
            }
            Instr::F32x4ReplaceLane(lane) => {
                self.lower_replace_lane(offset, LaneShape::F32x4, lane)?
            }
            Instr::I8x16ExtractLaneS(lane) => {
                self.lower_extract_lane(offset, LaneShape::I8x16, lane, true)?
            }
            Instr::I8x16ExtractLaneU(lane) => {
                self.lower_extract_lane(offset, LaneShape::I8x16, lane, false)?
            }
            Instr::I8x16ReplaceLane(lane) => {
                self.lower_replace_lane(offset, LaneShape::I8x16, lane)?
            }
            Instr::I16x8ExtractLaneS(lane) => {
                self.lower_extract_lane(offset, LaneShape::I16x8, lane, true)?
            }
            Instr::I16x8ExtractLaneU(lane) => {
                self.lower_extract_lane(offset, LaneShape::I16x8, lane, false)?
            }
            Instr::I16x8ReplaceLane(lane) => {
                self.lower_replace_lane(offset, LaneShape::I16x8, lane)?
            }
            Instr::I64x2ExtractLane(lane) => {
                self.lower_extract_lane(offset, LaneShape::I64x2, lane, true)?
            }
            Instr::I64x2ReplaceLane(lane) => {
                self.lower_replace_lane(offset, LaneShape::I64x2, lane)?
            }
            Instr::F64x2ExtractLane(lane) => {
                self.lower_extract_lane(offset, LaneShape::F64x2, lane, true)?
            }
            Instr::F64x2ReplaceLane(lane) => {
                self.lower_replace_lane(offset, LaneShape::F64x2, lane)?
            }
            Instr::V128Not => {
                let src = self.pop_expect(
                    offset,
                    "v128.not",
                    ValType::Vec(crate::types::VecType::V128),
                )?;
                let ty = ValType::Vec(crate::types::VecType::V128);
                let dst = self.alloc_reg(ty)?;
                self.push_stack(RegValue { reg: dst, ty })?;
                self.emit(offset, RegOp::V128Not { dst, src: src.reg })?;
            }
            Instr::V128And => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::And)?
            }
            Instr::V128Or => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::Or)?
            }
            Instr::V128Xor => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::Xor)?
            }
            Instr::V128Andnot => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::Andnot)?
            }
            Instr::I8x16Add => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::Add)?
            }
            Instr::I8x16Sub => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::Sub)?
            }
            Instr::I16x8Add => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::Add)?
            }
            Instr::I16x8Mul => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::Mul)?
            }
            Instr::I16x8Sub => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::Sub)?
            }
            Instr::I32x4Add => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::Add)?
            }
            Instr::I32x4Sub => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::Sub)?
            }
            Instr::I32x4Mul => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::Mul)?
            }
            Instr::I64x2Add => {
                self.lower_v128_binary(offset, LaneShape::I64x2, V128BinaryKind::Add)?
            }
            Instr::I64x2Mul => {
                self.lower_v128_binary(offset, LaneShape::I64x2, V128BinaryKind::Mul)?
            }
            Instr::I64x2Sub => {
                self.lower_v128_binary(offset, LaneShape::I64x2, V128BinaryKind::Sub)?
            }
            Instr::F32x4Add => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::Add)?
            }
            Instr::F32x4Sub => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::Sub)?
            }
            Instr::F32x4Mul => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::Mul)?
            }
            Instr::F32x4Div => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::Div)?
            }
            Instr::F64x2Add => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::Add)?
            }
            Instr::F64x2Sub => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::Sub)?
            }
            Instr::F64x2Mul => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::Mul)?
            }
            Instr::F64x2Div => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::Div)?
            }
            Instr::I8x16AddSatS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::AddSatS)?
            }
            Instr::I8x16AddSatU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::AddSatU)?
            }
            Instr::I8x16SubSatS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::SubSatS)?
            }
            Instr::I8x16SubSatU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::SubSatU)?
            }
            Instr::I8x16MinS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MinS)?
            }
            Instr::I8x16MinU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MinU)?
            }
            Instr::I8x16MaxS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MaxS)?
            }
            Instr::I8x16MaxU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MaxU)?
            }
            Instr::I8x16AvgrU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::AvgrU)?
            }
            Instr::I16x8AddSatS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::AddSatS)?
            }
            Instr::I16x8AddSatU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::AddSatU)?
            }
            Instr::I16x8SubSatS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::SubSatS)?
            }
            Instr::I16x8SubSatU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::SubSatU)?
            }
            Instr::I16x8MinS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MinS)?
            }
            Instr::I16x8MinU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MinU)?
            }
            Instr::I16x8MaxS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MaxS)?
            }
            Instr::I16x8MaxU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MaxU)?
            }
            Instr::I16x8AvgrU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::AvgrU)?
            }
            Instr::I16x8Q15MulrSatS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::Q15MulrSatS)?
            }
            Instr::I32x4MinS => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MinS)?
            }
            Instr::I32x4MinU => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MinU)?
            }
            Instr::I32x4MaxS => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MaxS)?
            }
            Instr::I32x4MaxU => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MaxU)?
            }
            Instr::I8x16Abs => {
                self.lower_v128_unary(offset, LaneShape::I8x16, V128UnaryKind::Abs)?
            }
            Instr::I8x16Neg => {
                self.lower_v128_unary(offset, LaneShape::I8x16, V128UnaryKind::Neg)?
            }
            Instr::I8x16Popcnt => {
                self.lower_v128_unary(offset, LaneShape::I8x16, V128UnaryKind::Popcnt)?
            }
            Instr::I16x8Abs => {
                self.lower_v128_unary(offset, LaneShape::I16x8, V128UnaryKind::Abs)?
            }
            Instr::I16x8Neg => {
                self.lower_v128_unary(offset, LaneShape::I16x8, V128UnaryKind::Neg)?
            }
            Instr::I32x4Abs => {
                self.lower_v128_unary(offset, LaneShape::I32x4, V128UnaryKind::Abs)?
            }
            Instr::I32x4Neg => {
                self.lower_v128_unary(offset, LaneShape::I32x4, V128UnaryKind::Neg)?
            }
            Instr::I64x2Abs => {
                self.lower_v128_unary(offset, LaneShape::I64x2, V128UnaryKind::Abs)?
            }
            Instr::I64x2Neg => {
                self.lower_v128_unary(offset, LaneShape::I64x2, V128UnaryKind::Neg)?
            }
            Instr::V128Bitselect => self.lower_v128_bitselect(offset)?,
            Instr::V128AnyTrue => self.lower_v128_any_true(offset)?,
            Instr::I8x16AllTrue => self.lower_v128_all_true(offset, LaneShape::I8x16)?,
            Instr::I8x16Bitmask => self.lower_v128_bitmask(offset, LaneShape::I8x16)?,
            Instr::I16x8AllTrue => self.lower_v128_all_true(offset, LaneShape::I16x8)?,
            Instr::I16x8Bitmask => self.lower_v128_bitmask(offset, LaneShape::I16x8)?,
            Instr::I32x4AllTrue => self.lower_v128_all_true(offset, LaneShape::I32x4)?,
            Instr::I32x4Bitmask => self.lower_v128_bitmask(offset, LaneShape::I32x4)?,
            Instr::I64x2AllTrue => self.lower_v128_all_true(offset, LaneShape::I64x2)?,
            Instr::I64x2Bitmask => self.lower_v128_bitmask(offset, LaneShape::I64x2)?,
            Instr::I8x16NarrowI16x8S => self.lower_v128_narrow(offset, LaneShape::I8x16, true)?,
            Instr::I8x16NarrowI16x8U => self.lower_v128_narrow(offset, LaneShape::I8x16, false)?,
            Instr::I16x8NarrowI32x4S => self.lower_v128_narrow(offset, LaneShape::I16x8, true)?,
            Instr::I16x8NarrowI32x4U => self.lower_v128_narrow(offset, LaneShape::I16x8, false)?,
            Instr::I16x8ExtAddPairwiseI8x16S => {
                self.lower_v128_extadd_pairwise(offset, LaneShape::I8x16, true)?
            }
            Instr::I16x8ExtAddPairwiseI8x16U => {
                self.lower_v128_extadd_pairwise(offset, LaneShape::I8x16, false)?
            }
            Instr::I32x4ExtAddPairwiseI16x8S => {
                self.lower_v128_extadd_pairwise(offset, LaneShape::I16x8, true)?
            }
            Instr::I32x4ExtAddPairwiseI16x8U => {
                self.lower_v128_extadd_pairwise(offset, LaneShape::I16x8, false)?
            }
            Instr::I16x8ExtendLowI8x16S => {
                self.lower_v128_extend(offset, LaneShape::I8x16, false, true)?
            }
            Instr::I16x8ExtendHighI8x16S => {
                self.lower_v128_extend(offset, LaneShape::I8x16, true, true)?
            }
            Instr::I16x8ExtendLowI8x16U => {
                self.lower_v128_extend(offset, LaneShape::I8x16, false, false)?
            }
            Instr::I16x8ExtendHighI8x16U => {
                self.lower_v128_extend(offset, LaneShape::I8x16, true, false)?
            }
            Instr::I32x4ExtendLowI16x8S => {
                self.lower_v128_extend(offset, LaneShape::I16x8, false, true)?
            }
            Instr::I32x4ExtendHighI16x8S => {
                self.lower_v128_extend(offset, LaneShape::I16x8, true, true)?
            }
            Instr::I32x4ExtendLowI16x8U => {
                self.lower_v128_extend(offset, LaneShape::I16x8, false, false)?
            }
            Instr::I32x4ExtendHighI16x8U => {
                self.lower_v128_extend(offset, LaneShape::I16x8, true, false)?
            }
            Instr::I64x2ExtendLowI32x4S => {
                self.lower_v128_extend(offset, LaneShape::I32x4, false, true)?
            }
            Instr::I64x2ExtendHighI32x4S => {
                self.lower_v128_extend(offset, LaneShape::I32x4, true, true)?
            }
            Instr::I64x2ExtendLowI32x4U => {
                self.lower_v128_extend(offset, LaneShape::I32x4, false, false)?
            }
            Instr::I64x2ExtendHighI32x4U => {
                self.lower_v128_extend(offset, LaneShape::I32x4, true, false)?
            }
            Instr::I16x8ExtMulLowI8x16S => {
                self.lower_v128_extmul(offset, LaneShape::I8x16, false, true)?
            }
            Instr::I16x8ExtMulHighI8x16S => {
                self.lower_v128_extmul(offset, LaneShape::I8x16, true, true)?
            }
            Instr::I16x8ExtMulLowI8x16U => {
                self.lower_v128_extmul(offset, LaneShape::I8x16, false, false)?
            }
            Instr::I16x8ExtMulHighI8x16U => {
                self.lower_v128_extmul(offset, LaneShape::I8x16, true, false)?
            }
            Instr::I32x4ExtMulLowI16x8S => {
                self.lower_v128_extmul(offset, LaneShape::I16x8, false, true)?
            }
            Instr::I32x4ExtMulHighI16x8S => {
                self.lower_v128_extmul(offset, LaneShape::I16x8, true, true)?
            }
            Instr::I32x4ExtMulLowI16x8U => {
                self.lower_v128_extmul(offset, LaneShape::I16x8, false, false)?
            }
            Instr::I32x4ExtMulHighI16x8U => {
                self.lower_v128_extmul(offset, LaneShape::I16x8, true, false)?
            }
            Instr::I64x2ExtMulLowI32x4S => {
                self.lower_v128_extmul(offset, LaneShape::I32x4, false, true)?
            }
            Instr::I64x2ExtMulHighI32x4S => {
                self.lower_v128_extmul(offset, LaneShape::I32x4, true, true)?
            }
            Instr::I64x2ExtMulLowI32x4U => {
                self.lower_v128_extmul(offset, LaneShape::I32x4, false, false)?
            }
            Instr::I64x2ExtMulHighI32x4U => {
                self.lower_v128_extmul(offset, LaneShape::I32x4, true, false)?
            }
            Instr::I32x4DotI16x8S => self.lower_v128_dot(offset)?,
            Instr::I8x16Eq => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::Eq)?
            }
            Instr::I8x16Ne => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::Ne)?
            }
            Instr::I8x16LtS => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::LtS)?
            }
            Instr::I8x16LtU => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::LtU)?
            }
            Instr::I8x16GtS => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::GtS)?
            }
            Instr::I8x16GtU => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::GtU)?
            }
            Instr::I8x16LeS => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::LeS)?
            }
            Instr::I8x16LeU => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::LeU)?
            }
            Instr::I8x16GeS => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::GeS)?
            }
            Instr::I8x16GeU => {
                self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::GeU)?
            }
            Instr::I16x8Eq => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::Eq)?
            }
            Instr::I16x8Ne => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::Ne)?
            }
            Instr::I16x8LtS => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::LtS)?
            }
            Instr::I16x8LtU => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::LtU)?
            }
            Instr::I16x8GtS => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::GtS)?
            }
            Instr::I16x8GtU => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::GtU)?
            }
            Instr::I16x8LeS => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::LeS)?
            }
            Instr::I16x8LeU => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::LeU)?
            }
            Instr::I16x8GeS => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::GeS)?
            }
            Instr::I16x8GeU => {
                self.lower_v128_compare(offset, LaneShape::I16x8, V128CompareKind::GeU)?
            }
            Instr::I32x4Eq => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::Eq)?
            }
            Instr::I32x4Ne => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::Ne)?
            }
            Instr::I32x4LtS => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::LtS)?
            }
            Instr::I32x4LtU => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::LtU)?
            }
            Instr::I32x4GtS => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::GtS)?
            }
            Instr::I32x4GtU => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::GtU)?
            }
            Instr::I32x4LeS => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::LeS)?
            }
            Instr::I32x4LeU => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::LeU)?
            }
            Instr::I32x4GeS => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::GeS)?
            }
            Instr::I32x4GeU => {
                self.lower_v128_compare(offset, LaneShape::I32x4, V128CompareKind::GeU)?
            }
            Instr::F32x4Eq => {
                self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Eq)?
            }
            Instr::F32x4Ne => {
                self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Ne)?
            }
            Instr::F32x4Lt => {
                self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Lt)?
            }
            Instr::F32x4Gt => {
                self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Gt)?
            }
            Instr::F32x4Le => {
                self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Le)?
            }
            Instr::F32x4Ge => {
                self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Ge)?
            }
            Instr::F64x2Eq => {
                self.lower_v128_compare(offset, LaneShape::F64x2, V128CompareKind::Eq)?
            }
            Instr::F64x2Ne => {
                self.lower_v128_compare(offset, LaneShape::F64x2, V128CompareKind::Ne)?
            }
            Instr::F64x2Lt => {
                self.lower_v128_compare(offset, LaneShape::F64x2, V128CompareKind::Lt)?
            }
            Instr::F64x2Gt => {
                self.lower_v128_compare(offset, LaneShape::F64x2, V128CompareKind::Gt)?
            }
            Instr::F64x2Le => {
                self.lower_v128_compare(offset, LaneShape::F64x2, V128CompareKind::Le)?
            }
            Instr::F64x2Ge => {
                self.lower_v128_compare(offset, LaneShape::F64x2, V128CompareKind::Ge)?
            }
            Instr::I64x2Eq => {
                self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::Eq)?
            }
            Instr::I64x2Ne => {
                self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::Ne)?
            }
            Instr::I64x2LtS => {
                self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::LtS)?
            }
            Instr::I64x2GtS => {
                self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::GtS)?
            }
            Instr::I64x2LeS => {
                self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::LeS)?
            }
            Instr::I64x2GeS => {
                self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::GeS)?
            }
            Instr::I8x16Shl => {
                self.lower_v128_shift(offset, LaneShape::I8x16, V128ShiftKind::Shl)?
            }
            Instr::I8x16ShrS => {
                self.lower_v128_shift(offset, LaneShape::I8x16, V128ShiftKind::ShrS)?
            }
            Instr::I8x16ShrU => {
                self.lower_v128_shift(offset, LaneShape::I8x16, V128ShiftKind::ShrU)?
            }
            Instr::I16x8Shl => {
                self.lower_v128_shift(offset, LaneShape::I16x8, V128ShiftKind::Shl)?
            }
            Instr::I16x8ShrS => {
                self.lower_v128_shift(offset, LaneShape::I16x8, V128ShiftKind::ShrS)?
            }
            Instr::I16x8ShrU => {
                self.lower_v128_shift(offset, LaneShape::I16x8, V128ShiftKind::ShrU)?
            }
            Instr::I32x4Shl => {
                self.lower_v128_shift(offset, LaneShape::I32x4, V128ShiftKind::Shl)?
            }
            Instr::I32x4ShrS => {
                self.lower_v128_shift(offset, LaneShape::I32x4, V128ShiftKind::ShrS)?
            }
            Instr::I32x4ShrU => {
                self.lower_v128_shift(offset, LaneShape::I32x4, V128ShiftKind::ShrU)?
            }
            Instr::I64x2Shl => {
                self.lower_v128_shift(offset, LaneShape::I64x2, V128ShiftKind::Shl)?
            }
            Instr::I64x2ShrS => {
                self.lower_v128_shift(offset, LaneShape::I64x2, V128ShiftKind::ShrS)?
            }
            Instr::I64x2ShrU => {
                self.lower_v128_shift(offset, LaneShape::I64x2, V128ShiftKind::ShrU)?
            }
            Instr::F32x4Abs => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Abs)?
            }
            Instr::F32x4Neg => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Neg)?
            }
            Instr::F32x4Sqrt => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Sqrt)?
            }
            Instr::F32x4Ceil => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Ceil)?
            }
            Instr::F32x4Floor => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Floor)?
            }
            Instr::F32x4Trunc => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Trunc)?
            }
            Instr::F32x4Nearest => {
                self.lower_v128_float_unary(offset, LaneShape::F32x4, V128FloatUnaryKind::Nearest)?
            }
            Instr::F64x2Abs => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Abs)?
            }
            Instr::F64x2Neg => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Neg)?
            }
            Instr::F64x2Sqrt => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Sqrt)?
            }
            Instr::F64x2Ceil => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Ceil)?
            }
            Instr::F64x2Floor => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Floor)?
            }
            Instr::F64x2Trunc => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Trunc)?
            }
            Instr::F64x2Nearest => {
                self.lower_v128_float_unary(offset, LaneShape::F64x2, V128FloatUnaryKind::Nearest)?
            }
            Instr::F32x4Min => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::FMin)?
            }
            Instr::F32x4Max => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::FMax)?
            }
            Instr::F32x4PMin => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::PMin)?
            }
            Instr::F32x4PMax => {
                self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::PMax)?
            }
            Instr::F64x2Min => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::FMin)?
            }
            Instr::F64x2Max => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::FMax)?
            }
            Instr::F64x2PMin => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::PMin)?
            }
            Instr::F64x2PMax => {
                self.lower_v128_binary(offset, LaneShape::F64x2, V128BinaryKind::PMax)?
            }
            Instr::I32x4TruncSatF32x4S => {
                self.lower_v128_convert(offset, V128ConvertKind::TruncSatF32x4S)?
            }
            Instr::I32x4TruncSatF32x4U => {
                self.lower_v128_convert(offset, V128ConvertKind::TruncSatF32x4U)?
            }
            Instr::F32x4ConvertI32x4S => {
                self.lower_v128_convert(offset, V128ConvertKind::ConvertI32x4S)?
            }
            Instr::F32x4ConvertI32x4U => {
                self.lower_v128_convert(offset, V128ConvertKind::ConvertI32x4U)?
            }
            Instr::I32x4TruncSatF64x2SZero => {
                self.lower_v128_convert(offset, V128ConvertKind::TruncSatF64x2SZero)?
            }
            Instr::I32x4TruncSatF64x2UZero => {
                self.lower_v128_convert(offset, V128ConvertKind::TruncSatF64x2UZero)?
            }
            Instr::F32x4DemoteF64x2Zero => {
                self.lower_v128_convert(offset, V128ConvertKind::DemoteF64x2Zero)?
            }
            Instr::F64x2PromoteLowF32x4 => {
                self.lower_v128_convert(offset, V128ConvertKind::PromoteLowF32x4)?
            }
            Instr::F64x2ConvertLowI32x4S => {
                self.lower_v128_convert(offset, V128ConvertKind::ConvertLowI32x4S)?
            }
            Instr::F64x2ConvertLowI32x4U => {
                self.lower_v128_convert(offset, V128ConvertKind::ConvertLowI32x4U)?
            }
            Instr::I8x16Swizzle => self.lower_v128_swizzle(offset)?,
            Instr::I8x16Shuffle(lanes) => self.lower_v128_shuffle(offset, lanes)?,
            Instr::V128Load8Lane { memarg, lane } => {
                self.lower_v128_load_lane(offset, 1, memarg, lane)?
            }
            Instr::V128Load16Lane { memarg, lane } => {
                self.lower_v128_load_lane(offset, 2, memarg, lane)?
            }
            Instr::V128Load32Lane { memarg, lane } => {
                self.lower_v128_load_lane(offset, 4, memarg, lane)?
            }
            Instr::V128Load64Lane { memarg, lane } => {
                self.lower_v128_load_lane(offset, 8, memarg, lane)?
            }
            Instr::V128Store8Lane { memarg, lane } => {
                self.lower_v128_store_lane(offset, 1, memarg, lane)?
            }
            Instr::V128Store16Lane { memarg, lane } => {
                self.lower_v128_store_lane(offset, 2, memarg, lane)?
            }
            Instr::V128Store32Lane { memarg, lane } => {
                self.lower_v128_store_lane(offset, 4, memarg, lane)?
            }
            Instr::V128Store64Lane { memarg, lane } => {
                self.lower_v128_store_lane(offset, 8, memarg, lane)?
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

    fn alloc_reg(&mut self, ty: ValType) -> Result<Reg, LowerError> {
        let reg = Reg(self.reg_types.len() as u32);
        push_fallible(
            &mut self.reg_types,
            ty,
            self.cur_offset,
            Some(self.func_idx),
            "register types",
        )?;
        Ok(reg)
    }

    fn emit(&mut self, offset: ByteOffset, op: RegOp) -> Result<(), LowerError> {
        push_fallible(
            &mut self.current_instrs,
            RegInstr { offset, op },
            offset,
            Some(self.func_idx),
            "lowered instructions",
        )
    }

    /// Push a value onto the lowering operand stack, growing it fallibly.
    fn push_stack(&mut self, value: RegValue) -> Result<(), LowerError> {
        push_fallible(
            &mut self.stack,
            value,
            self.cur_offset,
            Some(self.func_idx),
            "operand stack",
        )
    }

    /// Fallibly reserve `len` elements in a fresh temporary vector anchored at
    /// the instruction being lowered.
    fn temp_vec<T>(
        &self,
        len: usize,
        offset: ByteOffset,
        what: &'static str,
    ) -> Result<Vec<T>, LowerError> {
        vec_with_capacity(len, offset, Some(self.func_idx), what)
    }

    /// Resolve a struct type's unpacked field value types in declaration
    /// order. Lowering runs on validated modules, so a miss is a defensive
    /// `InvalidType` (matching the validator's `UnknownStruct`).
    fn gc_struct_field_types(
        &self,
        offset: ByteOffset,
        type_idx: TypeIdx,
    ) -> Result<Vec<ValType>, LowerError> {
        match self
            .tables
            .types
            .get(type_idx.0 as usize)
            .map(|sub| &sub.comp)
        {
            Some(crate::types::CompType::Struct(fields)) => {
                let mut out = self.temp_vec(fields.len(), offset, "struct field types")?;
                for field in fields {
                    push_fallible(
                        &mut out,
                        gc_unpacked(field.storage),
                        offset,
                        Some(self.func_idx),
                        "struct field types",
                    )?;
                }
                Ok(out)
            }
            _ => Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidType {
                    type_idx: type_idx.0,
                },
            }),
        }
    }

    fn gc_struct_field(
        &self,
        offset: ByteOffset,
        type_idx: TypeIdx,
        field: u32,
    ) -> Result<ValType, LowerError> {
        match self
            .tables
            .types
            .get(type_idx.0 as usize)
            .map(|sub| &sub.comp)
        {
            Some(crate::types::CompType::Struct(fields)) => fields
                .get(field as usize)
                .map(|field| gc_unpacked(field.storage))
                .ok_or(LowerError {
                    offset,
                    function: Some(self.func_idx),
                    kind: LowerErrorKind::InvalidType {
                        type_idx: type_idx.0,
                    },
                }),
            _ => Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidType {
                    type_idx: type_idx.0,
                },
            }),
        }
    }

    fn gc_array_field(&self, offset: ByteOffset, type_idx: TypeIdx) -> Result<ValType, LowerError> {
        match self
            .tables
            .types
            .get(type_idx.0 as usize)
            .map(|sub| &sub.comp)
        {
            Some(crate::types::CompType::Array(field)) => Ok(gc_unpacked(field.storage)),
            _ => Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidType {
                    type_idx: type_idx.0,
                },
            }),
        }
    }

    fn lower_gc_field_get(
        &mut self,
        offset: ByteOffset,
        type_idx: TypeIdx,
        field: u32,
        ext: GcExt,
    ) -> Result<(), LowerError> {
        let field_ty = self.gc_struct_field(offset, type_idx, field)?;
        let object = self.pop_expect(offset, "struct.get", gc_ref_type_nullable(type_idx))?;
        let ty = field_ty;
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::StructGet {
                dst,
                type_idx,
                field,
                ext,
                object: object.reg,
            },
        )?;
        let _ = object;
        Ok(())
    }

    fn lower_gc_array_get(
        &mut self,
        offset: ByteOffset,
        type_idx: TypeIdx,
        ext: GcExt,
    ) -> Result<(), LowerError> {
        let field_ty = self.gc_array_field(offset, type_idx)?;
        let index = self.pop_expect(offset, "array.get", ValType::Num(NumType::I32))?;
        let array = self.pop_expect(offset, "array.get", gc_ref_type_nullable(type_idx))?;
        let ty = field_ty;
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::ArrayGet {
                dst,
                type_idx,
                array: array.reg,
                index: index.reg,
                ext,
            },
        )?;
        Ok(())
    }

    fn pop_any(&mut self, offset: ByteOffset, op: &'static str) -> Result<RegValue, LowerError> {
        if self.at_frame_boundary() {
            if self.current_frame_unreachable() {
                // Polymorphic stack: synthesize an undefined register. The
                // surrounding code is unreachable, so the register is never
                // read at runtime.
                let ty = ValType::Num(NumType::I32);
                let reg = self.alloc_reg(ty)?;
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

    /// The operand type a `ref.test`/`ref.cast` reads: `(ref null top(ht))`.
    fn ref_operand_type(&self, ht: HeapType) -> ValType {
        ValType::Ref(crate::types::RefType::from_parts(
            true,
            self.top_of_heaptype(ht),
        ))
    }

    /// `top_of_heaptype` (`valid/match.ml:25`).
    fn top_of_heaptype(&self, ht: HeapType) -> HeapType {
        use HeapType::*;
        match ht {
            Any | None | Eq | Struct | Array | I31 => Any,
            Func | NoFunc => Func,
            Exn | NoExn => Exn,
            Extern | NoExtern => Extern,
            Type(idx) => match self.tables.types.get(idx.0 as usize).map(|sub| &sub.comp) {
                Some(crate::types::CompType::Func(_)) => Func,
                _ => Any,
            },
        }
    }

    /// Lower `br_on_cast`/`br_on_cast_fail` to a `ref.test` plus a
    /// conditional branch. The reference stays live on the operand stack on
    /// both the taken and fall-through paths (`exec/eval.ml:246-256`).
    fn lower_br_on_cast(
        &mut self,
        offset: ByteOffset,
        fail: bool,
        src: crate::types::RefType,
        tgt: crate::types::RefType,
        label: LabelIdx,
    ) -> Result<(), LowerError> {
        let value = self.pop_expect(offset, "br_on_cast", ValType::Ref(src))?;
        let frame_pos = self.label_position(offset, label)?;
        let (branch_types, loop_header) = self.branch_types_at_fallible(offset, frame_pos)?;
        let Some((_, leading)) = branch_types.split_last() else {
            return Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidLabel { label: label.0 },
            });
        };
        let leading = self.fallible_copy(offset, "br_on_cast leading types", leading)?;
        let mut values: Vec<Reg> = Vec::new();
        values
            .try_reserve(branch_types.len())
            .map_err(|_| self.resource_error(offset, "br_on_cast branch values"))?;
        for &expected in leading.iter().rev() {
            let found = self.pop_expect(offset, "br_on_cast", expected)?;
            values.push(found.reg);
        }
        values.reverse();
        // Fall-through keeps the leading values and the reference, retyped.
        for (&reg, &ty) in values.iter().zip(leading.iter()) {
            self.push_stack(RegValue { reg, ty })?;
        }
        let fall = if fail {
            tgt
        } else {
            lower_diff_reftype(src, tgt)
        };
        self.push_stack(RegValue {
            reg: value.reg,
            ty: ValType::Ref(fall),
        })?;

        let test_ty = ValType::Num(NumType::I32);
        let test_reg = self.alloc_reg(test_ty)?;
        self.emit(
            offset,
            RegOp::RefTest {
                dst: test_reg,
                value: value.reg,
                nullable: tgt.is_nullable(),
                ht: tgt.heap_type(),
            },
        )?;
        let cond = if fail {
            let inverted = self.alloc_reg(test_ty)?;
            self.emit(
                offset,
                RegOp::Unary {
                    op: UnaryOp::I32Eqz,
                    dst: inverted,
                    value: test_reg,
                },
            )?;
            inverted
        } else {
            test_reg
        };
        // The taken path forwards the reference as the label's last value.
        values.push(value.reg);
        let br_block_idx = self.blocks.len();
        if let Some(loop_target) = loop_header {
            let forwarded = self.fallible_copy(offset, "br_on_cast trampoline values", &values)?;
            self.pending_trampolines
                .try_reserve(1)
                .map_err(|_| self.resource_error(offset, "br_on_cast trampolines"))?;
            push_fallible(
                &mut self.pending_trampolines,
                TrampolineReq {
                    block: br_block_idx,
                    slot: BranchSlot::Single,
                    param_regs: loop_target.param_regs,
                    values: forwarded,
                    header: loop_target.header,
                    offset,
                },
                self.cur_offset,
                Some(self.func_idx),
                "pending trampolines",
            )?;
        } else {
            let forwarded = self.fallible_copy(offset, "br_on_cast pending values", &values)?;
            self.pending_branches
                .try_reserve(1)
                .map_err(|_| self.resource_error(offset, "br_on_cast pending branches"))?;
            push_fallible(
                &mut self.pending_branches,
                PendingBranch {
                    block: br_block_idx,
                    frame_pos,
                    values: forwarded,
                    slot: BranchSlot::Single,
                },
                self.cur_offset,
                Some(self.func_idx),
                "pending branches",
            )?;
        }
        self.finish_block(RegTerm::BrIf {
            cond,
            target_block: 0,
            values,
        })?;
        Ok(())
    }

    /// A clean lowering resource-limit error (a refused bookkeeping
    /// reservation), mirroring the runtime's `ResourceLimitExceeded`.
    fn resource_error(&self, offset: ByteOffset, what: &'static str) -> LowerError {
        LowerError {
            offset,
            function: Some(self.func_idx),
            kind: LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded { what }),
        }
    }

    /// Fallibly clone `src` into a fresh `Vec` (try_reserve-then-fill).
    fn fallible_copy<T: Clone>(
        &self,
        offset: ByteOffset,
        what: &'static str,
        src: &[T],
    ) -> Result<Vec<T>, LowerError> {
        fallible_copy_slice(self.func_idx, offset, what, src)
    }

    /// Fallible [`FuncBuilder::branch_types_at`] used by `br_on_cast`: every
    /// label/loop bookkeeping clone is try-reserved before it is filled.
    fn branch_types_at_fallible(
        &self,
        offset: ByteOffset,
        frame_pos: usize,
    ) -> Result<(Vec<ValType>, Option<LoopTarget>), LowerError> {
        let frame = &self.label_stack[frame_pos];
        match frame.kind {
            FrameKind::Loop { header_block } => Ok((
                self.fallible_copy(
                    offset,
                    "br_on_cast loop parameter types",
                    &frame.param_types,
                )?,
                Some(LoopTarget {
                    header: header_block,
                    param_regs: self.fallible_copy(
                        offset,
                        "br_on_cast loop parameter registers",
                        &frame.param_regs,
                    )?,
                }),
            )),
            _ => Ok((
                self.fallible_copy(offset, "br_on_cast label types", &frame.result_types)?,
                None,
            )),
        }
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
        let mut param_regs =
            self.temp_vec(param_types.len(), self.cur_offset, "lowering temporary")?;
        for &expected in param_types.iter().rev() {
            let found = self.pop_expect(offset, op, expected)?;
            param_regs.push(found.reg);
        }
        param_regs.reverse();
        let height = self.stack.len();
        for (&reg, &ty) in param_regs.iter().zip(param_types.iter()) {
            self.push_stack(RegValue { reg, ty })?;
        }
        let entry = LabelFrame {
            label: LabelIdx(self.label_stack.len() as u32),
            kind,
            result_types,
            param_types,
            param_regs,
            height,
            unreachable: false,
        };
        push_fallible(
            &mut self.label_stack,
            entry,
            self.cur_offset,
            Some(self.func_idx),
            "label stack",
        )?;
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
            BlockType::Val(ty) => {
                let mut results = self.temp_vec(1, offset, "block result types")?;
                push_fallible(
                    &mut results,
                    ty,
                    offset,
                    Some(self.func_idx),
                    "block result types",
                )?;
                Ok((Vec::new(), results))
            }
            BlockType::TypeIdx(idx) => {
                let ty = self.tables.func_type(idx as usize).ok_or(LowerError {
                    offset,
                    function: Some(self.func_idx),
                    kind: LowerErrorKind::InvalidType { type_idx: idx },
                })?;
                Ok((
                    self.fallible_copy(offset, "block parameter types", &ty.params)?,
                    self.fallible_copy(offset, "block result types", &ty.results)?,
                ))
            }
        }
    }

    /// Resolve a tag's parameter types via the index-space tag table.
    fn tag_params(&self, offset: ByteOffset, tag: TagIdx) -> Result<Vec<ValType>, LowerError> {
        let type_idx = self
            .tables
            .tag_types
            .get(tag.0 as usize)
            .ok_or(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidType { type_idx: tag.0 },
            })?;
        let ty = self
            .tables
            .func_type(type_idx.0 as usize)
            .ok_or(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::InvalidType {
                    type_idx: type_idx.0,
                },
            })?;
        self.fallible_copy(offset, "tag parameter types", &ty.params)
    }

    /// The value types a catch clause's landing writes into its target
    /// label: tag params, plus a non-null exnref last for `_ref` clauses.
    fn catch_value_types(
        &self,
        offset: ByteOffset,
        catch: &TryTableCatch,
    ) -> Result<Vec<ValType>, LowerError> {
        match catch.kind {
            TryTableCatchKind::Catch | TryTableCatchKind::CatchRef => {
                let tag = catch
                    .tag
                    .expect("decoder guarantees a tag for catch/catch_ref");
                let mut types = self.tag_params(offset, tag)?;
                if matches!(catch.kind, TryTableCatchKind::CatchRef) {
                    push_fallible(
                        &mut types,
                        ValType::Ref(RefType::from_parts(false, HeapType::Exn)),
                        offset,
                        Some(self.func_idx),
                        "catch value types",
                    )?;
                }
                Ok(types)
            }
            TryTableCatchKind::CatchAll => Ok(Vec::new()),
            TryTableCatchKind::CatchAllRef => {
                let mut types = self.temp_vec(1, offset, "catch value types")?;
                push_fallible(
                    &mut types,
                    ValType::Ref(RefType::from_parts(false, HeapType::Exn)),
                    offset,
                    Some(self.func_idx),
                    "catch value types",
                )?;
                Ok(types)
            }
        }
    }

    /// Emit copy instructions delivering `srcs` into `dsts` (used for
    /// unconditional loop back-edges; conditional branches use trampolines).
    fn emit_copies(
        &mut self,
        offset: ByteOffset,
        dsts: &[Reg],
        srcs: &[Reg],
    ) -> Result<(), LowerError> {
        for (&dst, &src) in dsts.iter().zip(srcs.iter()) {
            if dst != src {
                self.emit(offset, RegOp::Copy { dst, src })?;
            }
        }
        Ok(())
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
        let dst = self.alloc_reg(output)?;
        self.push_stack(RegValue {
            reg: dst,
            ty: output,
        })?;
        self.emit(
            offset,
            RegOp::Binary {
                op,
                dst,
                lhs: lhs.reg,
                rhs: rhs.reg,
            },
        )?;
        Ok(())
    }

    fn lower_splat(&mut self, offset: ByteOffset, shape: LaneShape) -> Result<(), LowerError> {
        let src = self.pop_expect(offset, "splat", shape.scalar_type())?;
        let ty = ValType::Vec(crate::types::VecType::V128);
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::V128Splat {
                dst,
                shape,
                src: src.reg,
            },
        )?;
        Ok(())
    }

    fn lower_extract_lane(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        lane: u8,
        signed: bool,
    ) -> Result<(), LowerError> {
        let src = self.pop_expect(
            offset,
            "extract_lane",
            ValType::Vec(crate::types::VecType::V128),
        )?;
        let ty = shape.scalar_type();
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::V128ExtractLane {
                dst,
                shape,
                src: src.reg,
                lane,
                signed,
            },
        )?;
        Ok(())
    }

    fn lower_replace_lane(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        lane: u8,
    ) -> Result<(), LowerError> {
        let scalar = self.pop_expect(offset, "replace_lane", shape.scalar_type())?;
        let vec = self.pop_expect(
            offset,
            "replace_lane",
            ValType::Vec(crate::types::VecType::V128),
        )?;
        let ty = ValType::Vec(crate::types::VecType::V128);
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::V128ReplaceLane {
                dst,
                shape,
                vec: vec.reg,
                scalar: scalar.reg,
                lane,
            },
        )?;
        Ok(())
    }

    fn lower_v128_binary(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        kind: V128BinaryKind,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let rhs = self.pop_expect(offset, "simd.binary", v128)?;
        let lhs = self.pop_expect(offset, "simd.binary", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Binary {
                shape,
                kind,
                dst,
                lhs: lhs.reg,
                rhs: rhs.reg,
            },
        )?;
        Ok(())
    }

    fn lower_v128_unary(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        kind: V128UnaryKind,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.unary", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Unary {
                dst,
                src: src.reg,
                shape,
                kind,
            },
        )?;
        Ok(())
    }

    fn lower_v128_compare(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        kind: V128CompareKind,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let b = self.pop_expect(offset, "simd.compare", v128)?;
        let a = self.pop_expect(offset, "simd.compare", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Compare {
                dst,
                a: a.reg,
                b: b.reg,
                shape,
                kind,
            },
        )?;
        Ok(())
    }

    fn lower_v128_shift(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        kind: V128ShiftKind,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let amount = self.pop_expect(offset, "simd.shift", ValType::Num(NumType::I32))?;
        let src = self.pop_expect(offset, "simd.shift", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Shift {
                dst,
                src: src.reg,
                amount: amount.reg,
                shape,
                kind,
            },
        )?;
        Ok(())
    }

    fn lower_v128_extend(
        &mut self,
        offset: ByteOffset,
        src_shape: LaneShape,
        high: bool,
        signed: bool,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.extend", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Extend {
                dst,
                src: src.reg,
                src_shape,
                high,
                signed,
            },
        )?;
        Ok(())
    }

    fn lower_v128_extmul(
        &mut self,
        offset: ByteOffset,
        src_shape: LaneShape,
        high: bool,
        signed: bool,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let b = self.pop_expect(offset, "simd.extmul", v128)?;
        let a = self.pop_expect(offset, "simd.extmul", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128ExtMul {
                dst,
                a: a.reg,
                b: b.reg,
                src_shape,
                high,
                signed,
            },
        )?;
        Ok(())
    }

    fn lower_v128_extadd_pairwise(
        &mut self,
        offset: ByteOffset,
        src_shape: LaneShape,
        signed: bool,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.extadd_pairwise", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128ExtAddPairwise {
                dst,
                src: src.reg,
                src_shape,
                signed,
            },
        )?;
        Ok(())
    }

    fn lower_v128_narrow(
        &mut self,
        offset: ByteOffset,
        dst_shape: LaneShape,
        signed: bool,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let b = self.pop_expect(offset, "simd.narrow", v128)?;
        let a = self.pop_expect(offset, "simd.narrow", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Narrow {
                dst,
                a: a.reg,
                b: b.reg,
                dst_shape,
                signed,
            },
        )?;
        Ok(())
    }

    fn lower_v128_dot(&mut self, offset: ByteOffset) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let b = self.pop_expect(offset, "i32x4.dot_i16x8_s", v128)?;
        let a = self.pop_expect(offset, "i32x4.dot_i16x8_s", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Dot {
                dst,
                a: a.reg,
                b: b.reg,
            },
        )?;
        Ok(())
    }

    fn lower_v128_bitselect(&mut self, offset: ByteOffset) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let mask = self.pop_expect(offset, "v128.bitselect", v128)?;
        let b = self.pop_expect(offset, "v128.bitselect", v128)?;
        let a = self.pop_expect(offset, "v128.bitselect", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Bitselect {
                dst,
                a: a.reg,
                b: b.reg,
                mask: mask.reg,
            },
        )?;
        Ok(())
    }

    fn lower_v128_any_true(&mut self, offset: ByteOffset) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "v128.any_true", v128)?;
        let ty = ValType::Num(crate::types::NumType::I32);
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(offset, RegOp::V128AnyTrue { dst, src: src.reg })?;
        Ok(())
    }

    fn lower_v128_all_true(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.all_true", v128)?;
        let ty = ValType::Num(crate::types::NumType::I32);
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::V128AllTrue {
                dst,
                src: src.reg,
                shape,
            },
        )?;
        Ok(())
    }

    fn lower_v128_bitmask(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.bitmask", v128)?;
        let ty = ValType::Num(crate::types::NumType::I32);
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::V128Bitmask {
                dst,
                src: src.reg,
                shape,
            },
        )?;
        Ok(())
    }

    fn lower_v128_float_unary(
        &mut self,
        offset: ByteOffset,
        shape: LaneShape,
        kind: V128FloatUnaryKind,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.float_unary", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128FloatUnary {
                dst,
                src: src.reg,
                shape,
                kind,
            },
        )?;
        Ok(())
    }

    fn lower_v128_convert(
        &mut self,
        offset: ByteOffset,
        kind: V128ConvertKind,
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let src = self.pop_expect(offset, "simd.convert", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Convert {
                dst,
                src: src.reg,
                kind,
            },
        )?;
        Ok(())
    }

    fn lower_v128_swizzle(&mut self, offset: ByteOffset) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let b = self.pop_expect(offset, "i8x16.swizzle", v128)?;
        let a = self.pop_expect(offset, "i8x16.swizzle", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Swizzle {
                dst,
                a: a.reg,
                b: b.reg,
            },
        )?;
        Ok(())
    }

    fn lower_v128_shuffle(
        &mut self,
        offset: ByteOffset,
        lanes: [u8; 16],
    ) -> Result<(), LowerError> {
        let v128 = ValType::Vec(crate::types::VecType::V128);
        let b = self.pop_expect(offset, "i8x16.shuffle", v128)?;
        let a = self.pop_expect(offset, "i8x16.shuffle", v128)?;
        let dst = self.alloc_reg(v128)?;
        self.push_stack(RegValue { reg: dst, ty: v128 })?;
        self.emit(
            offset,
            RegOp::V128Shuffle {
                dst,
                a: a.reg,
                b: b.reg,
                lanes,
            },
        )?;
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

    /// The declared address index type of a memory (validation has already
    /// proven the index exists).
    fn memory_num_type(&self, memory: MemIdx) -> NumType {
        self.tables
            .memory_addr_types
            .get(memory.0 as usize)
            .copied()
            .expect("validated memory index")
    }

    /// The declared address index type of a table (validation has already
    /// proven the index exists).
    fn table_num_type(&self, table: TableIdx) -> NumType {
        self.tables
            .table_addr_types
            .get(table.0 as usize)
            .copied()
            .expect("validated table index")
    }

    fn lower_load(
        &mut self,
        offset: ByteOffset,
        op: LoadOp,
        memarg: MemArg,
    ) -> Result<(), LowerError> {
        let addr = self.pop_expect(
            offset,
            "load",
            ValType::Num(self.memory_num_type(memarg.memory)),
        )?;
        let ty = op.result_type();
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::Load {
                op,
                dst,
                addr: addr.reg,
                memarg,
            },
        )?;
        Ok(())
    }

    fn lower_store(
        &mut self,
        offset: ByteOffset,
        op: StoreOp,
        memarg: MemArg,
    ) -> Result<(), LowerError> {
        let value = self.pop_expect(offset, "store", op.value_type())?;
        let addr = self.pop_expect(
            offset,
            "store",
            ValType::Num(self.memory_num_type(memarg.memory)),
        )?;
        self.emit(
            offset,
            RegOp::Store {
                op,
                addr: addr.reg,
                value: value.reg,
                memarg,
            },
        )?;
        Ok(())
    }

    fn lower_v128_load_lane(
        &mut self,
        offset: ByteOffset,
        width: u8,
        memarg: MemArg,
        lane: u8,
    ) -> Result<(), LowerError> {
        // Spec stack order `[i32 v128]`: the v128 operand is on top.
        let vec = self.pop_expect(
            offset,
            "v128.load_lane",
            ValType::Vec(crate::types::VecType::V128),
        )?;
        let addr = self.pop_expect(
            offset,
            "v128.load_lane",
            ValType::Num(self.memory_num_type(memarg.memory)),
        )?;
        let ty = ValType::Vec(crate::types::VecType::V128);
        let dst = self.alloc_reg(ty)?;
        self.push_stack(RegValue { reg: dst, ty })?;
        self.emit(
            offset,
            RegOp::V128LoadLane {
                width,
                dst,
                vec: vec.reg,
                addr: addr.reg,
                memarg,
                lane,
            },
        )?;
        Ok(())
    }

    fn lower_v128_store_lane(
        &mut self,
        offset: ByteOffset,
        width: u8,
        memarg: MemArg,
        lane: u8,
    ) -> Result<(), LowerError> {
        // Spec stack order `[i32 v128]`: the v128 operand is on top.
        let vec = self.pop_expect(
            offset,
            "v128.store_lane",
            ValType::Vec(crate::types::VecType::V128),
        )?;
        let addr = self.pop_expect(
            offset,
            "v128.store_lane",
            ValType::Num(self.memory_num_type(memarg.memory)),
        )?;
        self.emit(
            offset,
            RegOp::V128StoreLane {
                width,
                addr: addr.reg,
                vec: vec.reg,
                memarg,
                lane,
            },
        )?;
        Ok(())
    }

    fn lower_unary_op(&mut self, offset: ByteOffset, op: UnaryOp) -> Result<(), LowerError> {
        let input = op.input_type();
        let output = op.result_type();
        let value = self.pop_expect(offset, op.name(), input)?;
        let dst = self.alloc_reg(output)?;
        self.push_stack(RegValue {
            reg: dst,
            ty: output,
        })?;
        self.emit(
            offset,
            RegOp::Unary {
                op,
                dst,
                value: value.reg,
            },
        )?;
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
                let reg = self.alloc_reg(expected)?;
                return Ok(RegValue { reg, ty: expected });
            }
            return Err(LowerError {
                offset,
                function: Some(self.func_idx),
                kind: LowerErrorKind::StackUnderflow { op, expected },
            });
        }

        let found = self.stack.pop().expect("stack height checked");

        // In unreachable code every value is a polymorphic undef; precise
        // types were proven by the validator, and the undef registers
        // lowering synthesizes cannot represent bottom.
        if found.ty != expected
            && !ref_compatible(self.tables.types, found.ty, expected)
            && !self.current_frame_unreachable()
        {
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
        let results = self.fallible_copy(offset, "function result types", &self.results)?;
        let mut values: Vec<Reg> = Vec::new();
        values
            .try_reserve(results.len())
            .map_err(|_| self.resource_error(offset, "function result registers"))?;
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
        Instr::V128Load(memarg) => (LoadOp::V128, memarg),
        Instr::V128Load8x8S(memarg) => (LoadOp::V128Load8x8S, memarg),
        Instr::V128Load8x8U(memarg) => (LoadOp::V128Load8x8U, memarg),
        Instr::V128Load16x4S(memarg) => (LoadOp::V128Load16x4S, memarg),
        Instr::V128Load16x4U(memarg) => (LoadOp::V128Load16x4U, memarg),
        Instr::V128Load32x2S(memarg) => (LoadOp::V128Load32x2S, memarg),
        Instr::V128Load32x2U(memarg) => (LoadOp::V128Load32x2U, memarg),
        Instr::V128Load8Splat(memarg) => (LoadOp::V128Load8Splat, memarg),
        Instr::V128Load16Splat(memarg) => (LoadOp::V128Load16Splat, memarg),
        Instr::V128Load32Splat(memarg) => (LoadOp::V128Load32Splat, memarg),
        Instr::V128Load64Splat(memarg) => (LoadOp::V128Load64Splat, memarg),
        Instr::V128Load32Zero(memarg) => (LoadOp::V128Load32Zero, memarg),
        Instr::V128Load64Zero(memarg) => (LoadOp::V128Load64Zero, memarg),
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
        Instr::V128Store(memarg) => (StoreOp::V128, memarg),
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
        Instr::F32Copysign => Some(BinaryOp::F32Copysign),
        Instr::F64Copysign => Some(BinaryOp::F64Copysign),
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
        Instr::ReturnCall(_) => "return_call",
        Instr::ReturnCallIndirect { .. } => "return_call_indirect",
        Instr::ReturnCallRef(_) => "return_call_ref",
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
        // Body-end fallthrough into a continuation block ending in Return.
        assert_eq!(func.blocks.len(), 5);
        assert!(matches!(func.blocks[0].term, RegTerm::Fallthrough));
        assert!(matches!(
            func.blocks[func.blocks.len() - 2].term,
            RegTerm::Fallthrough
        ));
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
    fn lower_try_table_region_shape() {
        let reg_module = lower_wat(
            "(module
               (tag (param i32))
               (func (export \"f\") (result i32)
                 (block $h (result i32)
                   (try_table (catch 0 $h)
                     (block $h2 (result i32 (ref exn))
                       (try_table (result i32 (ref exn)) (catch_ref 0 $h2)
                         i32.const 1
                         throw 0))
                     drop
                     drop)
                   i32.const 7)))",
        );
        let func = &reg_module.funcs[0];
        assert_eq!(func.regions.len(), 2, "outer + inner try_table regions");
        let outer = &func.regions[0];
        let inner = &func.regions[1];
        assert!(
            inner.first_block > outer.first_block,
            "inner region must start after the outer's first block"
        );
        assert!(outer.end_block > outer.first_block);
        assert!(inner.end_block > inner.first_block);
        assert!(
            inner.end_block <= outer.end_block,
            "inner region must be nested inside the outer"
        );
        // Outer `catch 0` targets a label carrying the tag's i32 param.
        assert_eq!(outer.catches.len(), 1);
        assert_eq!(outer.catches[0].dst_regs.len(), 1);
        // Inner `catch_ref 0` lands the i32 param plus the exnref.
        assert_eq!(inner.catches.len(), 1);
        assert_eq!(inner.catches[0].dst_regs.len(), 2);
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
                RegTerm::Return { .. }
                | RegTerm::TailCall { .. }
                | RegTerm::TailCallIndirect { .. }
                | RegTerm::TailCallRef { .. } => break,
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

    #[test]
    fn fuel_exhaustion_stops_runaway_and_bounded_finishes() {
        // An infinite loop burns any budget.
        let reg_module = lower_wat("(module (func (export \"spin\") (loop br 0)))");
        let store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        store.set_fuel(Some(1_000));
        let error = crate::runtime::execute_export(&reg_module, &store, "spin", &[]).unwrap_err();
        assert_eq!(error.kind, crate::runtime::RuntimeErrorKind::FuelExhausted);
        assert_eq!(store.fuel(), Some(0));

        // A bounded loop fits inside a sufficient budget.
        let reg_module = lower_wat(
            "(module (func (export \"sum\") (result i32) (local i32 i32) \
             (block (loop \
             local.get 0 i32.const 10 i32.ge_s br_if 1 \
             local.get 1 local.get 0 i32.add local.set 1 \
             local.get 0 i32.const 1 i32.add local.set 0 \
             br 0)) \
             local.get 1))",
        );
        let store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        store.set_fuel(Some(10_000));
        let result = crate::runtime::execute_export(&reg_module, &store, "sum", &[]).unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(45)]);
        assert!(store.fuel().unwrap() < 10_000);

        // Unlimited by default: no budget, no exhaustion.
        let reg_module = lower_wat("(module (func (export \"one\") (result i32) i32.const 1))");
        let store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        assert_eq!(store.fuel(), None);
        let result = crate::runtime::execute_export(&reg_module, &store, "one", &[]).unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(1)]);
    }

    /// Execute an exported function with full module context (required for
    /// `call` instructions).
    fn run_wat_export(
        source: &str,
        name: &str,
        args: &[crate::runtime::Value],
    ) -> Result<Vec<crate::runtime::Value>, crate::runtime::RuntimeError> {
        let reg_module = lower_wat(source);
        let store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        crate::runtime::execute_export(&reg_module, &store, name, args)
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
    fn execute_call_ref_and_null_trap() {
        // Direct call through a funcref value.
        let result = run_wat_export(
            "(module (type $ii (func (param i32) (result i32)))\
             (elem declare func $dbl)\
             (func $dbl (type $ii) local.get 0 i32.const 2 i32.mul)\
             (func (export \"go\") (param i32) (result i32)\
             local.get 0 ref.func $dbl call_ref $ii))",
            "go",
            &[crate::runtime::Value::I32(21)],
        )
        .unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(42)]);

        // Null funcref traps with the canonical message.
        let error = run_wat_export(
            "(module (type $ii (func (param i32) (result i32)))\
             (func (export \"go\") (param i32) (result i32)\
             local.get 0 ref.null $ii call_ref $ii))",
            "go",
            &[crate::runtime::Value::I32(1)],
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(
                crate::runtime::RuntimeTrap::NullFunctionReference
            )
        );
    }

    #[test]
    fn execute_br_on_null_paths() {
        // Null ref takes the branch; non-null falls through keeping the ref.
        let source = "(module (type $ii (func (result i32)))\
             (elem declare func $f)\
             (func $f (type $ii) i32.const 7)\
             (func (export \"null\") (result i32)\
             (block ref.null $ii br_on_null 0 call_ref $ii return)\
             i32.const 99)\
             (func (export \"nonnull\") (result i32)\
             (block ref.func $f br_on_null 0 call_ref $ii return)\
             i32.const 99))";
        let result = run_wat_export(source, "null", &[]).unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(99)]);
        let result = run_wat_export(source, "nonnull", &[]).unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(7)]);
    }

    #[test]
    fn execute_br_on_non_null_forwards_ref() {
        // Non-null ref branches and is forwarded to the label; null falls
        // through with the ref consumed.
        let result = run_wat_export(
            "(module (type $ii (func (result i32)))\
             (elem declare func $f)\
             (func $f (type $ii) i32.const 7)\
             (func (export \"nonnull\") (result i32)\
             (block (result (ref $ii)) ref.func $f br_on_non_null 0 unreachable)\
             call_ref $ii)\
             (func (export \"null\") (result i32)\
             (block (result (ref $ii)) ref.null $ii br_on_non_null 0 unreachable)\
             call_ref $ii))",
            "nonnull",
            &[],
        )
        .unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(7)]);
    }

    #[test]
    fn execute_ref_as_non_null_trap_and_passthrough() {
        let error = run_wat_export(
            "(module (type $ii (func))\
             (func (export \"go\") (result i32)\
             ref.null $ii ref.as_non_null drop i32.const 0))",
            "go",
            &[],
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(crate::runtime::RuntimeTrap::NullReference)
        );

        let result = run_wat_export(
            "(module (type $ii (func (result i32)))\
             (elem declare func $f)\
             (func $f (type $ii) i32.const 7)\
             (func (export \"go\") (result i32)\
             ref.func $f ref.as_non_null call_ref $ii))",
            "go",
            &[],
        )
        .unwrap();
        assert_eq!(result, vec![crate::runtime::Value::I32(7)]);
    }

    #[test]
    fn execute_call_exhaustion_traps() {
        // Deep recursion needs a larger host stack than the default test
        // thread provides at this call depth.
        let error = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                run_wat_export(
                    "(module (func $boom (export \"boom\") call $boom))",
                    "boom",
                    &[],
                )
            })
            .expect("spawn exhaustion test thread")
            .join()
            .expect("exhaustion test thread panicked")
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
        let store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        assert_eq!(
            crate::runtime::execute_export(&reg_module, &store, "setup", &[]),
            Ok(vec![crate::runtime::Value::I32(3)])
        );
        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &store,
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
    fn lower_v128_binary_shape() {
        let reg_module = lower_wat(
            "(module (func (result v128)
               v128.const i32x4 1 2 3 4
               v128.const i32x4 5 6 7 8
               i32x4.add))",
        );
        let func = &reg_module.funcs[0];
        let op = func.blocks[0]
            .instrs
            .iter()
            .find_map(|instr| match &instr.op {
                RegOp::V128Binary {
                    shape, kind, dst, ..
                } => Some((shape, kind, dst)),
                _ => None,
            })
            .expect("expected a V128Binary op");
        assert_eq!(*op.0, LaneShape::I32x4);
        assert_eq!(*op.1, V128BinaryKind::Add);
        assert_eq!(*op.2, Reg(2));
    }

    #[test]
    fn execute_v128_arithmetic_and_memory() {
        let source = "(module
            (memory 1)
            (func (export \"add_store\") (param i32) (result i32)
              local.get 0
              v128.const i32x4 1 2 3 4
              v128.const i32x4 10 20 30 40
              i32x4.add
              v128.store
              local.get 0
              i32.load))";
        // First lane of (1+10) stored to memory and read back as a scalar.
        assert_eq!(
            run_wat_export(source, "add_store", &[crate::runtime::Value::I32(16)]),
            Ok(vec![crate::runtime::Value::I32(11)])
        );
    }

    #[test]
    fn execute_saturating_truncation_edges() {
        let source = "(module (func (export \"sat\") (param f32) (result i32)
            local.get 0
            i32.trunc_sat_f32_s))";
        // NaN -> 0.
        assert_eq!(
            run_wat_export(source, "sat", &[crate::runtime::Value::F32(f32::NAN)]),
            Ok(vec![crate::runtime::Value::I32(0)])
        );
        // Above range -> i32::MAX.
        assert_eq!(
            run_wat_export(source, "sat", &[crate::runtime::Value::F32(3e9)]),
            Ok(vec![crate::runtime::Value::I32(i32::MAX)])
        );
        // In range -> plain truncation.
        assert_eq!(
            run_wat_export(source, "sat", &[crate::runtime::Value::F32(-2.9)]),
            Ok(vec![crate::runtime::Value::I32(-2)])
        );
    }

    #[test]
    fn lower_bulk_memory_ops_shape() {
        let reg_module = lower_wat(
            "(module
               (memory 1)
               (data $d \"ab\")
               (func (export \"f\") (param i32)
                 local.get 0
                 i32.const 0
                 i32.const 2
                 memory.init $d
                 local.get 0
                 i32.const 1
                 i32.const 8
                 memory.fill
                 data.drop $d))",
        );
        let func = &reg_module.funcs[0];
        let ops: Vec<&RegOp> = func.blocks[0]
            .instrs
            .iter()
            .map(|instr| &instr.op)
            .collect();
        assert!(matches!(ops[3], RegOp::MemoryInit { .. }));
        assert!(matches!(ops[7], RegOp::MemoryFill { .. }));
        assert!(matches!(ops[8], RegOp::DataDrop { .. }));
    }

    #[test]
    fn execute_host_function_milestone() {
        // The Phase 4 milestone: a module importing env.print_i32 calls the
        // host, which records the call.
        let source = "(module
            (import \"env\" \"print_i32\" (func $print (param i32)))
            (func (export \"main\")
              i32.const 42
              call $print))";
        let reg_module = lower_wat(source);
        let mut store = crate::runtime::Store::instantiate(&reg_module).unwrap();

        let recorded = alloc::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let sink = recorded.clone();
        store
            .register_host_func(
                "env",
                "print_i32",
                crate::runtime::HostFunction::new(
                    crate::types::FuncType {
                        params: vec![ValType::Num(NumType::I32)],
                        results: vec![],
                    },
                    move |args| {
                        sink.borrow_mut().push(args[0]);
                        Ok(vec![])
                    },
                ),
            )
            .unwrap();

        let result = crate::runtime::execute_export(&reg_module, &store, "main", &[]);
        assert_eq!(result, Ok(vec![]));
        assert_eq!(
            recorded.borrow().as_slice(),
            &[crate::runtime::Value::I32(42)]
        );
    }

    #[test]
    fn execute_host_function_with_results() {
        let source = "(module
            (import \"env\" \"double\" (func $double (param i32) (result i32)))
            (func (export \"go\") (param i32) (result i32)
              local.get 0
              call $double
              i32.const 2
              i32.add))";
        let reg_module = lower_wat(source);
        let mut store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        store
            .register_host_func(
                "env",
                "double",
                crate::runtime::HostFunction::new(
                    crate::types::FuncType {
                        params: vec![ValType::Num(NumType::I32)],
                        results: vec![ValType::Num(NumType::I32)],
                    },
                    |args| {
                        let crate::runtime::Value::I32(v) = args[0] else {
                            unreachable!()
                        };
                        Ok(vec![crate::runtime::Value::I32(v * 2)])
                    },
                ),
            )
            .unwrap();

        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &store,
                "go",
                &[crate::runtime::Value::I32(20)],
            ),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn unregistered_import_fails_on_call_not_instantiation() {
        let source = "(module
            (import \"env\" \"missing\" (func $missing))
            (func (export \"go\")
              call $missing))";
        let reg_module = lower_wat(source);
        // Instantiation succeeds (lazy resolution).
        let store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        let error = crate::runtime::execute_export(&reg_module, &store, "go", &[])
            .expect_err("expected UnknownImport on call");
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::UnknownImport {
                module: "env".into(),
                name: "missing".into(),
            }
        );
    }

    #[test]
    fn host_registration_rejects_mismatch_and_unknown() {
        let source = "(module
            (import \"env\" \"f\" (func $f (param i32))))";
        let reg_module = lower_wat(source);
        let mut store = crate::runtime::Store::instantiate(&reg_module).unwrap();

        // Wrong signature.
        let error = store
            .register_host_func(
                "env",
                "f",
                crate::runtime::HostFunction::new(
                    crate::types::FuncType {
                        params: vec![],
                        results: vec![],
                    },
                    |_| Ok(vec![]),
                ),
            )
            .expect_err("expected ImportTypeMismatch");
        assert!(matches!(
            error.kind,
            crate::runtime::RuntimeErrorKind::ImportTypeMismatch { .. }
        ));

        // Undeclared import name.
        let error = store
            .register_host_func(
                "env",
                "nope",
                crate::runtime::HostFunction::new(
                    crate::types::FuncType {
                        params: vec![],
                        results: vec![],
                    },
                    |_| Ok(vec![]),
                ),
            )
            .expect_err("expected UnknownImport");
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::UnknownImport {
                module: "env".into(),
                name: "nope".into(),
            }
        );
    }

    #[test]
    fn execute_imported_memory() {
        let source = "(module
            (import \"env\" \"mem\" (memory 1 2))
            (func (export \"roundtrip\") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              i32.store
              local.get 0
              i32.load)
            (func (export \"grow\") (param i32) (result i32)
              local.get 0
              memory.grow))";
        let reg_module = lower_wat(source);
        let imports = crate::runtime::Imports::new().memory(
            "env",
            "mem",
            crate::types::MemType {
                limits: crate::types::Limits {
                    min: 1,
                    max: Some(2),
                    is64: false,
                },
            },
        );
        let store = crate::runtime::Store::instantiate_with_imports(&reg_module, &imports).unwrap();

        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &store,
                "roundtrip",
                &[
                    crate::runtime::Value::I32(8),
                    crate::runtime::Value::I32(42)
                ],
            ),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
        // Grow to the provided max (2 pages) succeeds; beyond fails with -1.
        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &store,
                "grow",
                &[crate::runtime::Value::I32(1)],
            ),
            Ok(vec![crate::runtime::Value::I32(1)])
        );
        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &store,
                "grow",
                &[crate::runtime::Value::I32(1)],
            ),
            Ok(vec![crate::runtime::Value::I32(-1)])
        );
    }

    #[test]
    fn execute_imported_global_and_chained_init() {
        let source = "(module
            (import \"env\" \"base\" (global $base i32))
            (global $derived i32 (i32.add (global.get $base) (i32.const 8)))
            (func (export \"get_derived\") (result i32)
              global.get $derived))";
        let reg_module = lower_wat(source);
        let imports = crate::runtime::Imports::new().global(
            "env",
            "base",
            crate::types::GlobalType {
                val_type: ValType::Num(NumType::I32),
                mutability: crate::types::Mutability::Const,
            },
            crate::runtime::Value::I32(42),
        );
        let store = crate::runtime::Store::instantiate_with_imports(&reg_module, &imports).unwrap();

        // The init expr read the imported global's value at instantiation.
        assert_eq!(
            crate::runtime::execute_export(&reg_module, &store, "get_derived", &[]),
            Ok(vec![crate::runtime::Value::I32(50)])
        );
    }

    #[test]
    fn execute_imported_mutable_global() {
        let source = "(module
            (import \"env\" \"counter\" (global $counter (mut i32)))
            (func (export \"bump\") (result i32)
              global.get $counter
              i32.const 1
              i32.add
              global.set $counter
              global.get $counter))";
        let reg_module = lower_wat(source);
        let imports = crate::runtime::Imports::new().global(
            "env",
            "counter",
            crate::types::GlobalType {
                val_type: ValType::Num(NumType::I32),
                mutability: crate::types::Mutability::Var,
            },
            crate::runtime::Value::I32(41),
        );
        let store = crate::runtime::Store::instantiate_with_imports(&reg_module, &imports).unwrap();
        assert_eq!(
            crate::runtime::execute_export(&reg_module, &store, "bump", &[]),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn execute_imported_table() {
        let source = "(module
            (import \"env\" \"tbl\" (table 2 funcref))
            (type $t (func (result i32)))
            (func $f (type $t) i32.const 42)
            (elem declare func $f)
            (func (export \"go\") (param i32) (result i32)
              local.get 0
              call_indirect (type $t))
            (func (export \"seed\")
              i32.const 1
              ref.func $f
              table.set))";
        let reg_module = lower_wat(source);
        let imports = crate::runtime::Imports::new().table(
            "env",
            "tbl",
            crate::types::TableType {
                elem: crate::types::RefType::FuncRef,
                limits: crate::types::Limits {
                    min: 2,
                    max: None,
                    is64: false,
                },
                init: None,
            },
        );
        let store = crate::runtime::Store::instantiate_with_imports(&reg_module, &imports).unwrap();

        crate::runtime::execute_export(&reg_module, &store, "seed", &[]).unwrap();
        assert_eq!(
            crate::runtime::execute_export(
                &reg_module,
                &store,
                "go",
                &[crate::runtime::Value::I32(1)],
            ),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn start_function_runs_at_instantiation() {
        let source = "(module
            (global $g (mut i32) (i32.const 0))
            (func $init
              i32.const 42
              global.set $g)
            (func (export \"get\") (result i32)
              global.get $g)
            (start $init))";
        let reg_module = lower_wat(source);
        let mut store = crate::runtime::Store::instantiate(&reg_module).unwrap();
        store.run_start(&reg_module).unwrap();
        assert_eq!(
            crate::runtime::execute_export(&reg_module, &store, "get", &[]),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn instantiate_missing_and_mismatched_state_imports() {
        let source = "(module
            (import \"env\" \"mem\" (memory 2))
            (import \"env\" \"g\" (global i32)))";
        let reg_module = lower_wat(source);

        // Missing providers fail eagerly at instantiation.
        let error =
            crate::runtime::Store::instantiate(&reg_module).expect_err("expected UnknownImport");
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::UnknownImport {
                module: "env".into(),
                name: "mem".into(),
            }
        );

        // Provided limits below declared min fail matching.
        let imports = crate::runtime::Imports::new()
            .memory(
                "env",
                "mem",
                crate::types::MemType {
                    limits: crate::types::Limits {
                        min: 1,
                        max: None,
                        is64: false,
                    },
                },
            )
            .global(
                "env",
                "g",
                crate::types::GlobalType {
                    val_type: ValType::Num(NumType::I32),
                    mutability: crate::types::Mutability::Const,
                },
                crate::runtime::Value::I32(0),
            );
        let error = crate::runtime::Store::instantiate_with_imports(&reg_module, &imports)
            .expect_err("expected ImportTypeMismatch");
        assert!(matches!(
            error.kind,
            crate::runtime::RuntimeErrorKind::ImportTypeMismatch { .. }
        ));
    }

    #[test]
    fn linked_function_call_across_modules() {
        // Module A exports "add"; module B imports it as ("a", "add") and
        // calls it through a link_func bridge.
        let module_a = alloc::rc::Rc::new(lower_wat(
            "(module
               (func (export \"add\") (param i32 i32) (result i32)
                 local.get 0 local.get 1 i32.add))",
        ));
        let store_a = alloc::rc::Rc::new(core::cell::RefCell::new(
            crate::runtime::Store::instantiate(&module_a).unwrap(),
        ));
        let func_idx = store_a.borrow().export_func("add").unwrap();

        let module_b = lower_wat(
            "(module
               (import \"a\" \"add\" (func $add (param i32 i32) (result i32)))
               (func (export \"go\") (param i32 i32) (result i32)
                 local.get 0 local.get 1 call $add))",
        );
        let mut store_b = crate::runtime::Store::instantiate(&module_b).unwrap();
        let ty = module_b.imported_funcs[0].ty.clone();
        store_b
            .register_host_func(
                "a",
                "add",
                crate::runtime::link_func(module_a.clone(), store_a.clone(), func_idx, ty),
            )
            .unwrap();

        assert_eq!(
            crate::runtime::execute_export(
                &module_b,
                &store_b,
                "go",
                &[
                    crate::runtime::Value::I32(20),
                    crate::runtime::Value::I32(22)
                ],
            ),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn shared_memory_across_modules() {
        // A exports its memory; B imports the same handle and writes through
        // it; the write is visible to A.
        let module_a = alloc::rc::Rc::new(lower_wat(
            "(module
               (memory (export \"mem\") 1)
               (func (export \"load\") (param i32) (result i32)
                 local.get 0 i32.load))",
        ));
        let store_a = alloc::rc::Rc::new(core::cell::RefCell::new(
            crate::runtime::Store::instantiate(&module_a).unwrap(),
        ));
        let (mem_ty, shared_mem) = store_a.borrow().export_memory("mem").unwrap();

        let module_b = lower_wat(
            "(module
               (import \"a\" \"mem\" (memory 1))
               (func (export \"store\") (param i32 i32)
                 local.get 0 local.get 1 i32.store))",
        );
        let imports = crate::runtime::Imports::new().shared_memory("a", "mem", mem_ty, shared_mem);
        let store_b = crate::runtime::Store::instantiate_with_imports(&module_b, &imports).unwrap();

        crate::runtime::execute_export(
            &module_b,
            &store_b,
            "store",
            &[
                crate::runtime::Value::I32(16),
                crate::runtime::Value::I32(42),
            ],
        )
        .unwrap();

        // A reads the value B wrote into the shared memory.
        let store_a_mut = store_a.borrow_mut();
        assert_eq!(
            crate::runtime::execute_export(
                &module_a,
                &store_a_mut,
                "load",
                &[crate::runtime::Value::I32(16)],
            ),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn shared_global_across_modules() {
        let module_a = alloc::rc::Rc::new(lower_wat(
            "(module
               (global (export \"counter\") (mut i32) (i32.const 41))
               (func (export \"get\") (result i32) global.get 0))",
        ));
        let store_a = alloc::rc::Rc::new(core::cell::RefCell::new(
            crate::runtime::Store::instantiate(&module_a).unwrap(),
        ));
        let (global_ty, shared_global) = store_a.borrow().export_global("counter").unwrap();

        let module_b = lower_wat(
            "(module
               (import \"a\" \"counter\" (global (mut i32)))
               (func (export \"bump\") (result i32)
                 global.get 0
                 i32.const 1
                 i32.add
                 global.set 0
                 global.get 0))",
        );
        let imports =
            crate::runtime::Imports::new().shared_global("a", "counter", global_ty, shared_global);
        let store_b = crate::runtime::Store::instantiate_with_imports(&module_b, &imports).unwrap();

        crate::runtime::execute_export(&module_b, &store_b, "bump", &[]).unwrap();

        let store_a_mut = store_a.borrow_mut();
        assert_eq!(
            crate::runtime::execute_export(&module_a, &store_a_mut, "get", &[]),
            Ok(vec![crate::runtime::Value::I32(42)])
        );
    }

    #[test]
    fn reentrant_store_call_fails_gracefully() {
        // Calling a linked function while its store is already executing
        // fails with ReentrantStore instead of deadlocking on the RefCell.
        let module_a = alloc::rc::Rc::new(lower_wat("(module (func (export \"noop\")))"));
        let store_a = alloc::rc::Rc::new(core::cell::RefCell::new(
            crate::runtime::Store::instantiate(&module_a).unwrap(),
        ));
        let func_idx = store_a.borrow().export_func("noop").unwrap();
        let mut guard = crate::runtime::link_func(
            module_a,
            store_a.clone(),
            func_idx,
            crate::types::FuncType {
                params: vec![],
                results: vec![],
            },
        );

        let _hold = store_a.borrow_mut(); // A is "executing"
        let error = guard.call(&[]).expect_err("expected ReentrantStore");
        assert_eq!(error.kind, crate::runtime::RuntimeErrorKind::ReentrantStore);
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
