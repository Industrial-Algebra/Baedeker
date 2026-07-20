//! Minimal register-IR execution core.
//!
//! This is the first Phase 2 runtime slice: execute straight-line lowered IR
//! independently from validation. Broader control flow, calls, memory, tables,
//! traps, and host integration are added in later checkpoints.

use alloc::{string::String, vec::Vec};

mod store;

pub mod gpu;

pub use store::{PAGE_SIZE, Store};

use crate::lower::{
    BinaryOp, LaneShape, Reg, RegFunc, RegInstr, RegModule, RegOp, RegTerm, UnaryOp, V128BinaryKind,
};
use crate::types::{FuncIdx, MemArg, NumType, RefType, TableIdx, ValType};

/// A runtime WebAssembly value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    /// A reference value: either null or a function index. Until host
    /// support lands, null also represents every `externref` value (the
    /// runtime cannot produce non-null externrefs yet).
    FuncRef(Option<u32>),
    /// A 128-bit vector, stored as raw little-endian bytes; lane
    /// interpretation happens per operation.
    V128([u8; 16]),
}

impl Value {
    fn val_type(self) -> ValType {
        match self {
            Value::I32(_) => ValType::Num(NumType::I32),
            Value::I64(_) => ValType::Num(NumType::I64),
            Value::F32(_) => ValType::Num(NumType::F32),
            Value::F64(_) => ValType::Num(NumType::F64),
            Value::FuncRef(_) => ValType::Ref(RefType::FuncRef),
            Value::V128(_) => ValType::Vec(crate::types::VecType::V128),
        }
    }
}

/// Runtime execution error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
}

/// Specific runtime execution failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    ArityMismatch { expected: usize, found: usize },
    TypeMismatch { expected: ValType, found: ValType },
    Trap(RuntimeTrap),
    UninitializedLocal { local: u32 },
    UninitializedRegister { reg: Reg },
    UnknownRegister { reg: Reg },
    UnknownExport { name: String },
    ExportedFunctionNotLowered { func: u32 },
    UnknownFunction { func: u32 },
    ImportedFunctionCallUnsupported { func: u32 },
    UnknownMemory { memory: u32 },
    UnknownGlobal { global: u32 },
    UnknownTable { table: u32 },
    UnknownElem { elem: u32 },
    UnknownType { type_idx: u32 },
    ImportedMemoryAccessUnsupported { memory: u32 },
    ImportedGlobalAccessUnsupported { global: u32 },
    ImportedTableAccessUnsupported { table: u32 },
    InvalidConstExpr,
    InvalidLaneIndex { lane: u8 },
    MissingStore,
    MissingReturn,
}

/// WebAssembly runtime traps surfaced by the interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTrap {
    Unreachable,
    CallStackExhausted,
    OutOfBoundsMemoryAccess,
    OutOfBoundsTableAccess,
    UndefinedElement,
    UninitializedElement,
    IndirectCallTypeMismatch,
    IntegerDivideByZero,
    IntegerOverflow,
    InvalidConversionToInteger,
}

impl RuntimeTrap {
    /// The canonical WAST assertion message for this trap.
    pub fn wast_message(self) -> &'static str {
        match self {
            RuntimeTrap::Unreachable => "unreachable",
            RuntimeTrap::CallStackExhausted => "call stack exhausted",
            RuntimeTrap::OutOfBoundsMemoryAccess => "out of bounds memory access",
            RuntimeTrap::OutOfBoundsTableAccess => "out of bounds table access",
            RuntimeTrap::UndefinedElement => "undefined element",
            RuntimeTrap::UninitializedElement => "uninitialized element",
            RuntimeTrap::IndirectCallTypeMismatch => "indirect call type mismatch",
            RuntimeTrap::IntegerDivideByZero => "integer divide by zero",
            RuntimeTrap::IntegerOverflow => "integer overflow",
            RuntimeTrap::InvalidConversionToInteger => "invalid conversion to integer",
        }
    }
}

/// Execute an exported lowered function by name.
pub fn execute_export(
    module: &RegModule,
    store: &mut Store,
    name: &str,
    args: &[Value],
) -> Result<Vec<Value>, RuntimeError> {
    let export = module
        .exports
        .iter()
        .find(|export| export.name == name)
        .ok_or_else(|| RuntimeError {
            kind: RuntimeErrorKind::UnknownExport { name: name.into() },
        })?;

    let func = module
        .funcs
        .iter()
        .find(|func| func.idx == export.func)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::ExportedFunctionNotLowered {
                func: export.func.0,
            },
        })?;

    execute_func_in(Some(module), Some(store), func, args, 0)
}

/// Resolve and invoke a direct call target.
fn execute_call(
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    callee_idx: &FuncIdx,
    call_args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
    })?;
    if callee_idx.0 < module.imported_func_count {
        // Host functions are not yet supported; fail explicitly.
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ImportedFunctionCallUnsupported { func: callee_idx.0 },
        });
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx == *callee_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
        })?;
    execute_func_in(Some(module), store, callee, call_args, depth + 1)
}

/// Resolve and invoke an indirect call target through a table.
#[allow(clippy::too_many_arguments)]
fn execute_call_indirect(
    module: Option<&RegModule>,
    store: Option<&mut Store>,
    type_idx: &crate::types::TypeIdx,
    table: &TableIdx,
    idx: u32,
    call_args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: 0 },
    })?;
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    if store.is_imported_table(table.0) {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ImportedTableAccessUnsupported { table: table.0 },
        });
    }
    let target = store
        .table(table.0)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownTable { table: table.0 },
        })?
        .get(idx as usize)
        .copied()
        .ok_or(trap(RuntimeTrap::UndefinedElement))?;
    let Value::FuncRef(func_idx) = target else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Ref(RefType::FuncRef),
                found: target.val_type(),
            },
        });
    };
    let Some(func_idx) = func_idx else {
        return Err(trap(RuntimeTrap::UninitializedElement));
    };
    if func_idx < module.imported_func_count {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ImportedFunctionCallUnsupported { func: func_idx },
        });
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx.0 == func_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
        })?;
    // Structural type check: the callee's type must match the declared one.
    let expected = module.types.get(type_idx.0 as usize).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownType {
            type_idx: type_idx.0,
        },
    })?;
    let actual = module
        .types
        .get(callee.type_idx.0 as usize)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownType {
                type_idx: callee.type_idx.0,
            },
        })?;
    if expected != actual {
        return Err(trap(RuntimeTrap::IndirectCallTypeMismatch));
    }
    execute_func_in(Some(module), Some(store), callee, call_args, depth + 1)
}

/// Maximum call depth before the interpreter traps with stack exhaustion.
/// Bounded so that unbounded recursion exhausts the interpreter before the
/// host thread's stack does (test threads run with small stacks).
const MAX_CALL_DEPTH: usize = 128;

/// Execute a single lowered function with positional arguments.
///
/// Functions containing `call` instructions require module context; use
/// [`execute_export`] for those (a bare `execute_func` call fails with
/// [`RuntimeErrorKind::UnknownFunction`] on any `call`).
pub fn execute_func(func: &RegFunc, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    execute_func_in(None, None, func, args, 0)
}

