// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Minimal register-IR execution core.
//!
//! This is the first Phase 2 runtime slice: execute straight-line lowered IR
//! independently from validation. Broader control flow, calls, memory, tables,
//! traps, and host integration are added in later checkpoints.

use alloc::{string::String, vec::Vec};

mod gcheap;
mod store;
mod table;

pub mod gpu;
pub mod host;
pub mod verify;

pub use table::Table;

pub use gcheap::{Ext, GcHeap, GcObject, GcSnapshot, GcStats, GcTypeId, GcTypeKind, ObjectId};
pub use host::{HostFunction, link_func};
pub use store::{
    ExceptionRecord, ExpandedComp, ExpandedField, ExpandedFunc, ExpandedStorage, ExpandedVal,
    Imports, LinkGroup, PAGE_SIZE, RecProvenance, Store, TagBinding,
};

use crate::lower::{
    BinaryOp, GcExt, LaneShape, Reg, RegFunc, RegInstr, RegModule, RegOp, RegTerm, UnaryOp,
    V128BinaryKind, V128CompareKind, V128ConvertKind, V128FloatUnaryKind, V128ShiftKind,
    V128UnaryKind,
};
use crate::runtime::gcheap::storage_width;
use crate::types::{
    CompType, FuncIdx, HeapType, MemArg, NumType, RefType, StorageType, TableIdx, TypeIdx, ValType,
};

/// A runtime WebAssembly value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    /// A reference value: either null or an `(instance, function)` pair
    /// identifying a function in some instance. Instance identity makes
    /// funcref values meaningful across linked modules; instance 0 is the
    /// default for unlinked execution.
    FuncRef(Option<(u32, u32)>),
    /// An external reference value from the host: either null or a
    /// host-assigned index.
    ExternRef(Option<u32>),
    /// A 128-bit vector, stored as raw little-endian bytes; lane
    /// interpretation happens per operation.
    V128([u8; 16]),
    /// An exception reference: `None` is null; `Some(idx)` indexes the
    /// store's exception arena. Arena records live as long as the store.
    ExnRef(Option<u32>),
    /// A GC aggregate reference: an index into the store's never-free
    /// [`GcHeap`]. Copies freely (no `Rc`); null GC references use the
    /// reference-kind null of the static type.
    GcRef(ObjectId),
    /// A 31-bit immediate integer (`ref.i31`). The payload is masked to
    /// 31 bits; `I31` never allocates a heap object.
    I31(i32),
}

impl Value {
    fn val_type(self) -> ValType {
        match self {
            Value::I32(_) => ValType::Num(NumType::I32),
            Value::I64(_) => ValType::Num(NumType::I64),
            Value::F32(_) => ValType::Num(NumType::F32),
            Value::F64(_) => ValType::Num(NumType::F64),
            Value::FuncRef(_) => ValType::Ref(RefType::FuncRef),
            Value::ExternRef(_) => ValType::Ref(RefType::ExternRef),
            Value::V128(_) => ValType::Vec(crate::types::VecType::V128),
            Value::ExnRef(_) => ValType::Ref(RefType::from_parts(true, HeapType::Exn)),
            Value::GcRef(_) => ValType::Ref(RefType::from_parts(true, HeapType::Any)),
            Value::I31(_) => ValType::Ref(RefType::from_parts(false, HeapType::I31)),
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
    ArityMismatch {
        expected: usize,
        found: usize,
    },
    TypeMismatch {
        expected: ValType,
        found: ValType,
    },
    Trap(RuntimeTrap),
    UninitializedLocal {
        local: u32,
    },
    UninitializedRegister {
        reg: Reg,
    },
    UnknownRegister {
        reg: Reg,
    },
    UnknownExport {
        name: String,
    },
    ExportedFunctionNotLowered {
        func: u32,
    },
    UnknownFunction {
        func: u32,
    },
    UnknownImport {
        module: String,
        name: String,
    },
    ImportTypeMismatch {
        module: String,
        name: String,
    },
    ReentrantStore,
    UnknownInstance {
        instance: u32,
    },
    ResourceLimitExceeded {
        what: &'static str,
    },
    /// The store's instruction fuel budget ran out (see `Store::set_fuel`).
    FuelExhausted,
    /// A host function returned a failure (FFI or embedding-layer error).
    HostError {
        message: String,
    },
    ImportedFunctionCallUnsupported {
        func: u32,
    },
    UnknownMemory {
        memory: u32,
    },
    UnknownDataSegment {
        data: u32,
    },
    UnknownGlobal {
        global: u32,
    },
    UnknownTable {
        table: u32,
    },
    UnknownElem {
        elem: u32,
    },
    UnknownType {
        type_idx: u32,
    },
    ImportedMemoryAccessUnsupported {
        memory: u32,
    },
    ImportedGlobalAccessUnsupported {
        global: u32,
    },
    ImportedTableAccessUnsupported {
        table: u32,
    },
    InvalidConstExpr,
    InvalidLaneIndex {
        lane: u8,
    },
    Gpu(crate::runtime::gpu::GpuError),
    MissingStore,
    MissingReturn,
    /// A wasm exception is in flight (arena index into the store).
    /// Never crosses the host boundary: `execute_export` converts an
    /// escaping `Throw` into `UncaughtException`.
    Throw {
        exn: u32,
    },
    /// An exception escaped to the host. `exn` is the arena index when a
    /// store backs execution; `None` when a storeless execution threw.
    UncaughtException {
        exn: Option<u32>,
    },
}

impl core::fmt::Display for RuntimeErrorKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Throw { .. } => write!(f, "exception in flight"),
            Self::UncaughtException { .. } => write!(f, "uncaught exception"),
            other => write!(f, "{other:?}"),
        }
    }
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
    NullFunctionReference,
    NullReference,
    /// `struct.get`/`struct.set` on a null structure reference.
    NullStructReference,
    /// A struct/array access on a null array reference.
    NullArrayReference,
    /// `i31.get_s`/`i31.get_u` on a null i31 reference.
    NullI31Reference,
    /// An array index/range out of bounds (`out of bounds array access`).
    OutOfBoundsArrayAccess,
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
            RuntimeTrap::NullFunctionReference => "null function reference",
            RuntimeTrap::NullReference => "null reference",
            RuntimeTrap::NullStructReference => "null structure reference",
            RuntimeTrap::NullArrayReference => "null array reference",
            RuntimeTrap::NullI31Reference => "null i31 reference",
            RuntimeTrap::OutOfBoundsArrayAccess => "out of bounds array access",
        }
    }
}

/// Execute an exported lowered function by name.
pub fn execute_export(
    module: &RegModule,
    store: &Store,
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

    let func_idx = match export.desc {
        crate::lower::RegExportDesc::Func(idx) => idx,
        _ => {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownExport { name: name.into() },
            });
        }
    };
    // Re-exported imported functions dispatch to the registered host
    // function; defined functions execute in the interpreter.
    if func_idx.0 < module.imported_func_count {
        return store.call_host(func_idx.0, args);
    }
    let func = module
        .funcs
        .iter()
        .find(|func| func.idx == func_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::ExportedFunctionNotLowered { func: func_idx.0 },
        })?;

    execute_func_in(Some(module), Some(store), func, args, 0).map_err(|error| match error.kind {
        RuntimeErrorKind::Throw { exn } => RuntimeError {
            kind: RuntimeErrorKind::UncaughtException { exn: Some(exn) },
        },
        _ => error,
    })
}

/// Resolve and invoke a direct call target.
fn execute_call(
    module: Option<&RegModule>,
    store: Option<&Store>,
    callee_idx: &FuncIdx,
    call_args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
    })?;
    if callee_idx.0 < module.imported_func_count {
        // Imported function: dispatch to a registered host function (lazy
        // import resolution).
        let store = store.ok_or(RuntimeError {
            kind: RuntimeErrorKind::MissingStore,
        })?;
        return store.call_host(callee_idx.0, call_args);
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
    store: Option<&Store>,
    type_idx: &crate::types::TypeIdx,
    table: &TableIdx,
    idx: u64,
    call_args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: 0 },
    })?;
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    let (instance_id, func_idx) = table_lookup_target(Some(store), table, idx)?;
    execute_funcref(
        module,
        store,
        type_idx,
        instance_id,
        func_idx,
        call_args,
        depth,
    )
}

/// Execute a `call_ref`: the function reference comes from the stack.
fn execute_call_ref(
    module: Option<&RegModule>,
    store: Option<&Store>,
    type_idx: &crate::types::TypeIdx,
    target: Value,
    call_args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: 0 },
    })?;
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    let Value::FuncRef(func_idx) = target else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Ref(RefType::FuncRef),
                found: target.val_type(),
            },
        });
    };
    let Some((instance_id, func_idx)) = func_idx else {
        return Err(trap(RuntimeTrap::NullFunctionReference));
    };
    execute_funcref(
        module,
        store,
        type_idx,
        instance_id,
        func_idx,
        call_args,
        depth,
    )
}

/// Shared tail of `call_indirect`/`call_ref`: resolve `(instance_id,
/// func_idx)` against the current instance or its link group, check the
/// structural type, and execute (host dispatch for imported targets).
#[allow(clippy::too_many_arguments)]
fn execute_funcref(
    module: &RegModule,
    store: &Store,
    type_idx: &crate::types::TypeIdx,
    instance_id: u32,
    func_idx: u32,
    call_args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    // The declared type index belongs to the CALLER's module — resolve the
    // expected signature before any remote-module shadowing. A
    // cross-instance target must be checked structurally against the
    // caller's declared type, whatever the remote module's own type
    // numbering is.
    let expected = module.types.get(type_idx.0 as usize).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownType {
            type_idx: type_idx.0,
        },
    })?;
    if instance_id != store.instance_id() {
        // Cross-instance funcref: resolve through the link group and execute
        // against the owning instance (structural type check against the
        // remote module's type entries).
        let (module, target_store) = {
            let group = store.link_group().ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownInstance {
                    instance: instance_id,
                },
            })?;
            let group = group.borrow();
            let Some((module, store)) = group.get(&instance_id) else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::UnknownInstance {
                        instance: instance_id,
                    },
                });
            };
            (module.clone(), store.clone())
        };
        let actual = if func_idx < module.imported_func_count {
            &module
                .imported_funcs
                .get(func_idx as usize)
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
                })?
                .ty
        } else {
            let callee = module
                .funcs
                .iter()
                .find(|func| func.idx.0 == func_idx)
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
                })?;
            module
                .types
                .get(callee.type_idx.0 as usize)
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownType {
                        type_idx: callee.type_idx.0,
                    },
                })?
        };
        if expected != actual {
            return Err(trap(RuntimeTrap::IndirectCallTypeMismatch));
        }
        let target_store = target_store.borrow();
        if func_idx < module.imported_func_count {
            return target_store.call_host(func_idx, call_args);
        }
        let callee = module
            .funcs
            .iter()
            .find(|func| func.idx.0 == func_idx)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
            })?;
        return execute_func_in(
            Some(&module),
            Some(&*target_store),
            callee,
            call_args,
            depth + 1,
        );
    }
    // Same-instance resolution below.
    // Structural type check: the callee's type must match the declared
    // one — for imported targets, the import declaration's type; for
    // defined targets, the function's type entry.
    if func_idx < module.imported_func_count {
        let import_ty = &module
            .imported_funcs
            .get(func_idx as usize)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
            })?
            .ty;
        if expected != import_ty {
            return Err(trap(RuntimeTrap::IndirectCallTypeMismatch));
        }
        // Indirect call targeting an imported function: host dispatch.
        return store.call_host(func_idx, call_args);
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx.0 == func_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
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
const MAX_CALL_DEPTH: usize = 512;

/// Execute a single lowered function with positional arguments.
///
/// Functions containing `call` instructions require module context; use
/// [`execute_export`] for those (a bare `execute_func` call fails with
/// [`RuntimeErrorKind::UnknownFunction`] on any `call`).
pub fn execute_func(func: &RegFunc, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    execute_func_in(None, None, func, args, 0)
}

/// A resolved tail-call target: `Defined` transfers control to a
/// same-instance function at the current depth (trampoline hop); `Ready`
/// carries the final results (host target, or cross-instance delegation
/// which recurses depth + 1 as before).
enum TailTarget<'a> {
    Defined(&'a RegFunc),
    Ready(Vec<Value>),
}

/// Resolve a direct `return_call` target. Direct indices are module-local,
/// so the target is either a host import (dispatched immediately) or a
/// same-instance defined function (trampoline hop).
fn resolve_tail_direct<'a>(
    module: Option<&'a RegModule>,
    store: Option<&Store>,
    callee_idx: &FuncIdx,
    call_args: &[Value],
) -> Result<TailTarget<'a>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
    })?;
    if callee_idx.0 < module.imported_func_count {
        // Tail call to an imported function: dispatch to the host; its
        // results are the tail-call results.
        let store = store.ok_or(RuntimeError {
            kind: RuntimeErrorKind::MissingStore,
        })?;
        return Ok(TailTarget::Ready(store.call_host(callee_idx.0, call_args)?));
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx == *callee_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
        })?;
    Ok(TailTarget::Defined(callee))
}