/// Execute a function with optional module context for resolving `call`
/// targets and optional store context for memory/global access, tracking
/// recursion depth for stack exhaustion.
fn execute_func_in(
    module: Option<&RegModule>,
    mut store: Option<&mut Store>,
    func: &RegFunc,
    args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    if depth >= MAX_CALL_DEPTH {
        return Err(trap(RuntimeTrap::CallStackExhausted));
    }
    if args.len() != func.params.len() {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ArityMismatch {
                expected: func.params.len(),
                found: args.len(),
            },
        });
    }

    for (&arg, &expected) in args.iter().zip(func.params.iter()) {
        let found = arg.val_type();
        if found != expected {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::TypeMismatch { expected, found },
            });
        }
    }

    let mut locals = alloc::vec![None; func.locals.len()];
    for (idx, &arg) in args.iter().enumerate() {
        locals[idx] = Some(arg);
    }
    // Non-parameter locals are zero-initialized per spec (null for
    // nullable references).
    for (slot, &ty) in locals.iter_mut().zip(func.locals.iter()).skip(args.len()) {
        if slot.is_none() {
            *slot = match ty {
                ValType::Num(NumType::I32) => Some(Value::I32(0)),
                ValType::Num(NumType::I64) => Some(Value::I64(0)),
                ValType::Num(NumType::F32) => Some(Value::F32(0.0)),
                ValType::Num(NumType::F64) => Some(Value::F64(0.0)),
                ValType::Ref(ref_type) if ref_nullable(&ref_type) => Some(Value::FuncRef(None)),
                ValType::Vec(_) => Some(Value::V128([0; 16])),
                _ => None,
            };
        }
    }

    let mut registers = alloc::vec![None; func.reg_types.len()];

    if func.blocks.is_empty() {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::MissingReturn,
        });
    }

    let mut block_idx: u32 = 0;
    // Coarse fuel guard against infinite loops. Loops consume one unit per
    // back-edge, so this must be generous; a configurable fuel mechanism is
    // future work.
    let max_iterations = 10_000_000;
    let mut iteration: usize = 0;
    loop {
        iteration += 1;
        if iteration > max_iterations {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::MissingReturn,
            });
        }
        let block = func.blocks.get(block_idx as usize).ok_or(RuntimeError {
            kind: RuntimeErrorKind::MissingReturn,
        })?;

        // Execute straight-line instructions in this block
        for instr in &block.instrs {
            if let RegOp::Call {
                func: callee_idx,
                args: arg_regs,
                results,
            } = &instr.op
            {
                let call_args = arg_regs
                    .iter()
                    .map(|&reg| get_reg(&registers, reg))
                    .collect::<Result<Vec<_>, _>>()?;
                let returned =
                    execute_call(module, store.as_deref_mut(), callee_idx, &call_args, depth)?;
                for (&dst, value) in results.iter().zip(returned) {
                    set_reg(&mut registers, dst, value)?;
                }
            } else if let RegOp::CallIndirect {
                type_idx,
                table,
                index,
                args: arg_regs,
                results,
            } = &instr.op
            {
                let idx = expect_addr(get_reg(&registers, *index)?)?;
                let call_args = arg_regs
                    .iter()
                    .map(|&reg| get_reg(&registers, reg))
                    .collect::<Result<Vec<_>, _>>()?;
                let returned = execute_call_indirect(
                    module,
                    store.as_deref_mut(),
                    type_idx,
                    table,
                    idx,
                    &call_args,
                    depth,
                )?;
                for (&dst, value) in results.iter().zip(returned) {
                    set_reg(&mut registers, dst, value)?;
                }
            } else {
                execute_reg_op(store.as_deref_mut(), &mut registers, &mut locals, instr)?;
            }
        }

        // Follow the terminator
        match &block.term {
            RegTerm::Return { values } => {
                let mut results = Vec::with_capacity(values.len());
                for &reg in values {
                    results.push(get_reg(&registers, reg)?);
                }
                return Ok(results);
            }
            RegTerm::IfFork {
                cond,
                then_block,
                else_block,
            } => {
                let val = get_reg(&registers, *cond)?;
                if let Value::I32(v) = val {
                    if v != 0 {
                        block_idx = *then_block;
                    } else {
                        block_idx = *else_block;
                    }
                } else {
                    block_idx = *else_block;
                }
            }
            RegTerm::Br { target_block, .. } => {
                block_idx = *target_block;
            }
            RegTerm::BrIf {
                cond, target_block, ..
            } => {
                let val = get_reg(&registers, *cond)?;
                if let Value::I32(v) = val {
                    if v != 0 {
                        block_idx = *target_block;
                    } else {
                        block_idx += 1;
                    }
                } else {
                    block_idx += 1;
                }
            }
            RegTerm::BrTable {
                index,
                targets,
                default,
                ..
            } => {
                let val = get_reg(&registers, *index)?;
                // The index is read as u32: negative i32 values are large
                // and fall through to the default target.
                let idx = match val {
                    Value::I32(v) => v as u32 as usize,
                    _ => targets.len(),
                };
                block_idx = if idx < targets.len() {
                    targets[idx]
                } else {
                    *default
                };
            }
            RegTerm::Fallthrough => {
                block_idx += 1;
                if block_idx as usize >= func.blocks.len() {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::MissingReturn,
                    });
                }
            }
            RegTerm::Trap => {
                return Err(trap(RuntimeTrap::Unreachable));
            }
        }
    }
}

fn execute_reg_op(
    store: Option<&mut Store>,
    registers: &mut [Option<Value>],
    locals: &mut [Option<Value>],
    instr: &RegInstr,
) -> Result<(), RuntimeError> {
    match &instr.op {
        RegOp::LocalGet { dst, local } => {
            let value = locals
                .get(local.0 as usize)
                .and_then(|value| *value)
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UninitializedLocal { local: local.0 },
                })?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::LocalSet { local, value } | RegOp::LocalTee { local, value } => {
            let value = get_reg(registers, *value)?;
            let slot = locals.get_mut(local.0 as usize).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UninitializedLocal { local: local.0 },
            })?;
            *slot = Some(value);
        }
        RegOp::Drop { value } => {
            get_reg(registers, *value)?;
        }
        RegOp::I32Const { dst, value } => {
            set_reg(registers, *dst, Value::I32(*value))?;
        }
        RegOp::I64Const { dst, value } => {
            set_reg(registers, *dst, Value::I64(*value))?;
        }
        RegOp::F32Const { dst, value } => {
            set_reg(registers, *dst, Value::F32(*value))?;
        }
        RegOp::F64Const { dst, value } => {
            set_reg(registers, *dst, Value::F64(*value))?;
        }
        RegOp::Unary { op, dst, value } => execute_unary_op(registers, *op, *dst, *value)?,
        RegOp::Binary { op, dst, lhs, rhs } => execute_binary_op(registers, *op, *dst, *lhs, *rhs)?,
        RegOp::Copy { dst, src } => {
            let value = get_reg(registers, *src)?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::Select { dst, v1, v2, cond } => {
            let cond_value = get_reg(registers, *cond)?;
            let taken = matches!(cond_value, Value::I32(v) if v != 0);
            let value = get_reg(registers, if taken { *v1 } else { *v2 })?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::Call { func, .. } => {
            // Calls are handled in `execute_func_in`, which has module
            // context; reaching this arm means there was none.
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: func.0 },
            });
        }
        RegOp::CallIndirect { .. } => {
            // Same, but without a direct function index to report.
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: 0 },
            });
        }
        RegOp::Load {
            op,
            dst,
            addr,
            memarg,
        } => {
            let addr = expect_addr(get_reg(registers, *addr)?)?;
            let bytes = memory_slice_mut(require_store(store)?, memarg, addr, op.byte_width())?;
            let value = match op {
                crate::lower::LoadOp::I32 => {
                    Value::I32(i32::from_le_bytes(bytes.try_into().expect("width checked")))
                }
                crate::lower::LoadOp::I64 => {
                    Value::I64(i64::from_le_bytes(bytes.try_into().expect("width checked")))
                }
                crate::lower::LoadOp::F32 => Value::F32(f32::from_bits(u32::from_le_bytes(
                    bytes.try_into().expect("width checked"),
                ))),
                crate::lower::LoadOp::F64 => Value::F64(f64::from_bits(u64::from_le_bytes(
                    bytes.try_into().expect("width checked"),
                ))),
                crate::lower::LoadOp::I32Load8S => Value::I32(bytes[0] as i8 as i32),
                crate::lower::LoadOp::I32Load8U => Value::I32(bytes[0] as i32),
                crate::lower::LoadOp::I32Load16S => {
                    Value::I32(i16::from_le_bytes(bytes.try_into().expect("width checked")) as i32)
                }
                crate::lower::LoadOp::I32Load16U => {
                    Value::I32(u16::from_le_bytes(bytes.try_into().expect("width checked")) as i32)
                }
                crate::lower::LoadOp::I64Load8S => Value::I64(bytes[0] as i8 as i64),
                crate::lower::LoadOp::I64Load8U => Value::I64(bytes[0] as i64),
                crate::lower::LoadOp::I64Load16S => {
                    Value::I64(i16::from_le_bytes(bytes.try_into().expect("width checked")) as i64)
                }
                crate::lower::LoadOp::I64Load16U => {
                    Value::I64(u16::from_le_bytes(bytes.try_into().expect("width checked")) as i64)
                }
                crate::lower::LoadOp::I64Load32S => {
                    Value::I64(i32::from_le_bytes(bytes.try_into().expect("width checked")) as i64)
                }
                crate::lower::LoadOp::I64Load32U => {
                    Value::I64(u32::from_le_bytes(bytes.try_into().expect("width checked")) as i64)
                }
                crate::lower::LoadOp::V128 => Value::V128(bytes.try_into().expect("width checked")),
            };
            set_reg(registers, *dst, value)?;
        }
        RegOp::Store {
            op,
            addr,
            value,
            memarg,
        } => {
            let addr = expect_addr(get_reg(registers, *addr)?)?;
            let value = get_reg(registers, *value)?;
            let bytes = memory_slice_mut(require_store(store)?, memarg, addr, op.byte_width())?;
            match (op, value) {
                (crate::lower::StoreOp::I32, Value::I32(v)) => {
                    bytes.copy_from_slice(&v.to_le_bytes());
                }
                (crate::lower::StoreOp::I64, Value::I64(v)) => {
                    bytes.copy_from_slice(&v.to_le_bytes());
                }
                (crate::lower::StoreOp::F32, Value::F32(v)) => {
                    bytes.copy_from_slice(&v.to_bits().to_le_bytes());
                }
                (crate::lower::StoreOp::F64, Value::F64(v)) => {
                    bytes.copy_from_slice(&v.to_bits().to_le_bytes());
                }
                (crate::lower::StoreOp::I32Store8, Value::I32(v)) => {
                    bytes[0] = v as u8;
                }
                (crate::lower::StoreOp::I64Store8, Value::I64(v)) => {
                    bytes[0] = v as u8;
                }
                (crate::lower::StoreOp::I32Store16, Value::I32(v)) => {
                    bytes.copy_from_slice(&(v as u16).to_le_bytes());
                }
                (crate::lower::StoreOp::I64Store16, Value::I64(v)) => {
                    bytes.copy_from_slice(&(v as u16).to_le_bytes());
                }
                (crate::lower::StoreOp::I64Store32, Value::I64(v)) => {
                    bytes.copy_from_slice(&(v as u32).to_le_bytes());
                }
                (crate::lower::StoreOp::V128, Value::V128(v)) => {
                    bytes.copy_from_slice(&v);
                }
                (op, value) => {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::TypeMismatch {
                            expected: op.value_type(),
                            found: value.val_type(),
                        },
                    });
                }
            }
        }
        RegOp::GlobalGet { dst, global } => {
            let store = require_store(store)?;
            if store.is_imported_global(global.0) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::ImportedGlobalAccessUnsupported { global: global.0 },
                });
            }
            let value = store.global(global.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
            })?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::GlobalSet { global, value } => {
            let store = require_store(store)?;
            if store.is_imported_global(global.0) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::ImportedGlobalAccessUnsupported { global: global.0 },
                });
            }
            let value = get_reg(registers, *value)?;
            store.set_global(global.0, value).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
            })?;
        }
        RegOp::MemorySize { dst, memory } => {
            let store = require_store(store)?;
            if store.is_imported_memory(memory.0) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::ImportedMemoryAccessUnsupported { memory: memory.0 },
                });
            }
            let pages = store
                .memory_mut(memory.0)
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
                })?
                .len()
                / PAGE_SIZE;
            set_reg(registers, *dst, Value::I32(pages as i32))?;
        }
        RegOp::MemoryGrow { dst, memory, delta } => {
            let store = require_store(store)?;
            if store.is_imported_memory(memory.0) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::ImportedMemoryAccessUnsupported { memory: memory.0 },
                });
            }
            let delta = expect_addr(get_reg(registers, *delta)?)?;
            let max_pages = store
                .memory_type(memory.0)
                .and_then(|ty| ty.limits.max)
                .unwrap_or(65536) as usize;
            let mem = store.memory_mut(memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
            })?;
            let old_pages = mem.len() / PAGE_SIZE;
            let result = match old_pages.checked_add(delta as usize) {
                Some(new_pages) if new_pages <= max_pages => {
                    mem.resize(new_pages * PAGE_SIZE, 0);
                    old_pages as i32
                }
                _ => -1,
            };
            set_reg(registers, *dst, Value::I32(result))?;
        }
        RegOp::TableGet { dst, table, index } => {
            let store = require_store(store)?;
            let idx = expect_addr(get_reg(registers, *index)?)?;
            let value = table_slot(store, *table, idx)?;
            set_reg(registers, *dst, *value)?;
        }
        RegOp::TableSet {
            table,
            index,
            value,
        } => {
            let store = require_store(store)?;
            let idx = expect_addr(get_reg(registers, *index)?)?;
            let value = get_reg(registers, *value)?;
            let slot = table_slot_mut(store, *table, idx)?;
            *slot = value;
        }
        RegOp::TableSize { dst, table } => {
            let store = require_store(store)?;
            let len = defined_table(store, table.0)?.len();
            set_reg(registers, *dst, Value::I32(len as i32))?;
        }
        RegOp::TableGrow {
            dst,
            table,
            value,
            delta,
        } => {
            let store = require_store(store)?;
            let delta = expect_addr(get_reg(registers, *delta)?)?;
            let value = get_reg(registers, *value)?;
            let max = store
                .table_type(table.0)
                .and_then(|ty| ty.limits.max)
                .map(|max| max as usize)
                .unwrap_or(usize::MAX);
            let tbl = defined_table_mut(store, table.0)?;
            let old = tbl.len();
            let result = match old.checked_add(delta as usize) {
                Some(new) if new <= max => {
                    tbl.resize(new, value);
                    old as i32
                }
                _ => -1,
            };
            set_reg(registers, *dst, Value::I32(result))?;
        }
        RegOp::TableFill {
            table,
            dst,
            value,
            count,
        } => {
            let store = require_store(store)?;
            let dst = expect_addr(get_reg(registers, *dst)?)? as usize;
            let count = expect_addr(get_reg(registers, *count)?)? as usize;
            let value = get_reg(registers, *value)?;
            let tbl = defined_table_mut(store, table.0)?;
            let Some(end) = dst.checked_add(count) else {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            };
            if end > tbl.len() {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            }
            tbl[dst..end].fill(value);
        }
        RegOp::TableCopy {
            dst_table,
            src_table,
            dst,
            src,
            count,
        } => {
            let store = require_store(store)?;
            let dst = expect_addr(get_reg(registers, *dst)?)? as usize;
            let src = expect_addr(get_reg(registers, *src)?)? as usize;
            let count = expect_addr(get_reg(registers, *count)?)? as usize;
            // Bounds-check both ranges before copying (via a temporary, so
            // overlapping copies within one table behave per spec).
            let src_len = defined_table(store, src_table.0)?.len();
            let dst_len = defined_table(store, dst_table.0)?.len();
            let (Some(src_end), Some(dst_end)) = (src.checked_add(count), dst.checked_add(count))
            else {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            };
            if src_end > src_len || dst_end > dst_len {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            }
            let temp: Vec<Value> = defined_table(store, src_table.0)?[src..src_end].to_vec();
            defined_table_mut(store, dst_table.0)?[dst..dst_end].copy_from_slice(&temp);
        }
        RegOp::TableInit {
            table,
            elem,
            dst,
            src,
            count,
        } => {
            let store = require_store(store)?;
            let dst = expect_addr(get_reg(registers, *dst)?)? as usize;
            let src = expect_addr(get_reg(registers, *src)?)? as usize;
            let count = expect_addr(get_reg(registers, *count)?)? as usize;
            let segment = store
                .elem(elem.0)
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownElem { elem: elem.0 },
                })?
                .as_ref()
                .ok_or(trap(RuntimeTrap::OutOfBoundsTableAccess))?;
            let table_len = defined_table(store, table.0)?.len();
            let (Some(src_end), Some(dst_end)) = (src.checked_add(count), dst.checked_add(count))
            else {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            };
            if src_end > segment.len() || dst_end > table_len {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            }
            let temp: Vec<Value> = segment[src..src_end].to_vec();
            defined_table_mut(store, table.0)?[dst..dst_end].copy_from_slice(&temp);
        }
        RegOp::ElemDrop { elem } => {
            let store = require_store(store)?;
            store.drop_elem(elem.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownElem { elem: elem.0 },
            })?;
        }
        RegOp::RefNull { dst } => {
            set_reg(registers, *dst, Value::FuncRef(None))?;
        }
        RegOp::RefFunc { dst, func } => {
            set_reg(registers, *dst, Value::FuncRef(Some(func.0)))?;
        }
        RegOp::RefIsNull { dst, value } => {
            let value = get_reg(registers, *value)?;
            let is_null = matches!(value, Value::FuncRef(None));
            set_reg(registers, *dst, Value::I32(is_null as i32))?;
        }
        RegOp::V128Const { dst, value } => {
            set_reg(registers, *dst, Value::V128(*value))?;
        }
        RegOp::V128Splat { dst, shape, src } => {
            let scalar = get_reg(registers, *src)?;
            let bytes = splat_bytes(*shape, scalar)?;
            set_reg(registers, *dst, Value::V128(bytes))?;
        }
        RegOp::V128ExtractLane {
            dst,
            shape,
            src,
            lane,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Vec(crate::types::VecType::V128),
                        found: get_reg(registers, *src)?.val_type(),
                    },
                });
            };
            let value = extract_lane(*shape, &bytes, *lane)?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::V128ReplaceLane {
            dst,
            shape,
            vec,
            scalar,
            lane,
        } => {
            let Value::V128(mut bytes) = get_reg(registers, *vec)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Vec(crate::types::VecType::V128),
                        found: get_reg(registers, *vec)?.val_type(),
                    },
                });
            };
            let scalar = get_reg(registers, *scalar)?;
            replace_lane(*shape, &mut bytes, *lane, scalar)?;
            set_reg(registers, *dst, Value::V128(bytes))?;
        }
        RegOp::V128Binary {
            shape,
            kind,
            dst,
            lhs,
            rhs,
        } => {
            let Value::V128(lhs_bytes) = get_reg(registers, *lhs)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Vec(crate::types::VecType::V128),
                        found: get_reg(registers, *lhs)?.val_type(),
                    },
                });
            };
            let Value::V128(rhs_bytes) = get_reg(registers, *rhs)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Vec(crate::types::VecType::V128),
                        found: get_reg(registers, *rhs)?.val_type(),
                    },
                });
            };
            let bytes = v128_binary(*shape, *kind, &lhs_bytes, &rhs_bytes);
            set_reg(registers, *dst, Value::V128(bytes))?;
        }
        RegOp::V128Not { dst, src } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Vec(crate::types::VecType::V128),
                        found: get_reg(registers, *src)?.val_type(),
                    },
                });
            };
            let mut out = [0u8; 16];
            for (dst_byte, src_byte) in out.iter_mut().zip(bytes.iter()) {
                *dst_byte = !src_byte;
            }
            set_reg(registers, *dst, Value::V128(out))?;
        }
    }
    Ok(())
}