/// Resolve an indirect/ref tail-call target. Host targets and cross-instance
/// targets are executed immediately (results in `Ready`); same-instance
/// defined targets transfer via the trampoline (`Defined`).
#[allow(clippy::too_many_arguments)]
fn resolve_tail_funcref<'a>(
    module: Option<&'a RegModule>,
    store: Option<&Store>,
    type_idx: &crate::types::TypeIdx,
    instance_id: u32,
    func_idx: u32,
    call_args: &[Value],
    depth: usize,
) -> Result<TailTarget<'a>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: 0 },
    })?;
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    if instance_id != store.instance_id() {
        // Cross-instance tail target: delegate to the existing funcref path
        // (structural type check against the remote module, depth + 1).
        let results = execute_funcref(
            module,
            store,
            type_idx,
            instance_id,
            func_idx,
            call_args,
            depth,
        )?;
        return Ok(TailTarget::Ready(results));
    }
    // Same instance: structural type check first (mirrors `execute_funcref`).
    let expected = module.types.get(type_idx.0 as usize).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownType {
            type_idx: type_idx.0,
        },
    })?;
    if func_idx < module.imported_func_count {
        let import_ty = &module
            .imported_funcs
            .get(func_idx as usize)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
            })?
            .ty;
        if expected != import_ty {
            return Err(trap(RuntimeTrap::IndirectCallTypeMismatch));
        }
        return Ok(TailTarget::Ready(store.call_host(func_idx, call_args)?));
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx.0 == func_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
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
    Ok(TailTarget::Defined(callee))
}

/// Look up an indirect-call target in a table: `(instance_id, func_idx)`.
fn table_lookup_target(
    store: Option<&Store>,
    table: &TableIdx,
    idx: u64,
) -> Result<(u32, u32), RuntimeError> {
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    let target = store
        .with_table(table.0, |table| table.get(idx))
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownTable { table: table.0 },
        })?
        .ok_or(trap(RuntimeTrap::UndefinedElement))?;
    let Value::FuncRef(func_idx) = target else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Ref(RefType::FuncRef),
                found: target.val_type(),
            },
        });
    };
    let Some((instance_id, func_idx)) = func_idx else {
        return Err(trap(RuntimeTrap::UninitializedElement));
    };
    Ok((instance_id, func_idx))
}

/// What a matched catch clause delivers into its destination registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatchDelivery {
    /// `catch_all`: the payload and exnref are discarded.
    None,
    /// `catch`: the tag payload only.
    Payload,
    /// `catch_ref`: the tag payload followed by the exnref.
    PayloadAndExn,
    /// `catch_all_ref`: the exnref only.
    ExnOnly,
}

/// Find the innermost protected region containing `block_idx` whose catch
/// clauses match the in-flight exception, and describe the landing.
/// `None` = not caught in this frame.
///
/// Regions are consulted innermost-first: a clause of a nested `try_table`
/// takes precedence over an enclosing one's, but an enclosing region is
/// still consulted when the nested region has no matching clause.
fn consult_catch<'a>(
    func: &'a RegFunc,
    block_idx: u32,
    exn_idx: u32,
    store: &Store,
) -> Option<(u32, &'a [Reg], CatchDelivery)> {
    let record = store.exception(exn_idx)?;
    let mut regions: Vec<&crate::lower::ProtectedRegion> = func
        .regions
        .iter()
        .filter(|r| r.first_block <= block_idx && block_idx < r.end_block)
        .collect();
    regions.sort_by_key(|r| core::cmp::Reverse(r.first_block));
    for region in regions {
        for catch in &region.catches {
            let delivery = match catch.tag {
                // catch_all discards the payload; catch_all_ref keeps only
                // the exnref (its destination is a single register).
                None => {
                    if catch.dst_regs.is_empty() {
                        CatchDelivery::None
                    } else {
                        CatchDelivery::ExnOnly
                    }
                }
                Some(tag) => {
                    let Some(binding) = store.tag_binding(tag.0) else {
                        continue;
                    };
                    if record.tag != binding.identity {
                        continue;
                    }
                    if catch.dst_regs.len() == record.payload.len() + 1 {
                        CatchDelivery::PayloadAndExn
                    } else {
                        CatchDelivery::Payload
                    }
                }
            };
            return Some((catch.target_block, &catch.dst_regs[..], delivery));
        }
    }
    None
}

/// Write the landing: payload and/or exnref into dst_regs, set block_idx.
fn land_catch(
    registers: &mut [Option<Value>],
    target: u32,
    dst_regs: &[Reg],
    exn_idx: u32,
    delivery: CatchDelivery,
    store: &Store,
    block_idx: &mut u32,
) -> Result<(), RuntimeError> {
    let record = store.exception(exn_idx).ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingReturn,
    })?; // arena invariant
    match delivery {
        CatchDelivery::None => {}
        CatchDelivery::ExnOnly => {
            debug_assert_eq!(dst_regs.len(), 1, "catch_all_ref writes one exnref");
            set_reg(registers, dst_regs[0], Value::ExnRef(Some(exn_idx)))?;
        }
        CatchDelivery::Payload => {
            debug_assert_eq!(dst_regs.len(), record.payload.len());
            for (&reg, &value) in dst_regs.iter().zip(record.payload.iter()) {
                set_reg(registers, reg, value)?;
            }
        }
        CatchDelivery::PayloadAndExn => {
            debug_assert_eq!(dst_regs.len(), record.payload.len() + 1);
            for (&reg, &value) in dst_regs.iter().zip(record.payload.iter()) {
                set_reg(registers, reg, value)?;
            }
            set_reg(
                registers,
                dst_regs[record.payload.len()],
                Value::ExnRef(Some(exn_idx)),
            )?;
        }
    }
    *block_idx = target;
    Ok(())
}

/// Execute a function with optional module context for resolving `call`
/// targets and optional store context for memory/global access, tracking
/// recursion depth for stack exhaustion.
pub(crate) fn execute_func_in(
    module: Option<&RegModule>,
    store: Option<&Store>,
    func: &RegFunc,
    args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    if depth >= MAX_CALL_DEPTH {
        return Err(trap(RuntimeTrap::CallStackExhausted));
    }

    // Tail-call trampoline: `return_call*` terminators replace the current
    // frame at the same depth instead of recursing, keeping tail recursion
    // stack-safe. `func`/`args` are reassigned per hop; `module`/`store`
    // stay fixed — cross-instance tail targets delegate through
    // `resolve_tail_funcref` with depth + 1 (correct, depth-bounded).
    let mut func = func;
    let mut owned_args: Vec<Value> = Vec::new();
    let mut use_owned_args = false;
    // Backstop against true infinite loops when no fuel budget is set (the
    // coarse guard predates configurable fuel; with fuel, exhaustion is the
    // honest `FuelExhausted` error instead of `MissingReturn`, so the cap is
    // skipped entirely while a budget is active). The cap spans tail hops:
    // an unbounded no-fuel tail loop is still bounded.
    let max_iterations = 10_000_000;
    let mut iteration: usize = 0;

    'tail: loop {
        let args: &[Value] = if use_owned_args { &owned_args } else { args };
        if args.len() != func.params.len() {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::ArityMismatch {
                    expected: func.params.len(),
                    found: args.len(),
                },
            });
        }

        for (&arg, &expected) in args.iter().zip(func.params.iter()) {
            if !value_satisfies(expected, arg) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected,
                        found: arg.val_type(),
                    },
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
                    ValType::Ref(ref_type) if ref_nullable(&ref_type) => {
                        Some(ref_null_value(&ref_type))
                    }
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
        'blocks: loop {
            iteration += 1;
            // The coarse backstop applies only when no fuel budget is set;
            // with a budget, fuel exhaustion is the honest bound — a large
            // budget must not surface as `MissingReturn` with fuel left.
            if iteration > max_iterations && store.is_none_or(|store| store.fuel().is_none()) {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::MissingReturn,
                });
            }
            if let Some(store) = store
                && !store.charge_fuel()
            {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::FuelExhausted,
                });
            }
            let block = func.blocks.get(block_idx as usize).ok_or(RuntimeError {
                kind: RuntimeErrorKind::MissingReturn,
            })?;

            // Execute straight-line instructions in this block
            for instr in &block.instrs {
                if let Some(store) = store
                    && !store.charge_fuel()
                {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::FuelExhausted,
                    });
                }
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
                    let returned = execute_call(module, store, callee_idx, &call_args, depth);
                    let returned = match returned {
                        Err(error) => {
                            if let RuntimeErrorKind::Throw { exn } = &error.kind {
                                let exn = *exn;
                                match store.and_then(|s| consult_catch(func, block_idx, exn, s)) {
                                    Some((target, dst_regs, delivery)) => {
                                        land_catch(
                                            &mut registers,
                                            target,
                                            dst_regs,
                                            exn,
                                            delivery,
                                            store.expect("consult_catch requires a store"),
                                            &mut block_idx,
                                        )?;
                                        continue 'blocks;
                                    }
                                    None => return Err(error),
                                }
                            }
                            return Err(error);
                        }
                        Ok(returned) => returned,
                    };
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
                    let idx = read_addr(
                        get_reg(&registers, *index)?,
                        store
                            .and_then(|s| s.table_type(table.0))
                            .is_some_and(|ty| ty.limits.is64),
                    )?;
                    let call_args = arg_regs
                        .iter()
                        .map(|&reg| get_reg(&registers, reg))
                        .collect::<Result<Vec<_>, _>>()?;
                    let returned = execute_call_indirect(
                        module, store, type_idx, table, idx, &call_args, depth,
                    );
                    let returned = match returned {
                        Err(error) => {
                            if let RuntimeErrorKind::Throw { exn } = &error.kind {
                                let exn = *exn;
                                match store.and_then(|s| consult_catch(func, block_idx, exn, s)) {
                                    Some((target, dst_regs, delivery)) => {
                                        land_catch(
                                            &mut registers,
                                            target,
                                            dst_regs,
                                            exn,
                                            delivery,
                                            store.expect("consult_catch requires a store"),
                                            &mut block_idx,
                                        )?;
                                        continue 'blocks;
                                    }
                                    None => return Err(error),
                                }
                            }
                            return Err(error);
                        }
                        Ok(returned) => returned,
                    };
                    for (&dst, value) in results.iter().zip(returned) {
                        set_reg(&mut registers, dst, value)?;
                    }
                } else if let RegOp::CallRef {
                    type_idx,
                    func: func_ref,
                    args: arg_regs,
                    results,
                } = &instr.op
                {
                    let target = get_reg(&registers, *func_ref)?;
                    let call_args = arg_regs
                        .iter()
                        .map(|&reg| get_reg(&registers, reg))
                        .collect::<Result<Vec<_>, _>>()?;
                    let returned =
                        execute_call_ref(module, store, type_idx, target, &call_args, depth);
                    let returned = match returned {
                        Err(error) => {
                            if let RuntimeErrorKind::Throw { exn } = &error.kind {
                                let exn = *exn;
                                match store.and_then(|s| consult_catch(func, block_idx, exn, s)) {
                                    Some((target, dst_regs, delivery)) => {
                                        land_catch(
                                            &mut registers,
                                            target,
                                            dst_regs,
                                            exn,
                                            delivery,
                                            store.expect("consult_catch requires a store"),
                                            &mut block_idx,
                                        )?;
                                        continue 'blocks;
                                    }
                                    None => return Err(error),
                                }
                            }
                            return Err(error);
                        }
                        Ok(returned) => returned,
                    };
                    for (&dst, value) in results.iter().zip(returned) {
                        set_reg(&mut registers, dst, value)?;
                    }
                } else {
                    execute_reg_op(module, store, &mut registers, &mut locals, instr)?;
                }
            }

            // Follow the terminator
            match &block.term {
                RegTerm::Throw { tag, values } => {
                    let mut payload = Vec::with_capacity(values.len());
                    for &reg in values {
                        payload.push(get_reg(&registers, reg)?);
                    }
                    let Some(store) = store else {
                        return Err(RuntimeError {
                            kind: RuntimeErrorKind::UncaughtException { exn: None },
                        });
                    };
                    let identity = store
                        .tag_binding(tag.0)
                        .map(|binding| binding.identity)
                        .unwrap_or_else(|| (store.instance_id(), tag.0));
                    let exn = store.allocate_exception(identity, payload);
                    match consult_catch(func, block_idx, exn, store) {
                        Some((target, dst_regs, delivery)) => {
                            land_catch(
                                &mut registers,
                                target,
                                dst_regs,
                                exn,
                                delivery,
                                store,
                                &mut block_idx,
                            )?;
                            continue 'blocks;
                        }
                        None => {
                            return Err(RuntimeError {
                                kind: RuntimeErrorKind::Throw { exn },
                            });
                        }
                    }
                }
                RegTerm::ThrowRef { exn } => {
                    let value = get_reg(&registers, *exn)?;
                    let idx = match value {
                        Value::ExnRef(Some(idx)) => idx,
                        // A null exnref rethrow traps; any other value is
                        // unreachable given validation.
                        _ => {
                            return Err(trap(RuntimeTrap::NullReference));
                        }
                    };
                    let Some(store) = store else {
                        return Err(RuntimeError {
                            kind: RuntimeErrorKind::UncaughtException { exn: None },
                        });
                    };
                    match consult_catch(func, block_idx, idx, store) {
                        Some((target, dst_regs, delivery)) => {
                            land_catch(
                                &mut registers,
                                target,
                                dst_regs,
                                idx,
                                delivery,
                                store,
                                &mut block_idx,
                            )?;
                            continue 'blocks;
                        }
                        None => {
                            return Err(RuntimeError {
                                kind: RuntimeErrorKind::Throw { exn: idx },
                            });
                        }
                    }
                }
                RegTerm::Return { values } => {
                    let mut results = Vec::with_capacity(values.len());
                    for &reg in values {
                        results.push(get_reg(&registers, reg)?);
                    }
                    return Ok(results);
                }
                RegTerm::TailCall {
                    func: callee_idx,
                    args: arg_regs,
                } => {
                    let call_args = arg_regs
                        .iter()
                        .map(|&reg| get_reg(&registers, reg))
                        .collect::<Result<Vec<_>, _>>()?;
                    match resolve_tail_direct(module, store, callee_idx, &call_args)? {
                        TailTarget::Defined(callee) => {
                            func = callee;
                            owned_args = call_args;
                            use_owned_args = true;
                            continue 'tail;
                        }
                        TailTarget::Ready(results) => return Ok(results),
                    }
                }
                RegTerm::TailCallIndirect {
                    type_idx,
                    table,
                    index,
                    args: arg_regs,
                } => {
                    let idx = read_addr(
                        get_reg(&registers, *index)?,
                        store
                            .and_then(|s| s.table_type(table.0))
                            .is_some_and(|ty| ty.limits.is64),
                    )?;
                    let call_args = arg_regs
                        .iter()
                        .map(|&reg| get_reg(&registers, reg))
                        .collect::<Result<Vec<_>, _>>()?;
                    let (instance_id, target_func_idx) = table_lookup_target(store, table, idx)?;
                    match resolve_tail_funcref(
                        module,
                        store,
                        type_idx,
                        instance_id,
                        target_func_idx,
                        &call_args,
                        depth,
                    )? {
                        TailTarget::Defined(callee) => {
                            func = callee;
                            owned_args = call_args;
                            use_owned_args = true;
                            continue 'tail;
                        }
                        TailTarget::Ready(results) => return Ok(results),
                    }
                }
                RegTerm::TailCallRef {
                    type_idx,
                    func: func_reg,
                    args: arg_regs,
                } => {
                    let target = get_reg(&registers, *func_reg)?;
                    let call_args = arg_regs
                        .iter()
                        .map(|&reg| get_reg(&registers, reg))
                        .collect::<Result<Vec<_>, _>>()?;
                    let Value::FuncRef(func_idx) = target else {
                        return Err(RuntimeError {
                            kind: RuntimeErrorKind::TypeMismatch {
                                expected: ValType::Ref(RefType::FuncRef),
                                found: target.val_type(),
                            },
                        });
                    };
                    let Some((instance_id, target_func_idx)) = func_idx else {
                        return Err(trap(RuntimeTrap::NullFunctionReference));
                    };
                    match resolve_tail_funcref(
                        module,
                        store,
                        type_idx,
                        instance_id,
                        target_func_idx,
                        &call_args,
                        depth,
                    )? {
                        TailTarget::Defined(callee) => {
                            func = callee;
                            owned_args = call_args;
                            use_owned_args = true;
                            continue 'tail;
                        }
                        TailTarget::Ready(results) => return Ok(results),
                    }
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
                RegTerm::BrIfNull {
                    value,
                    target_block,
                    ..
                } => {
                    let val = get_reg(&registers, *value)?;
                    if is_null_ref(&val) {
                        block_idx = *target_block;
                    } else {
                        block_idx += 1;
                    }
                }
                RegTerm::BrIfNonNull {
                    value,
                    target_block,
                    ..
                } => {
                    let val = get_reg(&registers, *value)?;
                    if is_null_ref(&val) {
                        block_idx += 1;
                    } else {
                        block_idx = *target_block;
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
}

fn execute_reg_op(
    module: Option<&RegModule>,
    store: Option<&Store>,
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
        RegOp::CallRef { .. } => {
            // Same: needs module context from `execute_func_in`.
            return Err(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: 0 },
            });
        }
        RegOp::RefAsNonNull { dst, value } => {
            let val = get_reg(registers, *value)?;
            if is_null_ref(&val) {
                return Err(trap(RuntimeTrap::NullReference));
            }
            set_reg(registers, *dst, val)?;
        }
        RegOp::StructNew {
            dst,
            type_idx,
            fields,
        } => {
            let kind = gc_type_kind(module, *type_idx)?;
            let mut values = Vec::with_capacity(fields.len());
            for &reg in fields {
                values.push(get_reg(registers, reg)?);
            }
            let store = require_store(store)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_struct(kind, values)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::StructNewDefault { dst, type_idx } => {
            let kind = gc_type_kind(module, *type_idx)?;
            let fields = match &kind {
                GcTypeKind::Struct(fields) => fields
                    .iter()
                    .map(|field| default_for_storage(field.storage))
                    .collect(),
                GcTypeKind::Array(_) => return Err(unknown_type_err(*type_idx)),
            };
            let store = require_store(store)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_struct(kind, fields)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::StructGet {
            dst,
            field,
            ext,
            object,
            ..
        } => {
            let object = get_reg(registers, *object)?;
            let id = expect_gc_ref(object, RuntimeTrap::NullStructReference)?;
            let store = require_store(store)?;
            let value = store.with_gc_heap(|heap| heap.struct_get(id, *field, heap_ext(*ext)))?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::StructSet {
            field,
            object,
            value,
            ..
        } => {
            let value = get_reg(registers, *value)?;
            let object = get_reg(registers, *object)?;
            let id = expect_gc_ref(object, RuntimeTrap::NullStructReference)?;
            let store = require_store(store)?;
            store.with_gc_heap_mut(|heap| heap.struct_set(id, *field, value))?;
        }
        RegOp::ArrayNew {
            dst,
            type_idx,
            len,
            value,
        } => {
            let len = expect_u32(get_reg(registers, *len)?)?;
            let value = get_reg(registers, *value)?;
            let kind = gc_type_kind(module, *type_idx)?;
            let store = require_store(store)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_array(kind, len, value)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::ArrayNewDefault { dst, type_idx, len } => {
            let len = expect_u32(get_reg(registers, *len)?)?;
            let kind = gc_type_kind(module, *type_idx)?;
            let fill = match &kind {
                GcTypeKind::Array(field) => default_for_storage(field.storage),
                GcTypeKind::Struct(_) => return Err(unknown_type_err(*type_idx)),
            };
            let store = require_store(store)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_array(kind, len, fill)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::ArrayNewFixed {
            dst,
            type_idx,
            values,
        } => {
            let mut elems = Vec::new();
            elems
                .try_reserve(values.len())
                .map_err(|_| gc_allocation_failed())?;
            for &reg in values {
                elems.push(get_reg(registers, reg)?);
            }
            let kind = gc_type_kind(module, *type_idx)?;
            let store = require_store(store)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_fixed(kind, elems)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::ArrayNewData {
            dst,
            type_idx,
            data,
            src,
            len,
        } => {
            let src = expect_u32(get_reg(registers, *src)?)?;
            let len = expect_u32(get_reg(registers, *len)?)?;
            let kind = gc_type_kind(module, *type_idx)?;
            let storage = gc_array_storage(&kind)?;
            let width = storage_width(storage);
            let store = require_store(store)?;
            let slot = store
                .with_data(data.0, |slot| slot.cloned())
                .ok_or_else(|| unknown_data_err(data.0))?;
            let bytes = slot.as_deref().unwrap_or(&[]);
            let elems = data_segment_values(bytes, src, len, width, storage)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_fixed(kind, elems)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::ArrayNewElem {
            dst,
            type_idx,
            elem,
            src,
            len,
        } => {
            let src = expect_u32(get_reg(registers, *src)?)?;
            let len = expect_u32(get_reg(registers, *len)?)?;
            let kind = gc_type_kind(module, *type_idx)?;
            let store = require_store(store)?;
            let slot = store
                .with_elem(elem.0, |slot| slot.cloned())
                .ok_or_else(|| unknown_elem_err(elem.0))?;
            let values = slot.as_deref().unwrap_or(&[]);
            let elems = elem_segment_values(values, src, len)?;
            let heap = store.gc_heap();
            let id = heap.borrow_mut().alloc_fixed(kind, elems)?;
            set_reg(registers, *dst, Value::GcRef(id))?;
        }
        RegOp::ArrayGet {
            dst,
            array,
            index,
            ext,
            ..
        } => {
            let index = expect_u32(get_reg(registers, *index)?)?;
            let array = get_reg(registers, *array)?;
            let id = expect_gc_ref(array, RuntimeTrap::NullArrayReference)?;
            let store = require_store(store)?;
            let len = with_array_len(store, id)?;
            if array_index_oob(index, 1, len) {
                return Err(trap(RuntimeTrap::OutOfBoundsArrayAccess));
            }
            let value = store.with_gc_heap(|heap| heap.array_get(id, index, heap_ext(*ext)))?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::ArraySet {
            array,
            index,
            value,
            ..
        } => {
            let value = get_reg(registers, *value)?;
            let index = expect_u32(get_reg(registers, *index)?)?;
            let array = get_reg(registers, *array)?;
            let id = expect_gc_ref(array, RuntimeTrap::NullArrayReference)?;
            let store = require_store(store)?;
            let len = with_array_len(store, id)?;
            if array_index_oob(index, 1, len) {
                return Err(trap(RuntimeTrap::OutOfBoundsArrayAccess));
            }
            store.with_gc_heap_mut(|heap| heap.array_set(id, index, value))?;
        }
        RegOp::ArrayLen { dst, array } => {
            let array = get_reg(registers, *array)?;
            let id = expect_gc_ref(array, RuntimeTrap::NullArrayReference)?;
            let store = require_store(store)?;
            let len = store
                .with_gc_heap(|heap| heap.array_len(id))
                .ok_or_else(|| unknown_type_err(TypeIdx(0)))?;
            set_reg(registers, *dst, Value::I32(len as i32))?;
        }
        RegOp::ArrayFill {
            array,
            index,
            value,
            len,
            ..
        } => {
            let len = expect_u32(get_reg(registers, *len)?)?;
            let value = get_reg(registers, *value)?;
            let index = expect_u32(get_reg(registers, *index)?)?;
            let array = get_reg(registers, *array)?;
            let id = expect_gc_ref(array, RuntimeTrap::NullArrayReference)?;
            let store = require_store(store)?;
            let array_len = with_array_len(store, id)?;
            if array_index_oob(index, len, array_len) {
                return Err(trap(RuntimeTrap::OutOfBoundsArrayAccess));
            }
            if len == 0 {
                return Ok(());
            }
            let heap = store.gc_heap();
            let mut heap = heap.borrow_mut();
            for i in 0..len {
                heap.array_set(id, index + i, value)?;
            }
        }
        RegOp::ArrayCopy {
            dst,
            dst_off,
            src,
            src_off,
            len,
            ..
        } => {
            let len = expect_u32(get_reg(registers, *len)?)?;
            let src_off = expect_u32(get_reg(registers, *src_off)?)?;
            let src_value = get_reg(registers, *src)?;
            let dst_off = expect_u32(get_reg(registers, *dst_off)?)?;
            let dst_value = get_reg(registers, *dst)?;
            // Null checks precede bounds; the reference checks the SOURCE
            // reference first (eval.ml's ArrayCopy pattern order).
            let src_id = expect_gc_ref(src_value, RuntimeTrap::NullArrayReference)?;
            let dst_id = expect_gc_ref(dst_value, RuntimeTrap::NullArrayReference)?;
            let store = require_store(store)?;
            let src_len = with_array_len(store, src_id)?;
            let dst_len = with_array_len(store, dst_id)?;
            if array_index_oob(src_off, len, src_len) || array_index_oob(dst_off, len, dst_len) {
                return Err(trap(RuntimeTrap::OutOfBoundsArrayAccess));
            }
            if len == 0 {
                return Ok(());
            }
            let ext = store.with_gc_heap(|heap| heap.array_ext(src_id));
            let heap = store.gc_heap();
            let mut heap = heap.borrow_mut();
            if dst_off <= src_off {
                for i in 0..len {
                    let value = heap.array_get(src_id, src_off + i, ext)?;
                    heap.array_set(dst_id, dst_off + i, value)?;
                }
            } else {
                for i in (0..len).rev() {
                    let value = heap.array_get(src_id, src_off + i, ext)?;
                    heap.array_set(dst_id, dst_off + i, value)?;
                }
            }
        }
        RegOp::ArrayInitData {
            type_idx,
            data,
            array,
            dst_off,
            src_off,
            len,
        } => {
            let len = expect_u32(get_reg(registers, *len)?)?;
            let src_off = expect_u32(get_reg(registers, *src_off)?)?;
            let dst_off = expect_u32(get_reg(registers, *dst_off)?)?;
            let array = get_reg(registers, *array)?;
            let id = expect_gc_ref(array, RuntimeTrap::NullArrayReference)?;
            let kind = gc_type_kind(module, *type_idx)?;
            let storage = gc_array_storage(&kind)?;
            let width = storage_width(storage);
            let store = require_store(store)?;
            let array_len = with_array_len(store, id)?;
            // Array bounds before segment bounds (trap identity matters).
            if array_index_oob(dst_off, len, array_len) {
                return Err(trap(RuntimeTrap::OutOfBoundsArrayAccess));
            }
            let slot = store
                .with_data(data.0, |slot| slot.cloned())
                .ok_or_else(|| unknown_data_err(data.0))?;
            let bytes = slot.as_deref().unwrap_or(&[]);
            let elems = data_segment_values(bytes, src_off, len, width, storage)?;
            if len == 0 {
                return Ok(());
            }
            let heap = store.gc_heap();
            let mut heap = heap.borrow_mut();
            for (i, value) in elems.into_iter().enumerate() {
                heap.array_set(id, dst_off + i as u32, value)?;
            }
        }
        RegOp::ArrayInitElem {
            type_idx,
            elem,
            array,
            dst_off,
            src_off,
            len,
        } => {
            let len = expect_u32(get_reg(registers, *len)?)?;
            let src_off = expect_u32(get_reg(registers, *src_off)?)?;
            let dst_off = expect_u32(get_reg(registers, *dst_off)?)?;
            let array = get_reg(registers, *array)?;
            let id = expect_gc_ref(array, RuntimeTrap::NullArrayReference)?;
            let _ = type_idx;
            let store = require_store(store)?;
            let array_len = with_array_len(store, id)?;
            if array_index_oob(dst_off, len, array_len) {
                return Err(trap(RuntimeTrap::OutOfBoundsArrayAccess));
            }
            let slot = store
                .with_elem(elem.0, |slot| slot.cloned())
                .ok_or_else(|| unknown_elem_err(elem.0))?;
            let values = slot.as_deref().unwrap_or(&[]);
            let elems = elem_segment_values(values, src_off, len)?;
            if len == 0 {
                return Ok(());
            }
            let heap = store.gc_heap();
            let mut heap = heap.borrow_mut();
            for (i, value) in elems.into_iter().enumerate() {
                heap.array_set(id, dst_off + i as u32, value)?;
            }
        }
        RegOp::RefI31 { dst, value } => {
            let value = expect_u32(get_reg(registers, *value)?)?;
            set_reg(registers, *dst, Value::I31((value & 0x7FFF_FFFF) as i32))?;
        }
        RegOp::I31Get { dst, value, signed } => {
            let value = get_reg(registers, *value)?;
            match value {
                Value::I31(payload) => {
                    let result = if *signed {
                        (payload << 1) >> 1
                    } else {
                        payload
                    };
                    set_reg(registers, *dst, Value::I32(result))?;
                }
                other if is_null_ref(&other) => {
                    return Err(trap(RuntimeTrap::NullI31Reference));
                }
                other => {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::TypeMismatch {
                            expected: ValType::Ref(RefType::from_parts(false, HeapType::I31)),
                            found: other.val_type(),
                        },
                    });
                }
            }
        }
        RegOp::Load {
            op,
            dst,
            addr,
            memarg,
        } => {
            let store = require_store(store)?;
            let is64 = store
                .memory_type(memarg.memory.0)
                .is_some_and(|ty| ty.limits.is64);
            let addr = read_addr(get_reg(registers, *addr)?, is64)?;
            let mem = store.shared_memory(memarg.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: memarg.memory.0,
                },
            })?;
            let mem = mem.borrow();
            let range = memory_bounds(&mem, memarg, addr, op.byte_width())?;
            let bytes = &mem[range];
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
                crate::lower::LoadOp::V128Load8x8S => {
                    let mut out = [0u8; 16];
                    for (lane, byte) in out.chunks_exact_mut(2).zip(bytes.iter()) {
                        let extended = (*byte as i8) as i16;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load8x8U => {
                    let mut out = [0u8; 16];
                    for (lane, byte) in out.chunks_exact_mut(2).zip(bytes.iter()) {
                        lane.copy_from_slice(&(*byte as u16).to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load16x4S => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(4).zip(bytes.chunks_exact(2)) {
                        let extended = i16::from_le_bytes(chunk.try_into().expect("width")) as i32;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load16x4U => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(4).zip(bytes.chunks_exact(2)) {
                        let extended = u16::from_le_bytes(chunk.try_into().expect("width")) as u32;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32x2S => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(8).zip(bytes.chunks_exact(4)) {
                        let extended = i32::from_le_bytes(chunk.try_into().expect("width")) as i64;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32x2U => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(8).zip(bytes.chunks_exact(4)) {
                        let extended = u32::from_le_bytes(chunk.try_into().expect("width")) as u64;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load8Splat => Value::V128([bytes[0]; 16]),
                crate::lower::LoadOp::V128Load16Splat => {
                    let mut out = [0u8; 16];
                    for lane in out.chunks_exact_mut(2) {
                        lane.copy_from_slice(&bytes[..2]);
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32Splat => {
                    let mut out = [0u8; 16];
                    for lane in out.chunks_exact_mut(4) {
                        lane.copy_from_slice(&bytes[..4]);
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load64Splat => {
                    let mut out = [0u8; 16];
                    for lane in out.chunks_exact_mut(8) {
                        lane.copy_from_slice(&bytes[..8]);
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32Zero => {
                    let mut out = [0u8; 16];
                    out[..4].copy_from_slice(&bytes[..4]);
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load64Zero => {
                    let mut out = [0u8; 16];
                    out[..8].copy_from_slice(&bytes[..8]);
                    Value::V128(out)
                }
            };
            set_reg(registers, *dst, value)?;
        }
        RegOp::Store {
            op,
            addr,
            value,
            memarg,
        } => {
            let store = require_store(store)?;
            let is64 = store
                .memory_type(memarg.memory.0)
                .is_some_and(|ty| ty.limits.is64);
            let addr = read_addr(get_reg(registers, *addr)?, is64)?;
            let value = get_reg(registers, *value)?;
            let mem = store.shared_memory(memarg.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: memarg.memory.0,
                },
            })?;
            let mut mem = mem.borrow_mut();
            let range = memory_bounds(&mem, memarg, addr, op.byte_width())?;
            let bytes = &mut mem[range];
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
        RegOp::V128LoadLane {
            width,
            dst,
            vec,
            addr,
            memarg,
            lane,
        } => {
            let store = require_store(store)?;
            let is64 = store
                .memory_type(memarg.memory.0)
                .is_some_and(|ty| ty.limits.is64);
            let addr = read_addr(get_reg(registers, *addr)?, is64)?;
            let Value::V128(mut bytes) = get_reg(registers, *vec)? else {
                return Err(type_mismatch_v128(registers, *vec));
            };
            let mem = store.shared_memory(memarg.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: memarg.memory.0,
                },
            })?;
            let mem = mem.borrow();
            let range = memory_bounds(&mem, memarg, addr, *width as usize)?;
            let start = *lane as usize * *width as usize;
            bytes[start..start + *width as usize].copy_from_slice(&mem[range]);
            drop(mem);
            set_reg(registers, *dst, Value::V128(bytes))?;
        }
        RegOp::V128StoreLane {
            width,
            addr,
            vec,
            memarg,
            lane,
        } => {
            let store = require_store(store)?;
            let is64 = store
                .memory_type(memarg.memory.0)
                .is_some_and(|ty| ty.limits.is64);
            let addr = read_addr(get_reg(registers, *addr)?, is64)?;
            let Value::V128(bytes) = get_reg(registers, *vec)? else {
                return Err(type_mismatch_v128(registers, *vec));
            };
            let mem = store.shared_memory(memarg.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: memarg.memory.0,
                },
            })?;
            let mut mem = mem.borrow_mut();
            let range = memory_bounds(&mem, memarg, addr, *width as usize)?;
            let start = *lane as usize * *width as usize;
            mem[range].copy_from_slice(&bytes[start..start + *width as usize]);
        }
        RegOp::GlobalGet { dst, global } => {
            let store = require_store(store)?;
            let value = store.global(global.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
            })?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::GlobalSet { global, value } => {
            let store = require_store(store)?;
            let value = get_reg(registers, *value)?;
            store.set_global(global.0, value).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownGlobal { global: global.0 },
            })?;
        }
        RegOp::MemorySize { dst, memory } => {
            let store = require_store(store)?;
            let is64 = store.memory_type(memory.0).is_some_and(|ty| ty.limits.is64);
            let pages = store
                .with_memory(memory.0, |mem| mem.len())
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
                })?
                / PAGE_SIZE;
            let value = if is64 {
                Value::I64(pages as i64)
            } else {
                Value::I32(pages as i32)
            };
            set_reg(registers, *dst, value)?;
        }
        RegOp::MemoryGrow { dst, memory, delta } => {
            let store = require_store(store)?;
            let is64 = store.memory_type(memory.0).is_some_and(|ty| ty.limits.is64);
            let delta = read_addr(get_reg(registers, *delta)?, is64)?;
            let max_pages = store
                .memory_type(memory.0)
                .and_then(|ty| ty.limits.max)
                .unwrap_or(if is64 { 1 << 48 } else { 1 << 16 });
            let mem = store.shared_memory(memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
            })?;
            let result: u64 = {
                let mut mem = mem.borrow_mut();
                let old_pages = (mem.len() / PAGE_SIZE) as u64;
                match old_pages.checked_add(delta) {
                    Some(new_pages)
                        if new_pages <= max_pages
                            && usize::try_from(new_pages)
                                .map(|pages| grow_memory_fallible(&mut mem, pages))
                                .unwrap_or(false) =>
                    {
                        old_pages
                    }
                    _ => u64::MAX,
                }
            };
            let value = if is64 {
                Value::I64(result as i64)
            } else {
                Value::I32(result as i32)
            };
            set_reg(registers, *dst, value)?;
        }
        RegOp::MemoryInit {
            memory,
            data,
            dst,
            src,
            count,
        } => {
            let store = require_store(store)?;
            let is64 = store.memory_type(memory.0).is_some_and(|ty| ty.limits.is64);
            let dst = read_addr(get_reg(registers, *dst)?, is64)?;
            let src = u64::from(expect_addr(get_reg(registers, *src)?)?);
            let count = u64::from(expect_addr(get_reg(registers, *count)?)?);
            // Bounds-check and copy the segment range before borrowing memory.
            let (bytes, dst_end) = {
                let segment = store
                    .with_data(data.0, |segment| segment.cloned())
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownDataSegment { data: data.0 },
                    })?
                    .ok_or(trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
                let segment = &segment;
                let (Some(src_end), Some(dst_end)) =
                    (src.checked_add(count), dst.checked_add(count))
                else {
                    return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                };
                if src_end > segment.len() as u64 {
                    return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                }
                (segment[src as usize..src_end as usize].to_vec(), dst_end)
            };
            store
                .with_memory_mut(memory.0, |mem| {
                    if dst_end > mem.len() as u64 {
                        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                    }
                    mem[dst as usize..dst_end as usize].copy_from_slice(&bytes);
                    Ok(())
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
                })??;
        }
        RegOp::DataDrop { data } => {
            let store = require_store(store)?;
            store.drop_data(data.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownDataSegment { data: data.0 },
            })?;
        }
        RegOp::MemoryCopy {
            dst_memory,
            src_memory,
            dst,
            src,
            count,
        } => {
            let store = require_store(store)?;
            let dst_is64 = store
                .memory_type(dst_memory.0)
                .is_some_and(|ty| ty.limits.is64);
            let src_is64 = store
                .memory_type(src_memory.0)
                .is_some_and(|ty| ty.limits.is64);
            let dst = read_addr(get_reg(registers, *dst)?, dst_is64)?;
            let src = read_addr(get_reg(registers, *src)?, src_is64)?;
            let count = read_addr(get_reg(registers, *count)?, dst_is64 && src_is64)?;
            // Bounds-check both ranges before copying (via a temporary, so
            // overlapping regions copy per spec).
            let src_len = store
                .with_memory(src_memory.0, |mem| mem.len())
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory {
                        memory: src_memory.0,
                    },
                })?;
            let dst_len = store
                .with_memory(dst_memory.0, |mem| mem.len())
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory {
                        memory: dst_memory.0,
                    },
                })?;
            let (Some(src_end), Some(dst_end)) = (src.checked_add(count), dst.checked_add(count))
            else {
                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
            };
            if src_end > src_len as u64 || dst_end > dst_len as u64 {
                return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
            }
            let temp: Vec<u8> = store
                .with_memory(src_memory.0, |mem| {
                    mem[src as usize..src_end as usize].to_vec()
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory {
                        memory: src_memory.0,
                    },
                })?;
            store
                .with_memory_mut(dst_memory.0, |mem| {
                    mem[dst as usize..dst_end as usize].copy_from_slice(&temp);
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory {
                        memory: dst_memory.0,
                    },
                })?;
        }
        RegOp::MemoryFill {
            memory,
            dst,
            value,
            count,
        } => {
            let store = require_store(store)?;
            let is64 = store.memory_type(memory.0).is_some_and(|ty| ty.limits.is64);
            let dst = read_addr(get_reg(registers, *dst)?, is64)?;
            let count = read_addr(get_reg(registers, *count)?, is64)?;
            let value = expect_addr(get_reg(registers, *value)?)? as u8;
            store
                .with_memory_mut(memory.0, |mem| {
                    let Some(end) = dst.checked_add(count) else {
                        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                    };
                    if end > mem.len() as u64 {
                        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
                    }
                    mem[dst as usize..end as usize].fill(value);
                    Ok(())
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownMemory { memory: memory.0 },
                })??;
        }
        RegOp::TableGet { dst, table, index } => {
            let store = require_store(store)?;
            let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
            let idx = read_addr(get_reg(registers, *index)?, is64)?;
            let value = store
                .with_table(table.0, |table| table.get(idx))
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                })?
                .ok_or(trap(RuntimeTrap::OutOfBoundsTableAccess))?;
            set_reg(registers, *dst, value)?;
        }
        RegOp::TableSet {
            table,
            index,
            value,
        } => {
            let store = require_store(store)?;
            let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
            let idx = read_addr(get_reg(registers, *index)?, is64)?;
            let value = get_reg(registers, *value)?;
            store
                .with_table_mut(table.0, |table| {
                    if table.set(idx, value) {
                        Ok(())
                    } else {
                        Err(trap(RuntimeTrap::OutOfBoundsTableAccess))
                    }
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                })??;
        }
        RegOp::TableSize { dst, table } => {
            let store = require_store(store)?;
            let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
            let len = store
                .with_table(table.0, |table| table.len())
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                })?;
            let value = if is64 {
                Value::I64(len as i64)
            } else {
                Value::I32(len as i32)
            };
            set_reg(registers, *dst, value)?;
        }
        RegOp::TableGrow {
            dst,
            table,
            value,
            delta,
        } => {
            let store = require_store(store)?;
            let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
            let delta = read_addr(get_reg(registers, *delta)?, is64)?;
            let value = get_reg(registers, *value)?;
            let tbl = store.shared_table(table.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownTable { table: table.0 },
            })?;
            let result: u64 = tbl.borrow_mut().grow(delta, value).unwrap_or(u64::MAX);
            let value = if is64 {
                Value::I64(result as i64)
            } else {
                Value::I32(result as i32)
            };
            set_reg(registers, *dst, value)?;
        }
        RegOp::TableFill {
            table,
            dst,
            value,
            count,
        } => {
            let store = require_store(store)?;
            let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
            let dst = read_addr(get_reg(registers, *dst)?, is64)?;
            let count = read_addr(get_reg(registers, *count)?, is64)?;
            let value = get_reg(registers, *value)?;
            store
                .with_table_mut(table.0, |tbl| {
                    if tbl.fill(dst, value, count) {
                        Ok(())
                    } else {
                        Err(trap(RuntimeTrap::OutOfBoundsTableAccess))
                    }
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                })??;
        }
        RegOp::TableCopy {
            dst_table,
            src_table,
            dst,
            src,
            count,
        } => {
            let store = require_store(store)?;
            let dst_is64 = store
                .table_type(dst_table.0)
                .is_some_and(|ty| ty.limits.is64);
            let src_is64 = store
                .table_type(src_table.0)
                .is_some_and(|ty| ty.limits.is64);
            let dst = read_addr(get_reg(registers, *dst)?, dst_is64)?;
            let src = read_addr(get_reg(registers, *src)?, src_is64)?;
            let count = read_addr(get_reg(registers, *count)?, dst_is64 && src_is64)?;
            // Bounds-check BOTH ranges before any copying or allocation:
            // a huge count against a short table must trap, never abort
            // on an oversized temporary (review r1 P1).
            let dst_len =
                store
                    .with_table(dst_table.0, |table| table.len())
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownTable { table: dst_table.0 },
                    })?;
            let src_len =
                store
                    .with_table(src_table.0, |table| table.len())
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownTable { table: src_table.0 },
                    })?;
            let dst_ok = dst.checked_add(count).is_some_and(|end| end <= dst_len);
            let src_ok = src.checked_add(count).is_some_and(|end| end <= src_len);
            if !dst_ok || !src_ok {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            }
            // Aliased imports (one shared table bound at two indices)
            // must take the same-table path too: borrowing one RefCell
            // twice would panic (review r2 P1).
            let dst_handle = store.shared_table(dst_table.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownTable { table: dst_table.0 },
            })?;
            let src_handle = store.shared_table(src_table.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownTable { table: src_table.0 },
            })?;
            if dst_table.0 == src_table.0 || alloc::rc::Rc::ptr_eq(&dst_handle, &src_handle) {
                // One table: overlap-preserving copy, no temporary. A
                // refused allocation inside the copy is a CLEAN runtime
                // failure — never a silent success with a partial
                // destination (review r5 P1).
                let copied = store
                    .with_table_mut(dst_table.0, |table| table.copy_within(dst, src, count))
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UnknownTable { table: dst_table.0 },
                    })?;
                if !copied {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::ResourceLimitExceeded { what: "table copy" },
                    });
                }
            } else {
                // Two tables: distinct handles, still no temporary — copy
                // in runs straight across.
                let src_ref = src_handle.borrow();
                let mut dst_ref = dst_handle.borrow_mut();
                if !dst_ref.copy_from(&src_ref, dst, src, count) {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::ResourceLimitExceeded { what: "table copy" },
                    });
                }
            }
        }
        RegOp::TableInit {
            table,
            elem,
            dst,
            src,
            count,
        } => {
            let store = require_store(store)?;
            let is64 = store.table_type(table.0).is_some_and(|ty| ty.limits.is64);
            let dst = read_addr(get_reg(registers, *dst)?, is64)?;
            let src = u64::from(expect_addr(get_reg(registers, *src)?)?);
            let count = u64::from(expect_addr(get_reg(registers, *count)?)?);
            let segment = store
                .with_elem(elem.0, |segment| segment.cloned())
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownElem { elem: elem.0 },
                })?
                .ok_or(trap(RuntimeTrap::OutOfBoundsTableAccess))?;
            let segment = &segment;
            let table_len = store
                .with_table(table.0, |table| table.len())
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                })?;
            let (Some(src_end), Some(dst_end)) = (src.checked_add(count), dst.checked_add(count))
            else {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            };
            if src_end > segment.len() as u64 || dst_end > table_len {
                return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
            }
            let temp: Vec<Value> = segment[src as usize..src_end as usize].to_vec();
            store
                .with_table_mut(table.0, |table| {
                    if table.write_slice(dst, &temp) {
                        Ok(())
                    } else {
                        Err(trap(RuntimeTrap::OutOfBoundsTableAccess))
                    }
                })
                .ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UnknownTable { table: table.0 },
                })??;
        }
        RegOp::ElemDrop { elem } => {
            let store = require_store(store)?;
            store.drop_elem(elem.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownElem { elem: elem.0 },
            })?;
        }
        RegOp::RefNull { dst, ref_type } => {
            set_reg(registers, *dst, ref_null_value(ref_type))?;
        }
        RegOp::RefFunc { dst, func } => {
            let store = require_store(store)?;
            set_reg(
                registers,
                *dst,
                Value::FuncRef(Some((store.instance_id(), func.0))),
            )?;
        }
        RegOp::RefIsNull { dst, value } => {
            let value = get_reg(registers, *value)?;
            set_reg(registers, *dst, Value::I32(is_null_ref(&value) as i32))?;
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
            signed,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Vec(crate::types::VecType::V128),
                        found: get_reg(registers, *src)?.val_type(),
                    },
                });
            };
            let value = extract_lane(*shape, &bytes, *lane, *signed)?;
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
        RegOp::V128Unary {
            dst,
            src,
            shape,
            kind,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let mut out = [0u8; 16];
            v128_unary(*shape, *kind, &mut out, &bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Extend {
            dst,
            src,
            src_shape,
            high,
            signed,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let mut out = [0u8; 16];
            v128_extend(*src_shape, *high, *signed, &mut out, &bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128ExtMul {
            dst,
            a,
            b,
            src_shape,
            high,
            signed,
        } => {
            let (Value::V128(a_bytes), Value::V128(b_bytes)) =
                (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *a));
            };
            let mut out = [0u8; 16];
            v128_extmul(*src_shape, *high, *signed, &mut out, &a_bytes, &b_bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128ExtAddPairwise {
            dst,
            src,
            src_shape,
            signed,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let mut out = [0u8; 16];
            v128_extadd_pairwise(*src_shape, *signed, &mut out, &bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Narrow {
            dst,
            a,
            b,
            dst_shape,
            signed,
        } => {
            let (Value::V128(a_bytes), Value::V128(b_bytes)) =
                (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *a));
            };
            let mut out = [0u8; 16];
            v128_narrow(*dst_shape, *signed, &mut out, &a_bytes, &b_bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Dot { dst, a, b } => {
            let (Value::V128(a_bytes), Value::V128(b_bytes)) =
                (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *a));
            };
            let mut out = [0u8; 16];
            v128_dot(&mut out, &a_bytes, &b_bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Bitselect { dst, a, b, mask } => {
            let (Value::V128(a), Value::V128(b), Value::V128(m)) = (
                get_reg(registers, *a)?,
                get_reg(registers, *b)?,
                get_reg(registers, *mask)?,
            ) else {
                return Err(type_mismatch_v128(registers, *mask));
            };
            let mut out = [0u8; 16];
            for i in 0..16 {
                out[i] = (a[i] & m[i]) | (b[i] & !m[i]);
            }
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128AnyTrue { dst, src } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            set_reg(
                registers,
                *dst,
                Value::I32(if bytes.iter().any(|&b| b != 0) { 1 } else { 0 }),
            )?;
        }
        RegOp::V128AllTrue { dst, src, shape } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let width = shape.lane_width();
            let all = bytes
                .chunks_exact(width)
                .all(|lane| lane.iter().any(|&b| b != 0));
            set_reg(registers, *dst, Value::I32(if all { 1 } else { 0 }))?;
        }
        RegOp::V128Bitmask { dst, src, shape } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let width = shape.lane_width();
            let mut bits = 0u32;
            for (i, lane) in bytes.chunks_exact(width).enumerate() {
                if lane[width - 1] & 0x80 != 0 {
                    bits |= 1 << i;
                }
            }
            set_reg(registers, *dst, Value::I32(bits as i32))?;
        }
        RegOp::V128Compare {
            dst,
            a,
            b,
            shape,
            kind,
        } => {
            let (Value::V128(a), Value::V128(b)) =
                (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *b));
            };
            let mut out = [0u8; 16];
            v128_compare(*shape, *kind, &mut out, &a, &b);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Shift {
            dst,
            src,
            amount,
            shape,
            kind,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let Value::I32(amount) = get_reg(registers, *amount)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::Num(NumType::I32),
                        found: get_reg(registers, *amount)?.val_type(),
                    },
                });
            };
            let mut out = [0u8; 16];
            v128_shift(*shape, *kind, &mut out, &bytes, amount);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128FloatUnary {
            dst,
            src,
            shape,
            kind,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let mut out = [0u8; 16];
            v128_float_unary(*shape, *kind, &mut out, &bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Convert { dst, src, kind } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let mut out = [0u8; 16];
            v128_convert(*kind, &mut out, &bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Swizzle { dst, a, b } => {
            let (Value::V128(a_bytes), Value::V128(b_bytes)) =
                (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *a));
            };
            let mut out = [0u8; 16];
            v128_swizzle(&mut out, &a_bytes, &b_bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Shuffle { dst, a, b, lanes } => {
            let (Value::V128(a_bytes), Value::V128(b_bytes)) =
                (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *a));
            };
            let mut out = [0u8; 16];
            v128_shuffle(&mut out, &a_bytes, &b_bytes, lanes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
    }
    Ok(())
}

fn require_store(store: Option<&Store>) -> Result<&Store, RuntimeError> {
    store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })
}