/// Read a defined table with imported/unknown handling.
fn defined_table(store: &Store, idx: u32) -> Result<&Vec<Value>, RuntimeError> {
    if store.is_imported_table(idx) {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ImportedTableAccessUnsupported { table: idx },
        });
    }
    store.table(idx).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownTable { table: idx },
    })
}

fn defined_table_mut(store: &mut Store, idx: u32) -> Result<&mut Vec<Value>, RuntimeError> {
    if store.is_imported_table(idx) {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ImportedTableAccessUnsupported { table: idx },
        });
    }
    store.table_mut(idx).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownTable { table: idx },
    })
}

fn table_slot(store: &Store, table: TableIdx, idx: u32) -> Result<&Value, RuntimeError> {
    defined_table(store, table.0)?
        .get(idx as usize)
        .ok_or(trap(RuntimeTrap::OutOfBoundsTableAccess))
}

fn table_slot_mut(
    store: &mut Store,
    table: TableIdx,
    idx: u32,
) -> Result<&mut Value, RuntimeError> {
    defined_table_mut(store, table.0)?
        .get_mut(idx as usize)
        .ok_or(trap(RuntimeTrap::OutOfBoundsTableAccess))
}

fn require_store(store: Option<&mut Store>) -> Result<&mut Store, RuntimeError> {
    store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })
}

/// Extract an i32 memory address operand as u32.
fn expect_addr(value: Value) -> Result<u32, RuntimeError> {
    match value {
        Value::I32(value) => Ok(value as u32),
        other => Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Num(NumType::I32),
                found: other.val_type(),
            },
        }),
    }
}

/// Bounds-checked mutable slice into linear memory for `addr + memarg.offset`
/// over `width` bytes.
fn memory_slice_mut<'s>(
    store: &'s mut Store,
    memarg: &MemArg,
    addr: u32,
    width: usize,
) -> Result<&'s mut [u8], RuntimeError> {
    if store.is_imported_memory(memarg.memory.0) {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::ImportedMemoryAccessUnsupported {
                memory: memarg.memory.0,
            },
        });
    }
    let mem = store.memory_mut(memarg.memory.0).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownMemory {
            memory: memarg.memory.0,
        },
    })?;
    let ea = addr as u64 + memarg.offset as u64;
    let end = ea
        .checked_add(width as u64)
        .ok_or(trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
    if end > mem.len() as u64 {
        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
    }
    Ok(&mut mem[ea as usize..end as usize])
}

/// Whether references of this type default to null (nullable refs).
fn ref_nullable(ref_type: &RefType) -> bool {
    match ref_type {
        RefType::FuncRef | RefType::ExternRef => true,
        RefType::Typed { nullable, .. } => *nullable,
    }
}

/// Lane-wise integer arithmetic (wrapping, per spec).
macro_rules! lane_int_op {
    ($kind:expr, $a:expr, $b:expr, $ty:ty) => {{
        let (a, b) = ($a as $ty, $b as $ty);
        match $kind {
            V128BinaryKind::Add => a.wrapping_add(b),
            V128BinaryKind::Sub => a.wrapping_sub(b),
            V128BinaryKind::Mul => a.wrapping_mul(b),
            V128BinaryKind::Div => a.wrapping_div(b),
            _ => unreachable!("bitwise kinds handled separately"),
        }
    }};
}

/// Lane-wise float arithmetic.
macro_rules! lane_float_op {
    ($kind:expr, $a:expr, $b:expr, $ty:ty) => {{
        let (a, b) = ($a as $ty, $b as $ty);
        match $kind {
            V128BinaryKind::Add => a + b,
            V128BinaryKind::Sub => a - b,
            V128BinaryKind::Mul => a * b,
            V128BinaryKind::Div => a / b,
            _ => unreachable!("bitwise kinds handled separately"),
        }
    }};
}

/// The little-endian bytes of a scalar at a shape's lane width (int lanes
/// truncate from i32, like splat).
fn scalar_lane_bytes(shape: LaneShape, scalar: Value) -> Result<[u8; 8], RuntimeError> {
    let mut buf = [0u8; 8];
    match (shape, scalar) {
        (LaneShape::I8x16, Value::I32(v)) => buf[0] = v as u8,
        (LaneShape::I16x8, Value::I32(v)) => buf[..2].copy_from_slice(&(v as u16).to_le_bytes()),
        (LaneShape::I32x4, Value::I32(v)) => buf[..4].copy_from_slice(&v.to_le_bytes()),
        (LaneShape::I64x2, Value::I64(v)) => buf.copy_from_slice(&v.to_le_bytes()),
        (LaneShape::F32x4, Value::F32(v)) => {
            buf[..4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        (LaneShape::F64x2, Value::F64(v)) => buf.copy_from_slice(&v.to_bits().to_le_bytes()),
        (shape, value) => {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::TypeMismatch {
                    expected: shape.scalar_type(),
                    found: value.val_type(),
                },
            });
        }
    }
    Ok(buf)
}

/// Broadcast a scalar into all 16 bytes per shape.
fn splat_bytes(shape: LaneShape, scalar: Value) -> Result<[u8; 16], RuntimeError> {
    let lane = scalar_lane_bytes(shape, scalar)?;
    let width = shape.lane_width();
    let mut out = [0u8; 16];
    for chunk in out.chunks_exact_mut(width) {
        chunk.copy_from_slice(&lane[..width]);
    }
    Ok(out)
}

/// Read one lane as a scalar Value.
fn extract_lane(shape: LaneShape, bytes: &[u8; 16], lane: u8) -> Result<Value, RuntimeError> {
    let width = shape.lane_width();
    let start = lane as usize * width;
    if start + width > 16 {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidLaneIndex { lane },
        });
    }
    let lane_bytes = &bytes[start..start + width];
    Ok(match shape {
        LaneShape::I8x16 => Value::I32(lane_bytes[0] as i8 as i32),
        LaneShape::I16x8 => {
            Value::I32(i16::from_le_bytes(lane_bytes.try_into().expect("width")) as i32)
        }
        LaneShape::I32x4 => Value::I32(i32::from_le_bytes(lane_bytes.try_into().expect("width"))),
        LaneShape::I64x2 => Value::I64(i64::from_le_bytes(lane_bytes.try_into().expect("width"))),
        LaneShape::F32x4 => Value::F32(f32::from_bits(u32::from_le_bytes(
            lane_bytes.try_into().expect("width"),
        ))),
        LaneShape::F64x2 => Value::F64(f64::from_bits(u64::from_le_bytes(
            lane_bytes.try_into().expect("width"),
        ))),
    })
}

/// Write a scalar into one lane in place.
fn replace_lane(
    shape: LaneShape,
    bytes: &mut [u8; 16],
    lane: u8,
    scalar: Value,
) -> Result<(), RuntimeError> {
    let width = shape.lane_width();
    let start = lane as usize * width;
    if start + width > 16 {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidLaneIndex { lane },
        });
    }
    let lane_bytes = scalar_lane_bytes(shape, scalar)?;
    bytes[start..start + width].copy_from_slice(&lane_bytes[..width]);
    Ok(())
}

/// Lane-wise (or bitwise) binary execution over 16-byte vectors.
fn v128_binary(shape: LaneShape, kind: V128BinaryKind, lhs: &[u8; 16], rhs: &[u8; 16]) -> [u8; 16] {
    let mut out = [0u8; 16];
    match kind {
        V128BinaryKind::And => {
            for i in 0..16 {
                out[i] = lhs[i] & rhs[i];
            }
        }
        V128BinaryKind::Or => {
            for i in 0..16 {
                out[i] = lhs[i] | rhs[i];
            }
        }
        V128BinaryKind::Xor => {
            for i in 0..16 {
                out[i] = lhs[i] ^ rhs[i];
            }
        }
        _ => {
            let width = shape.lane_width();
            for ((dst_lane, lhs_lane), rhs_lane) in out
                .chunks_exact_mut(width)
                .zip(lhs.chunks_exact(width))
                .zip(rhs.chunks_exact(width))
            {
                apply_lane_binary(shape, kind, dst_lane, lhs_lane, rhs_lane);
            }
        }
    }
    out
}

fn apply_lane_binary(
    shape: LaneShape,
    kind: V128BinaryKind,
    dst: &mut [u8],
    lhs: &[u8],
    rhs: &[u8],
) {
    match shape {
        LaneShape::I8x16 => {
            dst[0] = lane_int_op!(kind, lhs[0], rhs[0], i8) as u8;
        }
        LaneShape::I16x8 => {
            let a = i16::from_le_bytes(lhs.try_into().expect("width"));
            let b = i16::from_le_bytes(rhs.try_into().expect("width"));
            let result = lane_int_op!(kind, a, b, i16);
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::I32x4 => {
            let a = i32::from_le_bytes(lhs.try_into().expect("width"));
            let b = i32::from_le_bytes(rhs.try_into().expect("width"));
            let result = lane_int_op!(kind, a, b, i32);
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::I64x2 => {
            let a = i64::from_le_bytes(lhs.try_into().expect("width"));
            let b = i64::from_le_bytes(rhs.try_into().expect("width"));
            let result = lane_int_op!(kind, a, b, i64);
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::F32x4 => {
            let a = f32::from_bits(u32::from_le_bytes(lhs.try_into().expect("width")));
            let b = f32::from_bits(u32::from_le_bytes(rhs.try_into().expect("width")));
            let result = lane_float_op!(kind, a, b, f32);
            dst.copy_from_slice(&result.to_bits().to_le_bytes());
        }
        LaneShape::F64x2 => {
            let a = f64::from_bits(u64::from_le_bytes(lhs.try_into().expect("width")));
            let b = f64::from_bits(u64::from_le_bytes(rhs.try_into().expect("width")));
            let result = lane_float_op!(kind, a, b, f64);
            dst.copy_from_slice(&result.to_bits().to_le_bytes());
        }
    }
}

fn set_reg(registers: &mut [Option<Value>], reg: Reg, value: Value) -> Result<(), RuntimeError> {
    let slot = registers.get_mut(reg.0 as usize).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownRegister { reg },
    })?;
    *slot = Some(value);
    Ok(())
}

fn get_reg(registers: &[Option<Value>], reg: Reg) -> Result<Value, RuntimeError> {
    registers
        .get(reg.0 as usize)
        .copied()
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownRegister { reg },
        })?
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UninitializedRegister { reg },
        })
}