/// Whether a runtime value satisfies a value type at the boundary: exact
/// for numerics/vectors, nullable-aware subtyping for references (a
/// non-null value satisfies `(ref T)`; abstract funcref/externref accept
/// matching heap families at any nullability).
fn value_satisfies(expected: ValType, value: Value) -> bool {
    use crate::types::HeapType;
    match (expected, value) {
        (ValType::Ref(expected_ref), value) => match (expected_ref, value) {
            (RefType::FuncRef, Value::FuncRef(_)) => true,
            (RefType::ExternRef, Value::ExternRef(_)) => true,
            (RefType::Typed { nullable, heap }, value) => {
                let non_null = match value {
                    Value::FuncRef(inner) => inner.is_some(),
                    Value::ExternRef(inner) => inner.is_some(),
                    Value::ExnRef(inner) => inner.is_some(),
                    Value::GcRef(_) | Value::I31(_) => true,
                    _ => return false,
                };
                if !nullable && !non_null {
                    return false;
                }
                match value {
                    Value::GcRef(_) => matches!(
                        heap,
                        HeapType::Any
                            | HeapType::Eq
                            | HeapType::Struct
                            | HeapType::Array
                            | HeapType::I31
                            | HeapType::None
                            | HeapType::Type(_)
                    ),
                    Value::I31(_) => matches!(
                        heap,
                        HeapType::Any | HeapType::Eq | HeapType::I31 | HeapType::None
                    ),
                    _ => matches!(
                        (heap, value),
                        (HeapType::Func | HeapType::Type(_), Value::FuncRef(_))
                            | (HeapType::Extern, Value::ExternRef(_))
                            | (HeapType::Exn, Value::ExnRef(_))
                    ),
                }
            }
            _ => false,
        },
        (expected, value) => expected == value.val_type(),
    }
}