fn execute_unary_op(
    registers: &mut [Option<Value>],
    op: UnaryOp,
    dst: Reg,
    value: Reg,
) -> Result<(), RuntimeError> {
    match op {
        UnaryOp::I32Clz => {
            execute_i32_unary(registers, dst, value, |value| value.leading_zeros() as i32)
        }
        UnaryOp::I32Ctz => {
            execute_i32_unary(registers, dst, value, |value| value.trailing_zeros() as i32)
        }
        UnaryOp::I32Popcnt => {
            execute_i32_unary(registers, dst, value, |value| value.count_ones() as i32)
        }
        UnaryOp::I32Eqz => execute_i32_unary(registers, dst, value, |value| i32::from(value == 0)),
        UnaryOp::I32WrapI64 => execute_i64_to_i32(registers, dst, value, |value| value as i32),
        UnaryOp::I32Extend8S => {
            execute_i32_unary(registers, dst, value, |value| i32::from(value as i8))
        }
        UnaryOp::I32Extend16S => {
            execute_i32_unary(registers, dst, value, |value| i32::from(value as i16))
        }
        UnaryOp::I64Clz => {
            execute_i64_unary(registers, dst, value, |value| value.leading_zeros() as i64)
        }
        UnaryOp::I64Ctz => {
            execute_i64_unary(registers, dst, value, |value| value.trailing_zeros() as i64)
        }
        UnaryOp::I64Popcnt => {
            execute_i64_unary(registers, dst, value, |value| value.count_ones() as i64)
        }
        UnaryOp::I64Eqz => execute_i64_test(registers, dst, value, |value| i32::from(value == 0)),
        UnaryOp::I64ExtendI32S => execute_i32_to_i64(registers, dst, value, i64::from),
        UnaryOp::I64ExtendI32U => {
            execute_i32_to_i64(registers, dst, value, |value| i64::from(value as u32))
        }
        UnaryOp::I64Extend8S => {
            execute_i64_unary(registers, dst, value, |value| i64::from(value as i8))
        }
        UnaryOp::I64Extend16S => {
            execute_i64_unary(registers, dst, value, |value| i64::from(value as i16))
        }
        UnaryOp::I64Extend32S => {
            execute_i64_unary(registers, dst, value, |value| i64::from(value as i32))
        }
        UnaryOp::F32Neg => execute_f32_unary(registers, dst, value, |value| -value),
        UnaryOp::F32Abs => execute_f32_unary(registers, dst, value, libm::fabsf),
        UnaryOp::F32Sqrt => execute_f32_unary(registers, dst, value, libm::sqrtf),
        UnaryOp::F32Ceil => execute_f32_unary(registers, dst, value, libm::ceilf),
        UnaryOp::F32Floor => execute_f32_unary(registers, dst, value, libm::floorf),
        UnaryOp::F32Trunc => execute_f32_unary(registers, dst, value, libm::truncf),
        UnaryOp::F32Nearest => execute_f32_unary(registers, dst, value, libm::roundf),
        UnaryOp::F64Neg => execute_f64_unary(registers, dst, value, |value| -value),
        UnaryOp::F64Abs => execute_f64_unary(registers, dst, value, libm::fabs),
        UnaryOp::F64Sqrt => execute_f64_unary(registers, dst, value, libm::sqrt),
        UnaryOp::F64Ceil => execute_f64_unary(registers, dst, value, libm::ceil),
        UnaryOp::F64Floor => execute_f64_unary(registers, dst, value, libm::floor),
        UnaryOp::F64Trunc => execute_f64_unary(registers, dst, value, libm::trunc),
        UnaryOp::F64Nearest => execute_f64_unary(registers, dst, value, libm::round),
        UnaryOp::I32TruncF32S => {
            execute_f32_to_i32_checked(registers, dst, value, |v| v as i32, false)
        }
        UnaryOp::I32TruncF32U => {
            execute_f32_to_i32_checked(registers, dst, value, |v| v as u32 as i32, true)
        }
        UnaryOp::I32TruncF64S => {
            execute_f64_to_i32_checked(registers, dst, value, |v| v as i32, false)
        }
        UnaryOp::I32TruncF64U => {
            execute_f64_to_i32_checked(registers, dst, value, |v| v as u32 as i32, true)
        }
        UnaryOp::I64TruncF32S => {
            execute_f32_to_i64_checked(registers, dst, value, |v| v as i64, false)
        }
        UnaryOp::I64TruncF32U => {
            execute_f32_to_i64_checked(registers, dst, value, |v| v as u64 as i64, true)
        }
        UnaryOp::I64TruncF64S => {
            execute_f64_to_i64_checked(registers, dst, value, |v| v as i64, false)
        }
        UnaryOp::I64TruncF64U => {
            execute_f64_to_i64_checked(registers, dst, value, |v| v as u64 as i64, true)
        }
        UnaryOp::F32ConvertI32S => execute_i32_to_f32(registers, dst, value, |v| v as f32),
        UnaryOp::F32ConvertI32U => execute_i32_to_f32(registers, dst, value, |v| (v as u32) as f32),
        UnaryOp::F32ConvertI64S => execute_i64_to_f32(registers, dst, value, |v| v as f32),
        UnaryOp::F32ConvertI64U => execute_i64_to_f32(registers, dst, value, |v| (v as u64) as f32),
        UnaryOp::F64ConvertI32S => execute_i32_to_f64(registers, dst, value, |v| v as f64),
        UnaryOp::F64ConvertI32U => execute_i32_to_f64(registers, dst, value, |v| (v as u32) as f64),
        UnaryOp::F64ConvertI64S => execute_i64_to_f64(registers, dst, value, |v| v as f64),
        UnaryOp::F64ConvertI64U => execute_i64_to_f64(registers, dst, value, |v| (v as u64) as f64),
        UnaryOp::F32DemoteF64 => execute_f64_to_f32(registers, dst, value, |v| v as f32),
        UnaryOp::F64PromoteF32 => execute_f32_to_f64(registers, dst, value, |v| v as f64),
        UnaryOp::I32ReinterpretF32 => {
            execute_f32_to_i32(registers, dst, value, |v| v.to_bits() as i32)
        }
        UnaryOp::F32ReinterpretI32 => {
            execute_i32_to_f32(registers, dst, value, |v| f32::from_bits(v as u32))
        }
        UnaryOp::I64ReinterpretF64 => execute_f64_to_i64(registers, dst, value, |v| {
            i64::from_ne_bytes(v.to_ne_bytes())
        }),
        UnaryOp::F64ReinterpretI64 => execute_i64_to_f64(registers, dst, value, |v| {
            f64::from_ne_bytes(v.to_ne_bytes())
        }),
        UnaryOp::I32TruncSatF32S => {
            execute_f32_to_i32_saturating(registers, dst, value, |v: f32| -> i32 {
                if v.is_nan() {
                    0
                } else if v >= (i32::MAX as f32) {
                    i32::MAX
                } else if v <= (i32::MIN as f32) {
                    i32::MIN
                } else {
                    v as i32
                }
            })
        }
        UnaryOp::I32TruncSatF32U => {
            execute_f32_to_i32_saturating(registers, dst, value, |v: f32| -> i32 {
                if v.is_nan() || v <= -1.0 {
                    0
                } else if v >= (u32::MAX as f32) {
                    u32::MAX as i32
                } else {
                    v as u32 as i32
                }
            })
        }
        UnaryOp::I32TruncSatF64S => {
            execute_f64_to_i32_saturating(registers, dst, value, |v: f64| -> i32 {
                if v.is_nan() {
                    0
                } else if v >= (i32::MAX as f64) {
                    i32::MAX
                } else if v <= (i32::MIN as f64) {
                    i32::MIN
                } else {
                    v as i32
                }
            })
        }
        UnaryOp::I32TruncSatF64U => {
            execute_f64_to_i32_saturating(registers, dst, value, |v: f64| -> i32 {
                if v.is_nan() || v <= -1.0 {
                    0
                } else if v >= (u32::MAX as f64) {
                    u32::MAX as i32
                } else {
                    v as u32 as i32
                }
            })
        }
        UnaryOp::I64TruncSatF32S => {
            execute_f32_to_i64_saturating(registers, dst, value, |v: f32| -> i64 {
                if v.is_nan() {
                    0
                } else if v >= (i64::MAX as f32) {
                    i64::MAX
                } else if v <= (i64::MIN as f32) {
                    i64::MIN
                } else {
                    v as i64
                }
            })
        }
        UnaryOp::I64TruncSatF32U => {
            execute_f32_to_i64_saturating(registers, dst, value, |v: f32| -> i64 {
                if v.is_nan() || v <= -1.0 {
                    0
                } else if v >= (u64::MAX as f32) {
                    u64::MAX as i64
                } else {
                    v as u64 as i64
                }
            })
        }
        UnaryOp::I64TruncSatF64S => {
            execute_f64_to_i64_saturating(registers, dst, value, |v: f64| -> i64 {
                if v.is_nan() {
                    0
                } else if v >= (i64::MAX as f64) {
                    i64::MAX
                } else if v <= (i64::MIN as f64) {
                    i64::MIN
                } else {
                    v as i64
                }
            })
        }
        UnaryOp::I64TruncSatF64U => {
            execute_f64_to_i64_saturating(registers, dst, value, |v: f64| -> i64 {
                if v.is_nan() || v <= -1.0 {
                    0
                } else if v >= (u64::MAX as f64) {
                    u64::MAX as i64
                } else {
                    v as u64 as i64
                }
            })
        }
    }
}