/// WASM min: NaN propagates as the canonical NaN; -0 is smaller than +0.
fn wasm_f32_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::from_bits(0x7fc0_0000);
    }
    if a == b {
        if a == 0.0 && (a.is_sign_negative() || b.is_sign_negative()) {
            return -0.0;
        }
        return a;
    }
    if a < b { a } else { b }
}

/// WASM max: NaN propagates as the canonical NaN; +0 is larger than -0.
fn wasm_f32_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::from_bits(0x7fc0_0000);
    }
    if a == b {
        if a == 0.0 && (a.is_sign_positive() || b.is_sign_positive()) {
            return 0.0;
        }
        return a;
    }
    if a > b { a } else { b }
}

fn wasm_f64_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::from_bits(0x7ff8_0000_0000_0000);
    }
    if a == b {
        if a == 0.0 && (a.is_sign_negative() || b.is_sign_negative()) {
            return -0.0;
        }
        return a;
    }
    if a < b { a } else { b }
}

fn wasm_f64_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::from_bits(0x7ff8_0000_0000_0000);
    }
    if a == b {
        if a == 0.0 && (a.is_sign_positive() || b.is_sign_positive()) {
            return 0.0;
        }
        return a;
    }
    if a > b { a } else { b }
}

/// Grow a memory to `new_pages`, returning `false` when the allocation
/// fails (huge growth must yield -1, not abort the process).
fn grow_memory_fallible(mem: &mut Vec<u8>, new_pages: usize) -> bool {
    let additional = new_pages
        .saturating_mul(PAGE_SIZE)
        .saturating_sub(mem.len());
    if mem.try_reserve(additional).is_err() {
        return false;
    }
    mem.resize(new_pages * PAGE_SIZE, 0);
    true
}

/// Grow a table to `new` entries, returning `false` when the allocation
/// fails.
/// Bounds-check `addr + memarg.offset` over `width` bytes against a memory,
/// returning the valid byte range.
fn memory_bounds(
    mem: &[u8],
    memarg: &MemArg,
    addr: u64,
    width: usize,
) -> Result<core::ops::Range<usize>, RuntimeError> {
    let ea = addr
        .checked_add(memarg.offset)
        .ok_or(trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
    let end = ea
        .checked_add(width as u64)
        .ok_or(trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
    if end > mem.len() as u64 {
        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
    }
    // On 32-bit targets the in-bounds range may still not be addressable;
    // guard before narrowing to `usize` (trivially true on 64-bit).
    if ea > usize::MAX as u64 || end > usize::MAX as u64 {
        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
    }
    Ok(ea as usize..end as usize)
}

/// Whether a reference value is null (`ref.null`).
fn is_null_ref(value: &Value) -> bool {
    matches!(
        value,
        Value::FuncRef(None) | Value::ExternRef(None) | Value::ExnRef(None)
    )
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

/// Read a memory/table address operand by the target's index type.
///
/// `is64` selects the expected register width: 64-bit memories and tables
/// carry addresses in `I64`, 32-bit ones in `I32`. Validation already
/// guarantees the width matches, so a wrong-width register is reported as a
/// defensive `TypeMismatch`.
fn read_addr(value: Value, is64: bool) -> Result<u64, RuntimeError> {
    if is64 {
        match value {
            Value::I64(value) => Ok(value as u64),
            other => Err(RuntimeError {
                kind: RuntimeErrorKind::TypeMismatch {
                    expected: ValType::Num(NumType::I64),
                    found: other.val_type(),
                },
            }),
        }
    } else {
        match value {
            Value::I32(value) => Ok(value as u32 as u64),
            other => Err(RuntimeError {
                kind: RuntimeErrorKind::TypeMismatch {
                    expected: ValType::Num(NumType::I32),
                    found: other.val_type(),
                },
            }),
        }
    }
}

/// Whether references of this type default to null (nullable refs).
fn ref_nullable(ref_type: &RefType) -> bool {
    match ref_type {
        RefType::FuncRef | RefType::ExternRef => true,
        RefType::Typed { nullable, .. } => *nullable,
    }
}

/// The null value for a reference type (funcref null vs externref null).
pub(crate) fn ref_null_value(ref_type: &RefType) -> Value {
    match ref_type {
        RefType::ExternRef
        | RefType::Typed {
            heap: crate::types::HeapType::Extern,
            ..
        } => Value::ExternRef(None),
        RefType::Typed {
            heap: crate::types::HeapType::Exn,
            ..
        } => Value::ExnRef(None),
        _ => Value::FuncRef(None),
    }
}

/// The default value for a storage slot (numeric/vector zero, nullable ref
/// null). Validation guarantees the field is defaultable.
fn default_for_storage(storage: StorageType) -> Value {
    match storage {
        StorageType::I8 | StorageType::I16 => Value::I32(0),
        StorageType::Val(val) => default_for_val(val),
    }
}

fn default_for_val(val: ValType) -> Value {
    match val {
        ValType::Num(NumType::I32) => Value::I32(0),
        ValType::Num(NumType::I64) => Value::I64(0),
        ValType::Num(NumType::F32) => Value::F32(0.0),
        ValType::Num(NumType::F64) => Value::F64(0.0),
        ValType::Vec(_) => Value::V128([0; 16]),
        ValType::Ref(ref_type) => ref_null_value(&ref_type),
    }
}

/// The heap's packed-read extension for a lowered GC `get` op.
fn heap_ext(ext: GcExt) -> Option<Ext> {
    match ext {
        GcExt::None => None,
        GcExt::S => Some(Ext::S),
        GcExt::U => Some(Ext::U),
    }
}

fn gc_allocation_failed() -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::ResourceLimitExceeded { what: "gc heap" },
    }
}

fn unknown_type_err(type_idx: TypeIdx) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::UnknownType {
            type_idx: type_idx.0,
        },
    }
}

fn unknown_data_err(data: u32) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::UnknownDataSegment { data },
    }
}

fn unknown_elem_err(elem: u32) -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::UnknownElem { elem },
    }
}

/// Resolve a GC aggregate type's layout from the current module.
fn gc_type_kind(module: Option<&RegModule>, type_idx: TypeIdx) -> Result<GcTypeKind, RuntimeError> {
    let module = module.ok_or_else(|| unknown_type_err(type_idx))?;
    match module
        .type_defs
        .get(type_idx.0 as usize)
        .map(|sub| &sub.comp)
    {
        Some(CompType::Struct(fields)) => Ok(GcTypeKind::Struct(fields.clone())),
        Some(CompType::Array(field)) => Ok(GcTypeKind::Array(*field)),
        _ => Err(unknown_type_err(type_idx)),
    }
}

fn gc_array_storage(kind: &GcTypeKind) -> Result<StorageType, RuntimeError> {
    match kind {
        GcTypeKind::Array(field) => Ok(field.storage),
        GcTypeKind::Struct(_) => Err(unknown_type_err(TypeIdx(0))),
    }
}

fn with_array_len(store: &Store, id: ObjectId) -> Result<u32, RuntimeError> {
    store
        .with_gc_heap(|heap| heap.array_len(id))
        .ok_or_else(|| unknown_type_err(TypeIdx(0)))
}

/// Null-check a GC reference operand before any heap access (every
/// struct/array access traps on a null operand first).
fn expect_gc_ref(value: Value, null_trap: RuntimeTrap) -> Result<ObjectId, RuntimeError> {
    if is_null_ref(&value) {
        return Err(trap(null_trap));
    }
    match value {
        Value::GcRef(id) => Ok(id),
        other => Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Ref(RefType::from_parts(true, HeapType::Any)),
                found: other.val_type(),
            },
        }),
    }
}

fn expect_u32(value: Value) -> Result<u32, RuntimeError> {
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

/// `array_oob a i n` (eval.ml): `i + n > len`, unsigned with overflow as
/// out of bounds.
fn array_index_oob(index: u32, count: u32, len: u32) -> bool {
    (index as u64)
        .checked_add(count as u64)
        .is_none_or(|end| end > len as u64)
}

/// Load `len` elements of `storage` from a data segment starting at byte
/// `src` (`array.new_data` / `array.init_data`). Segment bounds precede any
/// payload build, and the payload reserve is fallible.
fn data_segment_values(
    bytes: &[u8],
    src: u32,
    len: u32,
    width: u64,
    storage: StorageType,
) -> Result<Vec<Value>, RuntimeError> {
    let byte_len = (len as u64)
        .checked_mul(width)
        .ok_or_else(|| trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
    let end = (src as u64)
        .checked_add(byte_len)
        .ok_or_else(|| trap(RuntimeTrap::OutOfBoundsMemoryAccess))?;
    if end > bytes.len() as u64 {
        return Err(trap(RuntimeTrap::OutOfBoundsMemoryAccess));
    }
    let mut values: Vec<Value> = Vec::new();
    values
        .try_reserve(len as usize)
        .map_err(|_| gc_allocation_failed())?;
    for i in 0..len as u64 {
        let off = (src as u64 + i * width) as usize;
        let width = width as usize;
        values.push(read_storage_value(&bytes[off..off + width], storage));
    }
    Ok(values)
}

/// Copy `len` element values from an element segment (`array.new_elem` /
/// `array.init_elem`). Segment bounds precede the payload reserve.
fn elem_segment_values(values: &[Value], src: u32, len: u32) -> Result<Vec<Value>, RuntimeError> {
    let end = (src as u64)
        .checked_add(len as u64)
        .ok_or_else(|| trap(RuntimeTrap::OutOfBoundsTableAccess))?;
    if end > values.len() as u64 {
        return Err(trap(RuntimeTrap::OutOfBoundsTableAccess));
    }
    let mut out: Vec<Value> = Vec::new();
    out.try_reserve(len as usize)
        .map_err(|_| gc_allocation_failed())?;
    out.extend_from_slice(&values[src as usize..end as usize]);
    Ok(out)
}

/// Read one storage value from a data segment (`Data.load_val_storage`):
/// little-endian, packed i8/i16 zero-extended into an `i32`.
fn read_storage_value(bytes: &[u8], storage: StorageType) -> Value {
    match storage {
        StorageType::I8 => Value::I32(bytes[0] as i32),
        StorageType::I16 => Value::I32(u16::from_le_bytes([bytes[0], bytes[1]]) as i32),
        StorageType::Val(val) => match val {
            ValType::Num(NumType::I32) => {
                Value::I32(i32::from_le_bytes(bytes.try_into().expect("width checked")))
            }
            ValType::Num(NumType::I64) => {
                Value::I64(i64::from_le_bytes(bytes.try_into().expect("width checked")))
            }
            ValType::Num(NumType::F32) => Value::F32(f32::from_bits(u32::from_le_bytes(
                bytes.try_into().expect("width checked"),
            ))),
            ValType::Num(NumType::F64) => Value::F64(f64::from_bits(u64::from_le_bytes(
                bytes.try_into().expect("width checked"),
            ))),
            ValType::Vec(_) => {
                let mut vec = [0u8; 16];
                vec.copy_from_slice(bytes);
                Value::V128(vec)
            }
            // Numeric/vector storages only are validated for data segments.
            ValType::Ref(_) => Value::FuncRef(None),
        },
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
            V128BinaryKind::AddSatS => a.saturating_add(b),
            V128BinaryKind::SubSatS => a.saturating_sub(b),
            V128BinaryKind::MinS => a.min(b),
            V128BinaryKind::MaxS => a.max(b),
            _ => unreachable!("unsigned kinds handled by lane_uint_op!"),
        }
    }};
}