fn execute_binary_op(
    registers: &mut [Option<Value>],
    op: BinaryOp,
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
) -> Result<(), RuntimeError> {
    match op {
        BinaryOp::I32Add => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs.wrapping_add(rhs))
        }
        BinaryOp::I32Sub => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs.wrapping_sub(rhs))
        }
        BinaryOp::I32Mul => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs.wrapping_mul(rhs))
        }
        BinaryOp::I32DivS => execute_i32_binary_checked(registers, dst, lhs, rhs, i32_div_s),
        BinaryOp::I32DivU => execute_i32_binary_checked(registers, dst, lhs, rhs, i32_div_u),
        BinaryOp::I32RemS => execute_i32_binary_checked(registers, dst, lhs, rhs, i32_rem_s),
        BinaryOp::I32RemU => execute_i32_binary_checked(registers, dst, lhs, rhs, i32_rem_u),
        BinaryOp::I32And => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs & rhs),
        BinaryOp::I32Or => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs | rhs),
        BinaryOp::I32Xor => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs ^ rhs),
        BinaryOp::I32Shl => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs.wrapping_shl(rhs as u32)
        }),
        BinaryOp::I32ShrS => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs >> ((rhs as u32) & 31)
        }),
        BinaryOp::I32ShrU => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            ((lhs as u32) >> ((rhs as u32) & 31)) as i32
        }),
        BinaryOp::I32Rotl => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs.rotate_left(rhs as u32)
        }),
        BinaryOp::I32Rotr => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs.rotate_right(rhs as u32)
        }),
        BinaryOp::I32Eq => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| i32::from(lhs == rhs))
        }
        BinaryOp::I32Ne => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| i32::from(lhs != rhs))
        }
        BinaryOp::I32LtS => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| i32::from(lhs < rhs))
        }
        BinaryOp::I32LtU => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            i32::from((lhs as u32) < (rhs as u32))
        }),
        BinaryOp::I32GtS => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| i32::from(lhs > rhs))
        }
        BinaryOp::I32GtU => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            i32::from((lhs as u32) > (rhs as u32))
        }),
        BinaryOp::I32LeS => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| i32::from(lhs <= rhs))
        }
        BinaryOp::I32LeU => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            i32::from((lhs as u32) <= (rhs as u32))
        }),
        BinaryOp::I32GeS => {
            execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| i32::from(lhs >= rhs))
        }
        BinaryOp::I32GeU => execute_i32_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            i32::from((lhs as u32) >= (rhs as u32))
        }),
        BinaryOp::I64Add => {
            execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs.wrapping_add(rhs))
        }
        BinaryOp::I64Sub => {
            execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs.wrapping_sub(rhs))
        }
        BinaryOp::I64Mul => {
            execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs.wrapping_mul(rhs))
        }
        BinaryOp::I64DivS => execute_i64_binary_checked(registers, dst, lhs, rhs, i64_div_s),
        BinaryOp::I64DivU => execute_i64_binary_checked(registers, dst, lhs, rhs, i64_div_u),
        BinaryOp::I64RemS => execute_i64_binary_checked(registers, dst, lhs, rhs, i64_rem_s),
        BinaryOp::I64RemU => execute_i64_binary_checked(registers, dst, lhs, rhs, i64_rem_u),
        BinaryOp::I64And => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs & rhs),
        BinaryOp::I64Or => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs | rhs),
        BinaryOp::I64Xor => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs ^ rhs),
        BinaryOp::I64Shl => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs.wrapping_shl(rhs as u32)
        }),
        BinaryOp::I64ShrS => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs >> ((rhs as u32) & 63)
        }),
        BinaryOp::I64ShrU => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            ((lhs as u64) >> ((rhs as u32) & 63)) as i64
        }),
        BinaryOp::I64Rotl => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs.rotate_left(rhs as u32)
        }),
        BinaryOp::I64Rotr => execute_i64_binary(registers, dst, lhs, rhs, |lhs, rhs| {
            lhs.rotate_right(rhs as u32)
        }),
        BinaryOp::I64Eq => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs == rhs),
        BinaryOp::I64Ne => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs != rhs),
        BinaryOp::I64LtS => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs < rhs),
        BinaryOp::I64LtU => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| {
            (lhs as u64) < (rhs as u64)
        }),
        BinaryOp::I64GtS => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs > rhs),
        BinaryOp::I64GtU => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| {
            (lhs as u64) > (rhs as u64)
        }),
        BinaryOp::I64LeS => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs <= rhs),
        BinaryOp::I64LeU => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| {
            (lhs as u64) <= (rhs as u64)
        }),
        BinaryOp::I64GeS => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs >= rhs),
        BinaryOp::I64GeU => execute_i64_compare(registers, dst, lhs, rhs, |lhs, rhs| {
            (lhs as u64) >= (rhs as u64)
        }),
        BinaryOp::F32Add => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs + rhs),
        BinaryOp::F32Sub => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs - rhs),
        BinaryOp::F32Mul => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs * rhs),
        BinaryOp::F32Div => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs / rhs),
        BinaryOp::F32Min => execute_f32_binary(registers, dst, lhs, rhs, libm::fminf),
        BinaryOp::F32Max => execute_f32_binary(registers, dst, lhs, rhs, libm::fmaxf),
        BinaryOp::F64Add => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs + rhs),
        BinaryOp::F64Sub => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs - rhs),
        BinaryOp::F64Mul => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs * rhs),
        BinaryOp::F64Div => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs / rhs),
        BinaryOp::F64Min => execute_f64_binary(registers, dst, lhs, rhs, libm::fmin),
        BinaryOp::F64Max => execute_f64_binary(registers, dst, lhs, rhs, libm::fmax),
        BinaryOp::F32Eq => execute_f32_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs == rhs),
        BinaryOp::F32Ne => execute_f32_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs != rhs),
        BinaryOp::F32Lt => execute_f32_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs < rhs),
        BinaryOp::F32Gt => execute_f32_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs > rhs),
        BinaryOp::F32Le => execute_f32_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs <= rhs),
        BinaryOp::F32Ge => execute_f32_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs >= rhs),
        BinaryOp::F64Eq => execute_f64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs == rhs),
        BinaryOp::F64Ne => execute_f64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs != rhs),
        BinaryOp::F64Lt => execute_f64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs < rhs),
        BinaryOp::F64Gt => execute_f64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs > rhs),
        BinaryOp::F64Le => execute_f64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs <= rhs),
        BinaryOp::F64Ge => execute_f64_compare(registers, dst, lhs, rhs, |lhs, rhs| lhs >= rhs),
    }
}

fn execute_i32_binary(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(i32, i32) -> i32,
) -> Result<(), RuntimeError> {
    let lhs = expect_i32(get_reg(registers, lhs)?)?;
    let rhs = expect_i32(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I32(op(lhs, rhs)))
}

fn execute_i32_binary_checked(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(i32, i32) -> Result<i32, RuntimeError>,
) -> Result<(), RuntimeError> {
    let lhs = expect_i32(get_reg(registers, lhs)?)?;
    let rhs = expect_i32(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I32(op(lhs, rhs)?))
}

fn execute_i32_unary(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i32) -> i32,
) -> Result<(), RuntimeError> {
    let value = expect_i32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_i64_to_i32(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i64) -> i32,
) -> Result<(), RuntimeError> {
    let value = expect_i64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_i32_to_i64(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i32) -> i64,
) -> Result<(), RuntimeError> {
    let value = expect_i32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I64(op(value)))
}

fn execute_i64_unary(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i64) -> i64,
) -> Result<(), RuntimeError> {
    let value = expect_i64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I64(op(value)))
}

fn execute_i64_binary(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(i64, i64) -> i64,
) -> Result<(), RuntimeError> {
    let lhs = expect_i64(get_reg(registers, lhs)?)?;
    let rhs = expect_i64(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I64(op(lhs, rhs)))
}

fn execute_i64_binary_checked(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(i64, i64) -> Result<i64, RuntimeError>,
) -> Result<(), RuntimeError> {
    let lhs = expect_i64(get_reg(registers, lhs)?)?;
    let rhs = expect_i64(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I64(op(lhs, rhs)?))
}

fn execute_i64_test(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i64) -> i32,
) -> Result<(), RuntimeError> {
    let value = expect_i64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_i64_compare(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(i64, i64) -> bool,
) -> Result<(), RuntimeError> {
    let lhs = expect_i64(get_reg(registers, lhs)?)?;
    let rhs = expect_i64(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I32(i32::from(op(lhs, rhs))))
}

fn i32_div_s(lhs: i32, rhs: i32) -> Result<i32, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    if lhs == i32::MIN && rhs == -1 {
        return Err(trap(RuntimeTrap::IntegerOverflow));
    }
    Ok(lhs / rhs)
}

fn i32_div_u(lhs: i32, rhs: i32) -> Result<i32, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    Ok(((lhs as u32) / (rhs as u32)) as i32)
}

fn i32_rem_s(lhs: i32, rhs: i32) -> Result<i32, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    if lhs == i32::MIN && rhs == -1 {
        return Ok(0);
    }
    Ok(lhs % rhs)
}

fn i32_rem_u(lhs: i32, rhs: i32) -> Result<i32, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    Ok(((lhs as u32) % (rhs as u32)) as i32)
}

fn i64_div_s(lhs: i64, rhs: i64) -> Result<i64, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    if lhs == i64::MIN && rhs == -1 {
        return Err(trap(RuntimeTrap::IntegerOverflow));
    }
    Ok(lhs / rhs)
}

fn i64_div_u(lhs: i64, rhs: i64) -> Result<i64, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    Ok(((lhs as u64) / (rhs as u64)) as i64)
}

fn i64_rem_s(lhs: i64, rhs: i64) -> Result<i64, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    if lhs == i64::MIN && rhs == -1 {
        return Ok(0);
    }
    Ok(lhs % rhs)
}