/// Lane-wise unsigned-interpretation integer arithmetic. The `_u` variants
/// reinterpret the lane bytes as unsigned before operating; `avgr_u`
/// widens so `(a + b + 1)` cannot overflow.
macro_rules! lane_uint_op {
    ($kind:expr, $a:expr, $b:expr, $ty:ty) => {{
        let (a, b) = ($a as $ty, $b as $ty);
        match $kind {
            V128BinaryKind::AddSatU => a.saturating_add(b),
            V128BinaryKind::SubSatU => a.saturating_sub(b),
            V128BinaryKind::MinU => a.min(b),
            V128BinaryKind::MaxU => a.max(b),
            V128BinaryKind::AvgrU => (a as u32 + b as u32).div_ceil(2) as $ty,
            _ => unreachable!("signed kinds handled by lane_int_op!"),
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

/// Read one lane as a scalar Value. `signed` only affects 8/16-bit int
/// lanes (`_s` sign-extends, `_u` zero-extends); wider/float shapes ignore
/// it.
fn extract_lane(
    shape: LaneShape,
    bytes: &[u8; 16],
    lane: u8,
    signed: bool,
) -> Result<Value, RuntimeError> {
    let width = shape.lane_width();
    let start = lane as usize * width;
    if start + width > 16 {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::InvalidLaneIndex { lane },
        });
    }
    let lane_bytes = &bytes[start..start + width];
    Ok(match shape {
        LaneShape::I8x16 => {
            if signed {
                Value::I32(lane_bytes[0] as i8 as i32)
            } else {
                Value::I32(lane_bytes[0] as i32)
            }
        }
        LaneShape::I16x8 => {
            let value = i16::from_le_bytes(lane_bytes.try_into().expect("width"));
            if signed {
                Value::I32(value as i32)
            } else {
                Value::I32(value as u16 as i32)
            }
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
        V128BinaryKind::Andnot => {
            for i in 0..16 {
                out[i] = lhs[i] & !rhs[i];
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
    // Unsigned-interpretation kinds route through `lane_uint_op!` on the
    // unsigned view of the lane width; everything else keeps the signed
    // path. q15mulr is i16-only and needs i32 intermediates.
    match shape {
        LaneShape::I8x16 => {
            if kind == V128BinaryKind::Q15MulrSatS {
                unreachable!("q15mulr_sat_s is i16x8-only");
            }
            if uses_unsigned_view(kind) {
                dst[0] = lane_uint_op!(kind, lhs[0], rhs[0], u8);
            } else {
                dst[0] = lane_int_op!(kind, lhs[0], rhs[0], i8) as u8;
            }
        }
        LaneShape::I16x8 => {
            let (a, b) = (
                i16::from_le_bytes(lhs.try_into().expect("width")),
                i16::from_le_bytes(rhs.try_into().expect("width")),
            );
            let result = if kind == V128BinaryKind::Q15MulrSatS {
                // q15: saturate_i16((a*b + 0x4000) >> 15) on i32 lanes.
                let product = a as i32 * b as i32;
                let shifted = (product + 0x4000) >> 15;
                shifted.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            } else if uses_unsigned_view(kind) {
                lane_uint_op!(kind, a, b, u16) as i16
            } else {
                lane_int_op!(kind, a, b, i16)
            };
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::I32x4 => {
            let (a, b) = (
                i32::from_le_bytes(lhs.try_into().expect("width")),
                i32::from_le_bytes(rhs.try_into().expect("width")),
            );
            let result = if uses_unsigned_view(kind) {
                lane_uint_op!(kind, a, b, u32) as i32
            } else {
                lane_int_op!(kind, a, b, i32)
            };
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::I64x2 => {
            let (a, b) = (
                i64::from_le_bytes(lhs.try_into().expect("width")),
                i64::from_le_bytes(rhs.try_into().expect("width")),
            );
            let result = lane_int_op!(kind, a, b, i64);
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::F32x4 => {
            let a = f32::from_bits(u32::from_le_bytes(lhs.try_into().expect("width")));
            let b = f32::from_bits(u32::from_le_bytes(rhs.try_into().expect("width")));
            let result = match kind {
                V128BinaryKind::FMin => wasm_f32_min(a, b),
                V128BinaryKind::FMax => wasm_f32_max(a, b),
                V128BinaryKind::PMin => {
                    if b < a {
                        b
                    } else {
                        a
                    }
                }
                V128BinaryKind::PMax => {
                    if a < b {
                        b
                    } else {
                        a
                    }
                }
                _ => lane_float_op!(kind, a, b, f32),
            };
            dst.copy_from_slice(&result.to_bits().to_le_bytes());
        }
        LaneShape::F64x2 => {
            let a = f64::from_bits(u64::from_le_bytes(lhs.try_into().expect("width")));
            let b = f64::from_bits(u64::from_le_bytes(rhs.try_into().expect("width")));
            let result = match kind {
                V128BinaryKind::FMin => wasm_f64_min(a, b),
                V128BinaryKind::FMax => wasm_f64_max(a, b),
                V128BinaryKind::PMin => {
                    if b < a {
                        b
                    } else {
                        a
                    }
                }
                V128BinaryKind::PMax => {
                    if a < b {
                        b
                    } else {
                        a
                    }
                }
                _ => lane_float_op!(kind, a, b, f64),
            };
            dst.copy_from_slice(&result.to_bits().to_le_bytes());
        }
    }
}

/// Whether the kind interprets lanes as unsigned (the `_u` families).
fn uses_unsigned_view(kind: V128BinaryKind) -> bool {
    matches!(
        kind,
        V128BinaryKind::AddSatU
            | V128BinaryKind::SubSatU
            | V128BinaryKind::MinU
            | V128BinaryKind::MaxU
            | V128BinaryKind::AvgrU
    )
}

/// Lane-wise integer unary. abs/neg WRAP on the type's MIN value
/// (spec: i8x16.abs(-128) = -128).
fn v128_unary(shape: LaneShape, kind: V128UnaryKind, dst: &mut [u8], src: &[u8]) {
    match shape {
        LaneShape::I8x16 => {
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d = match kind {
                    V128UnaryKind::Abs => (*s as i8).wrapping_abs() as u8,
                    V128UnaryKind::Neg => (*s as i8).wrapping_neg() as u8,
                    V128UnaryKind::Popcnt => s.count_ones() as u8,
                };
            }
        }
        LaneShape::I16x8 => {
            for (d, s) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                let v = i16::from_le_bytes(s.try_into().expect("width"));
                let r = match kind {
                    V128UnaryKind::Abs => v.wrapping_abs(),
                    V128UnaryKind::Neg => v.wrapping_neg(),
                    V128UnaryKind::Popcnt => unreachable!("no i16x8 popcnt"),
                };
                d.copy_from_slice(&r.to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                let r = match kind {
                    V128UnaryKind::Abs => v.wrapping_abs(),
                    V128UnaryKind::Neg => v.wrapping_neg(),
                    V128UnaryKind::Popcnt => unreachable!("no i32x4 popcnt"),
                };
                d.copy_from_slice(&r.to_le_bytes());
            }
        }
        LaneShape::I64x2 => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(8)) {
                let v = i64::from_le_bytes(s.try_into().expect("width"));
                let r = match kind {
                    V128UnaryKind::Abs => v.wrapping_abs(),
                    V128UnaryKind::Neg => v.wrapping_neg(),
                    V128UnaryKind::Popcnt => unreachable!("no i64x2 popcnt"),
                };
                d.copy_from_slice(&r.to_le_bytes());
            }
        }
        LaneShape::F32x4 | LaneShape::F64x2 => unreachable!("float unary is a later unit"),
    }
}

/// Lane-wise compare. Result lanes are all-ones (true) or all-zeros
/// (false) at the operand lane width. Integer shapes route s/u kinds
/// through the signed/unsigned lane view; float shapes use Rust's
/// ordered/unordered semantics (NaN compares false except `ne`;
/// -0.0 == +0.0).
fn v128_compare(shape: LaneShape, kind: V128CompareKind, dst: &mut [u8], a: &[u8], b: &[u8]) {
    let int_kind = |x: i64, y: i64, signed: bool| -> bool {
        match kind {
            V128CompareKind::Eq => x == y,
            V128CompareKind::Ne => x != y,
            V128CompareKind::LtS | V128CompareKind::Lt if signed => x < y,
            V128CompareKind::GtS | V128CompareKind::Gt if signed => x > y,
            V128CompareKind::LeS | V128CompareKind::Le if signed => x <= y,
            V128CompareKind::GeS | V128CompareKind::Ge if signed => x >= y,
            _ => unreachable!("unsigned kind on signed view"),
        }
    };
    let uint_kind = |x: u64, y: u64| -> bool {
        match kind {
            V128CompareKind::Eq => x == y,
            V128CompareKind::Ne => x != y,
            V128CompareKind::LtU => x < y,
            V128CompareKind::GtU => x > y,
            V128CompareKind::LeU => x <= y,
            V128CompareKind::GeU => x >= y,
            _ => unreachable!("signed/float kind on unsigned view"),
        }
    };
    let float_kind = |x: f64, y: f64| -> bool {
        match kind {
            V128CompareKind::Eq => x == y,
            V128CompareKind::Ne => x != y,
            V128CompareKind::Lt => x < y,
            V128CompareKind::Gt => x > y,
            V128CompareKind::Le => x <= y,
            V128CompareKind::Ge => x >= y,
            _ => unreachable!("integer kind on float shape"),
        }
    };
    let write = |d: &mut [u8], truth: bool| {
        for byte in d.iter_mut() {
            *byte = if truth { 0xFF } else { 0x00 };
        }
    };
    match shape {
        LaneShape::I8x16 => {
            for i in 0..16 {
                let (x, y) = (a[i] as i8, b[i] as i8);
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(a[i] as u64, b[i] as u64)
                } else {
                    int_kind(x as i64, y as i64, true)
                };
                write(&mut dst[i..i + 1], truth);
            }
        }
        LaneShape::I16x8 => {
            for i in 0..8 {
                let (x, y) = (
                    i16::from_le_bytes(a[i * 2..i * 2 + 2].try_into().expect("width")),
                    i16::from_le_bytes(b[i * 2..i * 2 + 2].try_into().expect("width")),
                );
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(x as u16 as u64, y as u16 as u64)
                } else {
                    int_kind(x as i64, y as i64, true)
                };
                write(&mut dst[i * 2..i * 2 + 2], truth);
            }
        }
        LaneShape::I32x4 => {
            for i in 0..4 {
                let (x, y) = (
                    i32::from_le_bytes(a[i * 4..i * 4 + 4].try_into().expect("width")),
                    i32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().expect("width")),
                );
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(x as u32 as u64, y as u32 as u64)
                } else {
                    int_kind(x as i64, y as i64, true)
                };
                write(&mut dst[i * 4..i * 4 + 4], truth);
            }
        }
        LaneShape::I64x2 => {
            for i in 0..2 {
                let (x, y) = (
                    i64::from_le_bytes(a[i * 8..i * 8 + 8].try_into().expect("width")),
                    i64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("width")),
                );
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(x as u64, y as u64)
                } else {
                    int_kind(x, y, true)
                };
                write(&mut dst[i * 8..i * 8 + 8], truth);
            }
        }
        LaneShape::F32x4 => {
            for i in 0..4 {
                let (x, y) = (
                    f32::from_bits(u32::from_le_bytes(
                        a[i * 4..i * 4 + 4].try_into().expect("width"),
                    )),
                    f32::from_bits(u32::from_le_bytes(
                        b[i * 4..i * 4 + 4].try_into().expect("width"),
                    )),
                );
                let truth = float_kind(x as f64, y as f64);
                write(&mut dst[i * 4..i * 4 + 4], truth);
            }
        }
        LaneShape::F64x2 => {
            for i in 0..2 {
                let (x, y) = (
                    f64::from_bits(u64::from_le_bytes(
                        a[i * 8..i * 8 + 8].try_into().expect("width"),
                    )),
                    f64::from_bits(u64::from_le_bytes(
                        b[i * 8..i * 8 + 8].try_into().expect("width"),
                    )),
                );
                let truth = float_kind(x, y);
                write(&mut dst[i * 8..i * 8 + 8], truth);
            }
        }
    }
}

/// Whether the compare kind interprets integer lanes as unsigned.
fn uses_unsigned_compare(kind: V128CompareKind) -> bool {
    matches!(
        kind,
        V128CompareKind::LtU | V128CompareKind::GtU | V128CompareKind::LeU | V128CompareKind::GeU
    )
}

/// Lane-wise shift by a scalar amount taken modulo the lane bit width.
fn v128_shift(shape: LaneShape, kind: V128ShiftKind, dst: &mut [u8], src: &[u8], amount: i32) {
    let bits = shape.lane_width() as u32 * 8;
    let s = (amount as u32) % bits;
    match shape {
        LaneShape::I8x16 => {
            for i in 0..16 {
                let x = src[i] as i8;
                let out: i8 = match kind {
                    V128ShiftKind::Shl => ((x as u8) << s) as i8,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u8) >> s) as i8,
                };
                dst[i] = out as u8;
            }
        }
        LaneShape::I16x8 => {
            for (d, chunk) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                let x = i16::from_le_bytes(chunk.try_into().expect("width"));
                let out: i16 = match kind {
                    V128ShiftKind::Shl => ((x as u16) << s) as i16,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u16) >> s) as i16,
                };
                d.copy_from_slice(&out.to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            for (d, chunk) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let x = i32::from_le_bytes(chunk.try_into().expect("width"));
                let out: i32 = match kind {
                    V128ShiftKind::Shl => ((x as u32) << s) as i32,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u32) >> s) as i32,
                };
                d.copy_from_slice(&out.to_le_bytes());
            }
        }
        LaneShape::I64x2 => {
            for (d, chunk) in dst.chunks_exact_mut(8).zip(src.chunks_exact(8)) {
                let x = i64::from_le_bytes(chunk.try_into().expect("width"));
                let out: i64 = match kind {
                    V128ShiftKind::Shl => ((x as u64) << s) as i64,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u64) >> s) as i64,
                };
                d.copy_from_slice(&out.to_le_bytes());
            }
        }
        LaneShape::F32x4 | LaneShape::F64x2 => unreachable!("no float shifts"),
    }
}