fn i64_rem_u(lhs: i64, rhs: i64) -> Result<i64, RuntimeError> {
    if rhs == 0 {
        return Err(trap(RuntimeTrap::IntegerDivideByZero));
    }
    Ok(((lhs as u64) % (rhs as u64)) as i64)
}

fn trap(trap: RuntimeTrap) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::Trap(trap),
    }
}

fn execute_f32_unary(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> f32,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F32(op(value)))
}

fn execute_f32_binary(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(f32, f32) -> f32,
) -> Result<(), RuntimeError> {
    let lhs = expect_f32(get_reg(registers, lhs)?)?;
    let rhs = expect_f32(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::F32(op(lhs, rhs)))
}

fn execute_f32_compare(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(f32, f32) -> bool,
) -> Result<(), RuntimeError> {
    let lhs = expect_f32(get_reg(registers, lhs)?)?;
    let rhs = expect_f32(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I32(i32::from(op(lhs, rhs))))
}

fn execute_f64_unary(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> f64,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F64(op(value)))
}

fn execute_f64_binary(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(f64, f64) -> f64,
) -> Result<(), RuntimeError> {
    let lhs = expect_f64(get_reg(registers, lhs)?)?;
    let rhs = expect_f64(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::F64(op(lhs, rhs)))
}

fn execute_f64_compare(
    registers: &mut [Option<Value>],
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
    op: impl FnOnce(f64, f64) -> bool,
) -> Result<(), RuntimeError> {
    let lhs = expect_f64(get_reg(registers, lhs)?)?;
    let rhs = expect_f64(get_reg(registers, rhs)?)?;
    set_reg(registers, dst, Value::I32(i32::from(op(lhs, rhs))))
}

fn execute_i32_to_f32(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i32) -> f32,
) -> Result<(), RuntimeError> {
    let value = expect_i32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F32(op(value)))
}

fn execute_i64_to_f32(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i64) -> f32,
) -> Result<(), RuntimeError> {
    let value = expect_i64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F32(op(value)))
}

fn execute_i32_to_f64(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i32) -> f64,
) -> Result<(), RuntimeError> {
    let value = expect_i32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F64(op(value)))
}

fn execute_i64_to_f64(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(i64) -> f64,
) -> Result<(), RuntimeError> {
    let value = expect_i64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F64(op(value)))
}

fn execute_f32_to_i32(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> i32,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_f64_to_i64(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> i64,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I64(op(value)))
}

fn execute_f32_to_f64(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> f64,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F64(op(value)))
}

fn execute_f64_to_f32(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> f32,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::F32(op(value)))
}

fn execute_f32_to_i32_checked(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> i32,
    unsigned: bool,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    if value.is_nan() {
        return Err(trap(RuntimeTrap::InvalidConversionToInteger));
    }
    if unsigned {
        if value <= -1.0 || value >= (u32::MAX as f32) + 0.5 {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i32::MAX as f32) + 0.5 || value < (i32::MIN as f32) - 0.5 {
        return Err(trap(RuntimeTrap::IntegerOverflow));
    }
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_f64_to_i32_checked(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> i32,
    unsigned: bool,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    if value.is_nan() {
        return Err(trap(RuntimeTrap::InvalidConversionToInteger));
    }
    if unsigned {
        if value <= -1.0 || value >= (u32::MAX as f64) + 0.5 {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i32::MAX as f64) + 0.5 || value < (i32::MIN as f64) - 0.5 {
        return Err(trap(RuntimeTrap::IntegerOverflow));
    }
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_f32_to_i64_checked(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> i64,
    unsigned: bool,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    if value.is_nan() {
        return Err(trap(RuntimeTrap::InvalidConversionToInteger));
    }
    if unsigned {
        if value <= -1.0 || value >= (u64::MAX as f32) + 0.5 {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i64::MAX as f32) + 0.5 || value < (i64::MIN as f32) - 0.5 {
        return Err(trap(RuntimeTrap::IntegerOverflow));
    }
    set_reg(registers, dst, Value::I64(op(value)))
}

fn execute_f64_to_i64_checked(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> i64,
    unsigned: bool,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    if value.is_nan() {
        return Err(trap(RuntimeTrap::InvalidConversionToInteger));
    }
    if unsigned {
        if value <= -1.0 || value >= (u64::MAX as f64) + 0.5 {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i64::MAX as f64) + 0.5 || value < (i64::MIN as f64) - 0.5 {
        return Err(trap(RuntimeTrap::IntegerOverflow));
    }
    set_reg(registers, dst, Value::I64(op(value)))
}

fn execute_f32_to_i32_saturating(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> i32,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_f64_to_i32_saturating(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> i32,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I32(op(value)))
}

fn execute_f32_to_i64_saturating(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f32) -> i64,
) -> Result<(), RuntimeError> {
    let value = expect_f32(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I64(op(value)))
}

fn execute_f64_to_i64_saturating(
    registers: &mut [Option<Value>],
    dst: Reg,
    value: Reg,
    op: impl FnOnce(f64) -> i64,
) -> Result<(), RuntimeError> {
    let value = expect_f64(get_reg(registers, value)?)?;
    set_reg(registers, dst, Value::I64(op(value)))
}

fn expect_i32(value: Value) -> Result<i32, RuntimeError> {
    match value {
        Value::I32(value) => Ok(value),
        value => Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Num(NumType::I32),
                found: value.val_type(),
            },
        }),
    }
}

fn expect_i64(value: Value) -> Result<i64, RuntimeError> {
    match value {
        Value::I64(value) => Ok(value),
        value => Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Num(NumType::I64),
                found: value.val_type(),
            },
        }),
    }
}

fn expect_f32(value: Value) -> Result<f32, RuntimeError> {
    match value {
        Value::F32(value) => Ok(value),
        value => Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Num(NumType::F32),
                found: value.val_type(),
            },
        }),
    }
}

fn expect_f64(value: Value) -> Result<f64, RuntimeError> {
    match value {
        Value::F64(value) => Ok(value),
        value => Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Num(NumType::F64),
                found: value.val_type(),
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::module::Module;

    #[test]
    fn execute_exported_add_by_name() {
        let reg_module = lowered_add_module();

        let mut store = Store::instantiate(&reg_module).unwrap();
        let result = execute_export(
            &reg_module,
            &mut store,
            "add",
            &[Value::I32(20), Value::I32(22)],
        )
        .unwrap();

        assert_eq!(result, alloc::vec![Value::I32(42)]);
    }

    #[test]
    fn reject_unknown_export_name() {
        let reg_module = lowered_add_module();

        let mut store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(&reg_module, &mut store, "missing", &[]).unwrap_err();

        assert_eq!(
            err.kind,
            RuntimeErrorKind::UnknownExport {
                name: "missing".into(),
            }
        );
    }

    #[test]
    fn reject_export_call_with_missing_arg() {
        let reg_module = lowered_add_module();

        let mut store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(&reg_module, &mut store, "add", &[Value::I32(20)]).unwrap_err();

        assert_eq!(
            err.kind,
            RuntimeErrorKind::ArityMismatch {
                expected: 2,
                found: 1,
            }
        );
    }

    #[test]
    fn reject_export_call_with_extra_arg() {
        let reg_module = lowered_add_module();

        let mut store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(
            &reg_module,
            &mut store,
            "add",
            &[Value::I32(20), Value::I32(22), Value::I32(1)],
        )
        .unwrap_err();

        assert_eq!(
            err.kind,
            RuntimeErrorKind::ArityMismatch {
                expected: 2,
                found: 3,
            }
        );
    }

    #[test]
    fn reject_export_call_with_wrong_arg_type() {
        let reg_module = lowered_add_module();

        let mut store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(
            &reg_module,
            &mut store,
            "add",
            &[Value::I64(20), Value::I32(22)],
        )
        .unwrap_err();

        assert_eq!(
            err.kind,
            RuntimeErrorKind::TypeMismatch {
                expected: ValType::Num(NumType::I32),
                found: ValType::Num(NumType::I64),
            }
        );
    }

    fn lowered_add_module() -> RegModule {
        let bytes = baedeker_testdata::fixture_bytes("add");
        let module = Module::decode(&bytes).unwrap();
        module.lower().unwrap()
    }
}