/// Widen `src`'s selected half to double lane width. Dst shape is
/// implied: I8x16→I16x8, I16x8→I32x4, I32x4→I64x2.
fn v128_extend(src_shape: LaneShape, high: bool, signed: bool, dst: &mut [u8], src: &[u8]) {
    match src_shape {
        LaneShape::I8x16 => {
            let half: &[u8] = if high { &src[8..] } else { &src[..8] };
            for (d, s) in dst.chunks_exact_mut(2).zip(half.iter()) {
                let lane: u16 = if signed {
                    (*s as i8) as i16 as u16
                } else {
                    *s as u16
                };
                d.copy_from_slice(&lane.to_le_bytes());
            }
        }
        LaneShape::I16x8 => {
            let half: &[u8] = if high { &src[8..] } else { &src[..8] };
            for (d, s) in dst.chunks_exact_mut(4).zip(half.chunks_exact(2)) {
                let v = i16::from_le_bytes(s.try_into().expect("width"));
                let lane: u32 = if signed {
                    v as i32 as u32
                } else {
                    v as u16 as u32
                };
                d.copy_from_slice(&lane.to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            let half: &[u8] = if high { &src[8..] } else { &src[..8] };
            for (d, s) in dst.chunks_exact_mut(8).zip(half.chunks_exact(4)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                let lane: u64 = if signed {
                    v as i64 as u64
                } else {
                    v as u32 as u64
                };
                d.copy_from_slice(&lane.to_le_bytes());
            }
        }
        _ => unreachable!("extend widens i8x16/i16x8/i32x4 only"),
    }
}

/// Extended multiply: widen the selected half of both operands, then
/// multiply lane-wise in double width.
#[allow(clippy::unnecessary_cast)]
fn v128_extmul(src_shape: LaneShape, high: bool, signed: bool, dst: &mut [u8], a: &[u8], b: &[u8]) {
    match src_shape {
        LaneShape::I8x16 => {
            let (a_half, b_half): (&[u8], &[u8]) = if high {
                (&a[8..], &b[8..])
            } else {
                (&a[..8], &b[..8])
            };
            for ((d, x), y) in dst
                .chunks_exact_mut(2)
                .zip(a_half.iter())
                .zip(b_half.iter())
            {
                let product: i32 = if signed {
                    (*x as i8 as i16 as i32) * (*y as i8 as i16 as i32)
                } else {
                    (*x as u8 as u16 as i32) * (*y as u8 as u16 as i32)
                };
                d.copy_from_slice(&(product as i16).to_le_bytes());
            }
        }
        LaneShape::I16x8 => {
            let (a_half, b_half): (&[u8], &[u8]) = if high {
                (&a[8..], &b[8..])
            } else {
                (&a[..8], &b[..8])
            };
            for ((d, x), y) in dst
                .chunks_exact_mut(4)
                .zip(a_half.chunks_exact(2))
                .zip(b_half.chunks_exact(2))
            {
                let x = i16::from_le_bytes(x.try_into().expect("width"));
                let y = i16::from_le_bytes(y.try_into().expect("width"));
                let product: i64 = if signed {
                    (x as i32 as i64) * (y as i32 as i64)
                } else {
                    (x as u16 as u32 as i64) * (y as u16 as u32 as i64)
                };
                d.copy_from_slice(&(product as i32).to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            let (a_half, b_half): (&[u8], &[u8]) = if high {
                (&a[8..], &b[8..])
            } else {
                (&a[..8], &b[..8])
            };
            for ((d, x), y) in dst
                .chunks_exact_mut(8)
                .zip(a_half.chunks_exact(4))
                .zip(b_half.chunks_exact(4))
            {
                let x = i32::from_le_bytes(x.try_into().expect("width"));
                let y = i32::from_le_bytes(y.try_into().expect("width"));
                let product: i128 = if signed {
                    (x as i64 as i128) * (y as i64 as i128)
                } else {
                    (x as u32 as u64 as i128) * (y as u32 as u64 as i128)
                };
                d.copy_from_slice(&(product as i64).to_le_bytes());
            }
        }
        _ => unreachable!("extmul widens i8x16/i16x8/i32x4 only"),
    }
}

/// Pairwise sum of adjacent lanes, doubling width. Never overflows the
/// wider lane.
#[allow(clippy::unnecessary_cast)]
fn v128_extadd_pairwise(src_shape: LaneShape, signed: bool, dst: &mut [u8], src: &[u8]) {
    match src_shape {
        LaneShape::I8x16 => {
            for (d, pair) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                let sum: i32 = if signed {
                    (pair[0] as i8 as i16 as i32) + (pair[1] as i8 as i16 as i32)
                } else {
                    (pair[0] as u8 as u16 as i32) + (pair[1] as u8 as u16 as i32)
                };
                d.copy_from_slice(&(sum as i16).to_le_bytes());
            }
        }
        LaneShape::I16x8 => {
            for (d, pair) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let (x, y) = (
                    i16::from_le_bytes(pair[..2].try_into().expect("width")),
                    i16::from_le_bytes(pair[2..].try_into().expect("width")),
                );
                let sum: i64 = if signed {
                    (x as i32 as i64) + (y as i32 as i64)
                } else {
                    (x as u16 as u32 as i64) + (y as u16 as u32 as i64)
                };
                d.copy_from_slice(&(sum as i32).to_le_bytes());
            }
        }
        _ => unreachable!("extadd_pairwise widens i8x16/i16x8 only"),
    }
}

/// Saturating narrow: a's lanes → dst low half, b's lanes → dst high
/// half. `_s` saturates to the signed range; `_u` clamps negatives to 0
/// and saturates at the unsigned max (operating on the SIGNED source
/// value).
#[allow(clippy::unnecessary_cast)]
fn v128_narrow(dst_shape: LaneShape, signed: bool, dst: &mut [u8], a: &[u8], b: &[u8]) {
    let (dst_w, src_w) = match dst_shape {
        LaneShape::I8x16 => (1usize, 2usize),
        LaneShape::I16x8 => (2usize, 4usize),
        _ => unreachable!("narrow halves width only"),
    };
    let mid = 16 / 2;
    for (d, s) in dst[..mid]
        .chunks_exact_mut(dst_w)
        .zip(a.chunks_exact(src_w))
    {
        let v = match src_w {
            2 => i16::from_le_bytes(s.try_into().expect("width")) as i32,
            _ => i32::from_le_bytes(s.try_into().expect("width")),
        };
        let out = if signed {
            v.clamp(
                (-(1i32 << (dst_w * 8 - 1))) as i32,
                (1i32 << (dst_w * 8 - 1)) - 1,
            )
        } else {
            v.clamp(0, (1i32 << (dst_w * 8)) - 1)
        };
        d.copy_from_slice(&(out as u16).to_le_bytes()[..dst_w]);
    }
    for (d, s) in dst[mid..]
        .chunks_exact_mut(dst_w)
        .zip(b.chunks_exact(src_w))
    {
        let v = match src_w {
            2 => i16::from_le_bytes(s.try_into().expect("width")) as i32,
            _ => i32::from_le_bytes(s.try_into().expect("width")),
        };
        let out = if signed {
            v.clamp(
                (-(1i32 << (dst_w * 8 - 1))) as i32,
                (1i32 << (dst_w * 8 - 1)) - 1,
            )
        } else {
            v.clamp(0, (1i32 << (dst_w * 8)) - 1)
        };
        d.copy_from_slice(&(out as u16).to_le_bytes()[..dst_w]);
    }
}

/// i32x4.dot_i16x8_s: dst lane i = a[2i]·b[2i] + a[2i+1]·b[2i+1] on
/// SIGNED i16 lanes, products and sum WRAPPING at i32 (2·(−32768)²
/// wraps to i32::MIN per spec).
fn v128_dot(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for (d, i) in dst.chunks_exact_mut(4).zip(0..4usize) {
        let lane = |bytes: &[u8], idx: usize| {
            i16::from_le_bytes(bytes[idx * 2..idx * 2 + 2].try_into().expect("width")) as i32
        };
        let acc = lane(a, 2 * i)
            .wrapping_mul(lane(b, 2 * i))
            .wrapping_add(lane(a, 2 * i + 1).wrapping_mul(lane(b, 2 * i + 1)));
        d.copy_from_slice(&acc.to_le_bytes());
    }
}

fn v128_float_unary(shape: LaneShape, kind: V128FloatUnaryKind, dst: &mut [u8], src: &[u8]) {
    let width = shape.lane_width();
    match width {
        4 => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                let r = match kind {
                    V128FloatUnaryKind::Abs => f32::from_bits(v.to_bits() & 0x7FFF_FFFF),
                    V128FloatUnaryKind::Neg => f32::from_bits(v.to_bits() ^ 0x8000_0000),
                    V128FloatUnaryKind::Sqrt => libm::sqrtf(v),
                    V128FloatUnaryKind::Ceil => libm::ceilf(v),
                    V128FloatUnaryKind::Floor => libm::floorf(v),
                    V128FloatUnaryKind::Trunc => libm::truncf(v),
                    V128FloatUnaryKind::Nearest => libm::rintf(v),
                };
                d.copy_from_slice(&r.to_bits().to_le_bytes());
            }
        }
        8 => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                let r = match kind {
                    V128FloatUnaryKind::Abs => f64::from_bits(v.to_bits() & 0x7FFF_FFFF_FFFF_FFFF),
                    V128FloatUnaryKind::Neg => f64::from_bits(v.to_bits() ^ 0x8000_0000_0000_0000),
                    V128FloatUnaryKind::Sqrt => libm::sqrt(v),
                    V128FloatUnaryKind::Ceil => libm::ceil(v),
                    V128FloatUnaryKind::Floor => libm::floor(v),
                    V128FloatUnaryKind::Trunc => libm::trunc(v),
                    V128FloatUnaryKind::Nearest => libm::rint(v),
                };
                d.copy_from_slice(&r.to_bits().to_le_bytes());
            }
        }
        _ => unreachable!("float unary is f32x4/f64x2 only"),
    }
}

fn v128_convert(kind: V128ConvertKind, dst: &mut [u8], src: &[u8]) {
    match kind {
        V128ConvertKind::TruncSatF32x4S => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as i32).to_le_bytes());
            }
        }
        V128ConvertKind::TruncSatF32x4U => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as u32).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertI32x4S => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f32).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertI32x4U => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = u32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f32).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::TruncSatF64x2SZero => {
            for (d, s) in dst.chunks_exact_mut(4).take(2).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as i32).to_le_bytes());
            }
            for d in dst[8..].chunks_exact_mut(4) {
                d.copy_from_slice(&0i32.to_le_bytes());
            }
        }
        V128ConvertKind::TruncSatF64x2UZero => {
            for (d, s) in dst.chunks_exact_mut(4).take(2).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as u32).to_le_bytes());
            }
            for d in dst[8..].chunks_exact_mut(4) {
                d.copy_from_slice(&0u32.to_le_bytes());
            }
        }
        V128ConvertKind::DemoteF64x2Zero => {
            for (d, s) in dst.chunks_exact_mut(4).take(2).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&((v as f32).to_bits()).to_le_bytes());
            }
            for d in dst[8..].chunks_exact_mut(4) {
                d.copy_from_slice(&0f32.to_bits().to_le_bytes());
            }
        }
        V128ConvertKind::PromoteLowF32x4 => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(4).take(2)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&((v as f64).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertLowI32x4S => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(4).take(2)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f64).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertLowI32x4U => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(4).take(2)) {
                let v = u32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f64).to_bits()).to_le_bytes());
            }
        }
    }
}

/// result[i] = b[i] < 16 ? a[b[i]] : 0.
fn v128_swizzle(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for i in 0..16 {
        dst[i] = if b[i] < 16 { a[b[i] as usize] } else { 0 };
    }
}

/// result[i] = lanes[i] < 16 ? a[lanes[i]] : b[lanes[i] - 16]. Lane
/// bounds are validated (< 32) before lowering.
fn v128_shuffle(dst: &mut [u8], a: &[u8], b: &[u8], lanes: &[u8; 16]) {
    for i in 0..16 {
        let idx = lanes[i];
        dst[i] = if idx < 16 {
            a[idx as usize]
        } else {
            b[(idx - 16) as usize]
        };
    }
}

fn type_mismatch_v128(registers: &[Option<Value>], reg: Reg) -> RuntimeError {
    let found = match get_reg(registers, reg) {
        Ok(value) => value.val_type(),
        Err(error) => return error,
    };
    RuntimeError {
        kind: RuntimeErrorKind::TypeMismatch {
            expected: ValType::Vec(crate::types::VecType::V128),
            found,
        },
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
        UnaryOp::F32Nearest => execute_f32_unary(registers, dst, value, libm::rintf),
        UnaryOp::F64Neg => execute_f64_unary(registers, dst, value, |value| -value),
        UnaryOp::F64Abs => execute_f64_unary(registers, dst, value, libm::fabs),
        UnaryOp::F64Sqrt => execute_f64_unary(registers, dst, value, libm::sqrt),
        UnaryOp::F64Ceil => execute_f64_unary(registers, dst, value, libm::ceil),
        UnaryOp::F64Floor => execute_f64_unary(registers, dst, value, libm::floor),
        UnaryOp::F64Trunc => execute_f64_unary(registers, dst, value, libm::trunc),
        UnaryOp::F64Nearest => execute_f64_unary(registers, dst, value, libm::rint),
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
        BinaryOp::F32Copysign => execute_f32_binary(registers, dst, lhs, rhs, libm::copysignf),
        BinaryOp::F32Sub => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs - rhs),
        BinaryOp::F32Mul => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs * rhs),
        BinaryOp::F32Div => execute_f32_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs / rhs),
        BinaryOp::F32Min => execute_f32_binary(registers, dst, lhs, rhs, wasm_f32_min),
        BinaryOp::F32Max => execute_f32_binary(registers, dst, lhs, rhs, wasm_f32_max),
        BinaryOp::F64Add => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs + rhs),
        BinaryOp::F64Copysign => execute_f64_binary(registers, dst, lhs, rhs, libm::copysign),
        BinaryOp::F64Sub => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs - rhs),
        BinaryOp::F64Mul => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs * rhs),
        BinaryOp::F64Div => execute_f64_binary(registers, dst, lhs, rhs, |lhs, rhs| lhs / rhs),
        BinaryOp::F64Min => execute_f64_binary(registers, dst, lhs, rhs, wasm_f64_min),
        BinaryOp::F64Max => execute_f64_binary(registers, dst, lhs, rhs, wasm_f64_max),
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
        if value <= -1.0 || value >= (u32::MAX as f32) {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i32::MAX as f32) || value < (i32::MIN as f32) {
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
        if value <= -1.0 || value >= 4294967296.0 {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= 2147483648.0 || value <= -2147483649.0 {
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
        if value <= -1.0 || value >= (u64::MAX as f32) {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i64::MAX as f32) || value < (i64::MIN as f32) {
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
        if value <= -1.0 || value >= (u64::MAX as f64) {
            return Err(trap(RuntimeTrap::IntegerOverflow));
        }
    } else if value >= (i64::MAX as f64) || value < (i64::MIN as f64) {
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

        let store = Store::instantiate(&reg_module).unwrap();
        let result = execute_export(
            &reg_module,
            &store,
            "add",
            &[Value::I32(20), Value::I32(22)],
        )
        .unwrap();

        assert_eq!(result, alloc::vec![Value::I32(42)]);
    }

    #[test]
    fn reject_unknown_export_name() {
        let reg_module = lowered_add_module();

        let store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(&reg_module, &store, "missing", &[]).unwrap_err();

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

        let store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(&reg_module, &store, "add", &[Value::I32(20)]).unwrap_err();

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

        let store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(
            &reg_module,
            &store,
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

        let store = Store::instantiate(&reg_module).unwrap();
        let err = execute_export(
            &reg_module,
            &store,
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
