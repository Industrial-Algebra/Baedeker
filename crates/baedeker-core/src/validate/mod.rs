// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! WebAssembly validation.
//!
//! This module begins Phase 1 validation with a deliberately small but useful subset:
//! - function type index resolution
//! - local index validation
//! - call target validation
//! - basic structured control-flow balancing
//! - basic operand/result typing for a small instruction subset
//!
//! Validation errors are reported against precise instruction byte offsets so
//! malformed functions can be diagnosed at the failing opcode rather than only
//! at the enclosing function body.

pub mod error;
pub mod state;

use alloc::{vec, vec::Vec};

use crate::binary::instr::{
    DecodedInstr, Instr, TryTableCatch, TryTableCatchKind, decode_instr_sequence_with_offsets,
};
use crate::binary::module::Module;
use crate::error::ByteOffset;
use crate::types::{
    BlockType, DataIdx, DataMode, ElemIdx, ElementInit, ElementMode, ExportDesc, FuncIdx, FuncType,
    GlobalIdx, HeapType, ImportDesc, LocalDecl, MemIdx, Mutability, RefType, TableIdx, TagIdx,
    TypeIdx, ValType,
};
use crate::validate::error::{ValidationError, ValidationErrorKind};
use crate::validate::state::{
    ControlKind, ControlKind::*, OperandType, Reachability, ValidationState,
};

pub use error::{ValidationError as Error, ValidationErrorKind as ErrorKind};
pub use state::{
    ControlFrame, OperandType as ValidationOperandType, Reachability as ValidationReachability,
    TypeStack, ValidationState as FunctionValidationState,
};

impl<'a> Module<'a> {
    /// Validate the currently decoded subset of the module.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_module(self)
    }
}

/// Validate a decoded module.
pub fn validate_module(module: &Module<'_>) -> Result<(), ValidationError> {
    for &type_idx in &module.functions {
        if module.types.get(type_idx.0 as usize).is_none() {
            return Err(ValidationError {
                offset: ByteOffset(0),
                function: None,
                kind: ValidationErrorKind::UnknownTypeIdx { idx: type_idx },
            });
        }
    }

    validate_type_definitions(module)?;
    validate_imports(module)?;
    validate_tags(module)?;
    validate_tables(module)?;
    validate_memories(module)?;
    validate_globals(module)?;
    validate_data_segments(module)?;
    validate_bulk_memory(module)?;
    validate_exports(module)?;
    validate_elements(module)?;
    validate_start(module)?;

    for (func_idx, (type_idx, code)) in module.functions.iter().zip(module.codes()).enumerate() {
        let Some(ty) = module.func_type(*type_idx) else {
            return Err(ValidationError {
                offset: ByteOffset(0),
                function: None,
                kind: ValidationErrorKind::UnknownTypeIdx { idx: *type_idx },
            });
        };
        validate_function(
            module,
            FuncIdx(func_idx as u32),
            ty,
            code.locals.as_slice(),
            code,
        )?;
    }

    Ok(())
}

/// Group spans covering every type index: explicit `rec` groups as decoded,
/// and every other index as its own singleton group.
///
/// Fallible throughout (resource discipline): a refused reservation is a
/// clean `ResourceLimitExceeded`, never an allocation abort — found by the
/// U3 declining-allocator sweep (a plain `Vec::push` here aborted a child
/// under allocation pressure; CI flake 2026-10-19).
fn type_group_spans(module: &Module<'_>) -> Result<Vec<(u32, u32)>, ()> {
    // Empty rec groups contribute no indices, so only non-empty groups form
    // spans; every other index is its own singleton group. The groups are
    // kept in a sorted vector consumed by an ascending cursor instead of a
    // `BTreeMap`: a `BTreeMap` insert allocates a node infallibly, which
    // aborts on allocator refusal.
    let mut groups: Vec<(u32, u32)> = Vec::new();
    groups
        .try_reserve(module.rec_groups.len())
        .map_err(|_| ())?;
    for &(start, len) in module.rec_groups.iter() {
        if len > 0 {
            groups.push((start, len));
        }
    }
    groups.sort_unstable_by_key(|&(start, _)| start);
    let mut spans = Vec::new();
    spans.try_reserve(module.types.len()).map_err(|_| ())?;
    let mut cursor = 0usize;
    let mut start: u32 = 0;
    while (start as usize) < module.types.len() {
        let mut len = 1;
        if cursor < groups.len() && groups[cursor].0 == start {
            len = groups[cursor].1;
        }
        while cursor < groups.len() && groups[cursor].0 <= start {
            cursor += 1;
        }
        spans.push((start, len));
        start += len;
    }
    Ok(spans)
}

/// Validate the type section: roll & install each rec group, then check every
/// supertype (`check_rectype` / `check_subtype_sub`, valid.ml:161-189).
///
/// A whole group is installed before its members are checked, so references
/// to *later members of the same group* are legal; references to later groups
/// are unknown-index errors. A supertype must strictly precede its member
/// (`xi < group_start + member_index`), so earlier in-group members are legal,
/// while self and later members are forward uses.
fn validate_type_definitions(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Type)
        .map(|section| section.offset)
        .unwrap_or(0);

    for (start, len) in type_group_spans(module).map_err(|()| ValidationError {
        offset: ByteOffset(offset),
        function: None,
        kind: ValidationErrorKind::ResourceLimitExceeded {
            what: "type group spans",
        },
    })? {
        let end = start + len;

        // 1. Every referenced type index must be installed (`< end`); this
        //    admits in-group forward references but rejects later groups.
        for idx in start..end {
            validate_sub_type_type_indices(&module.types[idx as usize], end, offset)?;
        }

        // 2. Supertype BOUND rules for every member, in member order, before
        //    any comptype matching. A supertype must strictly precede the
        //    member (`xi < group_start + member_index`, valid.ml:165): earlier
        //    members of the same group are legal; self and later members are
        //    forward uses. Checking all bounds first prevents a comptype match
        //    from traversing an unchecked cyclic supertype
        //    (reviewer reproduction: `03 50 00 5f 00 ... 50 01 03 5f 00`).
        for idx in start..end {
            let sub = &module.types[idx as usize];
            for &supertype in &sub.supertypes {
                if supertype >= idx {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: None,
                        kind: ValidationErrorKind::ForwardUseOfTypeInSubType {
                            sub: idx,
                            supertype,
                        },
                    });
                }
            }
        }

        // 3. Finality and comptype matching for every supertype. All bounds
        //    are known good here, so the declared-supertype walk in
        //    `deftype_matches` is acyclic (each step strictly decreases).
        for idx in start..end {
            let sub = &module.types[idx as usize];
            for &supertype in &sub.supertypes {
                let sup = &module.types[supertype as usize];
                if sup.is_final {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: None,
                        kind: ValidationErrorKind::FinalSupertype {
                            sub: idx,
                            supertype,
                        },
                    });
                }
                if !comptype_matches(module, &sub.comp, &sup.comp) {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: None,
                        kind: ValidationErrorKind::SubtypeComptypeMismatch {
                            sub: idx,
                            supertype,
                        },
                    });
                }
            }
        }
    }

    Ok(())
}

fn validate_sub_type_type_indices(
    sub: &crate::types::SubType,
    group_end: u32,
    offset: usize,
) -> Result<(), ValidationError> {
    for &supertype in &sub.supertypes {
        if supertype >= group_end {
            return Err(unknown_type_idx(TypeIdx(supertype), offset));
        }
    }
    validate_comptype_type_indices(&sub.comp, group_end, offset)
}

fn validate_comptype_type_indices(
    comp: &crate::types::CompType,
    group_end: u32,
    offset: usize,
) -> Result<(), ValidationError> {
    use crate::types::CompType;
    match comp {
        CompType::Struct(fields) => {
            for field in fields {
                validate_storage_type_indices(&field.storage, group_end, offset)?;
            }
            Ok(())
        }
        CompType::Array(field) => validate_storage_type_indices(&field.storage, group_end, offset),
        CompType::Func(func) => {
            for val in func.params.iter().chain(&func.results) {
                if let ValType::Ref(ref_type) = val
                    && let crate::types::HeapType::Type(idx) = ref_type.heap_type()
                    && idx.0 >= group_end
                {
                    return Err(unknown_type_idx(idx, offset));
                }
            }
            Ok(())
        }
    }
}

fn validate_storage_type_indices(
    storage: &crate::types::StorageType,
    group_end: u32,
    offset: usize,
) -> Result<(), ValidationError> {
    if let crate::types::StorageType::Val(ValType::Ref(ref_type)) = storage
        && let crate::types::HeapType::Type(idx) = ref_type.heap_type()
        && idx.0 >= group_end
    {
        return Err(unknown_type_idx(idx, offset));
    }
    Ok(())
}

fn unknown_type_idx(idx: TypeIdx, offset: usize) -> ValidationError {
    ValidationError {
        offset: ByteOffset(offset),
        function: None,
        kind: ValidationErrorKind::UnknownTypeIdx { idx },
    }
}

fn validate_imports(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Import)
        .map(|section| section.offset)
        .unwrap_or(0);

    for import in module.imports() {
        match &import.desc {
            ImportDesc::Func(type_idx) => {
                if module.func_type(*type_idx).is_none() {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: None,
                        kind: ValidationErrorKind::UnknownTypeIdx { idx: *type_idx },
                    });
                }
            }
            ImportDesc::Table(table) => {
                validate_reftype_type_indices(module, table.elem, None, offset)?;
                validate_table_limits(table.limits, offset)?;
            }
            ImportDesc::Global(global) => {
                validate_valtype_type_indices(module, global.val_type, None, offset)?;
            }
            ImportDesc::Mem(_) => {}
            ImportDesc::Tag(type_idx) => {
                let ty = resolve_type(module, *type_idx, None, offset)?;
                if !ty.results.is_empty() {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: None,
                        kind: ValidationErrorKind::NonEmptyTagResult {
                            type_idx: type_idx.0,
                        },
                    });
                }
            }
        }
    }

    Ok(())
}

/// Validate module-defined exception tags.
///
/// A tag's type must be a function type with empty results.
fn validate_tags(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Tag)
        .map(|section| section.offset)
        .unwrap_or(0);

    for type_idx in &module.tags {
        let ty = resolve_type(module, *type_idx, None, offset)?;
        if !ty.results.is_empty() {
            return Err(ValidationError {
                offset: ByteOffset(offset),
                function: None,
                kind: ValidationErrorKind::NonEmptyTagResult {
                    type_idx: type_idx.0,
                },
            });
        }
    }

    Ok(())
}

fn validate_memories(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Memory)
        .map(|section| section.offset)
        .unwrap_or(0);

    // Spec limits: at most 65536 pages, and min must not exceed max.
    for import in module.imports() {
        if let crate::types::ImportDesc::Mem(memory) = import.desc {
            validate_memory_limits(memory.limits, offset)?;
        }
    }
    for memory in &module.memories {
        validate_memory_limits(memory.limits, offset)?;
    }

    Ok(())
}

/// The numeric type used to address a memory/table: i64 for a 64-bit index
/// type, i32 otherwise (`numtype_of_addrtype` in the reference interpreter).
fn addr_num_type(limits: &crate::types::Limits) -> crate::types::NumType {
    if limits.is64 {
        crate::types::NumType::I64
    } else {
        crate::types::NumType::I32
    }
}

/// The narrower of two address types (`min at1 at2` in the reference): i32
/// unless both operands are i64.
fn min_addr_num_type(
    lhs: crate::types::NumType,
    rhs: crate::types::NumType,
) -> crate::types::NumType {
    if matches!(lhs, crate::types::NumType::I64) && matches!(rhs, crate::types::NumType::I64) {
        crate::types::NumType::I64
    } else {
        crate::types::NumType::I32
    }
}

fn validate_memory_limits(
    limits: crate::types::Limits,
    offset: usize,
) -> Result<(), ValidationError> {
    // Spec limits: i32 memories hold at most 2^16 pages, i64 memories at most
    // 2^48 pages (256 TiB); min must not exceed max.
    let max_allowed: u64 = if limits.is64 { 1 << 48 } else { 1 << 16 };
    if limits.min > max_allowed || limits.max.is_some_and(|max| max > max_allowed) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::MemorySizeOutOfRange,
        });
    }
    if limits.max.is_some_and(|max| limits.min > max) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::MemoryMinExceedsMax,
        });
    }
    Ok(())
}

/// Table limits: i32 tables are bounded by 2^32-1 entries, i64 tables are
/// unbounded; min must not exceed max. Applied to defined AND imported
/// tables (parity with `check_tabletype` in the reference).
fn validate_table_limits(
    limits: crate::types::Limits,
    offset: usize,
) -> Result<(), ValidationError> {
    let max_allowed: u64 = if limits.is64 {
        u64::MAX
    } else {
        u32::MAX as u64
    };
    if limits.min > max_allowed || limits.max.is_some_and(|max| max > max_allowed) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::TableSizeOutOfRange,
        });
    }
    if limits.max.is_some_and(|max| limits.min > max) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::TableMinExceedsMax,
        });
    }
    Ok(())
}

fn validate_tables(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Table)
        .map(|section| section.offset)
        .unwrap_or(0);

    for table in &module.tables {
        validate_reftype_type_indices(module, table.elem, None, offset)?;
        validate_table_limits(table.limits, offset)?;
        // A table with a non-nullable element type needs an initializer —
        // there is no default value to fill with.
        if table.init.is_none()
            && matches!(
                table.elem,
                crate::types::RefType::Typed {
                    nullable: false,
                    ..
                }
            )
        {
            return Err(ValidationError {
                offset: ByteOffset(offset),
                function: None,
                kind: ValidationErrorKind::TableTypeMismatch,
            });
        }
        if let Some(init) = &table.init {
            // Table init exprs reference imported globals only (matching the
            // global-init scope).
            validate_const_expr(
                module,
                init,
                offset,
                normalize_valtype(module, ValType::Ref(table.elem)),
                ConstExprGlobalScope::ImportedPlusDefined {
                    defined_globals_available: 0,
                },
                ConstExprKind::GlobalInit,
            )?;
        }
    }

    Ok(())
}

fn validate_globals(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Global)
        .map(|section| section.offset)
        .unwrap_or(0);

    for (defined_globals_available, global) in module.globals().iter().enumerate() {
        validate_valtype_type_indices(module, global.global_type.val_type, None, offset)?;
        validate_global_init_expr(module, global, defined_globals_available)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ConstExprGlobalScope {
    ImportedPlusDefined { defined_globals_available: usize },
    All,
}

#[derive(Debug, Clone, Copy)]
enum ConstExprKind {
    GlobalInit,
    ElementExpr,
    I32Offset,
}

fn validate_global_init_expr(
    module: &Module<'_>,
    global: &crate::types::Global<'_>,
    defined_globals_available: usize,
) -> Result<(), ValidationError> {
    let expected = normalize_valtype(module, global.global_type.val_type);
    validate_const_expr(
        module,
        global.init_expr,
        global.init_offset,
        expected,
        ConstExprGlobalScope::ImportedPlusDefined {
            defined_globals_available,
        },
        ConstExprKind::GlobalInit,
    )
}

fn resolve_const_global_val_type(
    module: &Module<'_>,
    idx: GlobalIdx,
    offset: usize,
    scope: ConstExprGlobalScope,
) -> Result<ValType, ValidationError> {
    let imported_globals = module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Global(global) => Some(global),
            _ => None,
        });
    let defined_globals = module.globals.iter().map(|global| global.global_type);

    // Enumerate the globals in scope into one vector. The vector is
    // module-scaled (one entry per import plus defined global), so it is
    // reserved fallibly before it is filled: a plain `collect` is an
    // infallible allocation that aborts on allocator refusal.
    let mut available_globals: Vec<_> = Vec::new();
    available_globals
        .try_reserve(module.imports.len() + module.globals.len())
        .map_err(|_| ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "const expression global scope",
            },
        })?;
    match scope {
        ConstExprGlobalScope::ImportedPlusDefined {
            defined_globals_available,
        } => {
            available_globals.extend(imported_globals);
            available_globals.extend(defined_globals.take(defined_globals_available));
        }
        ConstExprGlobalScope::All => {
            available_globals.extend(imported_globals);
            available_globals.extend(defined_globals);
        }
    }

    let available = available_globals.len() as u32;
    let global_type = available_globals
        .get(idx.0 as usize)
        .copied()
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::UnknownGlobalIdx { idx, available },
        })?;

    if global_type.mutability != Mutability::Const {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::MutableGlobalInInitExpr { idx },
        });
    }

    Ok(normalize_valtype(module, global_type.val_type))
}

fn validate_data_segments(module: &Module<'_>) -> Result<(), ValidationError> {
    if let Some(count) = module.data_count()
        && count as usize != module.data().len()
    {
        return Err(ValidationError {
            offset: ByteOffset(0),
            function: None,
            kind: ValidationErrorKind::UnknownDataIdx {
                idx: DataIdx(count),
                available: module.data().len() as u32,
            },
        });
    }

    for segment in module.data() {
        if let DataMode::Active {
            memory,
            offset_expr,
            offset_offset,
        } = &segment.mode
        {
            let memory_type = resolve_memory_type(module, *memory, FuncIdx(0), *offset_offset)?;
            validate_const_num_expr(
                module,
                offset_expr,
                addr_num_type(&memory_type.limits),
                *offset_offset,
            )?;
        }
    }

    Ok(())
}

/// Validate an active-segment offset constant expression against the target
/// memory's/table's address type.
fn validate_const_num_expr(
    module: &Module<'_>,
    expr: &[u8],
    ty: crate::types::NumType,
    offset: usize,
) -> Result<(), ValidationError> {
    validate_const_expr(
        module,
        expr,
        offset,
        ValType::Num(ty),
        ConstExprGlobalScope::All,
        ConstExprKind::I32Offset,
    )
}

fn validate_const_ref_expr(
    module: &Module<'_>,
    expr: &[u8],
    offset: usize,
    expected: RefType,
) -> Result<(), ValidationError> {
    validate_const_expr(
        module,
        expr,
        offset,
        normalize_valtype(module, ValType::Ref(expected)),
        ConstExprGlobalScope::All,
        ConstExprKind::ElementExpr,
    )
}

fn validate_const_expr(
    module: &Module<'_>,
    expr: &[u8],
    offset: usize,
    expected: ValType,
    global_scope: ConstExprGlobalScope,
    kind: ConstExprKind,
) -> Result<(), ValidationError> {
    let instrs = decode_instr_sequence_with_offsets(expr, offset).map_err(|e| ValidationError {
        offset: e.offset,
        function: None,
        kind: e.into(),
    })?;

    if !matches!(
        instrs.last().map(|decoded| &decoded.instr),
        Some(Instr::End)
    ) {
        return Err(const_expr_invalid_shape_error(kind, ByteOffset(offset)));
    }

    let mut stack = Vec::new();
    // Reserve the const-expression operand stack up front, fallibly: a const
    // expression is a module-controlled byte string, and every instruction
    // pushes at most one operand, so `instrs.len()` is a safe upper bound on
    // the depth. Reserving here keeps every `stack.push` below a
    // non-allocating push (resource discipline: a refused allocation is a
    // clean `ResourceLimitExceeded`, never a process abort).
    stack
        .try_reserve(instrs.len())
        .map_err(|_| ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "const expression operands",
            },
        })?;
    for decoded in &instrs[..instrs.len().saturating_sub(1)] {
        validate_const_instr(module, &mut stack, decoded, global_scope, kind)?;
    }

    if stack.len() != 1 {
        return Err(const_expr_invalid_shape_error(
            kind,
            instrs
                .first()
                .map(|decoded| decoded.offset)
                .unwrap_or(ByteOffset(offset)),
        ));
    }

    let found = stack.pop().expect("const expr stack length checked");
    if !valtype_matches(module, found, expected) {
        return Err(match kind {
            ConstExprKind::GlobalInit | ConstExprKind::I32Offset => ValidationError {
                offset: instrs
                    .first()
                    .map(|decoded| decoded.offset)
                    .unwrap_or(ByteOffset(offset)),
                function: None,
                kind: ValidationErrorKind::GlobalInitTypeMismatch { expected, found },
            },
            ConstExprKind::ElementExpr => ValidationError {
                offset: instrs
                    .first()
                    .map(|decoded| decoded.offset)
                    .unwrap_or(ByteOffset(offset)),
                function: None,
                kind: ValidationErrorKind::ElementExprTypeMismatch { expected, found },
            },
        });
    }

    Ok(())
}

fn validate_const_instr(
    module: &Module<'_>,
    stack: &mut Vec<ValType>,
    decoded: &DecodedInstr,
    global_scope: ConstExprGlobalScope,
    kind: ConstExprKind,
) -> Result<(), ValidationError> {
    match decoded.instr {
        // Constants are const-eligible in EVERY const context; the final
        // stack-shape/type check enforces the segment's declared type
        // (`valid.ml:1029-1040` `is_const` has no per-context constant
        // restriction).
        Instr::I32Const(_) => stack.push(ValType::Num(crate::types::NumType::I32)),
        Instr::I64Const(_) => stack.push(ValType::Num(crate::types::NumType::I64)),
        Instr::F32Const(_) => stack.push(ValType::Num(crate::types::NumType::F32)),
        Instr::F64Const(_) => stack.push(ValType::Num(crate::types::NumType::F64)),
        Instr::V128Const(_) => stack.push(ValType::Vec(crate::types::VecType::V128)),
        Instr::RefNull(ref_type) => {
            validate_reftype_type_indices(module, ref_type, None, decoded.offset.0)?;
            stack.push(normalize_valtype(module, ValType::Ref(ref_type)));
        }
        Instr::RefI31 => {
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            stack.push(ValType::Ref(RefType::from_parts(false, HeapType::I31)));
        }
        Instr::StructNew(type_idx) => {
            // `struct.new` pops one value per field, last field first.
            let fields = struct_field_types(module, FuncIdx(0), type_idx, decoded.offset.0)
                .map_err(without_function)?;
            for field in fields.iter().rev() {
                pop_const_expect(
                    module,
                    stack,
                    unpacked_valtype(field.storage),
                    decoded.offset,
                    kind,
                )?;
            }
            stack.push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::StructNewDefault(type_idx) => {
            let fields = struct_field_types(module, FuncIdx(0), type_idx, decoded.offset.0)
                .map_err(without_function)?;
            ensure_defaultable(
                fields.iter().map(|field| field.storage),
                type_idx,
                FuncIdx(0),
                decoded.offset.0,
            )
            .map_err(without_function)?;
            stack.push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::ArrayNew(type_idx) => {
            // Operands are `[value, len]` with `len` on top.
            let field = array_field_type(module, FuncIdx(0), type_idx, decoded.offset.0)
                .map_err(without_function)?;
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            pop_const_expect(
                module,
                stack,
                unpacked_valtype(field.storage),
                decoded.offset,
                kind,
            )?;
            stack.push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::ArrayNewDefault(type_idx) => {
            let field = array_field_type(module, FuncIdx(0), type_idx, decoded.offset.0)
                .map_err(without_function)?;
            ensure_defaultable(
                core::iter::once(field.storage),
                type_idx,
                FuncIdx(0),
                decoded.offset.0,
            )
            .map_err(without_function)?;
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            stack.push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::ArrayNewFixed { type_idx, count } => {
            let field = array_field_type(module, FuncIdx(0), type_idx, decoded.offset.0)
                .map_err(without_function)?;
            let expected = unpacked_valtype(field.storage);
            // Each pop shrinks the stack, so the loop is bounded by the
            // current stack height even for a huge `count`.
            for _ in 0..count {
                pop_const_expect(module, stack, expected, decoded.offset, kind)?;
            }
            stack.push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::AnyConvertExtern => {
            // `[RefT (nul, ExternHT)] --> [RefT (nul, AnyHT)]`, the result
            // keeping the operand's nullability (`valid.ml` `peek_ref`).
            let found = pop_const_operand(
                module,
                stack,
                ValType::Ref(RefType::from_parts(true, HeapType::Extern)),
                decoded.offset,
                kind,
            )?;
            push_converted_ref(stack, found, HeapType::Any);
        }
        Instr::ExternConvertAny => {
            // `[RefT (nul, AnyHT)] --> [RefT (nul, ExternHT)]`.
            let found = pop_const_operand(
                module,
                stack,
                ValType::Ref(RefType::from_parts(true, HeapType::Any)),
                decoded.offset,
                kind,
            )?;
            push_converted_ref(stack, found, HeapType::Extern);
        }
        Instr::RefFunc(idx) => {
            let type_idx = resolve_func_type_idx_for_module(module, idx, decoded.offset.0)?;
            if !is_declared_function_ref(module, idx) {
                return Err(ValidationError {
                    offset: decoded.offset,
                    function: None,
                    kind: ValidationErrorKind::UndeclaredFuncRef { idx },
                });
            }
            stack.push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::GlobalGet(idx) => stack.push(resolve_const_global_val_type(
            module,
            idx,
            decoded.offset.0,
            global_scope,
        )?),
        Instr::I32Add | Instr::I32Sub | Instr::I32Mul => {
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            stack.push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I64Add | Instr::I64Sub | Instr::I64Mul => {
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I64),
                decoded.offset,
                kind,
            )?;
            pop_const_expect(
                module,
                stack,
                ValType::Num(crate::types::NumType::I64),
                decoded.offset,
                kind,
            )?;
            stack.push(ValType::Num(crate::types::NumType::I64));
        }
        _ => return Err(const_expr_non_constant_error(kind, decoded.offset)),
    }

    Ok(())
}

fn pop_const_expect(
    module: &Module<'_>,
    stack: &mut Vec<ValType>,
    expected: ValType,
    offset: ByteOffset,
    kind: ConstExprKind,
) -> Result<(), ValidationError> {
    pop_const_operand(module, stack, expected, offset, kind).map(|_| ())
}

/// Pop one const-context operand of type `expected` (module-aware subtyping),
/// returning the popped type.
fn pop_const_operand(
    module: &Module<'_>,
    stack: &mut Vec<ValType>,
    expected: ValType,
    offset: ByteOffset,
    kind: ConstExprKind,
) -> Result<ValType, ValidationError> {
    let Some(found) = stack.pop() else {
        return Err(const_expr_type_mismatch(kind, offset, expected, expected));
    };

    if !valtype_matches(module, found, expected) {
        return Err(const_expr_type_mismatch(kind, offset, expected, found));
    }

    Ok(found)
}

/// Push the result of an extern/any conversion const instruction: the target
/// heap type with the operand reference's nullability (`peek_ref` semantics).
fn push_converted_ref(stack: &mut Vec<ValType>, found: ValType, target: HeapType) {
    let nullable = match found {
        ValType::Ref(ref_type) => ref_type.is_nullable(),
        _ => true,
    };
    stack.push(ValType::Ref(RefType::from_parts(nullable, target)));
}

/// Drop the function attribution from a helper error raised in a const
/// context (const-expression errors carry `function: None`).
fn without_function(mut error: ValidationError) -> ValidationError {
    error.function = None;
    error
}

fn const_expr_type_mismatch(
    kind: ConstExprKind,
    offset: ByteOffset,
    expected: ValType,
    found: ValType,
) -> ValidationError {
    let kind = match kind {
        ConstExprKind::ElementExpr => {
            ValidationErrorKind::ElementExprTypeMismatch { expected, found }
        }
        ConstExprKind::GlobalInit | ConstExprKind::I32Offset => {
            ValidationErrorKind::GlobalInitTypeMismatch { expected, found }
        }
    };
    ValidationError {
        offset,
        function: None,
        kind,
    }
}

fn const_expr_non_constant_error(kind: ConstExprKind, offset: ByteOffset) -> ValidationError {
    let kind = match kind {
        ConstExprKind::GlobalInit | ConstExprKind::I32Offset => {
            ValidationErrorKind::NonConstantGlobalInitExpr
        }
        ConstExprKind::ElementExpr => ValidationErrorKind::NonConstantElementExpr,
    };
    ValidationError {
        offset,
        function: None,
        kind,
    }
}

fn const_expr_invalid_shape_error(kind: ConstExprKind, offset: ByteOffset) -> ValidationError {
    let kind = match kind {
        ConstExprKind::GlobalInit | ConstExprKind::I32Offset => {
            ValidationErrorKind::InvalidGlobalInitExpr
        }
        ConstExprKind::ElementExpr => ValidationErrorKind::InvalidElementExpr,
    };
    ValidationError {
        offset,
        function: None,
        kind,
    }
}

fn validate_bulk_memory(module: &Module<'_>) -> Result<(), ValidationError> {
    if module.data_count().is_some() {
        return Ok(());
    }

    for (func_idx, code) in module.codes().iter().enumerate() {
        let function = FuncIdx(func_idx as u32);
        let instrs = code
            .instructions_with_offsets()
            .map_err(|e| ValidationError {
                offset: e.offset,
                function: Some(function),
                kind: e.into(),
            })?;

        for decoded in instrs {
            let op = match decoded.instr {
                Instr::MemoryInit(_, _) => Some("memory.init"),
                Instr::DataDrop(_) => Some("data.drop"),
                _ => None,
            };

            if let Some(op) = op {
                return Err(ValidationError {
                    offset: decoded.offset,
                    function: Some(function),
                    kind: ValidationErrorKind::MissingDataCountSection { op },
                });
            }
        }
    }

    Ok(())
}

fn validate_exports(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Export)
        .map(|section| section.offset)
        .unwrap_or(0);
    // Export-name uniqueness is tracked in a sorted `Vec` rather than a
    // `BTreeSet`: a `BTreeSet` insert allocates a leaf node infallibly, which
    // aborts on allocator refusal, while the vector's growth is try-reserved
    // and a binary-search lookup keeps the duplicate scan logarithmic.
    let mut names: Vec<&str> = Vec::new();

    for export in module.exports() {
        let name = export.name.as_str();
        names.try_reserve(1).map_err(|_| ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "export names",
            },
        })?;
        let position = match names.binary_search(&name) {
            Ok(_) => {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: None,
                    kind: ValidationErrorKind::DuplicateExportName {
                        name: export.name.clone(),
                    },
                });
            }
            Err(position) => position,
        };
        names.insert(position, name);

        match export.desc {
            ExportDesc::Func(idx) => {
                let _ = resolve_func_type_for_module(module, idx, offset)?;
            }
            ExportDesc::Table(idx) => {
                let _ = resolve_table_type_for_module(module, idx, offset)?;
            }
            ExportDesc::Mem(idx) => {
                let _ = resolve_memory_type_for_module(module, idx, offset)?;
            }
            ExportDesc::Global(idx) => {
                let _ = resolve_global_type_for_module(module, idx, offset)?;
            }
            ExportDesc::Tag(idx) => {
                let _ = resolve_tag_type_for_module(module, idx, offset)?;
            }
        }
    }

    Ok(())
}

fn validate_elements(module: &Module<'_>) -> Result<(), ValidationError> {
    let section_offset = module
        .section(crate::binary::section::SectionId::Element)
        .map(|section| section.offset)
        .unwrap_or(0);

    for element in module.elements() {
        validate_reftype_type_indices(module, element.elem_type, None, section_offset)?;

        if let ElementMode::Active {
            table,
            offset_expr,
            offset_offset,
        } = &element.mode
        {
            let table_type = resolve_table_type_for_module(module, *table, *offset_offset)?;
            let elem_type = normalize_reftype(module, element.elem_type);
            // FuncIndices segments carry only `ref.func` values, which are
            // non-null by construction, so they fit both `funcref` and
            // non-null `(ref func)` tables. Expression segments match on
            // their declared type.
            let matches = match &element.init {
                crate::types::ElementInit::FuncIndices(_) => matches!(
                    table_type.elem,
                    RefType::FuncRef
                        | RefType::Typed {
                            heap: crate::types::HeapType::Func,
                            ..
                        }
                ),
                crate::types::ElementInit::Expressions(_) => {
                    reftype_matches(module, elem_type, table_type.elem)
                }
            };
            if !matches {
                return Err(ValidationError {
                    offset: ByteOffset(*offset_offset),
                    function: None,
                    kind: ValidationErrorKind::ElementTableTypeMismatch {
                        expected: table_type.elem,
                        found: elem_type,
                    },
                });
            }
            validate_const_num_expr(
                module,
                offset_expr,
                addr_num_type(&table_type.limits),
                *offset_offset,
            )?;
        }

        match &element.init {
            ElementInit::FuncIndices(funcs) => {
                for &func in funcs {
                    let _ = resolve_func_type_for_module(module, func, section_offset)?;
                }
            }
            ElementInit::Expressions(exprs) => {
                for expr in exprs {
                    validate_const_ref_expr(
                        module,
                        expr.expr,
                        expr.offset,
                        normalize_reftype(module, element.elem_type),
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn validate_start(module: &Module<'_>) -> Result<(), ValidationError> {
    let Some(start) = module.start() else {
        return Ok(());
    };

    let offset = module
        .section(crate::binary::section::SectionId::Start)
        .map(|section| section.offset)
        .unwrap_or(0);
    let ty = resolve_func_type_for_module(module, start, offset)?;

    if !ty.params.is_empty() || !ty.results.is_empty() {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: None,
            kind: ValidationErrorKind::InvalidStartFunctionType {
                params: ty.params.clone(),
                results: ty.results.clone(),
            },
        });
    }

    Ok(())
}

fn validate_function(
    module: &Module<'_>,
    function: FuncIdx,
    ty: &FuncType,
    local_decls: &[LocalDecl],
    code: &crate::types::CodeBody<'_>,
) -> Result<(), ValidationError> {
    let ty = normalize_func_type(module, ty).map_err(|()| ValidationError {
        offset: ByteOffset(code.body_offset),
        function: Some(function),
        kind: ValidationErrorKind::ResourceLimitExceeded {
            what: "function type",
        },
    })?;
    let mut locals = copy_valtypes_fallible(
        function,
        code.body_offset,
        "function parameter types",
        &ty.params,
    )?;
    expand_locals(module, function, &mut locals, local_decls, code.body_offset)?;

    let instrs = code
        .instructions_with_offsets()
        .map_err(|e| ValidationError {
            offset: e.offset,
            function: Some(function),
            kind: e.into(),
        })?;

    let local_inits = initial_local_inits(&locals, ty.params.len(), function, code.body_offset)?;
    let result_types = copy_valtypes_fallible(
        function,
        code.body_offset,
        "function result types",
        &ty.results,
    )?;
    let mut state =
        ValidationState::new(locals, local_inits, result_types).ok_or(ValidationError {
            offset: ByteOffset(code.body_offset),
            function: Some(function),
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "block frame",
            },
        })?;

    for decoded in &instrs {
        validate_instr(module, function, decoded, &mut state)?;
    }

    if state.controls.len() != 1 {
        return Err(ValidationError {
            offset: ByteOffset(code.body_offset),
            function: Some(function),
            kind: ValidationErrorKind::UnterminatedControlFrames,
        });
    }

    if ensure_frame_end_types(
        module,
        &state,
        state.controls[0].outer_height,
        &state.controls[0].end_types,
    )
    .is_err()
    {
        let full_stack = concrete_stack(&state);
        let found_len = core::cmp::min(full_stack.len(), ty.results.len());
        let found = full_stack[full_stack.len().saturating_sub(found_len)..].to_vec();
        return Err(ValidationError {
            offset: final_result_offset(code, &instrs),
            function: Some(function),
            kind: ValidationErrorKind::FunctionResultTypeMismatch {
                expected: ty.results.clone(),
                found,
                full_stack,
            },
        });
    }

    Ok(())
}

fn final_result_offset(code: &crate::types::CodeBody<'_>, instrs: &[DecodedInstr]) -> ByteOffset {
    instrs
        .last()
        .map(|decoded| decoded.offset)
        .unwrap_or(ByteOffset(code.body_offset))
}

fn initial_local_inits(
    locals: &[ValType],
    param_count: usize,
    function: FuncIdx,
    offset: usize,
) -> Result<Vec<bool>, ValidationError> {
    let mut out = Vec::new();
    out.try_reserve(locals.len()).map_err(|_| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::ResourceLimitExceeded {
            what: "local-init flags",
        },
    })?;
    for (idx, ty) in locals.iter().enumerate() {
        out.push(idx < param_count || valtype_is_defaultable(*ty));
    }
    Ok(out)
}

fn valtype_is_defaultable(ty: ValType) -> bool {
    match ty {
        ValType::Num(_) | ValType::Vec(_) => true,
        ValType::Ref(ref_type) => ref_type.is_nullable(),
    }
}

fn validate_instr(
    module: &Module<'_>,
    function: FuncIdx,
    decoded: &DecodedInstr,
    state: &mut ValidationState,
) -> Result<(), ValidationError> {
    let offset = decoded.offset.0;

    match &decoded.instr {
        Instr::Unreachable => state.enter_unreachable(),
        Instr::Nop => {}
        Instr::Else => {
            // Take the frame's start/end vectors out (no ownership copy) so
            // the operand pops — which need `&mut state` — can read
            // `end_types`; both are put back on the frame below. The frame
            // stays on the stack for the else-body.
            let (outer_height, start_types, end_types) = {
                let frame = state.current_frame_mut();
                if frame.kind != If {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: Some(function),
                        kind: ValidationErrorKind::ElseOutsideIf,
                    });
                }
                if frame.has_else {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: Some(function),
                        kind: ValidationErrorKind::UnexpectedElse,
                    });
                }
                frame.has_else = true;
                (
                    frame.outer_height,
                    core::mem::take(&mut frame.start_types),
                    core::mem::take(&mut frame.end_types),
                )
            };
            // Fallibly copy the if-frame's local-init snapshot for the reset.
            let mut local_inits = Vec::new();
            if local_inits
                .try_reserve(state.current_frame().local_inits.len())
                .is_err()
            {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ResourceLimitExceeded {
                        what: "else local-init snapshot",
                    },
                });
            }
            local_inits.extend_from_slice(&state.current_frame().local_inits);
            // The then-body must have produced exactly the frame's result
            // types before the stack resets for the else-body.
            if let Err(found) = ensure_frame_end_types(module, state, outer_height, &end_types) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ControlResultTypeMismatch {
                        expected: end_types,
                        found,
                    },
                });
            }
            pop_control_result_types(module, function, state, &end_types, offset)?;
            state.operands.truncate(outer_height);
            state.local_inits = local_inits;
            if !state.operands.try_reserve(start_types.len()) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ResourceLimitExceeded {
                        what: "else parameter operands",
                    },
                });
            }
            for ty in &start_types {
                push_operand(function, offset, state, OperandType::Typed(*ty))?;
            }
            let floor = state.operands.len();
            let frame = state.current_frame_mut();
            frame.start_types = start_types;
            frame.end_types = end_types;
            frame.stack_floor = floor;
            state.reachability = Reachability::Reachable;
        }
        Instr::Block(block_type) => {
            let sig = resolve_block_type(module, function, *block_type, offset)?;
            pop_exact(module, function, state, &sig.params, offset)?;
            push_control_frame(
                function,
                offset,
                "block frame",
                state,
                Block,
                *block_type,
                sig.params,
                sig.results,
            )?;
        }
        Instr::Loop(block_type) => {
            let sig = resolve_block_type(module, function, *block_type, offset)?;
            pop_exact(module, function, state, &sig.params, offset)?;
            push_control_frame(
                function,
                offset,
                "loop frame",
                state,
                Loop,
                *block_type,
                sig.params,
                sig.results,
            )?;
        }
        Instr::If(block_type) => {
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "if",
            )?;
            let sig = resolve_block_type(module, function, *block_type, offset)?;
            pop_exact(module, function, state, &sig.params, offset)?;
            push_control_frame(
                function,
                offset,
                "if frame",
                state,
                If,
                *block_type,
                sig.params,
                sig.results,
            )?;
        }
        Instr::End => {
            if state.controls.len() > 1 {
                finish_frame(module, function, state, offset)?;
            }
        }
        Instr::Br(label) => {
            let label_types = copy_valtypes_fallible(
                function,
                offset,
                "branch label types",
                validate_label(function, state, *label, offset)?,
            )?;
            pop_branch_types(module, function, state, *label, &label_types, offset)?;
            state.enter_unreachable();
        }
        Instr::BrIf(label) => {
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "br_if",
            )?;
            let label_types = copy_valtypes_fallible(
                function,
                offset,
                "branch label types",
                validate_label(function, state, *label, offset)?,
            )?;
            pop_branch_types(module, function, state, *label, &label_types, offset)?;
            for ty in label_types {
                push_operand(function, offset, state, OperandType::Typed(ty))?;
            }
        }
        Instr::BrOnNull(label) => {
            let label_types = copy_valtypes_fallible(
                function,
                offset,
                "branch label types",
                validate_label(function, state, *label, offset)?,
            )?;
            let found = pop_ref_type(function, state, offset, "br_on_null")?;
            pop_branch_types(module, function, state, *label, &label_types, offset)?;
            for ty in label_types {
                push_operand(function, offset, state, OperandType::Typed(ty))?;
            }
            match found {
                Some(found) => push_operand(
                    function,
                    offset,
                    state,
                    OperandType::Typed(ValType::Ref(found.as_non_null())),
                )?,
                None => push_operand(function, offset, state, OperandType::Bottom)?,
            }
        }
        Instr::BrOnNonNull(label) => {
            let label_types = copy_valtypes_fallible(
                function,
                offset,
                "branch label types",
                validate_label(function, state, *label, offset)?,
            )?;
            let Some((ValType::Ref(expected), rest)) = label_types.split_last() else {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::InvalidBrOnNonNullTarget {
                        label: *label,
                        found: label_types,
                    },
                });
            };
            let found = pop_ref_type(function, state, offset, "br_on_non_null")?;
            let expected_input = expected.as_nullable();
            if let Some(found) = found
                && !reftype_matches(module, found, expected_input)
            {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::TypeMismatch {
                        op: "br_on_non_null",
                        expected: ValType::Ref(expected_input),
                        found: ValType::Ref(found),
                    },
                });
            }
            pop_branch_types(module, function, state, *label, rest, offset)?;
            for ty in rest {
                push_operand(function, offset, state, OperandType::Typed(*ty))?;
            }
        }
        Instr::BrTable { targets, default } => {
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "br_table",
            )?;
            let default_types = copy_valtypes_fallible(
                function,
                offset,
                "branch label types",
                validate_label(function, state, *default, offset)?,
            )?;
            // Every target must agree on arity; the operand must be a
            // subtype of each target's label types (they need not be
            // identical — the join is the operand's own type).
            for label in targets {
                let label_types = validate_label(function, state, *label, offset)?;
                if label_types.len() != default_types.len() {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: Some(function),
                        kind: ValidationErrorKind::InconsistentBranchTypes {
                            expected: default_types.clone(),
                            found: label_types.to_vec(),
                        },
                    });
                }
                ensure_stack_types(module, state, label_types).map_err(|found| {
                    ValidationError {
                        offset: ByteOffset(offset),
                        function: Some(function),
                        kind: ValidationErrorKind::BranchTypeMismatch {
                            label: *label,
                            expected: label_types.to_vec(),
                            found,
                        },
                    }
                })?;
            }
            ensure_stack_types(module, state, &default_types).map_err(|found| ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::BranchTypeMismatch {
                    label: *default,
                    expected: default_types.clone(),
                    found,
                },
            })?;
            pop_exact(module, function, state, &default_types, offset)?;
            state.enter_unreachable();
        }
        Instr::Return => {
            let expected = copy_valtypes_fallible(
                function,
                offset,
                "function result types",
                &state.controls[0].end_types,
            )?;
            pop_control_result_types(module, function, state, &expected, offset)?;
            state.enter_unreachable();
        }
        Instr::Call(idx) => {
            let ty = resolve_func_type(module, *idx, function, offset)?;
            pop_exact(module, function, state, &ty.params, offset)?;
            for result in &ty.results {
                push_operand(function, offset, state, OperandType::Typed(*result))?;
            }
        }
        Instr::ReturnCall(idx) => {
            let ty = resolve_func_type(module, *idx, function, offset)?;
            validate_tail_call_results(module, function, state, &ty.results, offset)?;
            pop_exact(module, function, state, &ty.params, offset)?;
            state.enter_unreachable();
        }
        Instr::CallRef(type_idx) => {
            let ty = resolve_type(module, *type_idx, Some(function), offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(
                    true,
                    canonicalize_func_type_idx(module, *type_idx),
                )),
                offset,
                "call_ref",
            )?;
            pop_exact(module, function, state, &ty.params, offset)?;
            for result in &ty.results {
                push_operand(function, offset, state, OperandType::Typed(*result))?;
            }
        }
        Instr::ReturnCallRef(type_idx) => {
            let ty = resolve_type(module, *type_idx, Some(function), offset)?;
            validate_tail_call_results(module, function, state, &ty.results, offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(
                    true,
                    canonicalize_func_type_idx(module, *type_idx),
                )),
                offset,
                "return_call_ref",
            )?;
            pop_exact(module, function, state, &ty.params, offset)?;
            state.enter_unreachable();
        }
        Instr::CallIndirect {
            type_idx,
            table_idx,
        } => {
            let table_type =
                resolve_table_type_with_context(module, *table_idx, Some(function), offset)?;
            if !reftype_matches(module, table_type.elem, RefType::FuncRef) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::InvalidCallIndirectTableType {
                        expected: RefType::FuncRef,
                        found: table_type.elem,
                    },
                });
            }
            let ty = resolve_type(module, *type_idx, Some(function), offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&table_type.limits)),
                offset,
                "call_indirect",
            )?;
            pop_exact(module, function, state, &ty.params, offset)?;
            for result in &ty.results {
                push_operand(function, offset, state, OperandType::Typed(*result))?;
            }
        }
        Instr::ReturnCallIndirect {
            type_idx,
            table_idx,
        } => {
            let table_type =
                resolve_table_type_with_context(module, *table_idx, Some(function), offset)?;
            if !reftype_matches(module, table_type.elem, RefType::FuncRef) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::InvalidCallIndirectTableType {
                        expected: RefType::FuncRef,
                        found: table_type.elem,
                    },
                });
            }
            let ty = resolve_type(module, *type_idx, Some(function), offset)?;
            validate_tail_call_results(module, function, state, &ty.results, offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&table_type.limits)),
                offset,
                "return_call_indirect",
            )?;
            pop_exact(module, function, state, &ty.params, offset)?;
            state.enter_unreachable();
        }
        Instr::Drop => {
            pop_any(function, state, offset, "drop")?;
        }
        Instr::Select => {
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "select",
            )?;
            let rhs = pop_operand_type(function, state, offset, "select")?;
            let lhs = pop_operand_type(function, state, offset, "select")?;
            match (lhs, rhs) {
                (OperandType::Bottom, OperandType::Bottom) => {
                    push_operand(function, offset, state, OperandType::Bottom)?
                }
                (OperandType::Bottom, OperandType::Typed(rhs))
                | (OperandType::Typed(rhs), OperandType::Bottom) => {
                    push_operand(function, offset, state, OperandType::Typed(rhs))?
                }
                (OperandType::Typed(lhs), OperandType::Typed(rhs)) => {
                    if lhs != rhs {
                        return Err(ValidationError {
                            offset: ByteOffset(offset),
                            function: Some(function),
                            kind: ValidationErrorKind::SelectOperandTypeMismatch {
                                expected: lhs,
                                found: vec![lhs, rhs],
                            },
                        });
                    }
                    // Untyped select accepts number and vector operands only
                    // (references require the typed form).
                    if !matches!(lhs, ValType::Num(_) | ValType::Vec(_)) {
                        return Err(ValidationError {
                            offset: ByteOffset(offset),
                            function: Some(function),
                            kind: ValidationErrorKind::SelectOperandTypeMismatch {
                                expected: ValType::Num(crate::types::NumType::I32),
                                found: vec![lhs, rhs],
                            },
                        });
                    }
                    push_operand(function, offset, state, OperandType::Typed(lhs))?;
                }
            }
        }
        Instr::SelectTyped(types) => {
            if types.len() != 1 {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::InvalidSelectResultArity { found: types.len() },
                });
            }
            validate_valtype_type_indices(module, types[0], Some(function), offset)?;
            let expected = normalize_valtype(module, types[0]);
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "select_typed",
            )?;
            let rhs = pop_operand_type(function, state, offset, "select_typed")?;
            let lhs = pop_operand_type(function, state, offset, "select_typed")?;
            if !operand_matches(module, lhs, expected) || !operand_matches(module, rhs, expected) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::SelectOperandTypeMismatch {
                        expected,
                        found: vec![
                            operand_to_valtype(lhs, expected),
                            operand_to_valtype(rhs, expected),
                        ],
                    },
                });
            }
            push_operand(function, offset, state, OperandType::Typed(expected))?;
        }
        Instr::LocalGet(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            if !state
                .local_inits
                .get(idx.0 as usize)
                .copied()
                .unwrap_or(false)
            {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::UninitializedLocal { idx: *idx },
                });
            }
            push_operand(function, offset, state, OperandType::Typed(ty))?;
        }
        Instr::LocalSet(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(module, function, state, ty, offset, "local.set")?;
            state.local_inits[idx.0 as usize] = true;
        }
        Instr::LocalTee(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(module, function, state, ty, offset, "local.tee")?;
            state.local_inits[idx.0 as usize] = true;
            push_operand(function, offset, state, OperandType::Typed(ty))?;
        }
        Instr::GlobalGet(idx) => {
            let ty = resolve_global_type(module, *idx, function, offset)?;
            push_operand(function, offset, state, OperandType::Typed(ty.val_type))?;
        }
        Instr::GlobalSet(idx) => {
            let ty = resolve_global_type(module, *idx, function, offset)?;
            if ty.mutability != Mutability::Var {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableGlobalSet { idx: *idx },
                });
            }
            pop_expect(module, function, state, ty.val_type, offset, "global.set")?;
        }
        Instr::TableGet(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&ty.limits)),
                offset,
                "table.get",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(ty.elem)),
            )?;
        }
        Instr::TableSet(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(ty.elem),
                offset,
                "table.set",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&ty.limits)),
                offset,
                "table.set",
            )?;
        }
        Instr::V128Load(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 4,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load8x8S(memarg) | Instr::V128Load8x8U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load8x8",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load16x4S(memarg) | Instr::V128Load16x4U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load16x4",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load32x2S(memarg) | Instr::V128Load32x2U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load32x2",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load8Splat(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load8_splat",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load16Splat(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load16_splat",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load32Splat(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load32_splat",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load64Splat(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load64_splat",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load32Zero(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load32_zero",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load64Zero(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "v128.load64_zero",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Store(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "v128.store",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 4,
                stored: ValType::Vec(crate::types::VecType::V128),
            },
        )?,
        Instr::V128Load8Lane { memarg, lane } => validate_simd_load_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.load8_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                lane: *lane,
                max_lane: 15,
            },
        )?,
        Instr::V128Load16Lane { memarg, lane } => validate_simd_load_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.load16_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                lane: *lane,
                max_lane: 7,
            },
        )?,
        Instr::V128Load32Lane { memarg, lane } => validate_simd_load_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.load32_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                lane: *lane,
                max_lane: 3,
            },
        )?,
        Instr::V128Load64Lane { memarg, lane } => validate_simd_load_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.load64_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                lane: *lane,
                max_lane: 1,
            },
        )?,
        Instr::V128Store8Lane { memarg, lane } => validate_simd_store_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.store8_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                lane: *lane,
                max_lane: 15,
            },
        )?,
        Instr::V128Store16Lane { memarg, lane } => validate_simd_store_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.store16_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                lane: *lane,
                max_lane: 7,
            },
        )?,
        Instr::V128Store32Lane { memarg, lane } => validate_simd_store_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.store32_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                lane: *lane,
                max_lane: 3,
            },
        )?,
        Instr::V128Store64Lane { memarg, lane } => validate_simd_store_lane(
            module,
            function,
            state,
            offset,
            SimdLaneValidation {
                op: "v128.store64_lane",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                lane: *lane,
                max_lane: 1,
            },
        )?,
        Instr::I32Load(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i32.load",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                result: ValType::Num(crate::types::NumType::I32),
            },
        )?,
        Instr::I64Load(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i64.load",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::F32Load(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "f32.load",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                result: ValType::Num(crate::types::NumType::F32),
            },
        )?,
        Instr::F64Load(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "f64.load",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                result: ValType::Num(crate::types::NumType::F64),
            },
        )?,
        Instr::I32Load8S(memarg) | Instr::I32Load8U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i32.load8",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                result: ValType::Num(crate::types::NumType::I32),
            },
        )?,
        Instr::I32Load16S(memarg) | Instr::I32Load16U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i32.load16",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                result: ValType::Num(crate::types::NumType::I32),
            },
        )?,
        Instr::I64Load8S(memarg) | Instr::I64Load8U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i64.load8",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                result: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::I64Load16S(memarg) | Instr::I64Load16U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i64.load16",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                result: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::I64Load32S(memarg) | Instr::I64Load32U(memarg) => validate_load(
            module,
            function,
            state,
            offset,
            MemLoadValidation {
                op: "i64.load32",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                result: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::I32Store(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i32.store",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                stored: ValType::Num(crate::types::NumType::I32),
            },
        )?,
        Instr::I64Store(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i64.store",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                stored: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::F32Store(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "f32.store",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                stored: ValType::Num(crate::types::NumType::F32),
            },
        )?,
        Instr::F64Store(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "f64.store",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 3,
                stored: ValType::Num(crate::types::NumType::F64),
            },
        )?,
        Instr::I32Store8(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i32.store8",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                stored: ValType::Num(crate::types::NumType::I32),
            },
        )?,
        Instr::I32Store16(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i32.store16",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                stored: ValType::Num(crate::types::NumType::I32),
            },
        )?,
        Instr::I64Store8(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i64.store8",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 0,
                stored: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::I64Store16(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i64.store16",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 1,
                stored: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::I64Store32(memarg) => validate_store(
            module,
            function,
            state,
            offset,
            MemStoreValidation {
                op: "i64.store32",
                memory: memarg.memory,
                found_align: memarg.align,
                offset_value: memarg.offset,
                max_align: 2,
                stored: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::MemoryInit(data_idx, mem_idx) => {
            resolve_data_segment(module, *data_idx, function, offset)?;
            let mem_type = resolve_memory_type(module, *mem_idx, function, offset)?;
            // Stack `[addr; i32; i32]`: count and src are always i32; the
            // destination is the memory's address type.
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.init",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.init",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&mem_type.limits)),
                offset,
                "memory.init",
            )?;
        }
        Instr::DataDrop(data_idx) => {
            resolve_data_segment(module, *data_idx, function, offset)?;
        }
        Instr::MemoryCopy { dst, src } => {
            let dst_ty = resolve_memory_type(module, *dst, function, offset)?;
            let src_ty = resolve_memory_type(module, *src, function, offset)?;
            let count_ty =
                min_addr_num_type(addr_num_type(&dst_ty.limits), addr_num_type(&src_ty.limits));
            // Stack `[dst addr; src addr; min(dst, src)]`.
            pop_expect(
                module,
                function,
                state,
                ValType::Num(count_ty),
                offset,
                "memory.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&src_ty.limits)),
                offset,
                "memory.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&dst_ty.limits)),
                offset,
                "memory.copy",
            )?;
        }
        Instr::MemoryFill(mem_idx) => {
            let mem_type = resolve_memory_type(module, *mem_idx, function, offset)?;
            // Stack `[dst addr; i32 value; addr count]`: the fill value is
            // always i32; dst and count use the memory's address type.
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&mem_type.limits)),
                offset,
                "memory.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&mem_type.limits)),
                offset,
                "memory.fill",
            )?;
        }
        Instr::TableInit {
            elem_idx,
            table_idx,
        } => {
            let elem = resolve_element_segment(module, *elem_idx, function, offset)?;
            let elem_type = normalize_reftype(module, elem.elem_type);
            let table =
                resolve_table_type_with_context(module, *table_idx, Some(function), offset)?;
            if !reftype_matches(module, elem_type, table.elem) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ElementTableTypeMismatch {
                        expected: table.elem,
                        found: elem_type,
                    },
                });
            }
            // Stack `[dst addr; i32 src; i32 count]`: src and count are
            // always i32; the destination is the table's address type.
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.init",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.init",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_num_type(&table.limits)),
                offset,
                "table.init",
            )?;
        }
        Instr::ElemDrop(elem_idx) => {
            let _ = resolve_element_segment(module, *elem_idx, function, offset)?;
        }
        Instr::TableCopy { dst, src } => {
            let dst_ty = resolve_table_type_with_context(module, *dst, Some(function), offset)?;
            let src_ty = resolve_table_type_with_context(module, *src, Some(function), offset)?;
            if !reftype_matches(module, src_ty.elem, dst_ty.elem)
                || !reftype_matches(module, dst_ty.elem, src_ty.elem)
            {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ElementTableTypeMismatch {
                        expected: dst_ty.elem,
                        found: src_ty.elem,
                    },
                });
            }
            let dst_addr_ty = addr_num_type(&dst_ty.limits);
            let src_addr_ty = addr_num_type(&src_ty.limits);
            let count_ty = min_addr_num_type(dst_addr_ty, src_addr_ty);
            // Stack `[dst addr; src addr; min(dst, src)]`.
            pop_expect(
                module,
                function,
                state,
                ValType::Num(count_ty),
                offset,
                "table.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(src_addr_ty),
                offset,
                "table.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(dst_addr_ty),
                offset,
                "table.copy",
            )?;
        }
        Instr::TableGrow(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            let addr_ty = addr_num_type(&ty.limits);
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_ty),
                offset,
                "table.grow",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(ty.elem),
                offset,
                "table.grow",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(addr_ty)),
            )?;
        }
        Instr::TableSize(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(addr_num_type(&ty.limits))),
            )?;
        }
        Instr::TableFill(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            let addr_ty = addr_num_type(&ty.limits);
            // Stack `[index addr; reftype value; addr count]`.
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_ty),
                offset,
                "table.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(ty.elem),
                offset,
                "table.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_ty),
                offset,
                "table.fill",
            )?;
        }
        Instr::MemorySize(idx) => {
            let mem_type = resolve_memory_type(module, *idx, function, offset)?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(addr_num_type(&mem_type.limits))),
            )?;
        }
        Instr::MemoryGrow(idx) => {
            let mem_type = resolve_memory_type(module, *idx, function, offset)?;
            let addr_ty = addr_num_type(&mem_type.limits);
            pop_expect(
                module,
                function,
                state,
                ValType::Num(addr_ty),
                offset,
                "memory.grow",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(addr_ty)),
            )?;
        }
        Instr::AtomicFence => {
            // `atomic.fence`: [] -> []. No memory requirement at the
            // threads pin (valid.ml:311 checks nothing but the shape).
        }
        Instr::Atomic(op) => validate_atomic(module, function, state, offset, op)?,
        Instr::I32Const(_) => push_operand(
            function,
            offset,
            state,
            OperandType::Typed(ValType::Num(crate::types::NumType::I32)),
        )?,
        Instr::I64Const(_) => push_operand(
            function,
            offset,
            state,
            OperandType::Typed(ValType::Num(crate::types::NumType::I64)),
        )?,
        Instr::F32Const(_) => push_operand(
            function,
            offset,
            state,
            OperandType::Typed(ValType::Num(crate::types::NumType::F32)),
        )?,
        Instr::F64Const(_) => push_operand(
            function,
            offset,
            state,
            OperandType::Typed(ValType::Num(crate::types::NumType::F64)),
        )?,
        Instr::RefNull(ref_type) => {
            validate_reftype_type_indices(module, *ref_type, Some(function), offset)?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(normalize_valtype(module, ValType::Ref(*ref_type))),
            )?;
        }
        Instr::RefIsNull => validate_ref_is_null(function, state, offset)?,
        Instr::RefAsNonNull => validate_ref_as_non_null(function, state, offset)?,
        Instr::RefFunc(idx) => {
            let type_idx = resolve_func_type_idx(module, *idx, function, offset)?;
            if !is_declared_function_ref(module, *idx) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::UndeclaredFuncRef { idx: *idx },
                });
            }
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, type_idx))),
            )?;
        }
        Instr::V128Const(_) => push_operand(
            function,
            offset,
            state,
            OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
        )?,
        Instr::I8x16Splat => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i8x16.splat",
            ValType::Num(crate::types::NumType::I32),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I16x8Splat => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i16x8.splat",
            ValType::Num(crate::types::NumType::I32),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I32x4Splat => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i32x4.splat",
            ValType::Num(crate::types::NumType::I32),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I64x2Splat => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i64x2.splat",
            ValType::Num(crate::types::NumType::I64),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::F32x4Splat => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "f32x4.splat",
            ValType::Num(crate::types::NumType::F32),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::F64x2Splat => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "f64x2.splat",
            ValType::Num(crate::types::NumType::F64),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I32x4ExtractLane(lane) => {
            validate_simd_lane_idx(function, offset, "i32x4.extract_lane", *lane, 3)?;
            validate_numeric_unary(
                module,
                function,
                state,
                offset,
                "i32x4.extract_lane",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Num(crate::types::NumType::I32),
            )?
        }
        Instr::I8x16ExtractLaneS(lane) | Instr::I8x16ExtractLaneU(lane) => {
            validate_simd_lane_idx(function, offset, "simd.extract_lane", *lane, 15)?;
            validate_numeric_unary(
                module,
                function,
                state,
                offset,
                "simd.extract_lane",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Num(crate::types::NumType::I32),
            )?
        }
        Instr::I16x8ExtractLaneS(lane) | Instr::I16x8ExtractLaneU(lane) => {
            validate_simd_lane_idx(function, offset, "simd.extract_lane", *lane, 7)?;
            validate_numeric_unary(
                module,
                function,
                state,
                offset,
                "simd.extract_lane",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Num(crate::types::NumType::I32),
            )?
        }
        Instr::I64x2ExtractLane(lane) => {
            validate_simd_lane_idx(function, offset, "i64x2.extract_lane", *lane, 1)?;
            validate_numeric_unary(
                module,
                function,
                state,
                offset,
                "i64x2.extract_lane",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Num(crate::types::NumType::I64),
            )?
        }
        Instr::F64x2ExtractLane(lane) => {
            validate_simd_lane_idx(function, offset, "f64x2.extract_lane", *lane, 1)?;
            validate_numeric_unary(
                module,
                function,
                state,
                offset,
                "f64x2.extract_lane",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Num(crate::types::NumType::F64),
            )?
        }
        Instr::F32x4ExtractLane(lane) => {
            validate_simd_lane_idx(function, offset, "f32x4.extract_lane", *lane, 3)?;
            validate_numeric_unary(
                module,
                function,
                state,
                offset,
                "f32x4.extract_lane",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Num(crate::types::NumType::F32),
            )?
        }
        Instr::I32x4ReplaceLane(lane) => {
            validate_simd_lane_idx(function, offset, "i32x4.replace_lane", *lane, 3)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "i32x4.replace_lane",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "i32x4.replace_lane",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::I8x16ReplaceLane(lane) | Instr::I16x8ReplaceLane(lane) => {
            let max = if matches!(decoded.instr, Instr::I8x16ReplaceLane(_)) {
                15
            } else {
                7
            };
            validate_simd_lane_idx(function, offset, "simd.replace_lane", *lane, max)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "simd.replace_lane",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "simd.replace_lane",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::F32x4ReplaceLane(lane) => {
            validate_simd_lane_idx(function, offset, "f32x4.replace_lane", *lane, 3)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::F32),
                offset,
                "f32x4.replace_lane",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "f32x4.replace_lane",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::I64x2ReplaceLane(lane) => {
            validate_simd_lane_idx(function, offset, "i64x2.replace_lane", *lane, 1)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I64),
                offset,
                "i64x2.replace_lane",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "i64x2.replace_lane",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::F64x2ReplaceLane(lane) => {
            validate_simd_lane_idx(function, offset, "f64x2.replace_lane", *lane, 1)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::F64),
                offset,
                "f64x2.replace_lane",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "f64x2.replace_lane",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::V128Not => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "v128.not",
            ValType::Vec(crate::types::VecType::V128),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::V128And | Instr::V128Or | Instr::V128Xor => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "v128.bitwise",
            ValType::Vec(crate::types::VecType::V128),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I8x16Add
        | Instr::I8x16Sub
        | Instr::I16x8Add
        | Instr::I16x8Mul
        | Instr::I16x8Sub
        | Instr::I32x4Add
        | Instr::I32x4Sub
        | Instr::I32x4Mul
        | Instr::I64x2Add
        | Instr::I64x2Mul
        | Instr::I64x2Sub
        | Instr::F32x4Add
        | Instr::F32x4Sub
        | Instr::F32x4Mul
        | Instr::F32x4Div
        | Instr::F64x2Add
        | Instr::F64x2Sub
        | Instr::F64x2Mul
        | Instr::F64x2Div
        | Instr::V128Andnot
        | Instr::I8x16AddSatS
        | Instr::I8x16AddSatU
        | Instr::I8x16SubSatS
        | Instr::I8x16SubSatU
        | Instr::I8x16MinS
        | Instr::I8x16MinU
        | Instr::I8x16MaxS
        | Instr::I8x16MaxU
        | Instr::I8x16AvgrU
        | Instr::I16x8AddSatS
        | Instr::I16x8AddSatU
        | Instr::I16x8SubSatS
        | Instr::I16x8SubSatU
        | Instr::I16x8MinS
        | Instr::I16x8MinU
        | Instr::I16x8MaxS
        | Instr::I16x8MaxU
        | Instr::I16x8AvgrU
        | Instr::I16x8Q15MulrSatS
        | Instr::I32x4MinS
        | Instr::I32x4MinU
        | Instr::I32x4MaxS
        | Instr::I32x4MaxU
        | Instr::I8x16Eq
        | Instr::I8x16Ne
        | Instr::I8x16LtS
        | Instr::I8x16LtU
        | Instr::I8x16GtS
        | Instr::I8x16GtU
        | Instr::I8x16LeS
        | Instr::I8x16LeU
        | Instr::I8x16GeS
        | Instr::I8x16GeU
        | Instr::I16x8Eq
        | Instr::I16x8Ne
        | Instr::I16x8LtS
        | Instr::I16x8LtU
        | Instr::I16x8GtS
        | Instr::I16x8GtU
        | Instr::I16x8LeS
        | Instr::I16x8LeU
        | Instr::I16x8GeS
        | Instr::I16x8GeU
        | Instr::I32x4Eq
        | Instr::I32x4Ne
        | Instr::I32x4LtS
        | Instr::I32x4LtU
        | Instr::I32x4GtS
        | Instr::I32x4GtU
        | Instr::I32x4LeS
        | Instr::I32x4LeU
        | Instr::I32x4GeS
        | Instr::I32x4GeU
        | Instr::F32x4Eq
        | Instr::F32x4Ne
        | Instr::F32x4Lt
        | Instr::F32x4Gt
        | Instr::F32x4Le
        | Instr::F32x4Ge
        | Instr::F64x2Eq
        | Instr::F64x2Ne
        | Instr::F64x2Lt
        | Instr::F64x2Gt
        | Instr::F64x2Le
        | Instr::F64x2Ge
        | Instr::I64x2Eq
        | Instr::I64x2Ne
        | Instr::I64x2LtS
        | Instr::I64x2GtS
        | Instr::I64x2LeS
        | Instr::I64x2GeS
        | Instr::F32x4Min
        | Instr::F32x4Max
        | Instr::F32x4PMin
        | Instr::F32x4PMax
        | Instr::F64x2Min
        | Instr::F64x2Max
        | Instr::F64x2PMin
        | Instr::F64x2PMax
        | Instr::I8x16Swizzle => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "simd.arithmetic",
            ValType::Vec(crate::types::VecType::V128),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I8x16Abs
        | Instr::I8x16Neg
        | Instr::I8x16Popcnt
        | Instr::I16x8Abs
        | Instr::I16x8Neg
        | Instr::I32x4Abs
        | Instr::I32x4Neg
        | Instr::I64x2Abs
        | Instr::I64x2Neg
        | Instr::I16x8ExtAddPairwiseI8x16S
        | Instr::I16x8ExtAddPairwiseI8x16U
        | Instr::I32x4ExtAddPairwiseI16x8S
        | Instr::I32x4ExtAddPairwiseI16x8U
        | Instr::I16x8ExtendLowI8x16S
        | Instr::I16x8ExtendHighI8x16S
        | Instr::I16x8ExtendLowI8x16U
        | Instr::I16x8ExtendHighI8x16U
        | Instr::I32x4ExtendLowI16x8S
        | Instr::I32x4ExtendHighI16x8S
        | Instr::I32x4ExtendLowI16x8U
        | Instr::I32x4ExtendHighI16x8U
        | Instr::I64x2ExtendLowI32x4S
        | Instr::I64x2ExtendHighI32x4S
        | Instr::I64x2ExtendLowI32x4U
        | Instr::I64x2ExtendHighI32x4U
        | Instr::F32x4Abs
        | Instr::F32x4Neg
        | Instr::F32x4Sqrt
        | Instr::F32x4Ceil
        | Instr::F32x4Floor
        | Instr::F32x4Trunc
        | Instr::F32x4Nearest
        | Instr::F64x2Abs
        | Instr::F64x2Neg
        | Instr::F64x2Sqrt
        | Instr::F64x2Ceil
        | Instr::F64x2Floor
        | Instr::F64x2Trunc
        | Instr::F64x2Nearest
        | Instr::I32x4TruncSatF32x4S
        | Instr::I32x4TruncSatF32x4U
        | Instr::F32x4ConvertI32x4S
        | Instr::F32x4ConvertI32x4U
        | Instr::I32x4TruncSatF64x2SZero
        | Instr::I32x4TruncSatF64x2UZero
        | Instr::F32x4DemoteF64x2Zero
        | Instr::F64x2PromoteLowF32x4
        | Instr::F64x2ConvertLowI32x4S
        | Instr::F64x2ConvertLowI32x4U => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "simd.unary",
            ValType::Vec(crate::types::VecType::V128),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::I8x16NarrowI16x8S
        | Instr::I8x16NarrowI16x8U
        | Instr::I16x8NarrowI32x4S
        | Instr::I16x8NarrowI32x4U
        | Instr::I16x8ExtMulLowI8x16S
        | Instr::I16x8ExtMulHighI8x16S
        | Instr::I16x8ExtMulLowI8x16U
        | Instr::I16x8ExtMulHighI8x16U
        | Instr::I32x4ExtMulLowI16x8S
        | Instr::I32x4ExtMulHighI16x8S
        | Instr::I32x4ExtMulLowI16x8U
        | Instr::I32x4ExtMulHighI16x8U
        | Instr::I64x2ExtMulLowI32x4S
        | Instr::I64x2ExtMulHighI32x4S
        | Instr::I64x2ExtMulLowI32x4U
        | Instr::I64x2ExtMulHighI32x4U
        | Instr::I32x4DotI16x8S => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "simd.extmul",
            ValType::Vec(crate::types::VecType::V128),
            ValType::Vec(crate::types::VecType::V128),
        )?,
        Instr::V128Bitselect => {
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "v128.bitselect",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "v128.bitselect",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "v128.bitselect",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::I8x16Shuffle(lanes) => {
            if let Some(&found) = lanes.iter().find(|&&lane| lane >= 32) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::InvalidSimdLaneIdx {
                        op: "i8x16.shuffle",
                        max: 31,
                        found,
                    },
                });
            }
            validate_numeric_binary(
                module,
                function,
                state,
                offset,
                "i8x16.shuffle",
                ValType::Vec(crate::types::VecType::V128),
                ValType::Vec(crate::types::VecType::V128),
            )?;
        }
        Instr::I8x16Shl
        | Instr::I8x16ShrS
        | Instr::I8x16ShrU
        | Instr::I16x8Shl
        | Instr::I16x8ShrS
        | Instr::I16x8ShrU
        | Instr::I32x4Shl
        | Instr::I32x4ShrS
        | Instr::I32x4ShrU
        | Instr::I64x2Shl
        | Instr::I64x2ShrS
        | Instr::I64x2ShrU => {
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "simd.shift",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Vec(crate::types::VecType::V128),
                offset,
                "simd.shift",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
            )?;
        }
        Instr::V128AnyTrue
        | Instr::I8x16AllTrue
        | Instr::I16x8AllTrue
        | Instr::I32x4AllTrue
        | Instr::I64x2AllTrue
        | Instr::I8x16Bitmask
        | Instr::I16x8Bitmask
        | Instr::I32x4Bitmask
        | Instr::I64x2Bitmask => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "simd.test",
            ValType::Vec(crate::types::VecType::V128),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32Eqz => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i32.eqz",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32Eq
        | Instr::I32Ne
        | Instr::I32LtS
        | Instr::I32LtU
        | Instr::I32GtS
        | Instr::I32GtU
        | Instr::I32LeS
        | Instr::I32LeU
        | Instr::I32GeS
        | Instr::I32GeU => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "i32.compare",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Eqz => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i64.eqz",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Eq
        | Instr::I64Ne
        | Instr::I64LtS
        | Instr::I64LtU
        | Instr::I64GtS
        | Instr::I64GtU
        | Instr::I64LeS
        | Instr::I64LeU
        | Instr::I64GeS
        | Instr::I64GeU => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "i64.compare",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::F32Eq | Instr::F32Ne | Instr::F32Lt | Instr::F32Gt | Instr::F32Le | Instr::F32Ge => {
            validate_numeric_binary(
                module,
                function,
                state,
                offset,
                "f32.compare",
                ValType::Num(crate::types::NumType::F32),
                ValType::Num(crate::types::NumType::I32),
            )?
        }
        Instr::F64Eq | Instr::F64Ne | Instr::F64Lt | Instr::F64Gt | Instr::F64Le | Instr::F64Ge => {
            validate_numeric_binary(
                module,
                function,
                state,
                offset,
                "f64.compare",
                ValType::Num(crate::types::NumType::F64),
                ValType::Num(crate::types::NumType::I32),
            )?
        }
        Instr::I32Clz | Instr::I32Ctz | Instr::I32Popcnt => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i32.unary",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32Add
        | Instr::I32Sub
        | Instr::I32Mul
        | Instr::I32DivS
        | Instr::I32DivU
        | Instr::I32RemS
        | Instr::I32RemU
        | Instr::I32And
        | Instr::I32Or
        | Instr::I32Xor
        | Instr::I32Shl
        | Instr::I32ShrS
        | Instr::I32ShrU
        | Instr::I32Rotl
        | Instr::I32Rotr => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "i32.binary",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Clz | Instr::I64Ctz | Instr::I64Popcnt => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "i64.unary",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64Add
        | Instr::I64Sub
        | Instr::I64Mul
        | Instr::I64DivS
        | Instr::I64DivU
        | Instr::I64RemS
        | Instr::I64RemU
        | Instr::I64And
        | Instr::I64Or
        | Instr::I64Xor
        | Instr::I64Shl
        | Instr::I64ShrS
        | Instr::I64ShrU
        | Instr::I64Rotl
        | Instr::I64Rotr => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "i64.binary",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::F32Abs
        | Instr::F32Neg
        | Instr::F32Ceil
        | Instr::F32Floor
        | Instr::F32Trunc
        | Instr::F32Nearest
        | Instr::F32Sqrt => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "f32.unary",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F32Add
        | Instr::F32Sub
        | Instr::F32Mul
        | Instr::F32Div
        | Instr::F32Min
        | Instr::F32Max
        | Instr::F32Copysign => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "f32.binary",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F64Abs
        | Instr::F64Neg
        | Instr::F64Ceil
        | Instr::F64Floor
        | Instr::F64Trunc
        | Instr::F64Nearest
        | Instr::F64Sqrt => validate_numeric_unary(
            module,
            function,
            state,
            offset,
            "f64.unary",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::F64Add
        | Instr::F64Sub
        | Instr::F64Mul
        | Instr::F64Div
        | Instr::F64Min
        | Instr::F64Max
        | Instr::F64Copysign => validate_numeric_binary(
            module,
            function,
            state,
            offset,
            "f64.binary",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::I32WrapI64 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.wrap_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32TruncF32S | Instr::I32TruncF32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.trunc_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32TruncF64S | Instr::I32TruncF64U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.trunc_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64ExtendI32S | Instr::I64ExtendI32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i64.extend_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64TruncF32S | Instr::I64TruncF32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i64.trunc_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64TruncF64S | Instr::I64TruncF64U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i64.trunc_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::F32ConvertI32S | Instr::F32ConvertI32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f32.convert_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F32ConvertI64S | Instr::F32ConvertI64U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f32.convert_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F32DemoteF64 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f32.demote_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F64ConvertI32S | Instr::F64ConvertI32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f64.convert_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::F64ConvertI64S | Instr::F64ConvertI64U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f64.convert_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::F64PromoteF32 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f64.promote_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::I32ReinterpretF32 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.reinterpret_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64ReinterpretF64 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i64.reinterpret_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::F32ReinterpretI32 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f32.reinterpret_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F64ReinterpretI64 => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "f64.reinterpret_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::I32Extend8S | Instr::I32Extend16S => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.sign_extend",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Extend8S | Instr::I64Extend16S | Instr::I64Extend32S => {
            validate_numeric_conversion(
                module,
                function,
                state,
                offset,
                "i64.sign_extend",
                ValType::Num(crate::types::NumType::I64),
                ValType::Num(crate::types::NumType::I64),
            )?
        }
        Instr::I32TruncSatF32S | Instr::I32TruncSatF32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.trunc_sat_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32TruncSatF64S | Instr::I32TruncSatF64U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i32.trunc_sat_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64TruncSatF32S | Instr::I64TruncSatF32U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i64.trunc_sat_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64TruncSatF64S | Instr::I64TruncSatF64U => validate_numeric_conversion(
            module,
            function,
            state,
            offset,
            "i64.trunc_sat_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::TryTable(try_table) => {
            let sig = resolve_block_type(module, function, try_table.block_type, offset)?;
            pop_exact(module, function, state, &sig.params, offset)?;
            for catch in &try_table.catches {
                validate_catch(module, function, state, catch, offset)?;
            }
            push_control_frame(
                function,
                offset,
                "try_table frame",
                state,
                TryTable,
                try_table.block_type,
                sig.params,
                sig.results,
            )?;
        }
        Instr::Throw(tag) => {
            let (params, _results) = resolve_tag_type(module, function, *tag, offset)?;
            pop_exact(module, function, state, &params, offset)?;
            state.enter_unreachable();
        }
        Instr::ThrowRef => {
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::from_parts(true, HeapType::Exn)),
                offset,
                "throw_ref",
            )?;
            state.enter_unreachable();
        }
        Instr::StructNew(type_idx) => {
            let fields = struct_field_types(module, function, *type_idx, offset)?;
            for field in fields.iter().rev() {
                pop_expect(
                    module,
                    function,
                    state,
                    unpacked_valtype(field.storage),
                    offset,
                    "struct.new",
                )?;
            }
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::StructNewDefault(type_idx) => {
            let fields = struct_field_types(module, function, *type_idx, offset)?;
            ensure_defaultable(
                fields.iter().map(|field| field.storage),
                *type_idx,
                function,
                offset,
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::StructGet { type_idx, field } => {
            validate_gc_field_get(module, function, state, *type_idx, *field, None, offset)?;
        }
        Instr::StructGetS { type_idx, field } => {
            validate_gc_field_get(
                module,
                function,
                state,
                *type_idx,
                *field,
                Some(Ext::S),
                offset,
            )?;
        }
        Instr::StructGetU { type_idx, field } => {
            validate_gc_field_get(
                module,
                function,
                state,
                *type_idx,
                *field,
                Some(Ext::U),
                offset,
            )?;
        }
        Instr::StructSet { type_idx, field } => {
            let fields = struct_field_types(module, function, *type_idx, offset)?;
            let field_type = field_at(fields, *field, *type_idx, function, offset)?;
            if !field_type.mutable {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableField {
                        idx: *type_idx,
                        field: *field,
                    },
                });
            }
            let t = unpacked_valtype(field_type.storage);
            pop_expect(module, function, state, t, offset, "struct.set")?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *type_idx)),
                offset,
                "struct.set",
            )?;
        }
        Instr::ArrayNew(type_idx) => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.new",
            )?;
            pop_expect(
                module,
                function,
                state,
                unpacked_valtype(field.storage),
                offset,
                "array.new",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::ArrayNewDefault(type_idx) => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            ensure_defaultable(core::iter::once(field.storage), *type_idx, function, offset)?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.new_default",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::ArrayNewFixed { type_idx, count } => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            let t = unpacked_valtype(field.storage);
            if !is_stack_polymorphic(state) {
                // Non-polymorphic: each successful pop shrinks the stack, so
                // the loop is bounded by the current stack height even when
                // `count` is huge. A huge `count` on a shallow stack underflows
                // cleanly instead of allocating a `count`-sized vector.
                for _ in 0..*count {
                    pop_expect(module, function, state, t, offset, "array.new_fixed")?;
                }
            }
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::ArrayNewData { type_idx, data } => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            resolve_data_segment(module, *data, function, offset)?;
            if !is_num_or_vec(field.storage) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ArrayNotNumOrVec { idx: *type_idx },
                });
            }
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.new_data",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.new_data",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::ArrayNewElem { type_idx, elem } => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            let segment = resolve_element_segment(module, *elem, function, offset)?;
            let field_ty = unpacked_valtype(field.storage);
            if !valtype_matches_type(module, ValType::Ref(segment.elem_type), field_ty) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ElemFieldTypeMismatch {
                        elem: segment.elem_type,
                        field: field_ty,
                    },
                });
            }
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.new_elem",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.new_elem",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::concrete(false, *type_idx))),
            )?;
        }
        Instr::ArrayGet(type_idx) => {
            validate_array_get(module, function, state, *type_idx, None, offset)?;
        }
        Instr::ArrayGetS(type_idx) => {
            validate_array_get(module, function, state, *type_idx, Some(Ext::S), offset)?;
        }
        Instr::ArrayGetU(type_idx) => {
            validate_array_get(module, function, state, *type_idx, Some(Ext::U), offset)?;
        }
        Instr::ArraySet(type_idx) => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            if !field.mutable {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableArray { idx: *type_idx },
                });
            }
            pop_expect(
                module,
                function,
                state,
                unpacked_valtype(field.storage),
                offset,
                "array.set",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.set",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *type_idx)),
                offset,
                "array.set",
            )?;
        }
        Instr::ArrayLen => {
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::from_parts(true, HeapType::Array)),
                offset,
                "array.len",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(crate::types::NumType::I32)),
            )?;
        }
        Instr::ArrayFill(type_idx) => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            if !field.mutable {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableArray { idx: *type_idx },
                });
            }
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                unpacked_valtype(field.storage),
                offset,
                "array.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.fill",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *type_idx)),
                offset,
                "array.fill",
            )?;
        }
        Instr::ArrayCopy { dst, src } => {
            let dst_field = array_field_type(module, function, *dst, offset)?;
            let src_field = array_field_type(module, function, *src, offset)?;
            if !dst_field.mutable {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableArray { idx: *dst },
                });
            }
            if !storage_matches(module, src_field.storage, dst_field.storage) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ArrayStorageMismatch {
                        dst: *dst,
                        src: *src,
                    },
                });
            }
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *src)),
                offset,
                "array.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "array.copy",
            )?;
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *dst)),
                offset,
                "array.copy",
            )?;
        }
        Instr::ArrayInitData { type_idx, data } => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            if !field.mutable {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableArray { idx: *type_idx },
                });
            }
            resolve_data_segment(module, *data, function, offset)?;
            if !is_num_or_vec(field.storage) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ArrayNotNumOrVec { idx: *type_idx },
                });
            }
            for _ in 0..3 {
                pop_expect(
                    module,
                    function,
                    state,
                    ValType::Num(crate::types::NumType::I32),
                    offset,
                    "array.init_data",
                )?;
            }
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *type_idx)),
                offset,
                "array.init_data",
            )?;
        }
        Instr::ArrayInitElem { type_idx, elem } => {
            let field = array_field_type(module, function, *type_idx, offset)?;
            if !field.mutable {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ImmutableArray { idx: *type_idx },
                });
            }
            let segment = resolve_element_segment(module, *elem, function, offset)?;
            let field_ty = unpacked_valtype(field.storage);
            if !valtype_matches_type(module, ValType::Ref(segment.elem_type), field_ty) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ElemFieldTypeMismatch {
                        elem: segment.elem_type,
                        field: field_ty,
                    },
                });
            }
            for _ in 0..3 {
                pop_expect(
                    module,
                    function,
                    state,
                    ValType::Num(crate::types::NumType::I32),
                    offset,
                    "array.init_elem",
                )?;
            }
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::concrete(true, *type_idx)),
                offset,
                "array.init_elem",
            )?;
        }
        Instr::RefI31 => {
            pop_expect(
                module,
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "ref.i31",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::from_parts(false, HeapType::I31))),
            )?;
        }
        Instr::I31GetS | Instr::I31GetU => {
            pop_expect(
                module,
                function,
                state,
                ValType::Ref(RefType::from_parts(true, HeapType::I31)),
                offset,
                "i31.get",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(crate::types::NumType::I32)),
            )?;
        }
        Instr::RefTest { nullable, ht } => {
            validate_reftype_type_indices(
                module,
                RefType::from_parts(*nullable, *ht),
                Some(function),
                offset,
            )?;
            let operand = ValType::Ref(RefType::from_parts(true, top_of_heaptype(module, *ht)));
            pop_expect(module, function, state, operand, offset, "ref.test")?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(crate::types::NumType::I32)),
            )?;
        }
        Instr::RefCast { nullable, ht } => {
            validate_reftype_type_indices(
                module,
                RefType::from_parts(*nullable, *ht),
                Some(function),
                offset,
            )?;
            let operand = ValType::Ref(RefType::from_parts(true, top_of_heaptype(module, *ht)));
            pop_expect(module, function, state, operand, offset, "ref.cast")?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::from_parts(*nullable, *ht))),
            )?;
        }
        Instr::RefEq => {
            let eq = ValType::Ref(RefType::from_parts(true, HeapType::Eq));
            for _ in 0..2 {
                pop_expect(module, function, state, eq, offset, "ref.eq")?;
            }
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Num(crate::types::NumType::I32)),
            )?;
        }
        Instr::BrOnCast {
            fail,
            src,
            tgt,
            label,
        } => {
            validate_br_on_cast(module, function, state, *fail, *src, *tgt, *label, offset)?;
        }
        Instr::AnyConvertExtern => {
            let nullable = pop_ref_expect_heap(
                module,
                function,
                state,
                HeapType::Extern,
                offset,
                "any.convert_extern",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::from_parts(nullable, HeapType::Any))),
            )?;
        }
        Instr::ExternConvertAny => {
            let nullable = pop_ref_expect_heap(
                module,
                function,
                state,
                HeapType::Any,
                offset,
                "extern.convert_any",
            )?;
            push_operand(
                function,
                offset,
                state,
                OperandType::Typed(ValType::Ref(RefType::from_parts(
                    nullable,
                    HeapType::Extern,
                ))),
            )?;
        }
    }

    Ok(())
}

/// A packed-field read extension marker (validator view of `Some`/`None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ext {
    S,
    U,
}

fn struct_field_types<'a>(
    module: &'a Module<'_>,
    function: FuncIdx,
    type_idx: TypeIdx,
    offset: usize,
) -> Result<&'a [crate::types::FieldType], ValidationError> {
    match module.types.get(type_idx.0 as usize).map(|sub| &sub.comp) {
        Some(crate::types::CompType::Struct(fields)) => Ok(fields),
        _ => Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownStruct { idx: type_idx },
        }),
    }
}

fn array_field_type(
    module: &Module<'_>,
    function: FuncIdx,
    type_idx: TypeIdx,
    offset: usize,
) -> Result<crate::types::FieldType, ValidationError> {
    match module.types.get(type_idx.0 as usize).map(|sub| &sub.comp) {
        Some(crate::types::CompType::Array(field)) => Ok(*field),
        _ => Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownArray { idx: type_idx },
        }),
    }
}

fn field_at(
    fields: &[crate::types::FieldType],
    field: u32,
    type_idx: TypeIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<crate::types::FieldType, ValidationError> {
    fields.get(field as usize).copied().ok_or(ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::UnknownField {
            idx: type_idx,
            field,
        },
    })
}

fn unpacked_valtype(storage: crate::types::StorageType) -> ValType {
    match storage {
        crate::types::StorageType::Val(val) => val,
        crate::types::StorageType::I8 | crate::types::StorageType::I16 => {
            ValType::Num(crate::types::NumType::I32)
        }
    }
}

fn is_packed(storage: crate::types::StorageType) -> bool {
    matches!(
        storage,
        crate::types::StorageType::I8 | crate::types::StorageType::I16
    )
}

fn is_num_or_vec(storage: crate::types::StorageType) -> bool {
    match storage {
        crate::types::StorageType::I8 | crate::types::StorageType::I16 => true,
        crate::types::StorageType::Val(val) => matches!(val, ValType::Num(_) | ValType::Vec(_)),
    }
}

fn valtype_defaultable(val: ValType) -> bool {
    match val {
        ValType::Num(_) | ValType::Vec(_) => true,
        ValType::Ref(ref_type) => ref_type.is_nullable(),
    }
}

fn ensure_defaultable(
    storages: impl Iterator<Item = crate::types::StorageType>,
    type_idx: TypeIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<(), ValidationError> {
    for storage in storages {
        if !valtype_defaultable(unpacked_valtype(storage)) {
            return Err(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::NonDefaultable { idx: type_idx },
            });
        }
    }
    Ok(())
}

fn validate_gc_field_get(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    type_idx: TypeIdx,
    field: u32,
    ext: Option<Ext>,
    offset: usize,
) -> Result<(), ValidationError> {
    let fields = struct_field_types(module, function, type_idx, offset)?;
    let field_type = field_at(fields, field, type_idx, function, offset)?;
    if (ext.is_some()) != is_packed(field_type.storage) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::PackedFieldMismatch {
                idx: type_idx,
                field,
                packed: is_packed(field_type.storage),
            },
        });
    }
    pop_expect(
        module,
        function,
        state,
        ValType::Ref(RefType::concrete(true, type_idx)),
        offset,
        "struct.get",
    )?;
    push_operand(
        function,
        offset,
        state,
        OperandType::Typed(unpacked_valtype(field_type.storage)),
    )?;
    Ok(())
}

fn validate_array_get(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    type_idx: TypeIdx,
    ext: Option<Ext>,
    offset: usize,
) -> Result<(), ValidationError> {
    let field = array_field_type(module, function, type_idx, offset)?;
    if (ext.is_some()) != is_packed(field.storage) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::PackedFieldMismatch {
                idx: type_idx,
                field: 0,
                packed: is_packed(field.storage),
            },
        });
    }
    pop_expect(
        module,
        function,
        state,
        ValType::Num(crate::types::NumType::I32),
        offset,
        "array.get",
    )?;
    pop_expect(
        module,
        function,
        state,
        ValType::Ref(RefType::concrete(true, type_idx)),
        offset,
        "array.get",
    )?;
    push_operand(
        function,
        offset,
        state,
        OperandType::Typed(unpacked_valtype(field.storage)),
    )?;
    Ok(())
}

fn validate_numeric_unary(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    input: ValType,
    result: ValType,
) -> Result<(), ValidationError> {
    pop_expect(module, function, state, input, offset, op)?;
    push_operand(function, offset, state, OperandType::Typed(result))?;
    Ok(())
}

fn validate_numeric_binary(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    input: ValType,
    result: ValType,
) -> Result<(), ValidationError> {
    pop_expect(module, function, state, input, offset, op)?;
    pop_expect(module, function, state, input, offset, op)?;
    push_operand(function, offset, state, OperandType::Typed(result))?;
    Ok(())
}

fn validate_numeric_conversion(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    input: ValType,
    result: ValType,
) -> Result<(), ValidationError> {
    pop_expect(module, function, state, input, offset, op)?;
    push_operand(function, offset, state, OperandType::Typed(result))?;
    Ok(())
}

fn validate_ref_is_null(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    let _ = pop_ref_type(function, state, offset, "ref.is_null")?;
    push_operand(
        function,
        offset,
        state,
        OperandType::Typed(ValType::Num(crate::types::NumType::I32)),
    )?;
    Ok(())
}

fn validate_ref_as_non_null(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    let found = pop_ref_type(function, state, offset, "ref.as_non_null")?;
    match found {
        Some(found) => push_operand(
            function,
            offset,
            state,
            OperandType::Typed(ValType::Ref(found.as_non_null())),
        )?,
        None => push_operand(function, offset, state, OperandType::Bottom)?,
    }
    Ok(())
}

struct MemLoadValidation {
    op: &'static str,
    memory: MemIdx,
    found_align: u32,
    offset_value: u64,
    max_align: u32,
    result: ValType,
}

struct MemStoreValidation {
    op: &'static str,
    memory: MemIdx,
    found_align: u32,
    offset_value: u64,
    max_align: u32,
    stored: ValType,
}

struct SimdLaneValidation {
    op: &'static str,
    memory: MemIdx,
    found_align: u32,
    offset_value: u64,
    max_align: u32,
    lane: u8,
    max_lane: u8,
}

/// memory64 rule: an i32-indexed memory's memarg offset must fit in
/// u32; i64 memories accept the full u64 range.
fn validate_memarg_offset(
    module: &Module<'_>,
    memory: MemIdx,
    offset_value: u64,
    function: FuncIdx,
    offset: usize,
) -> Result<(), ValidationError> {
    let memory_type = resolve_memory_type(module, memory, function, offset)?;
    if !memory_type.limits.is64 && offset_value > u32::MAX as u64 {
        return Err(ValidationError {
            offset: crate::error::ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::MemArgOffsetOutOfRange,
        });
    }
    Ok(())
}

/// Classic-threads atomic validation (threads U1). Rules per the pin's
/// reference (valid.ml:151-160 `check_memop` shared by every atomic case,
/// :295-335 shapes): the memarg's memory defined, align <= natural (the
/// SAME rule as plain memory ops — the pin's rst text demanding equality
/// is the contract's recorded ambiguity; its interpreter and corpus make
/// unaligned access a RUNTIME trap, not a validation error), and NO
/// shared-memory context requirement. Operand shapes:
/// load [addr] -> [t]; store [addr t] -> []; rmw [addr t] -> [t];
/// cmpxchg [addr t t] -> [t]; notify [addr i32] -> [i32];
/// waitN [addr t i64] -> [i32]; fence [] -> [].
#[allow(clippy::too_many_lines)]
fn validate_atomic(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &crate::binary::instr::AtomicOp,
) -> Result<(), ValidationError> {
    use crate::binary::instr::AtomicOp as Op;
    use crate::types::MemArg;
    use crate::types::NumType::{I32, I64};
    use crate::types::ValType::Num;

    let (name, memarg): (&'static str, MemArg) = match op {
        Op::Notify(m) => ("memory.atomic.notify", *m),
        Op::Wait32(m) => ("memory.atomic.wait32", *m),
        Op::Wait64(m) => ("memory.atomic.wait64", *m),
        Op::Load(w, m) => (ATOMIC_LOAD_NAMES[width_index(*w)], *m),
        Op::Store(w, m) => (ATOMIC_STORE_NAMES[width_index(*w)], *m),
        Op::Rmw(kind, w, m) => {
            let row = rmw_index(*kind);
            debug_assert!(row < ATOMIC_RMW_NAMES.len(), "xchg handled separately");
            // xchg lives in the 0x41..=0x47 decode range but shares the
            // rmw table shape; map it past the 5 table rows to its own
            // handling below.
            let name = if row < ATOMIC_RMW_NAMES.len() {
                ATOMIC_RMW_NAMES[row][width_index(*w)]
            } else {
                XCHG_NAMES[width_index(*w)]
            };
            (name, *m)
        }
        Op::Cmpxchg(w, m) => (ATOMIC_CMPXCHG_NAMES[width_index(*w)], *m),
    };
    let natural = match op {
        Op::Notify(_) | Op::Wait32(_) => 2,
        Op::Wait64(_) => 3,
        Op::Load(w, _) | Op::Store(w, _) | Op::Rmw(_, w, _) | Op::Cmpxchg(w, _) => {
            w.natural_align_log2()
        }
    };
    validate_memarg_align(function, offset, name, memarg.align, natural)?;
    validate_memarg_offset(module, memarg.memory, memarg.offset, function, offset)?;
    let memory_type = resolve_memory_type(module, memarg.memory, function, offset)?;
    let addr = addr_num_type(&memory_type.limits);

    match op {
        Op::Load(w, _) => {
            pop_expect(
                module,
                function,
                state,
                Num(addr),
                offset,
                "atomic load address",
            )?;
            push_operand(function, offset, state, OperandType::Typed(w.val_type()))?;
        }
        Op::Store(w, _) => {
            let stored = w.val_type();
            pop_expect(
                module,
                function,
                state,
                stored,
                offset,
                "atomic store value",
            )?;
            pop_expect(
                module,
                function,
                state,
                Num(addr),
                offset,
                "atomic store address",
            )?;
        }
        Op::Rmw(_, w, _) => {
            let ty = w.val_type();
            pop_expect(module, function, state, ty, offset, "atomic rmw operand")?;
            pop_expect(
                module,
                function,
                state,
                Num(addr),
                offset,
                "atomic rmw address",
            )?;
            push_operand(function, offset, state, OperandType::Typed(ty))?;
        }
        Op::Cmpxchg(w, _) => {
            let ty = w.val_type();
            pop_expect(
                module,
                function,
                state,
                ty,
                offset,
                "atomic cmpxchg replacement",
            )?;
            pop_expect(
                module,
                function,
                state,
                ty,
                offset,
                "atomic cmpxchg expected",
            )?;
            pop_expect(
                module,
                function,
                state,
                Num(addr),
                offset,
                "atomic cmpxchg address",
            )?;
            push_operand(function, offset, state, OperandType::Typed(ty))?;
        }
        Op::Notify(_) => {
            pop_expect(module, function, state, Num(I32), offset, "notify count")?;
            pop_expect(module, function, state, Num(addr), offset, "notify address")?;
            push_operand(function, offset, state, OperandType::Typed(Num(I32)))?;
        }
        Op::Wait32(_) => {
            pop_expect(module, function, state, Num(I64), offset, "wait timeout")?;
            pop_expect(module, function, state, Num(I32), offset, "wait expected")?;
            pop_expect(module, function, state, Num(addr), offset, "wait address")?;
            push_operand(function, offset, state, OperandType::Typed(Num(I32)))?;
        }
        Op::Wait64(_) => {
            pop_expect(module, function, state, Num(I64), offset, "wait timeout")?;
            pop_expect(module, function, state, Num(I64), offset, "wait expected")?;
            pop_expect(module, function, state, Num(addr), offset, "wait address")?;
            push_operand(function, offset, state, OperandType::Typed(Num(I32)))?;
        }
    }
    Ok(())
}

/// WAT-style names for validation messages: `&'static str` (the error
/// kinds carry 'static op names). Rows follow `width_at`'s layout:
/// [i32, i64, i32.8u, i32.16u, i64.8u, i64.16u, i64.32u].
const ATOMIC_LOAD_NAMES: [&str; 7] = [
    "i32.atomic.load",
    "i64.atomic.load",
    "i32.atomic.load8_u",
    "i32.atomic.load16_u",
    "i64.atomic.load8_u",
    "i64.atomic.load16_u",
    "i64.atomic.load32_u",
];
const ATOMIC_STORE_NAMES: [&str; 7] = [
    "i32.atomic.store",
    "i64.atomic.store",
    "i32.atomic.store8",
    "i32.atomic.store16",
    "i64.atomic.store8",
    "i64.atomic.store16",
    "i64.atomic.store32",
];
const XCHG_NAMES: [&str; 7] = [
    "i32.atomic.rmw.xchg",
    "i64.atomic.rmw.xchg",
    "i32.atomic.rmw8.xchg_u",
    "i32.atomic.rmw16.xchg_u",
    "i64.atomic.rmw8.xchg_u",
    "i64.atomic.rmw16.xchg_u",
    "i64.atomic.rmw32.xchg_u",
];
const ATOMIC_CMPXCHG_NAMES: [&str; 7] = [
    "i32.atomic.rmw.cmpxchg",
    "i64.atomic.rmw.cmpxchg",
    "i32.atomic.rmw8.cmpxchg_u",
    "i32.atomic.rmw16.cmpxchg_u",
    "i64.atomic.rmw8.cmpxchg_u",
    "i64.atomic.rmw16.cmpxchg_u",
    "i64.atomic.rmw32.cmpxchg_u",
];
const ATOMIC_RMW_NAMES: [[&str; 7]; 5] = [
    [
        "i32.atomic.rmw.add",
        "i64.atomic.rmw.add",
        "i32.atomic.rmw8.add_u",
        "i32.atomic.rmw16.add_u",
        "i64.atomic.rmw8.add_u",
        "i64.atomic.rmw16.add_u",
        "i64.atomic.rmw32.add_u",
    ],
    [
        "i32.atomic.rmw.sub",
        "i64.atomic.rmw.sub",
        "i32.atomic.rmw8.sub_u",
        "i32.atomic.rmw16.sub_u",
        "i64.atomic.rmw8.sub_u",
        "i64.atomic.rmw16.sub_u",
        "i64.atomic.rmw32.sub_u",
    ],
    [
        "i32.atomic.rmw.and",
        "i64.atomic.rmw.and",
        "i32.atomic.rmw8.and_u",
        "i32.atomic.rmw16.and_u",
        "i64.atomic.rmw8.and_u",
        "i64.atomic.rmw16.and_u",
        "i64.atomic.rmw32.and_u",
    ],
    [
        "i32.atomic.rmw.or",
        "i64.atomic.rmw.or",
        "i32.atomic.rmw8.or_u",
        "i32.atomic.rmw16.or_u",
        "i64.atomic.rmw8.or_u",
        "i64.atomic.rmw16.or_u",
        "i64.atomic.rmw32.or_u",
    ],
    [
        "i32.atomic.rmw.xor",
        "i64.atomic.rmw.xor",
        "i32.atomic.rmw8.xor_u",
        "i32.atomic.rmw16.xor_u",
        "i64.atomic.rmw8.xor_u",
        "i64.atomic.rmw16.xor_u",
        "i64.atomic.rmw32.xor_u",
    ],
];

fn width_index(w: crate::binary::instr::AtomicWidth) -> usize {
    use crate::binary::instr::AtomicWidth::*;
    match w {
        I32 => 0,
        I64 => 1,
        I32N8 => 2,
        I32N16 => 3,
        I64N8 => 4,
        I64N16 => 5,
        I64N32 => 6,
    }
}

fn rmw_index(kind: crate::binary::instr::AtomicRmw) -> usize {
    use crate::binary::instr::AtomicRmw::*;
    match kind {
        Add => 0,
        Sub => 1,
        And => 2,
        Or => 3,
        Xor => 4,
        Xchg => ATOMIC_RMW_NAMES.len(), // out of range -> debug assert below
    }
}

fn validate_load(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    load: MemLoadValidation,
) -> Result<(), ValidationError> {
    validate_memarg_align(function, offset, load.op, load.found_align, load.max_align)?;
    validate_memarg_offset(module, load.memory, load.offset_value, function, offset)?;
    let memory_type = resolve_memory_type(module, load.memory, function, offset)?;
    pop_expect(
        module,
        function,
        state,
        ValType::Num(addr_num_type(&memory_type.limits)),
        offset,
        load.op,
    )?;
    push_operand(function, offset, state, OperandType::Typed(load.result))?;
    Ok(())
}

fn validate_store(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    store: MemStoreValidation,
) -> Result<(), ValidationError> {
    validate_memarg_align(
        function,
        offset,
        store.op,
        store.found_align,
        store.max_align,
    )?;
    validate_memarg_offset(module, store.memory, store.offset_value, function, offset)?;
    let memory_type = resolve_memory_type(module, store.memory, function, offset)?;
    pop_expect(module, function, state, store.stored, offset, store.op)?;
    pop_expect(
        module,
        function,
        state,
        ValType::Num(addr_num_type(&memory_type.limits)),
        offset,
        store.op,
    )?;
    Ok(())
}

fn validate_simd_load_lane(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    lane: SimdLaneValidation,
) -> Result<(), ValidationError> {
    validate_memarg_align(function, offset, lane.op, lane.found_align, lane.max_align)?;
    validate_memarg_offset(module, lane.memory, lane.offset_value, function, offset)?;
    validate_simd_lane_idx(function, offset, lane.op, lane.lane, lane.max_lane)?;
    let memory_type = resolve_memory_type(module, lane.memory, function, offset)?;
    pop_expect(
        module,
        function,
        state,
        ValType::Vec(crate::types::VecType::V128),
        offset,
        lane.op,
    )?;
    pop_expect(
        module,
        function,
        state,
        ValType::Num(addr_num_type(&memory_type.limits)),
        offset,
        lane.op,
    )?;
    push_operand(
        function,
        offset,
        state,
        OperandType::Typed(ValType::Vec(crate::types::VecType::V128)),
    )?;
    Ok(())
}

fn validate_simd_store_lane(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    lane: SimdLaneValidation,
) -> Result<(), ValidationError> {
    validate_memarg_align(function, offset, lane.op, lane.found_align, lane.max_align)?;
    validate_memarg_offset(module, lane.memory, lane.offset_value, function, offset)?;
    validate_simd_lane_idx(function, offset, lane.op, lane.lane, lane.max_lane)?;
    let memory_type = resolve_memory_type(module, lane.memory, function, offset)?;
    pop_expect(
        module,
        function,
        state,
        ValType::Vec(crate::types::VecType::V128),
        offset,
        lane.op,
    )?;
    pop_expect(
        module,
        function,
        state,
        ValType::Num(addr_num_type(&memory_type.limits)),
        offset,
        lane.op,
    )?;
    Ok(())
}

fn validate_memarg_align(
    function: FuncIdx,
    offset: usize,
    op: &'static str,
    found: u32,
    max: u32,
) -> Result<(), ValidationError> {
    if found > max {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::InvalidMemArgAlign { op, max, found },
        });
    }

    Ok(())
}

fn validate_simd_lane_idx(
    function: FuncIdx,
    offset: usize,
    op: &'static str,
    found: u8,
    max: u8,
) -> Result<(), ValidationError> {
    if found > max {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::InvalidSimdLaneIdx { op, max, found },
        });
    }

    Ok(())
}

fn resolve_block_type(
    module: &Module<'_>,
    function: FuncIdx,
    block_type: BlockType,
    offset: usize,
) -> Result<FuncType, ValidationError> {
    match block_type {
        BlockType::Empty => Ok(FuncType {
            params: Vec::new(),
            results: Vec::new(),
        }),
        BlockType::Val(val) => {
            validate_valtype_type_indices(module, val, Some(function), offset)?;
            Ok(FuncType {
                params: Vec::new(),
                results: single_valtype(
                    function,
                    offset,
                    "block type",
                    normalize_valtype(module, val),
                )?,
            })
        }
        BlockType::TypeIdx(idx) => {
            let ty = module.func_type_at(idx as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::InvalidBlockType { block_type },
            })?;
            normalize_func_type(module, ty).map_err(|()| ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::ResourceLimitExceeded { what: "block type" },
            })
        }
    }
}

fn resolve_type(
    module: &Module<'_>,
    idx: TypeIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<FuncType, ValidationError> {
    let idx = canonicalize_func_type_idx(module, idx);
    let ty = module.func_type(idx).ok_or(ValidationError {
        offset: ByteOffset(offset),
        function,
        kind: ValidationErrorKind::UnknownTypeIdx { idx },
    })?;
    normalize_func_type(module, ty).map_err(|()| ValidationError {
        offset: ByteOffset(offset),
        function,
        kind: ValidationErrorKind::ResourceLimitExceeded {
            what: "function type",
        },
    })
}

fn resolve_func_type(
    module: &Module<'_>,
    idx: FuncIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<FuncType, ValidationError> {
    let type_idx = resolve_func_type_idx_with_context(module, idx, Some(function), offset)?;
    resolve_type(module, type_idx, Some(function), offset)
}

fn contains_ref_func_expr(
    _module: &Module<'_>,
    expr: &[u8],
    offset: usize,
    target: FuncIdx,
) -> bool {
    decode_instr_sequence_with_offsets(expr, offset)
        .ok()
        .is_some_and(|instrs| {
            instrs
                .into_iter()
                .any(|instr| matches!(instr.instr, Instr::RefFunc(idx) if idx == target))
        })
}

fn is_declared_function_ref(module: &Module<'_>, target: FuncIdx) -> bool {
    if module
        .exports()
        .iter()
        .any(|export| matches!(export.desc, ExportDesc::Func(idx) if idx == target))
    {
        return true;
    }

    if module
        .globals()
        .iter()
        .any(|global| contains_ref_func_expr(module, global.init_expr, global.init_offset, target))
    {
        return true;
    }

    if module.tables.iter().any(|table| {
        table
            .init
            .as_ref()
            .is_some_and(|init| contains_ref_func_expr(module, init, 0, target))
    }) {
        return true;
    }

    module.elements().iter().any(|element| match &element.init {
        ElementInit::FuncIndices(funcs) => funcs.contains(&target),
        ElementInit::Expressions(exprs) => exprs
            .iter()
            .any(|expr| contains_ref_func_expr(module, expr.expr, expr.offset, target)),
    })
}

fn resolve_func_type_idx_for_module(
    module: &Module<'_>,
    idx: FuncIdx,
    offset: usize,
) -> Result<TypeIdx, ValidationError> {
    resolve_func_type_idx_with_context(module, idx, None, offset)
        .map(|idx| canonicalize_func_type_idx(module, idx))
}

fn resolve_func_type_idx(
    module: &Module<'_>,
    idx: FuncIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<TypeIdx, ValidationError> {
    resolve_func_type_idx_with_context(module, idx, Some(function), offset)
        .map(|idx| canonicalize_func_type_idx(module, idx))
}

fn resolve_func_type_for_module<'m>(
    module: &'m Module<'_>,
    idx: FuncIdx,
    offset: usize,
) -> Result<&'m FuncType, ValidationError> {
    resolve_func_type_with_context(module, idx, None, offset)
}

fn resolve_func_type_with_context<'m>(
    module: &'m Module<'_>,
    idx: FuncIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<&'m FuncType, ValidationError> {
    let type_idx = resolve_func_type_idx_with_context(module, idx, function, offset)?;

    module.func_type(type_idx).ok_or(ValidationError {
        offset: ByteOffset(offset),
        function,
        kind: ValidationErrorKind::UnknownTypeIdx { idx: type_idx },
    })
}

fn resolve_func_type_idx_with_context(
    module: &Module<'_>,
    idx: FuncIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<TypeIdx, ValidationError> {
    let imported_funcs = module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Func(type_idx) => Some(type_idx),
            _ => None,
        });
    let defined_funcs = module.functions.iter().copied();

    imported_funcs
        .chain(defined_funcs)
        .nth(idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function,
            kind: ValidationErrorKind::UnknownFuncIdx { idx },
        })
}

fn resolve_global_type(
    module: &Module<'_>,
    idx: GlobalIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<crate::types::GlobalType, ValidationError> {
    resolve_global_type_with_context(module, idx, Some(function), offset)
}

fn resolve_global_type_for_module(
    module: &Module<'_>,
    idx: GlobalIdx,
    offset: usize,
) -> Result<crate::types::GlobalType, ValidationError> {
    resolve_global_type_with_context(module, idx, None, offset)
}

fn resolve_global_type_with_context(
    module: &Module<'_>,
    idx: GlobalIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<crate::types::GlobalType, ValidationError> {
    let imported_count = module
        .imports
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Global(_)))
        .count();
    let defined_count = module.globals.len();
    let available = (imported_count + defined_count) as u32;

    let imported = module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Global(global) => Some(global),
            _ => None,
        });
    let defined = module.globals.iter().map(|global| global.global_type);

    imported
        .chain(defined)
        .nth(idx.0 as usize)
        .map(|global| normalize_global_type(module, global))
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function,
            kind: ValidationErrorKind::UnknownGlobalIdx { idx, available },
        })
}

/// Resolve a tag index to its `(params, results)` func type.
///
/// Index space: imported tags first, then module-defined tags.
fn resolve_tag_type(
    module: &Module<'_>,
    function: FuncIdx,
    idx: TagIdx,
    offset: usize,
) -> Result<(Vec<ValType>, Vec<ValType>), ValidationError> {
    resolve_tag_type_with_context(module, idx, Some(function), offset)
}

/// Resolve a tag index with no enclosing function context (module level).
fn resolve_tag_type_for_module(
    module: &Module<'_>,
    idx: TagIdx,
    offset: usize,
) -> Result<(Vec<ValType>, Vec<ValType>), ValidationError> {
    resolve_tag_type_with_context(module, idx, None, offset)
}

fn resolve_tag_type_with_context(
    module: &Module<'_>,
    idx: TagIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<(Vec<ValType>, Vec<ValType>), ValidationError> {
    let idx_usize = idx.0 as usize;
    let imported = module
        .imports
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Tag(_)))
        .count();
    let type_idx = if idx_usize < imported {
        module
            .imports
            .iter()
            .filter_map(|import| match import.desc {
                ImportDesc::Tag(type_idx) => Some(type_idx),
                _ => None,
            })
            .nth(idx_usize)
            .expect("tag import count checked above")
    } else {
        let defined = idx_usize - imported;
        match module.tags.get(defined) {
            Some(type_idx) => *type_idx,
            None => {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function,
                    kind: ValidationErrorKind::UnknownTag { idx: idx.0 },
                });
            }
        }
    };

    let func_type = resolve_type(module, type_idx, function, offset)?;
    Ok((func_type.params, func_type.results))
}

fn resolve_table_type_for_module(
    module: &Module<'_>,
    idx: TableIdx,
    offset: usize,
) -> Result<crate::types::TableType, ValidationError> {
    resolve_table_type_with_context(module, idx, None, offset)
}

fn resolve_table_type_with_context(
    module: &Module<'_>,
    idx: TableIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<crate::types::TableType, ValidationError> {
    let imported_count = module
        .imports
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Table(_)))
        .count();
    let defined_count = module.tables.len();
    let available = (imported_count + defined_count) as u32;

    let imported = module
        .imports
        .iter()
        .filter_map(|import| match &import.desc {
            ImportDesc::Table(table) => Some(table),
            _ => None,
        });
    let defined = module.tables.iter();

    imported
        .chain(defined)
        .nth(idx.0 as usize)
        .map(|table| {
            Ok(normalize_table_type(
                module,
                clone_table_type_fallible(table, function, offset)?,
            ))
        })
        .unwrap_or_else(|| {
            Err(ValidationError {
                offset: ByteOffset(offset),
                function,
                kind: ValidationErrorKind::UnknownTableIdx { idx, available },
            })
        })
}

fn resolve_data_segment<'a>(
    module: &'a Module<'a>,
    idx: DataIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<&'a crate::types::DataSegment<'a>, ValidationError> {
    let available = module.data().len() as u32;
    module.data().get(idx.0 as usize).ok_or(ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::UnknownDataIdx { idx, available },
    })
}

fn resolve_element_segment<'a>(
    module: &'a Module<'a>,
    idx: ElemIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<&'a crate::types::ElementSegment<'a>, ValidationError> {
    let available = module.elements().len() as u32;
    module
        .elements()
        .get(idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownElemIdx { idx, available },
        })
}

fn resolve_memory_type(
    module: &Module<'_>,
    idx: MemIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<crate::types::MemType, ValidationError> {
    resolve_memory_type_with_context(module, idx, Some(function), offset)
}

fn resolve_memory_type_for_module(
    module: &Module<'_>,
    idx: MemIdx,
    offset: usize,
) -> Result<crate::types::MemType, ValidationError> {
    resolve_memory_type_with_context(module, idx, None, offset)
}

fn resolve_memory_type_with_context(
    module: &Module<'_>,
    idx: MemIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<crate::types::MemType, ValidationError> {
    let imported_count = module
        .imports
        .iter()
        .filter(|import| matches!(import.desc, ImportDesc::Mem(_)))
        .count();
    let defined_count = module.memories.len();
    let available = (imported_count + defined_count) as u32;

    let imported = module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Mem(memory) => Some(memory),
            _ => None,
        });
    let defined = module.memories.iter().copied();

    imported
        .chain(defined)
        .nth(idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function,
            kind: ValidationErrorKind::UnknownMemIdx { idx, available },
        })
}

fn validate_label(
    function: FuncIdx,
    state: &ValidationState,
    label: crate::types::LabelIdx,
    offset: usize,
) -> Result<&[ValType], ValidationError> {
    state.current_label_types(label.0).ok_or(ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::UnknownLabelIdx { idx: label },
    })
}

/// Validate one catch clause of a `try_table`.
///
/// Labels resolve in the CURRENT frame stack — before the `try_table`'s own
/// frame is pushed (the reference checks clauses in the outer context `c`).
fn validate_catch(
    module: &Module<'_>,
    function: FuncIdx,
    state: &ValidationState,
    catch: &TryTableCatch,
    offset: usize,
) -> Result<(), ValidationError> {
    let label = crate::types::LabelIdx(catch.label);
    let label_types = copy_valtypes_fallible(
        function,
        offset,
        "catch label types",
        validate_label(function, state, label, offset)?,
    )?;
    let expected: Vec<ValType> = match catch.kind {
        TryTableCatchKind::Catch | TryTableCatchKind::CatchRef => {
            let tag = catch
                .tag
                .expect("decoder guarantees a tag for catch/catch_ref");
            let (params, _results) = resolve_tag_type(module, function, tag, offset)?;
            let mut expected = params;
            if matches!(catch.kind, TryTableCatchKind::CatchRef) {
                // The appended `exnref` is reserved fallibly rather than left
                // to an infallible `Vec` growth (resource discipline).
                expected.try_reserve(1).map_err(|_| ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ResourceLimitExceeded {
                        what: "catch clause types",
                    },
                })?;
                expected.push(ValType::Ref(RefType::from_parts(false, HeapType::Exn)));
            }
            expected
        }
        TryTableCatchKind::CatchAll => Vec::new(),
        TryTableCatchKind::CatchAllRef => single_valtype(
            function,
            offset,
            "catch clause types",
            ValType::Ref(RefType::from_parts(false, HeapType::Exn)),
        )?,
    };

    if label_types.len() != expected.len()
        || !label_types
            .iter()
            .zip(expected.iter())
            // Subtyping direction per the reference `match_result_type`:
            // each clause-expected type must be a SUBTYPE of the label's
            // declared type (e.g. non-null `(ref exn)` lands on a label
            // declared with the nullable `exnref` shorthand). This match is
            // module-aware so concrete reference types resolve through the
            // type section's declared supertypes.
            .all(|(declared, want)| valtype_matches(module, *want, *declared))
    {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::BranchTypeMismatch {
                label,
                expected,
                found: label_types,
            },
        });
    }

    Ok(())
}

/// Push a control frame, mapping a refused reservation into a clean
/// [`ValidationErrorKind::ResourceLimitExceeded`].
#[allow(clippy::too_many_arguments)]
fn push_control_frame(
    function: FuncIdx,
    offset: usize,
    what: &'static str,
    state: &mut ValidationState,
    kind: ControlKind,
    block_type: BlockType,
    start_types: Vec<ValType>,
    end_types: Vec<ValType>,
) -> Result<(), ValidationError> {
    if state.push_frame(kind, block_type, start_types, end_types) {
        Ok(())
    } else {
        Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ResourceLimitExceeded { what },
        })
    }
}

fn finish_frame(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    // Read the frame's fixed-size metadata without cloning its module-sized
    // vectors (the previous `ControlFrame::clone` scaled with result arity and
    // terminated on allocator refusal). `end_types` is then moved out of the
    // frame so the operand pops — which need `&mut state` — can read it
    // without an ownership copy; nothing on the pop path reads the frame's
    // `end_types`.
    let (kind, has_else, outer_height, types_differ) = {
        let frame = state.current_frame();
        (
            frame.kind,
            frame.has_else,
            frame.outer_height,
            frame.start_types != frame.end_types,
        )
    };

    if kind == Function {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnexpectedEnd,
        });
    }

    // An `if` without `else` is valid only when the if's parameter and
    // result types are identical (the implicit else is the identity).
    if kind == If && !has_else && types_differ {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::MissingElseForResult,
        });
    }

    let end_types = core::mem::take(&mut state.current_frame_mut().end_types);

    if let Err(found) = ensure_frame_end_types(module, state, outer_height, &end_types) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ControlResultTypeMismatch {
                expected: end_types,
                found,
            },
        });
    }

    pop_control_result_types(module, function, state, &end_types, offset)?;
    let frame = state.pop_frame().ok_or(ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::UnexpectedEnd,
    })?;
    state.operands.truncate(outer_height);
    state.local_inits = frame.local_inits;
    // The result operands were popped just above, so this normally reuses the
    // freed capacity; the reservation exists so a refused re-growth is a
    // clean resource error rather than an abort.
    if !state.operands.try_reserve(end_types.len()) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "control frame result operands",
            },
        });
    }
    for ty in end_types {
        push_operand(function, offset, state, OperandType::Typed(ty))?;
    }
    // Restore the ENCLOSING frame's reachability: a block that ends inside
    // unreachable code leaves its parent unreachable (the parent's operand
    // floor is then polymorphic again once the block's results are consumed).
    state.reachability = if frame.entry_unreachable {
        Reachability::Unreachable
    } else {
        Reachability::Reachable
    };
    Ok(())
}

fn canonicalize_func_type_idx(module: &Module<'_>, idx: TypeIdx) -> TypeIdx {
    match module.type_canon.canonical_id(idx) {
        Some(canonical) => TypeIdx(canonical),
        None => idx,
    }
}

/// Module-aware reference-value subtyping (`match_valtype`, match.ml:128-159)
/// used by type-section validation (comptype matching) and declared-supertype
/// equality.
fn valtype_matches_type(module: &Module<'_>, found: ValType, expected: ValType) -> bool {
    match (found, expected) {
        (ValType::Num(found), ValType::Num(expected)) => found == expected,
        (ValType::Vec(found), ValType::Vec(expected)) => found == expected,
        (ValType::Ref(found), ValType::Ref(expected)) => {
            reftype_matches_type(module, found, expected)
        }
        _ => false,
    }
}

fn resulttype_matches(module: &Module<'_>, found: &[ValType], expected: &[ValType]) -> bool {
    found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|(found, expected)| valtype_matches_type(module, *found, *expected))
}

fn reftype_matches_type(module: &Module<'_>, found: RefType, expected: RefType) -> bool {
    (!found.is_nullable() || expected.is_nullable())
        && heaptype_matches(module, found.heap_type(), expected.heap_type())
}

fn heaptype_matches(module: &Module<'_>, found: HeapType, expected: HeapType) -> bool {
    use HeapType::*;
    match (found, expected) {
        (Eq | Struct | Array | I31, Any) => true,
        (I31 | Struct | Array, Eq) => true,
        (None, t) => heaptype_matches(module, t, Any),
        (NoFunc, t) => heaptype_matches(module, t, Func),
        (NoExn, t) => heaptype_matches(module, t, Exn),
        (NoExtern, t) => heaptype_matches(module, t, Extern),
        (Type(found), Type(expected)) => deftype_matches(module, found, expected),
        (Type(found), t) => concrete_heap_matches(module, found, t),
        _ => found == expected,
    }
}

fn concrete_heap_matches(module: &Module<'_>, idx: TypeIdx, expected: HeapType) -> bool {
    use HeapType::*;
    match module.types.get(idx.0 as usize).map(|sub| &sub.comp) {
        Some(crate::types::CompType::Struct(_)) => matches!(expected, Any | Eq | Struct),
        Some(crate::types::CompType::Array(_)) => matches!(expected, Any | Eq | Array),
        Some(crate::types::CompType::Func(_)) => matches!(expected, Func),
        _ => false,
    }
}

/// `match_deftype` (match.ml:151-159): canonical equality of rolled closed
/// forms, or the declared supertype chain of `found` contains `expected`.
fn deftype_matches(module: &Module<'_>, found: TypeIdx, expected: TypeIdx) -> bool {
    if module.type_canon.equivalent(found, expected) {
        return true;
    }
    match module.types.get(found.0 as usize) {
        Some(sub) => sub
            .supertypes
            .iter()
            .any(|&supertype| deftype_matches(module, TypeIdx(supertype), expected)),
        None => false,
    }
}

fn storage_matches(
    module: &Module<'_>,
    found: crate::types::StorageType,
    expected: crate::types::StorageType,
) -> bool {
    use crate::types::StorageType::*;
    match (found, expected) {
        (Val(found), Val(expected)) => valtype_matches_type(module, found, expected),
        (I8, I8) | (I16, I16) => true,
        _ => false,
    }
}

fn field_matches(
    module: &Module<'_>,
    found: crate::types::FieldType,
    expected: crate::types::FieldType,
) -> bool {
    if found.mutable != expected.mutable
        || !storage_matches(module, found.storage, expected.storage)
    {
        return false;
    }
    // Mutable fields are invariant; const fields are covariant.
    !found.mutable || storage_matches(module, expected.storage, found.storage)
}

/// `match_comptype` (match.ml:140-149).
fn comptype_matches(
    module: &Module<'_>,
    found: &crate::types::CompType,
    expected: &crate::types::CompType,
) -> bool {
    use crate::types::CompType::*;
    match (found, expected) {
        (Struct(found), Struct(expected)) => {
            found.len() >= expected.len()
                && found
                    .iter()
                    .zip(expected)
                    .all(|(found, expected)| field_matches(module, *found, *expected))
        }
        (Array(found), Array(expected)) => field_matches(module, *found, *expected),
        (Func(found), Func(expected)) => {
            // Params are contravariant, results covariant.
            resulttype_matches(module, &expected.params, &found.params)
                && resulttype_matches(module, &found.results, &expected.results)
        }
        _ => false,
    }
}

fn validate_valtype_type_indices(
    module: &Module<'_>,
    ty: ValType,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<(), ValidationError> {
    if let ValType::Ref(ref_type) = ty {
        validate_reftype_type_indices(module, ref_type, function, offset)?;
    }
    Ok(())
}

fn validate_reftype_type_indices(
    module: &Module<'_>,
    ty: RefType,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<(), ValidationError> {
    if let crate::types::HeapType::Type(idx) = ty.heap_type()
        && module.types.get(idx.0 as usize).is_none()
    {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function,
            kind: ValidationErrorKind::UnknownTypeIdx { idx },
        });
    }

    Ok(())
}

fn normalize_reftype(module: &Module<'_>, ty: RefType) -> RefType {
    match ty.heap_type() {
        crate::types::HeapType::Type(idx) => RefType::from_parts(
            ty.is_nullable(),
            crate::types::HeapType::Type(canonicalize_func_type_idx(module, idx)),
        ),
        _ => ty,
    }
}

fn normalize_valtype(module: &Module<'_>, ty: ValType) -> ValType {
    match ty {
        ValType::Ref(ref_type) => ValType::Ref(normalize_reftype(module, ref_type)),
        _ => ty,
    }
}

/// Normalize a function type's params/results, reserving both vectors
/// fallibly: they scale with module-controlled arity and must not abort on
/// allocator refusal.
fn normalize_func_type(module: &Module<'_>, ty: &FuncType) -> Result<FuncType, ()> {
    let mut params = Vec::new();
    params.try_reserve(ty.params.len()).map_err(|_| ())?;
    params.extend(
        ty.params
            .iter()
            .copied()
            .map(|ty| normalize_valtype(module, ty)),
    );
    let mut results = Vec::new();
    results.try_reserve(ty.results.len()).map_err(|_| ())?;
    results.extend(
        ty.results
            .iter()
            .copied()
            .map(|ty| normalize_valtype(module, ty)),
    );
    Ok(FuncType { params, results })
}

fn normalize_global_type(
    module: &Module<'_>,
    ty: crate::types::GlobalType,
) -> crate::types::GlobalType {
    crate::types::GlobalType {
        val_type: normalize_valtype(module, ty.val_type),
        mutability: ty.mutability,
    }
}

fn normalize_table_type(
    module: &Module<'_>,
    ty: crate::types::TableType,
) -> crate::types::TableType {
    crate::types::TableType {
        elem: normalize_reftype(module, ty.elem),
        limits: ty.limits,
        // The initializer expression is moved: callers obtain an owned table
        // type through [`clone_table_type_fallible`], which reserves the
        // expression copy fallibly.
        init: ty.init,
    }
}

/// Fallibly clone a table type: its initializer expression (module-controlled
/// bytes) is reserved before it is filled, so a refused allocation surfaces as
/// a clean `ResourceLimitExceeded` rather than an abort.
fn clone_table_type_fallible(
    table: &crate::types::TableType,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<crate::types::TableType, ValidationError> {
    let init = match &table.init {
        Some(expr) => {
            let mut copy = Vec::new();
            copy.try_reserve(expr.len()).map_err(|_| ValidationError {
                offset: ByteOffset(offset),
                function,
                kind: ValidationErrorKind::ResourceLimitExceeded {
                    what: "table initializer expression",
                },
            })?;
            copy.extend_from_slice(expr);
            Some(copy)
        }
        None => None,
    };
    Ok(crate::types::TableType {
        elem: table.elem,
        limits: table.limits,
        init,
    })
}

fn reftype_matches(module: &Module<'_>, found: RefType, expected: RefType) -> bool {
    reftype_matches_type(module, found, expected)
}

/// The top of a heap type's hierarchy (`valid/match.ml` `top_of_heaptype`):
/// the four hierarchy tops any / func / exn / extern. Concrete struct and
/// array types sit under `any`; concrete function types under `func`.
fn top_of_heaptype(module: &Module<'_>, ht: HeapType) -> HeapType {
    use HeapType::*;
    match ht {
        Any | None | Eq | Struct | Array | I31 => Any,
        Func | NoFunc => Func,
        Exn | NoExn => Exn,
        Extern | NoExtern => Extern,
        Type(idx) => match crate::types::concrete_kind(&module.types, idx) {
            Some(crate::types::ConcreteKind::Func) => Func,
            _ => Any,
        },
    }
}

/// `diff_reftype` (`valid/valid.ml:234`): the source heaptype with the
/// target's nullness stripped.
fn diff_reftype(src: RefType, tgt: RefType) -> RefType {
    if tgt.is_nullable() {
        src.as_non_null()
    } else {
        src
    }
}

/// Pop a reference operand whose heap type is a subtype of `(ref null heap)`
/// and return the operand's nullness (mirrors the reference's `peek_ref`).
fn pop_ref_expect_heap(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    heap: HeapType,
    offset: usize,
    op: &'static str,
) -> Result<bool, ValidationError> {
    let expected = ValType::Ref(RefType::from_parts(true, heap));
    let found = pop_operand(function, state, offset, op, &[expected])?;
    if !operand_matches(module, found, expected) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::TypeMismatch {
                op,
                expected,
                found: operand_to_valtype(found, expected),
            },
        });
    }
    Ok(match found {
        OperandType::Typed(ValType::Ref(ref_type)) => ref_type.is_nullable(),
        _ => false,
    })
}

/// Push an operand onto the validation operand stack, mapping a refused
/// reservation into a clean [`ValidationErrorKind::ResourceLimitExceeded`]
/// (resource discipline: never an allocation abort).
fn push_operand(
    function: FuncIdx,
    offset: usize,
    state: &mut ValidationState,
    operand: OperandType,
) -> Result<(), ValidationError> {
    if state.operands.try_push(operand) {
        Ok(())
    } else {
        Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "operand stack",
            },
        })
    }
}

/// Fallibly build a single-element value-type vector (a `vec![ty]` bootstrap is
/// an infallible allocation that aborts on allocator refusal).
fn single_valtype(
    function: FuncIdx,
    offset: usize,
    what: &'static str,
    ty: ValType,
) -> Result<Vec<ValType>, ValidationError> {
    let mut out = Vec::new();
    out.try_reserve(1).map_err(|_| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::ResourceLimitExceeded { what },
    })?;
    out.push(ty);
    Ok(out)
}

/// Fallibly copy a slice of value types (mirrors the runtime's
/// try_reserve-then-fill discipline). A refused reservation is a clean
/// [`ValidationErrorKind::ResourceLimitExceeded`], never a process abort.
fn copy_valtypes_fallible(
    function: FuncIdx,
    offset: usize,
    what: &'static str,
    types: &[ValType],
) -> Result<Vec<ValType>, ValidationError> {
    let mut out = Vec::new();
    out.try_reserve(types.len()).map_err(|_| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::ResourceLimitExceeded { what },
    })?;
    out.extend_from_slice(types);
    Ok(out)
}

/// `br_on_cast` / `br_on_cast_fail` (`valid/valid.ml:492-519`).
#[allow(clippy::too_many_arguments)]
fn validate_br_on_cast(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    fail: bool,
    src: RefType,
    tgt: RefType,
    label: crate::types::LabelIdx,
    offset: usize,
) -> Result<(), ValidationError> {
    validate_reftype_type_indices(module, src, Some(function), offset)?;
    validate_reftype_type_indices(module, tgt, Some(function), offset)?;
    // rt2 <: rt1.
    if !reftype_matches(module, tgt, src) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::TypeMismatch {
                op: "br_on_cast",
                expected: ValType::Ref(src),
                found: ValType::Ref(tgt),
            },
        });
    }

    let label_types = copy_valtypes_fallible(
        function,
        offset,
        "br_on_cast label types",
        validate_label(function, state, label, offset)?,
    )?;
    let branch_ref = if fail { diff_reftype(src, tgt) } else { tgt };
    let last_ok = label_types
        .split_last()
        .is_some_and(|(last, _)| valtype_matches(module, ValType::Ref(branch_ref), *last));
    if !last_ok {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::BranchTypeMismatch {
                label,
                expected: copy_valtypes_fallible(
                    function,
                    offset,
                    "br_on_cast expected type",
                    &[ValType::Ref(branch_ref)],
                )?,
                found: label_types,
            },
        });
    }

    // The input is `ts0 @ [RefT rt1]`; the ref is popped first, then the
    // leading types (which remain on the stack afterwards).
    pop_expect(
        module,
        function,
        state,
        ValType::Ref(src),
        offset,
        "br_on_cast",
    )?;
    let (_, leading) = label_types.split_last().expect("label non-empty checked");
    let leading = copy_valtypes_fallible(function, offset, "br_on_cast leading types", leading)?;
    // The pops below free at least this much capacity, so this reservation
    // normally reuses it; it exists so a refused operand-stack growth is a
    // clean resource error rather than an abort.
    if !state.operands.try_reserve(leading.len() + 1) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ResourceLimitExceeded {
                what: "br_on_cast operand stack",
            },
        });
    }
    pop_branch_types(module, function, state, label, &leading, offset)?;
    for ty in leading {
        push_operand(function, offset, state, OperandType::Typed(ty))?;
    }
    let fall = if fail { tgt } else { diff_reftype(src, tgt) };
    push_operand(
        function,
        offset,
        state,
        OperandType::Typed(ValType::Ref(fall)),
    )?;
    Ok(())
}

fn valtype_matches(module: &Module<'_>, found: ValType, expected: ValType) -> bool {
    valtype_matches_type(module, found, expected)
}

fn valtype_vec_matches(module: &Module<'_>, found: &[ValType], expected: &[ValType]) -> bool {
    found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|(found, expected)| valtype_matches(module, *found, *expected))
}

fn is_stack_polymorphic(state: &ValidationState) -> bool {
    state.reachability == Reachability::Unreachable
        && state.operands.len() <= state.current_frame().outer_height
}

fn operand_matches(module: &Module<'_>, found: OperandType, expected: ValType) -> bool {
    match found {
        OperandType::Typed(found) => valtype_matches(module, found, expected),
        OperandType::Bottom => true,
    }
}

fn operand_to_valtype(found: OperandType, fallback: ValType) -> ValType {
    match found {
        OperandType::Typed(found) => found,
        OperandType::Bottom => fallback,
    }
}

fn concrete_stack(state: &ValidationState) -> Vec<ValType> {
    concrete_operands(state.operands.as_slice())
}

fn concrete_operands(operands: &[OperandType]) -> Vec<ValType> {
    operands
        .iter()
        .filter_map(|operand| match operand {
            OperandType::Typed(ty) => Some(*ty),
            OperandType::Bottom => None,
        })
        .collect()
}

fn stack_found(actual: &[OperandType], expected: &[ValType]) -> Vec<ValType> {
    let expected_start = expected.len().saturating_sub(actual.len());
    actual
        .iter()
        .enumerate()
        .filter_map(|(idx, operand)| match operand {
            OperandType::Typed(ty) => Some(*ty),
            OperandType::Bottom => expected.get(expected_start + idx).copied(),
        })
        .collect()
}

fn ensure_frame_end_types(
    module: &Module<'_>,
    state: &ValidationState,
    outer_height: usize,
    expected: &[ValType],
) -> Result<(), Vec<ValType>> {
    let operands = state.operands.as_slice();
    if operands.len() < outer_height {
        return Err(Vec::new());
    }
    let actual = &operands[outer_height..];

    // Compute the `found` vector only on the failure path: `stack_found`
    // allocates a module-scaled vector (arity = operand count), so calling it
    // on the success path would abort on allocator refusal for a valid
    // module.
    if actual.len() > expected.len() {
        return Err(stack_found(actual, expected));
    }

    let expected_suffix = &expected[expected.len().saturating_sub(actual.len())..];
    if actual
        .iter()
        .zip(expected_suffix)
        .all(|(found, expected)| operand_matches(module, *found, *expected))
        && (state.reachability == Reachability::Unreachable || actual.len() == expected.len())
    {
        Ok(())
    } else {
        Err(stack_found(actual, expected))
    }
}

/// Pop one operand, bounded by the CURRENT control frame's floor.
///
/// Review r3 P2: the spec's validation algorithm (appendix) which
/// compares the operand stack height against the current control frame's
/// height rather than the stack's absolute emptiness:
///
/// ```text
/// func pop_val() : val_type =
///   if (vals.size() = ctrls[0].val_height && ctrls[0].unreachable) return Bot
///   error_if(vals.size() = ctrls[0].val_height)
///   return vals.pop()
/// ```
///
/// (verbatim from `document/core/appendix/algorithm.rst:181-185` at this
/// pin — the interpreter's `valid.ml` implements the same rule inline
/// with frame-local checking but defines no `pop_opd`; the earlier
/// attribution here was wrong, caught in review r4)
///
/// Operands below the current frame's `outer_height` belong to ENCLOSING
/// frames and are never this frame's to pop — not even when the shared operand
/// vector happens to be long enough. Skipping that rule let a REACHABLE frame
/// silently consume an enclosing frame's operand:
///
/// ```wat
/// (module (func i32.const 7 i32.const 8 block drop i32.const 9 end drop drop))
/// ```
///
/// The block's floor is 2, its `drop` took `vec[1]`, `i32.const 9` restored the
/// length to 2, and `end`'s (frame-local) result check saw an empty frame-local
/// slice against empty results — so the invalid module validated. The pin
/// rejects it ("type mismatch: expected a type but nothing on stack").
///
/// At (or below) the floor, an UNREACHABLE frame is polymorphic: the pinned
/// algorithm yields `Bot`, so popping below the floor in unreachable code stays
/// legal and never consumes an enclosing operand.
fn pop_operand(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    expected: &[ValType],
) -> Result<OperandType, ValidationError> {
    let outer_height = state.current_frame().outer_height;
    if state.operands.len() <= outer_height {
        // At (or below) the current frame's floor.
        if state.reachability == Reachability::Unreachable {
            return Ok(OperandType::Bottom);
        }
        return Err(underflow_error(function, state, op, expected, offset));
    }

    match state.operands.pop() {
        Some(found) => Ok(found),
        None => Err(underflow_error(function, state, op, expected, offset)),
    }
}

fn underflow_error(
    function: FuncIdx,
    state: &ValidationState,
    op: &'static str,
    expected: &[ValType],
    offset: usize,
) -> ValidationError {
    // Report the CURRENT frame's operands as available: the floor bounds this
    // frame, so enclosing frames' values are never reachable from here and
    // must not be named as candidates.
    let outer_height = state.current_frame().outer_height;
    let available = concrete_operands(state.operands.as_slice().get(outer_height..).unwrap_or(&[]));
    ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::StackUnderflow {
            op,
            expected: expected.to_vec(),
            available,
        },
    }
}

fn pop_expect(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    expected: ValType,
    offset: usize,
    op: &'static str,
) -> Result<(), ValidationError> {
    let found = pop_operand(function, state, offset, op, &[expected])?;
    if !operand_matches(module, found, expected) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::TypeMismatch {
                op,
                expected,
                found: operand_to_valtype(found, expected),
            },
        });
    }
    Ok(())
}

fn pop_exact(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    expected: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    for expected_ty in expected.iter().rev() {
        pop_expect(module, function, state, *expected_ty, offset, "stack")?;
    }
    Ok(())
}

fn validate_tail_call_results(
    module: &Module<'_>,
    function: FuncIdx,
    state: &ValidationState,
    found: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    let expected = &state.controls[0].end_types;
    if !valtype_vec_matches(module, found, expected) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ResultTypeMismatch {
                expected: expected.clone(),
                found: found.to_vec(),
            },
        });
    }

    Ok(())
}

fn pop_branch_types(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    label: crate::types::LabelIdx,
    expected: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    ensure_stack_types(module, state, expected).map_err(|found| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::BranchTypeMismatch {
            label,
            expected: expected.to_vec(),
            found,
        },
    })?;
    pop_exact(module, function, state, expected, offset)
}

fn pop_control_result_types(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    expected: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    ensure_stack_types(module, state, expected).map_err(|found| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::ControlResultTypeMismatch {
            expected: expected.to_vec(),
            found,
        },
    })?;
    pop_exact(module, function, state, expected, offset)
}

/// Check that the operands the current frame has produced are compatible with
/// `expected` (a block's end types, a label's branch types, or the function's
/// result types for `return`).
///
/// Review r2 P1: this check is FRAME-LOCAL. The operand stack below the
/// current frame's `outer_height` belongs to enclosing frames; an enclosing
/// frame's operand is never a candidate for this frame's missing result. The
/// reviewer's minimized case
///
/// ```wat
/// (module (func
///   i32.const 1
///   block (result i64 f32)
///     unreachable
///     f32.const 0
///   end
///   drop drop drop))
/// ```
///
/// is valid (the block's missing `i64` is polymorphic because the block body
/// is unreachable) but a whole-stack read matched the enclosing `i32` against
/// the expected `i64` and reported `ControlResultTypeMismatch`. Operands
/// missing from an unreachable frame stay polymorphic — compatible with
/// anything — which is what `found_len` being clipped to the frame-local
/// length below encodes; the arity requirement applies only while reachable.
fn ensure_stack_types(
    module: &Module<'_>,
    state: &ValidationState,
    expected: &[ValType],
) -> Result<(), Vec<ValType>> {
    if is_stack_polymorphic(state) {
        return Ok(());
    }

    let outer_height = state.current_frame().outer_height;
    let operands = state.operands.as_slice();
    let actual = operands.get(outer_height..).unwrap_or(&[]);
    let found_len = core::cmp::min(actual.len(), expected.len());
    let found = &actual[actual.len().saturating_sub(found_len)..];
    let expected_suffix = &expected[expected.len() - found_len..];

    if (state.reachability == Reachability::Reachable && actual.len() < expected.len())
        || !found
            .iter()
            .zip(expected_suffix)
            .all(|(found, expected)| operand_matches(module, *found, *expected))
    {
        return Err(found
            .iter()
            .zip(expected_suffix)
            .map(|(found, expected)| operand_to_valtype(*found, *expected))
            .collect());
    }

    Ok(())
}

fn pop_operand_type(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
) -> Result<OperandType, ValidationError> {
    pop_operand(function, state, offset, op, &[])
}

fn pop_ref_type(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
) -> Result<Option<RefType>, ValidationError> {
    let found = pop_operand_type(function, state, offset, op)?;
    match found {
        OperandType::Bottom => Ok(None),
        OperandType::Typed(ValType::Ref(ref_type)) => Ok(Some(ref_type)),
        OperandType::Typed(found) => Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::TypeMismatch {
                op,
                expected: ValType::Ref(RefType::ExternRef),
                found,
            },
        }),
    }
}

fn pop_any(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
) -> Result<(), ValidationError> {
    pop_operand(function, state, offset, op, &[]).map(|_| ())
}

fn expand_locals(
    module: &Module<'_>,
    function: FuncIdx,
    locals: &mut Vec<ValType>,
    local_decls: &[LocalDecl],
    offset: usize,
) -> Result<(), ValidationError> {
    for decl in local_decls {
        validate_valtype_type_indices(module, decl.val_type, Some(function), offset)?;
        if locals.try_reserve(decl.count as usize).is_err() {
            return Err(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::ResourceLimitExceeded {
                    what: "function locals",
                },
            });
        }
        for _ in 0..decl.count {
            locals.push(normalize_valtype(module, decl.val_type));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::module::Module;
    use crate::types::{LabelIdx, LocalIdx};

    #[test]
    fn validate_simple_add_module() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7F,
            0x7F, 0x01, 0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00,
            0x20, 0x01, 0x6A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_unknown_local_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x06, 0x01, 0x04, 0x00, 0x20, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownLocalIdx { .. }
        ));
        assert_eq!(err.offset, ByteOffset(24));
    }

    #[test]
    fn validate_forward_mutual_recursion() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/forward-mutual-recursion.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_local_get_after_set_for_non_defaultable_local() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/local-init-get-after-set.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_local_get_after_tee_for_non_defaultable_local() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/local-init-get-after-tee.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_local_get_in_block_after_set_for_non_defaultable_local() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/local-init-get-in-block-after-set.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_local_tee_init_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/local-init-tee-init.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_uninitialized_non_defaultable_local() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/local-init-uninitialized-local.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UninitializedLocal { idx: LocalIdx(0) }
        ));
    }

    #[test]
    fn reject_non_defaultable_local_initialized_only_inside_block() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/local-init-uninitialized-after-end.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(40));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UninitializedLocal { idx: LocalIdx(1) }
        ));
    }

    #[test]
    fn reject_non_defaultable_local_get_in_else_without_prior_init() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/local-init-uninitialized-in-else.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(37));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UninitializedLocal { idx: LocalIdx(1) }
        ));
    }

    #[test]
    fn reject_non_defaultable_local_init_not_escaping_if() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/local-init-uninitialized-from-if.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(42));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UninitializedLocal { idx: LocalIdx(1) }
        ));
    }

    #[test]
    fn validate_unreached_call_ref() {
        let bytes = include_bytes!("../../../baedeker-testdata/spec/valid/unreached-call-ref.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_after_unreachable_with_bottom_operands() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreached-valid-select-after-unreachable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreached_core_stack_polymorphism_cases() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/unreached-valid-core.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreached_bottom_heap_type_cases() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/unreached-bottom-heap-type.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreached_meet_bottom_br_table() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/unreached-meet-bottom.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreached_select_i64_result_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreached-valid-select-i64-result.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_unreached_select_result_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/unreached-select-result-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(30));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, .. }
                if expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && found == vec![ValType::Num(crate::types::NumType::I64)]
        ));
    }

    #[test]
    fn reject_unreached_unconsumed_const() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/unreached-unconsumed-const.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, .. }
                if expected.is_empty()
        ));
    }

    #[test]
    fn reject_unknown_local_index_in_unreachable_code() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/unreached-unknown-local-index.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownLocalIdx { idx: LocalIdx(0) }
        ));
    }

    #[test]
    fn reject_unknown_global_index_in_unreachable_code() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/unreached-unknown-global-index.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownGlobalIdx {
                idx: GlobalIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn reject_unknown_function_index_in_unreachable_code() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/unreached-unknown-function-index.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownFuncIdx { idx: FuncIdx(1) }
        ));
    }

    #[test]
    fn reject_unknown_label_index_in_unreachable_code() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/unreached-unknown-label-index.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownLabelIdx { idx: LabelIdx(1) }
        ));
    }

    #[test]
    fn validate_unreachable_function_end_with_result_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x05, 0x01, 0x03, 0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn report_stack_underflow_with_operation_context() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x05, 0x01, 0x03, 0x00, 0x6A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(23));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::StackUnderflow { op, expected, available }
                if op == "i32.binary"
                    && expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && available.is_empty()
        ));
    }

    #[test]
    fn report_type_mismatch_with_operation_context() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x42, 0x01, 0x41, 0x02, 0x6A,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(27));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "i32.binary"
                    && expected == ValType::Num(crate::types::NumType::I32)
                    && found == ValType::Num(crate::types::NumType::I64)
        ));
    }

    #[test]
    fn report_precise_offset_for_later_instruction() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20,
            0x01, 0x6A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownLocalIdx { .. }
        ));
        assert_eq!(err.offset, ByteOffset(27));
    }

    #[test]
    fn reject_unknown_function_type_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x03, 0x02, 0x01,
            0x01, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { .. }
        ));
    }

    #[test]
    fn reject_import_with_unknown_type_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x02, 0x0D, 0x01, 0x04, b't', b'e', b's', b't', 0x04, b'f', b'u', b'n', b'c',
            0x00, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx {
                idx: crate::types::TypeIdx(1)
            }
        ));
    }

    #[test]
    fn reject_typed_function_type_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x63,
            0x01, 0x00, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0x20, 0x00, 0x1A,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(10));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_imported_typed_global_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x02, 0x0B, 0x01, 0x03, 0x65, 0x6E, 0x76, 0x01, 0x67, 0x03, 0x63, 0x01, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(16));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_typed_table_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x04, 0x05, 0x01, 0x63, 0x01, 0x00, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(16));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_typed_element_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x09, 0x08, 0x01, 0x05, 0x63, 0x01, 0x01, 0xD0, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(16));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_typed_local_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x01, 0x01, 0x63, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_ref_null_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0xD0, 0x01, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(23));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_block_result_with_unknown_concrete_type_idx() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0B, 0x01, 0x09, 0x00, 0x02, 0x63, 0x01, 0xD0, 0x01,
            0x1A, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(23));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn reject_forward_type_reference_outside_rec_group() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/type-forward-ref-outside-rec-group.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(10));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTypeIdx { idx: TypeIdx(1) }
        ));
    }

    #[test]
    fn report_function_result_stack_suffix() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x08, 0x01, 0x06, 0x00, 0x41, 0x01, 0x41, 0x02,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(28));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, full_stack }
                if expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && found == vec![ValType::Num(crate::types::NumType::I32)]
                    && full_stack == vec![
                        ValType::Num(crate::types::NumType::I32),
                        ValType::Num(crate::types::NumType::I32),
                    ]
        ));
    }

    #[test]
    fn validate_unreachable_stack_polymorphism_after_br() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0A, 0x01, 0x08, 0x00, 0x02, 0x40, 0x0C, 0x00, 0x1A,
            0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_unreachable_block_dead_ref_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x10, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x09, 0x00, 0x02, 0x63, 0x01, 0x00, 0xd2, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_unreachable_block_dead_ref_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x10, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x09, 0x00, 0x02, 0x63, 0x01, 0x00, 0xd2, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(54));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_br_dead_ref_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03,
            0x02, 0x01, 0x02, 0x07, 0x06, 0x01, 0x02, 0x66, 0x31, 0x00, 0x00, 0x0a, 0x13, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x0c, 0x00, 0x02, 0x63, 0x00, 0xd2, 0x00, 0x0c, 0x00,
            0xd0, 0x01, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_dead_ref_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-dead-ref-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(58));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_select_to_br_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-select-to-br-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_select_to_br_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-to-br-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(57));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_table_init_to_br_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-to-br-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_to_br_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-to-br-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(86));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_br_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-br-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_br_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-br-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(97));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
        ));
    }

    #[test]
    fn validate_imported_global_get_and_set() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x02, 0x0A, 0x01, 0x03, b'e', b'n', b'v', 0x01, b'g', 0x03, 0x7F, 0x01, 0x03, 0x02,
            0x01, 0x00, 0x0A, 0x08, 0x01, 0x06, 0x00, 0x23, 0x00, 0x24, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_global_set_on_immutable_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x06, 0x09, 0x01, 0x7D, 0x00, 0x43, 0x00, 0x00, 0x00, 0x00,
            0x0B, 0x0A, 0x0B, 0x01, 0x09, 0x00, 0x43, 0x00, 0x00, 0x80, 0x3F, 0x24, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ImmutableGlobalSet { idx: GlobalIdx(0) }
        ));
        assert_eq!(err.offset.0, 39);
    }

    #[test]
    fn validate_defined_global_get() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x06, 0x06, 0x01, 0x7F, 0x00, 0x41, 0x2A, 0x0B, 0x0A,
            0x06, 0x01, 0x04, 0x00, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_ref_is_null() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0xD0, 0x6F, 0xD1, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_ref_is_null_on_non_ref() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0x41, 0x00, 0xD1, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "ref.is_null",
                found: ValType::Num(crate::types::NumType::I32),
                ..
            }
        ));
    }

    #[test]
    fn validate_ref_as_non_null() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x01, 0x63, 0x00, 0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x01, 0x07, 0x05,
            0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x0E, 0x02, 0x04, 0x00, 0x41, 0x07, 0x0B, 0x07,
            0x00, 0x20, 0x00, 0xD4, 0x14, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_ref_as_non_null_after_unreachable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/ref-as-non-null-unreachable.wasm"
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_ref_as_non_null_direct_call_ref_func_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/ref-as-non-null-direct-call-ref-func.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_ref_as_non_null_on_non_ref() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x08, 0x01, 0x06, 0x00, 0x41, 0x00, 0xD4, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(25));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "ref.as_non_null",
                expected: ValType::Ref(RefType::ExternRef),
                found: ValType::Num(crate::types::NumType::I32),
            }
        ));
    }

    #[test]
    fn reject_ref_as_non_null_null_to_nonnull_call_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/ref-as-non-null-null-to-nonnull-call.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(42));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "stack"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn validate_typed_ref_as_non_null_global_set_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-ref-as-non-null-global-set-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_ref_as_non_null_global_set_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-ref-as-non-null-global-set-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(51));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "global.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn validate_typed_ref_as_non_null_if_join_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-ref-as-non-null-if-join-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_ref_as_non_null_if_join_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-ref-as-non-null-if-join-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(46));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_ref_as_non_null_function_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-ref-as-non-null-function-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(40));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch {
                expected,
                found,
                ..
            } if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_br_if_to_loop_param_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-br-if-to-loop-param-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_if_to_loop_param_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-if-to-loop-param-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(72));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_br_on_null() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x6F,
            0x01, 0x6F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0E, 0x01, 0x0C, 0x00, 0x02, 0x40, 0x20,
            0x00, 0xD5, 0x00, 0x0F, 0x0B, 0xD0, 0x6F, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_br_on_null_with_non_ref_input() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x01, 0x7F,
            0x00, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0xD5, 0x00,
            0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "br_on_null",
                expected: ValType::Ref(RefType::ExternRef),
                found: ValType::Num(crate::types::NumType::I32),
            }
        ));
    }

    #[test]
    fn validate_typed_br_on_null_fallthrough_to_call_ref_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-br-on-null-call-ref-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_on_null_unreachable_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-on-null-unreachable.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_on_null_fallthrough_to_call_ref_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-on-null-call-ref-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(60));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_br_on_null_stack_mismatch_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/br-on-null-stack-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(47));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "stack"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::FuncRef)
        ));
    }

    #[test]
    fn reject_typed_br_on_null_block_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-on-null-block-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(57));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_br_on_non_null() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x6F,
            0x01, 0x6F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0F, 0x01, 0x0D, 0x00, 0x02, 0x64, 0x6F,
            0x20, 0x00, 0xD6, 0x00, 0xD0, 0x6F, 0x0F, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_br_on_non_null_with_non_ref_target() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x01, 0x6F,
            0x00, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0D, 0x01, 0x0B, 0x00, 0x02, 0x7F, 0x20, 0x00,
            0xD6, 0x00, 0x41, 0x00, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(28));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidBrOnNonNullTarget {
                label: crate::types::LabelIdx(0),
                found,
            } if found == vec![ValType::Num(crate::types::NumType::I32)]
        ));
    }

    #[test]
    fn validate_typed_br_on_non_null_branch_to_call_ref_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-br-on-non-null-call-ref-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_on_non_null_ref_as_non_null_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-on-non-null-ref-as-non-null.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_on_non_null_unreachable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-on-non-null-unreachable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_on_non_null_branch_to_call_ref_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-on-non-null-call-ref-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(53));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "br_on_non_null"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_br_on_non_null_stack_mismatch_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/br-on-non-null-stack-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(47));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "stack"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::FuncRef)
        ));
    }

    #[test]
    fn reject_typed_br_on_non_null_block_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-on-non-null-block-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(63));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_global_init_expr_from_imported_const_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0A, 0x01, 0x03, b'e', b'n',
            b'v', 0x01, b'g', 0x03, 0x7F, 0x00, 0x06, 0x06, 0x01, 0x7F, 0x00, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_global_init_expr_from_defined_const_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x06, 0x0B, 0x02, 0x7F, 0x00, 0x41,
            0x00, 0x0B, 0x7F, 0x00, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_global_init_expr_with_extended_const_arithmetic() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0A, 0x01, 0x03, 0x65, 0x6E,
            0x76, 0x01, 0x67, 0x03, 0x7F, 0x00, 0x06, 0x09, 0x01, 0x7F, 0x00, 0x23, 0x00, 0x41,
            0x2A, 0x6A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_reference_global_init_exprs() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x06, 0x0B, 0x02, 0x6F, 0x00, 0xD0, 0x6F, 0x0B, 0x70, 0x00,
            0xD2, 0x00, 0x0B, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_ref_func_declared_by_exported_import() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x02, 0x09, 0x01, 0x03, b'e', b'n', b'v', 0x01, b'f', 0x00, 0x00, 0x03, 0x02, 0x01,
            0x00, 0x07, 0x05, 0x01, 0x01, b'f', 0x00, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0xD2,
            0x00, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_ref_func_declared_by_declarative_element() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x03, 0x02, 0x00, 0x00, 0x09, 0x05, 0x01, 0x03, 0x00, 0x01, 0x00, 0x0A, 0x0A,
            0x02, 0x02, 0x00, 0x0B, 0x05, 0x00, 0xD2, 0x00, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_undeclared_ref_func_self_reference() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0xD2, 0x00, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UndeclaredFuncRef {
                idx: crate::types::FuncIdx(0)
            }
        ));
    }

    #[test]
    fn reject_ref_func_when_start_is_only_declaration_source() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0xD2, 0x00,
            0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UndeclaredFuncRef {
                idx: crate::types::FuncIdx(0)
            }
        ));
    }

    #[test]
    fn reject_global_init_expr_from_mutable_imported_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0A, 0x01, 0x03, b'e', b'n',
            b'v', 0x01, b'g', 0x03, 0x7F, 0x01, 0x06, 0x06, 0x01, 0x7F, 0x00, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(25));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::MutableGlobalInInitExpr {
                idx: crate::types::GlobalIdx(0)
            }
        ));
    }

    #[test]
    fn reject_global_init_expr_type_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x06, 0x06, 0x01, 0x7E, 0x00, 0x41,
            0x2A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(13));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::GlobalInitTypeMismatch {
                expected: ValType::Num(crate::types::NumType::I64),
                found: ValType::Num(crate::types::NumType::I32),
            }
        ));
    }

    #[test]
    fn reject_non_constant_global_init_expr() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x06, 0x08, 0x01, 0x7F, 0x00, 0x41,
            0x01, 0x41, 0x02, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(13));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidGlobalInitExpr
        ));
    }

    #[test]
    fn validate_active_data_offset_from_imported_const_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0A, 0x01, 0x03, b'e', b'n',
            b'v', 0x01, b'g', 0x03, 0x7F, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0B, 0x07, 0x01,
            0x00, 0x23, 0x00, 0x0B, 0x01, 0xAA,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_active_data_offset_from_defined_const_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x06,
            0x06, 0x01, 0x7F, 0x00, 0x41, 0x00, 0x0B, 0x0B, 0x07, 0x01, 0x00, 0x23, 0x00, 0x0B,
            0x01, 0x61,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_active_data_offset_with_extended_const_arithmetic() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0A, 0x01, 0x03, 0x65, 0x6E,
            0x76, 0x01, 0x67, 0x03, 0x7F, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0B, 0x10, 0x01,
            0x00, 0x41, 0x02, 0x23, 0x00, 0x41, 0x01, 0x6B, 0x41, 0x02, 0x6A, 0x6C, 0x0B, 0x01,
            0x61,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_element_expr_from_imported_const_ref_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0A, 0x01, 0x03, b'e', b'n',
            b'v', 0x01, b'g', 0x03, 0x6F, 0x00, 0x04, 0x04, 0x01, 0x6F, 0x00, 0x01, 0x09, 0x0B,
            0x01, 0x06, 0x00, 0x41, 0x00, 0x0B, 0x6F, 0x01, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_active_element_offset_from_defined_const_global() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01, 0x06, 0x06, 0x01, 0x7F,
            0x00, 0x41, 0x00, 0x0B, 0x09, 0x07, 0x01, 0x00, 0x23, 0x00, 0x0B, 0x01, 0x00, 0x0A,
            0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_active_element_offset_with_extended_const_arithmetic() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x02, 0x0A, 0x01, 0x03, 0x65, 0x6E, 0x76, 0x01, 0x67, 0x03, 0x7F, 0x00, 0x03, 0x02,
            0x01, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x08, 0x09, 0x14, 0x01, 0x06, 0x00, 0x41,
            0x02, 0x23, 0x00, 0x41, 0x01, 0x6B, 0x41, 0x02, 0x6A, 0x6C, 0x0B, 0x70, 0x01, 0xD2,
            0x00, 0x0B, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_memory_size_and_grow_for_imported_memory() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x02, 0x0C, 0x01, 0x03, b'e', b'n', b'v', 0x03, b'm', b'e', b'm', 0x02, 0x00,
            0x01, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0B, 0x01, 0x09, 0x00, 0x41, 0x01, 0x40, 0x00,
            0x1A, 0x3F, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_memory_size_and_grow_for_defined_memory() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0B, 0x01, 0x09,
            0x00, 0x41, 0x01, 0x40, 0x00, 0x1A, 0x3F, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_memory_size_and_grow_for_nonzero_memory_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x02, 0x0C, 0x01, 0x03, b'e', b'n', b'v', 0x03, b'm', b'e', b'm', 0x02, 0x00,
            0x01, 0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0B, 0x01, 0x09,
            0x00, 0x41, 0x01, 0x40, 0x01, 0x1A, 0x3F, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_i32_load_and_store_for_defined_memory() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x10, 0x01, 0x0E,
            0x00, 0x41, 0x00, 0x41, 0x2A, 0x36, 0x02, 0x00, 0x41, 0x00, 0x28, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_i32_load_and_store_for_nonzero_memory_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x05, 0x05, 0x02, 0x00, 0x01,
            0x00, 0x01, 0x0A, 0x15, 0x02, 0x08, 0x00, 0x41, 0x00, 0x28, 0x42, 0x01, 0x00, 0x0B,
            0x0A, 0x00, 0x41, 0x00, 0x41, 0x01, 0x36, 0x42, 0x01, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_narrow_memory_ops_for_defined_memory() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x01, 0x7E, 0x03, 0x03, 0x02, 0x00, 0x01, 0x05, 0x03, 0x01, 0x00,
            0x01, 0x0A, 0x1D, 0x02, 0x0D, 0x00, 0x41, 0x00, 0x2C, 0x00, 0x00, 0x1A, 0x41, 0x00,
            0x2F, 0x01, 0x00, 0x0B, 0x0D, 0x00, 0x41, 0x00, 0x30, 0x00, 0x00, 0x1A, 0x41, 0x00,
            0x35, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_narrow_memory_stores_for_defined_memory() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x27, 0x01, 0x25, 0x00,
            0x41, 0x00, 0x41, 0x7F, 0x3A, 0x00, 0x00, 0x41, 0x00, 0x41, 0x7F, 0x3B, 0x01, 0x00,
            0x41, 0x00, 0x42, 0x01, 0x3C, 0x00, 0x00, 0x41, 0x00, 0x42, 0x01, 0x3D, 0x01, 0x00,
            0x41, 0x00, 0x42, 0x01, 0x3E, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_v128_load_and_store_for_defined_memory() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x01,
            0x7B, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x05, 0x03, 0x01, 0x00, 0x01,
            0x0A, 0x25, 0x02, 0x08, 0x00, 0x41, 0x00, 0xFD, 0x00, 0x04, 0x00, 0x0B, 0x1A, 0x00,
            0x41, 0x00, 0xFD, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFD, 0x0B, 0x04, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_v128_load_and_store_for_nonzero_memory_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x01,
            0x7B, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x05, 0x05, 0x02, 0x00, 0x01,
            0x00, 0x01, 0x0A, 0x27, 0x02, 0x09, 0x00, 0x41, 0x00, 0xFD, 0x00, 0x44, 0x01, 0x00,
            0x0B, 0x1B, 0x00, 0x41, 0x00, 0xFD, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFD, 0x0B, 0x44, 0x01, 0x00,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_data_segments_and_bulk_memory_ops() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x24, 0x01, 0x22, 0x00,
            0x41, 0x00, 0x41, 0x00, 0x41, 0x02, 0xFC, 0x08, 0x00, 0x00, 0xFC, 0x09, 0x00, 0x41,
            0x00, 0x41, 0x00, 0x41, 0x02, 0xFC, 0x0A, 0x00, 0x00, 0x41, 0x00, 0x41, 0x7F, 0x41,
            0x02, 0xFC, 0x0B, 0x00, 0x0B, 0x0B, 0x08, 0x01, 0x00, 0x41, 0x00, 0x0B, 0x02, 0xAA,
            0xBB, 0x0C, 0x01, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_unknown_data_index_in_memory_init() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0C, 0x01, 0x00, 0x0A, 0x0E,
            0x01, 0x0C, 0x00, 0x41, 0x00, 0x41, 0x00, 0x41, 0x01, 0xFC, 0x08, 0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownDataIdx {
                idx: crate::types::DataIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn validate_v128_lane_memory_ops() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x37, 0x01, 0x35, 0x00,
            0x41, 0x00, 0xFD, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFD, 0x54, 0x00, 0x00, 0x0F, 0x1A, 0x41, 0x00,
            0xFD, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xFD, 0x58, 0x00, 0x00, 0x0F, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_v128_load16_lane_with_invalid_lane_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x1E, 0x01, 0x1C, 0x00,
            0x41, 0x00, 0xFD, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFD, 0x55, 0x01, 0x00, 0x08, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidSimdLaneIdx {
                op: "v128.load16_lane",
                max: 7,
                found: 8,
            }
        ));
    }

    #[test]
    fn reject_i64_load32_with_invalid_memarg_align() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7E, 0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0A, 0x01, 0x08,
            0x00, 0x41, 0x00, 0x35, 0x03, 0x00, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidMemArgAlign {
                op: "i64.load32",
                max: 2,
                found: 3,
            }
        ));
    }

    #[test]
    fn reject_i64_store32_with_wrong_value_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0B, 0x01, 0x09, 0x00,
            0x41, 0x00, 0x41, 0x01, 0x3E, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "i64.store32",
                expected: ValType::Num(crate::types::NumType::I64),
                found: ValType::Num(crate::types::NumType::I32),
            }
        ));
    }

    #[test]
    fn reject_i32_load_with_invalid_memarg_align() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0C, 0x01, 0x0A,
            0x00, 0x41, 0x00, 0x28, 0x03, 0x00, 0x1A, 0x41, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidMemArgAlign {
                op: "i32.load",
                max: 2,
                found: 3,
            }
        ));
    }

    #[test]
    fn reject_i32_store_with_wrong_value_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0B, 0x01, 0x09, 0x00,
            0x41, 0x00, 0x42, 0x01, 0x36, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "i32.store",
                expected: ValType::Num(crate::types::NumType::I32),
                found: ValType::Num(crate::types::NumType::I64),
            }
        ));
    }

    #[test]
    fn reject_unknown_imported_global_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x06, 0x01, 0x04, 0x00, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownGlobalIdx {
                idx: crate::types::GlobalIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn reject_unknown_imported_memory_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x06, 0x01, 0x04, 0x00, 0x3F, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownMemIdx {
                idx: crate::types::MemIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn validate_i64_add() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7E, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x42, 0x01, 0x42, 0x02,
            0x7C, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_if_result_without_else() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0C, 0x01, 0x0A, 0x00, 0x41, 0x01, 0x04, 0x7F, 0x41,
            0x02, 0x0B, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::MissingElseForResult
        ));
    }

    #[test]
    fn map_unknown_opcode_into_validation_error() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x05, 0x01, 0x03, 0x00, 0xFF, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(23));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnknownOpcode { byte: 0xFF },
            }
        ));
    }

    #[test]
    fn map_unterminated_body_into_validation_error() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x05, 0x01, 0x03, 0x00, 0x20, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(25));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn map_truncated_call_ref_immediate_into_validation_error() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/body-decode-truncated-call-ref-typeidx.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn map_truncated_return_call_ref_immediate_into_validation_error() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/body-decode-truncated-return-call-ref-typeidx.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn map_truncated_br_on_null_immediate_into_validation_error() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/body-decode-truncated-br-on-null-labelidx.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn map_truncated_br_on_non_null_immediate_into_validation_error() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/body-decode-truncated-br-on-non-null-labelidx.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn map_truncated_ref_null_heaptype_into_validation_error() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/body-decode-truncated-ref-null-heaptype.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(24));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn map_truncated_typed_block_result_heaptype_into_validation_error() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/body-decode-truncated-block-result-heaptype.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(25));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::Decode {
                context: crate::error::DecodeContext::CodeSection,
                kind: crate::error::DecodeErrorKind::UnexpectedEof,
            }
        ));
    }

    #[test]
    fn report_branch_type_mismatch_with_label_types() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0C, 0x01, 0x0A, 0x00, 0x02, 0x7F, 0x42, 0x00, 0x0C,
            0x00, 0x0B, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(27));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch {
                label: crate::types::LabelIdx(0),
                expected,
                found,
            } if expected == vec![ValType::Num(crate::types::NumType::I32)]
                && found == vec![ValType::Num(crate::types::NumType::I64)]
        ));
    }

    #[test]
    fn report_control_result_type_mismatch_at_end() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0A, 0x01, 0x08, 0x00, 0x02, 0x7F, 0x42, 0x00, 0x0B,
            0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(27));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && found == vec![ValType::Num(crate::types::NumType::I64)]
        ));
    }

    #[test]
    fn reject_block_end_with_extra_operand() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x10, 0x01, 0x0E, 0x00, 0x02, 0x40, 0x43, 0x00, 0x00,
            0x00, 0x00, 0x41, 0x01, 0x0D, 0x00, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(34));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected.is_empty()
                    && found == vec![ValType::Num(crate::types::NumType::F32)]
        ));
    }

    #[test]
    fn reject_block_end_after_consuming_outer_operand() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x00, 0x0A, 0x0D, 0x01, 0x0B, 0x00,
            0x41, 0x00, 0x02, 0x40, 0x28, 0x00, 0x00, 0x1A, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        // Review r3 P2: the `i32.load` inside the empty block sits on the
        // block's floor, so its address pop must NOT reach the enclosing
        // `i32.const 0`. The rejection is therefore reported at the offending
        // pop (offset 32, the `i32.load`) rather than downstream at `end`
        // (offset 36) once the length had shifted — which is exactly where
        // wasmparser at this pin reports it ("type mismatch: expected i32 but
        // nothing on stack" at offset 0x20).
        assert_eq!(err.offset, ByteOffset(32));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::StackUnderflow { op, ref expected, ref available }
                if op == "i32.load"
                    && *expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && available.is_empty()
        ));
    }

    #[test]
    fn reject_folded_syntax_equivalent_br_if_operand_use() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0D, 0x01, 0x0B, 0x00, 0x02, 0x40, 0x41, 0x01, 0x0D,
            0x00, 0x8C, 0x01, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::StackUnderflow { op, expected, .. }
                if op == "f32.unary"
                    && expected == vec![ValType::Num(crate::types::NumType::F32)]
        ));
    }

    #[test]
    fn validate_call_as_call_all_operands_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-call-all-operands.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_br_table_last_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-br-table-last.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_call_indirect_last_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-as-call-indirect-last.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_memory_grow_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-memory-grow-value.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_local_tee_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-local-tee-value.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_load_operand_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-load-operand.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_compare_right_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-compare-right.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_as_convert_operand_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-as-convert-operand.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_func_mid_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/unreachable-as-func-mid.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_block_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-block-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_br_table_value_index_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-br-table-value-index.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_if_then_no_else_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-if-then-no-else.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_call_indirect_first_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-call-indirect-first.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_local_tee_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-local-tee-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_store_n_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-storeN-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_unreachable_as_convert_operand_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/unreachable-as-convert-operand.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_select_last_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-select-last.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_br_if_first_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-br-if-first.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_store_last_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-store-last.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_memory_grow_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-memory-grow-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_local_tee_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-local-tee-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_load_operand_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-load-operand.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_compare_right_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-compare-right.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_indirect_as_convert_operand_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-indirect-as-convert-operand.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_block_as_select_cond_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/block-as-select-cond.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_block_as_load_address_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/block-as-load-address.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_if_as_call_indirect_last_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/if-as-call-indirect-last.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_if_as_memory_grow_size_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/if-as-memory-grow-size.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_loop_as_local_tee_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/loop-as-local-tee-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_loop_as_memory_grow_size_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/loop-as-memory-grow-size.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_return_as_call_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/return-as-call-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_return_as_br_value_official_case() {
        let bytes = include_bytes!("../../../baedeker-testdata/spec/valid/return-as-br-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_br_if_value_cond_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-as-br-if-value-cond.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_select_all_official_case() {
        let bytes = include_bytes!("../../../baedeker-testdata/spec/valid/br-as-select-all.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_call_indirect_all_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-as-call-indirect-all.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_local_tee_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-as-local-tee-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_load_address_official_case() {
        let bytes = include_bytes!("../../../baedeker-testdata/spec/valid/br-as-load-address.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_store_n_value_official_case() {
        let bytes = include_bytes!("../../../baedeker-testdata/spec/valid/br-as-storeN-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_as_memory_grow_size_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-as-memory-grow-size.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_br_if_value_cond_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-if-as-br-if-value-cond.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_select_cond_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-if-as-select-cond.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_call_indirect_last_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-if-as-call-indirect-last.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_local_tee_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-if-as-local-tee-value.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_load_address_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-if-as-load-address.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_store_n_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-if-as-storeN-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_if_as_memory_grow_size_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-if-as-memory-grow-size.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_with_matching_label_types() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x10, 0x01, 0x0E, 0x00, 0x02, 0x7F, 0x41, 0x01, 0x41,
            0x00, 0x0E, 0x01, 0x00, 0x00, 0x0B, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_type_f64_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-table-type-f64-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_br_if_value_cond_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-table-as-br-if-value-cond.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_call_indirect_func_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-table-as-call-indirect-func.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_local_set_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-table-as-local-set-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_load_address_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-table-as-load-address.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_store_value_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-table-as-store-value.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_compare_left_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/br-table-as-compare-left.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_br_table_as_memory_grow_size_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/br-table-as-memory-grow-size.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_block_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-block.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_loop1_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-loop1.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_loop2_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-loop2.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_loop3_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-loop3.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_loop4_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-loop4.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_if_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-if.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_if2_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-if2.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_return_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-return.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_br_if0_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-br-if0.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_br_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-br.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_switch_stmt_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-switch-stmt.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_switch_expr_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-switch-expr.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_switch_arg_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-switch-arg.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_ref_typed_syntax_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-ref-typed-syntax.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_loop5_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-loop5.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_loop6_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-loop6.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_br_if1_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-br-if1.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_br_if2_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-br-if2.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_br_if3_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-br-if3.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_shadowing_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-shadowing.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_redefinition_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/official-labels-redefinition.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_labels_switch_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-labels-switch.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_official_switch_corner_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/official-switch-corner.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_br_if_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x13, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x0c, 0x00, 0x02, 0x63, 0x01, 0xd2, 0x00, 0x20, 0x00,
            0x0d, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_if_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x13, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x0c, 0x00, 0x02, 0x63, 0x01, 0xd2, 0x00, 0x20, 0x00,
            0x0d, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(56));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_select_to_br_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-select-to-br-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_select_to_br_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-to-br-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(60));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_table_init_to_br_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-to-br-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_to_br_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-to-br-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(89));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_br_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-br-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_br_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-br-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(100));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_br_to_loop_param_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x17, 0x04, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x00, 0x60, 0x01,
            0x63, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x01, 0x02, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0a, 0x17, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x10, 0x00, 0x02,
            0x63, 0x00, 0xd0, 0x00, 0x03, 0x03, 0x1a, 0xd2, 0x00, 0x0c, 0x00, 0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_to_loop_param_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x17, 0x04, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x00, 0x60, 0x01,
            0x63, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x01, 0x02, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0a, 0x17, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x10, 0x00, 0x02,
            0x63, 0x00, 0xd0, 0x00, 0x03, 0x03, 0x1a, 0xd2, 0x00, 0x0c, 0x00, 0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(65));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_br_to_loop_param_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-br-to-loop-param-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_to_loop_param_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-br-to-loop-param-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(69));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_br_if_to_loop_param_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x17, 0x04, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x00, 0x60, 0x01,
            0x63, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x01, 0x02, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0a, 0x19, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x12, 0x00, 0x02,
            0x63, 0x00, 0xd0, 0x00, 0x03, 0x03, 0x1a, 0xd2, 0x00, 0x41, 0x01, 0x0d, 0x00, 0x0b,
            0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_if_to_loop_param_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x17, 0x04, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x00, 0x60, 0x01,
            0x63, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x01, 0x02, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0a, 0x19, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x12, 0x00, 0x02,
            0x63, 0x00, 0xd0, 0x00, 0x03, 0x03, 0x1a, 0xd2, 0x00, 0x41, 0x01, 0x0d, 0x00, 0x0b,
            0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(67));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_br_table_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x15, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x0e, 0x00, 0x02, 0x63, 0x01, 0xd2, 0x00, 0x41, 0x00, 0x0e,
            0x01, 0x00, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_table_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x15, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x0e, 0x00, 0x02, 0x63, 0x01, 0xd2, 0x00, 0x41, 0x00, 0x0e,
            0x01, 0x00, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(55));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_select_to_br_table_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-select-to-br-table-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_select_to_br_table_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-to-br-table-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(59));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_table_init_to_br_table_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-to-br-table-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_to_br_table_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-to-br-table-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(88));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_br_table_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-br-table-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_br_table_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-br-table-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(99));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
        ));
    }

    /// Nullability-differing br_table targets are VALID when the operand is
    /// a subtype of both: (ref $t0) branches to (ref $t0) and
    /// (ref null $t0) labels alike (the join is the operand's own type).
    #[test]
    fn validate_typed_br_table_nullability_targets_join() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-br-table-nullability-targets-join.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_select() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7E, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0D, 0x01, 0x0B, 0x00, 0x42, 0x01, 0x42, 0x02,
            0x41, 0x00, 0x1C, 0x01, 0x7E, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_as_br_table_last_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/select-as-br-table-last.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_as_call_indirect_last_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/select-as-call-indirect-last.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_as_memory_grow_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/select-as-memory-grow-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_as_global_set_value_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/select-as-global-set-value.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_as_convert_operand_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/select-as-convert-operand.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_select_as_if_condition_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/select-as-if-condition.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_select_with_equivalent_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x13, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00, 0xD2, 0x00, 0xD0, 0x01, 0x20, 0x00, 0x1C,
            0x01, 0x63, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_select_wrong_operand_types() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0E, 0x01, 0x0C, 0x00, 0x41, 0x01, 0x41, 0x02, 0x41,
            0x00, 0x1C, 0x01, 0x7E, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::SelectOperandTypeMismatch { expected, found }
                if expected == ValType::Num(crate::types::NumType::I64)
                    && found == vec![
                        ValType::Num(crate::types::NumType::I32),
                        ValType::Num(crate::types::NumType::I32),
                    ]
        ));
    }

    #[test]
    fn reject_typed_select_with_invalid_result_arity() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x0D, 0x01, 0x0B, 0x00, 0x41, 0x01, 0x41, 0x02, 0x41,
            0x00, 0x1C, 0x00, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidSelectResultArity { found: 0 }
        ));
    }

    #[test]
    fn reject_typed_select_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7E, 0x60, 0x01, 0x7F, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x13, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00, 0xD2, 0x00, 0xD0, 0x01, 0x20, 0x00, 0x1C,
            0x01, 0x63, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(55));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::SelectOperandTypeMismatch { expected, found }
                if expected == ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                }) && found == vec![
                    ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    }),
                    ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    }),
                ]
        ));
    }

    #[test]
    fn reject_typed_select_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(50));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::SelectOperandTypeMismatch { expected, found }
                if expected == ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }) && found == vec![
                    ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    }),
                    ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    }),
                ]
        ));
    }

    #[test]
    fn reject_typed_select_function_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-function-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(59));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch {
                expected,
                found,
                ..
            } if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_select_block_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-block-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(62));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_select_if_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-if-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(64));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_select_function_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-select-function-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(54));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch {
                expected,
                found,
                ..
            } if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_br_table_with_inconsistent_target_types() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x10, 0x01, 0x0E, 0x00, 0x02, 0x7F, 0x02, 0x7E, 0x41,
            0x00, 0x0E, 0x01, 0x00, 0x01, 0x0B, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        // Label types may differ under the join rule, but the (empty) stack
        // cannot supply an i64 to label 0.
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(0)
                    && expected == vec![ValType::Num(crate::types::NumType::I64)]
                    && found.is_empty()
        ));
    }

    #[test]
    fn validate_typed_br_table_multi_block_targets_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x1a, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x13, 0x00, 0x02, 0x63, 0x01, 0x02, 0x63, 0x00, 0xd2,
            0x00, 0x20, 0x00, 0x0e, 0x02, 0x00, 0x01, 0x01, 0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_table_multi_block_targets_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x1a, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x13, 0x00, 0x02, 0x63, 0x01, 0x02, 0x63, 0x00, 0xd2,
            0x00, 0x20, 0x00, 0x0e, 0x02, 0x00, 0x01, 0x01, 0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(59));
        // The (ref $t0) operand satisfies label 0 (ref null $t0) but not
        // label 1 (ref null $t1).
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(1)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_typed_br_table_multi_loop_block_targets_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x18, 0x04, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x60,
            0x01, 0x63, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01,
            0x01, 0x66, 0x00, 0x00, 0x0a, 0x1e, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x17, 0x00,
            0x02, 0x63, 0x01, 0xd0, 0x00, 0x03, 0x03, 0x1a, 0xd2, 0x00, 0x20, 0x00, 0x0e, 0x02,
            0x00, 0x01, 0x01, 0xd0, 0x00, 0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_br_table_multi_loop_block_targets_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x18, 0x04, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x60,
            0x01, 0x63, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01,
            0x01, 0x66, 0x00, 0x00, 0x0a, 0x1e, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x17, 0x00,
            0x02, 0x63, 0x01, 0xd0, 0x00, 0x03, 0x03, 0x1a, 0xd2, 0x00, 0x20, 0x00, 0x0e, 0x02,
            0x00, 0x01, 0x01, 0xd0, 0x00, 0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(68));
        // The (ref $t0) operand satisfies the loop's (ref null $t0) param
        // but not the block's (ref null $t1) result.
        assert!(matches!(
            err.kind,
            ValidationErrorKind::BranchTypeMismatch { label, expected, found }
                if label == crate::types::LabelIdx(1)
                    && expected == vec![ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })]
                    && found == vec![ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })]
        ));
    }

    #[test]
    fn validate_start_function_with_empty_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_unknown_start_function_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x08, 0x01, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(10));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownFuncIdx {
                idx: crate::types::FuncIdx(0)
            }
        ));
    }

    #[test]
    fn reject_start_function_with_params() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x01, 0x7F,
            0x00, 0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(21));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidStartFunctionType { params, results }
                if params == vec![ValType::Num(crate::types::NumType::I32)]
                    && results.is_empty()
        ));
    }

    #[test]
    fn reject_start_function_with_results() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x08, 0x01, 0x00, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(21));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidStartFunctionType { params, results }
                if params.is_empty()
                    && results == vec![ValType::Num(crate::types::NumType::I32)]
        ));
    }

    #[test]
    fn validate_element_expression_initializers() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01, 0x09, 0x0A, 0x01, 0x05,
            0x70, 0x02, 0xD0, 0x70, 0x0B, 0xD2, 0x00, 0x0B, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_element_expr_type_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x6F, 0x00, 0x01,
            0x09, 0x07, 0x01, 0x05, 0x6F, 0x01, 0xD0, 0x70, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementExprTypeMismatch {
                expected: ValType::Ref(RefType::ExternRef),
                found: ValType::Ref(RefType::FuncRef),
            }
        ));
    }

    #[test]
    fn reject_numeric_element_expr_by_type() {
        // `(elem funcref (item (i32.const 0)))`: `i32.const` IS const-eligible
        // in an element expression (GC U4 — `is_const` has no per-context
        // constant restriction; `struct.new` operands need it), so the pin's
        // rejection is the segment TYPE mismatch, not "constant expression
        // required".
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01,
            0x09, 0x07, 0x01, 0x05, 0x70, 0x01, 0x41, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementExprTypeMismatch {
                expected: ValType::Ref(RefType::FuncRef),
                found: ValType::Num(crate::types::NumType::I32),
            }
        ));
    }

    #[test]
    fn reject_memory_init_without_data_count_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0E, 0x01, 0x0C, 0x00,
            0x41, 0x00, 0x41, 0x00, 0x41, 0x00, 0xFC, 0x08, 0x00, 0x00, 0x0B, 0x0B, 0x03, 0x01,
            0x01, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::MissingDataCountSection { op: "memory.init" }
        ));
    }

    #[test]
    fn reject_data_drop_without_data_count_section() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0xFC, 0x09, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::MissingDataCountSection { op: "data.drop" }
        ));
    }

    #[test]
    fn reject_active_element_table_type_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x6F, 0x00, 0x01,
            0x09, 0x07, 0x01, 0x00, 0x41, 0x00, 0x0B, 0x01, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementTableTypeMismatch {
                expected: RefType::ExternRef,
                found: RefType::FuncRef,
            }
        ));
    }

    #[test]
    fn validate_active_element_table_type_match_for_imported_table() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0B, 0x01, 0x03, b'e', b'n',
            b'v', 0x01, b't', 0x01, 0x6F, 0x00, 0x01, 0x09, 0x0B, 0x01, 0x06, 0x00, 0x41, 0x00,
            0x0B, 0x6F, 0x01, 0xD0, 0x6F, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_multiple_tables_across_imports_and_definitions() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0B, 0x01, 0x03, b'e', b'n',
            b'v', 0x01, b't', 0x01, 0x70, 0x00, 0x01, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_multiple_memories_across_imports_and_definitions() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0C, 0x01, 0x03, b'e', b'n',
            b'v', 0x03, b'm', b'e', b'm', 0x02, 0x00, 0x01, 0x05, 0x03, 0x01, 0x00, 0x01,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_duplicate_export_names() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x07, 0x0D, 0x02, 0x03, b'd', b'u', b'p', 0x00, 0x00, 0x03,
            b'd', b'u', b'p', 0x00, 0x00, 0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::DuplicateExportName { name } if name == "dup"
        ));
    }

    #[test]
    fn validate_exports_and_elements_indices() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x00,
            0x60, 0x01, 0x7F, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x04, 0x04, 0x01, 0x70, 0x00,
            0x02, 0x05, 0x03, 0x01, 0x00, 0x01, 0x06, 0x06, 0x01, 0x7F, 0x00, 0x41, 0x00, 0x0B,
            0x07, 0x11, 0x04, 0x01, b'f', 0x00, 0x00, 0x01, b't', 0x01, 0x00, 0x01, b'm', 0x02,
            0x00, 0x01, b'g', 0x03, 0x00, 0x09, 0x08, 0x01, 0x00, 0x41, 0x00, 0x0B, 0x02, 0x00,
            0x01, 0x0A, 0x07, 0x02, 0x02, 0x00, 0x0B, 0x02, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_unknown_export_table_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x07, 0x05, 0x01, 0x01, b't', 0x01,
            0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(10));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTableIdx {
                idx: crate::types::TableIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn reject_unknown_export_memory_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x07, 0x05, 0x01, 0x01, b'm', 0x02,
            0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(10));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownMemIdx {
                idx: crate::types::MemIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn reject_unknown_export_global_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x07, 0x05, 0x01, 0x01, b'g', 0x03,
            0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(10));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownGlobalIdx {
                idx: crate::types::GlobalIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn reject_unknown_element_table_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x09, 0x09, 0x01, 0x02, 0x00, 0x41,
            0x00, 0x0B, 0x00, 0x01, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(13));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownTableIdx {
                idx: crate::types::TableIdx(0),
                available: 0,
            }
        ));
    }

    #[test]
    fn reject_unknown_function_index_in_element_init() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01,
            0x09, 0x07, 0x01, 0x00, 0x41, 0x00, 0x0B, 0x01, 0x00,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(16));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownFuncIdx {
                idx: crate::types::FuncIdx(0)
            }
        ));
    }

    #[test]
    fn validate_call_indirect_with_funcref_table() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x00, 0x00, 0x02, 0x0D, 0x01, 0x03, b'e', b'n', b'v', 0x03, b't',
            b'a', b'b', 0x01, 0x70, 0x00, 0x01, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0B, 0x01, 0x09,
            0x00, 0x20, 0x00, 0x41, 0x00, 0x11, 0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_block_result_to_call_indirect_ref_param_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-block-to-call-indirect-ref-param-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_block_result_to_call_indirect_ref_param_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-block-to-call-indirect-ref-param-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(88));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "stack"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn validate_return_call() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x03, 0x02, 0x00, 0x00, 0x0A, 0x0B, 0x02, 0x04, 0x00, 0x41, 0x00, 0x0B,
            0x04, 0x00, 0x12, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_return_call_result_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x01, 0x7E, 0x03, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0B, 0x02, 0x04,
            0x00, 0x42, 0x00, 0x0B, 0x04, 0x00, 0x12, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(34));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch {
                expected,
                found,
            } if expected == vec![ValType::Num(crate::types::NumType::I32)]
                && found == vec![ValType::Num(crate::types::NumType::I64)]
        ));
    }

    #[test]
    fn validate_return_call_indirect_with_funcref_table() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01, 0x0A,
            0x10, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x09, 0x00, 0x20, 0x00, 0x41, 0x00, 0x13,
            0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_to_return_call_indirect_result_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-to-return-call-indirect-result-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_if_result_to_return_call_indirect_ref_param_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-if-to-return-call-indirect-ref-param-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_if_result_to_return_call_indirect_ref_param_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-if-to-return-call-indirect-ref-param-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(89));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "stack"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_table_init_to_return_call_indirect_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-to-return-call-indirect-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(81));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_return_call_indirect_result_nullable_official_case()
     {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-return-call-indirect-result-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_return_call_indirect_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-return-call-indirect-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(130));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_call_ref() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x00, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00,
            0x0A, 0x0F, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x08, 0x00, 0x41, 0x07, 0xD2, 0x00,
            0x14, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_ref_with_equivalent_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x00, 0x07, 0x05,
            0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x0F, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x08,
            0x00, 0x20, 0x00, 0xD2, 0x00, 0x14, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_block_result_to_call_ref_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-block-to-call-ref-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_call_ref_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-call-ref-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_call_ref_if_abstract_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-call-ref-if-abstract-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(56));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::FuncRef)
        ));
    }

    #[test]
    fn validate_call_ref_run_nested_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/call-ref-run-nested.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_ref_unreachable_ref_func_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-ref-unreachable-ref-func.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_call_ref_unreachable_call_drop_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/call-ref-unreachable-call-drop.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_call_ref_function_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-call-ref-function-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(57));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch {
                expected,
                found,
                ..
            } if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_call_ref_block_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-call-ref-block-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(60));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_call_ref_if_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-call-ref-if-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(63));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_block_result_to_call_ref_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-block-to-call-ref-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(51));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn validate_typed_ref_global_init_from_ref_func() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x03, 0x02, 0x01, 0x00, 0x06, 0x07, 0x01, 0x63, 0x00, 0x00, 0xD2, 0x00,
            0x0B, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x06, 0x01, 0x04, 0x00, 0x20,
            0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_ref_global_init_from_equivalent_ref_func() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x03, 0x02, 0x01, 0x00, 0x06, 0x07, 0x01,
            0x63, 0x01, 0x00, 0xD2, 0x00, 0x0B, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A,
            0x06, 0x01, 0x04, 0x00, 0x20, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_ref_global_init_from_imported_typed_global_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x02, 0x0B, 0x01, 0x03, 0x65, 0x6E, 0x76,
            0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x06, 0x07, 0x01, 0x63, 0x01, 0x00, 0x23, 0x00,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_ref_global_init_from_defined_typed_global_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x03, 0x02, 0x01, 0x00, 0x06, 0x0D, 0x02,
            0x63, 0x00, 0x00, 0xD2, 0x00, 0x0B, 0x63, 0x01, 0x00, 0x23, 0x00, 0x0B, 0x07, 0x05,
            0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x06, 0x01, 0x04, 0x00, 0x20, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_ref_func_to_imported_mut_typed_global_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0E, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x00, 0x00, 0x02, 0x0B, 0x01, 0x03,
            0x65, 0x6E, 0x76, 0x01, 0x67, 0x03, 0x63, 0x01, 0x01, 0x03, 0x03, 0x02, 0x00, 0x02,
            0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x0D, 0x02, 0x04, 0x00, 0x20, 0x00,
            0x0B, 0x06, 0x00, 0xD2, 0x00, 0x24, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_get_to_defined_mut_global_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-to-defined-mut-global-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_imported_typed_table_get_to_defined_mut_global_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/imported-typed-table-to-defined-mut-global-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_local_set_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-local-set-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_local_if_nullable_to_call_ref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-local-if-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_global_set_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-global-set-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_global_set_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-global-set-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_global_if_nullable_to_call_ref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-global-if-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_set_if_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-set-if-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_if_nullable_to_call_ref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-if-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_passive_element_nullable_from_nonnull_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-passive-element-nullable-from-nonnull.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_defined_nonnull_global_passive_element_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-defined-nonnull-global-passive-element-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_nullable_to_call_ref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_defined_global_passive_element_table_init_nullable_to_call_ref_official_case()
    {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-defined-global-passive-element-table-init-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_imported_global_passive_element_table_init_nullable_to_call_ref_official_case()
     {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-imported-global-passive-element-table-init-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_imported_nonnull_global_passive_element_table_init_nullable_to_call_ref_official_case()
     {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-imported-nonnull-global-passive-element-table-init-nullable-to-call-ref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_set_get() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x00, 0x01, 0x63, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x04, 0x05,
            0x01, 0x63, 0x00, 0x00, 0x01, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x13,
            0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00, 0x41, 0x00, 0xD2, 0x00, 0x26, 0x00,
            0x41, 0x00, 0x25, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_set_get_with_equivalent_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x04, 0x05, 0x01, 0x63, 0x01, 0x00, 0x01, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0A, 0x13, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00, 0x41,
            0x00, 0xD2, 0x00, 0x26, 0x00, 0x41, 0x00, 0x25, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_imported_typed_global_to_defined_table_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x00, 0x01, 0x63, 0x01, 0x02, 0x0B,
            0x01, 0x03, 0x65, 0x6E, 0x76, 0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x03, 0x02, 0x01,
            0x02, 0x04, 0x05, 0x01, 0x63, 0x01, 0x00, 0x01, 0x0A, 0x0E, 0x01, 0x0C, 0x00, 0x41,
            0x00, 0x23, 0x00, 0x26, 0x00, 0x41, 0x00, 0x25, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_imported_typed_global_to_imported_table_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x00, 0x01, 0x63, 0x01, 0x02, 0x16,
            0x02, 0x03, 0x65, 0x6E, 0x76, 0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x03, 0x65, 0x6E,
            0x76, 0x01, 0x74, 0x01, 0x63, 0x01, 0x00, 0x01, 0x03, 0x02, 0x01, 0x02, 0x0A, 0x0E,
            0x01, 0x0C, 0x00, 0x41, 0x00, 0x23, 0x00, 0x26, 0x00, 0x41, 0x00, 0x25, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_element_expr_from_imported_typed_global_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x02, 0x0B, 0x01, 0x03, 0x65, 0x6E, 0x76,
            0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x04, 0x05, 0x01, 0x63, 0x01, 0x00, 0x01, 0x09,
            0x0C, 0x01, 0x06, 0x00, 0x41, 0x00, 0x0B, 0x63, 0x01, 0x01, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_passive_element_from_equivalent_ref_func() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x03, 0x02, 0x01, 0x00, 0x07, 0x05, 0x01,
            0x01, 0x66, 0x00, 0x00, 0x09, 0x08, 0x01, 0x05, 0x63, 0x01, 0x01, 0xD2, 0x00, 0x0B,
            0x0A, 0x06, 0x01, 0x04, 0x00, 0x20, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_passive_element_from_defined_typed_global_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/defined-typed-global-passive-element-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_passive_element_from_imported_typed_global_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/imported-typed-global-passive-element-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_from_defined_typed_global_passive_element_with_equivalent_signature()
     {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/defined-typed-global-passive-element-table-init-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_from_imported_typed_global_passive_element_with_equivalent_signature()
     {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/imported-typed-global-passive-element-table-init-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_table_init_with_equivalent_typed_element_segment() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0E, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00,
            0x02, 0x04, 0x05, 0x01, 0x63, 0x00, 0x00, 0x04, 0x07, 0x0C, 0x02, 0x01, 0x66, 0x00,
            0x00, 0x04, 0x69, 0x6E, 0x69, 0x74, 0x00, 0x01, 0x09, 0x08, 0x01, 0x05, 0x63, 0x01,
            0x01, 0xD2, 0x00, 0x0B, 0x0A, 0x13, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00,
            0x41, 0x00, 0x41, 0x00, 0x41, 0x01, 0xFC, 0x0C, 0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_block_result_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x13, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00, 0x02, 0x63, 0x01, 0xD2, 0x00, 0x0C, 0x00, 0xD0,
            0x01, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_block_result_nullable_from_nonnull_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-block-result-nullable-from-nonnull.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_block_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-block-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(35));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_block_result_feeds_loop_param_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x17, 0x04, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x60, 0x01,
            0x63, 0x01, 0x01, 0x63, 0x01, 0x03, 0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0a, 0x12, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x0b, 0x00, 0x02,
            0x63, 0x00, 0xd2, 0x00, 0x0b, 0x03, 0x03, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_block_result_feeds_loop_param_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x17, 0x04, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x60, 0x01,
            0x63, 0x01, 0x01, 0x63, 0x01, 0x03, 0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0a, 0x12, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0b, 0x0b, 0x00, 0x02,
            0x63, 0x00, 0xd2, 0x00, 0x0b, 0x03, 0x03, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(61));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "stack"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn validate_typed_loop_result_feeds_block_result_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x13, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x0c, 0x00, 0x02, 0x63, 0x01, 0x03, 0x63, 0x00, 0xd2, 0x00,
            0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_loop_result_feeds_block_result_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x13, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x0c, 0x00, 0x02, 0x63, 0x01, 0x03, 0x63, 0x00, 0xd2, 0x00,
            0x0b, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(57));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_if_result_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x14, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0B, 0x0D, 0x00, 0x20, 0x00, 0x04, 0x63, 0x01, 0xD2, 0x00,
            0x05, 0xD0, 0x01, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_if_result_nullable_from_join_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-if-result-nullable-from-join.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_if_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-if-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(54));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_if_with_return_in_then_and_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x15, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x0e, 0x00, 0x20, 0x00, 0x04, 0x63, 0x01, 0xd2, 0x00,
            0x0f, 0x05, 0xd0, 0x01, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_if_with_return_in_then_and_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x15, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0b, 0x0e, 0x00, 0x20, 0x00, 0x04, 0x63, 0x01, 0xd2, 0x00,
            0x0f, 0x05, 0xd0, 0x01, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(56));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_return_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x0e, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x07, 0x00, 0xd2, 0x00, 0x0f, 0xd0, 0x01, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_return_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x0e, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x07, 0x00, 0xd2, 0x00, 0x0f, 0xd0, 0x01, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(50));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_loop_result_with_equivalent_signature() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7f,
            0x01, 0x7f, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x0f, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x08, 0x00, 0x03, 0x63, 0x01, 0xd2, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_loop_result_nullable_from_nonnull_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-loop-result-nullable-from-nonnull.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_loop_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-loop-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(35));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_loop_result_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7e,
            0x01, 0x7e, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x63, 0x01, 0x03, 0x03,
            0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x0f, 0x02, 0x04,
            0x00, 0x20, 0x00, 0x0b, 0x08, 0x00, 0x03, 0x63, 0x01, 0xd2, 0x00, 0x0b, 0x0b,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(53));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_return_if_nullable_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/typed-return-if-nullable.wasm",);
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_return_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-return-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(55));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_return_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-return-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_return_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-return-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(94));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })]
        ));
    }

    #[test]
    fn validate_return_call_ref() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x06, 0x01, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x00, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00,
            0x0A, 0x0F, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x08, 0x00, 0x20, 0x00, 0xD2, 0x00,
            0x15, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_return_call_ref_with_equivalent_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7F, 0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x00, 0x07, 0x05,
            0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x0F, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x08,
            0x00, 0x20, 0x00, 0xD2, 0x00, 0x15, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_if_result_to_return_call_ref_with_equivalent_signature() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-if-to-return-call-ref-equivalent-signature.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_return_call_ref_count_official_case() {
        let bytes =
            include_bytes!("../../../baedeker-testdata/spec/valid/return-call-ref-count.wasm");
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_to_return_call_ref_result_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-to-return-call-ref-result-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_typed_table_init_shared_source_to_return_call_ref_nullable_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/valid/typed-table-init-shared-source-to-return-call-ref-nullable.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_typed_return_call_ref_result_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-return-call-ref-result-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(55));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_return_call_ref_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-return-call-ref-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(50));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_if_result_to_return_call_ref_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-if-to-return-call-ref-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(62));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "return_call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_table_init_to_return_call_ref_result_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-to-return-call-ref-result-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(98));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_return_call_ref_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-return-call-ref-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(94));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_call_ref_with_non_funcref_reference() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x08, 0x01, 0x06, 0x00, 0xD0, 0x6F, 0x14, 0x00,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "call_ref",
                expected: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }),
                found: ValType::Ref(RefType::ExternRef),
            }
        ));
    }

    #[test]
    fn reject_call_ref_non_funcref_externref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/call-ref-non-funcref-externref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::ExternRef)
        ));
    }

    #[test]
    fn reject_call_ref_non_funcref_funcref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/call-ref-non-funcref-funcref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::FuncRef)
        ));
    }

    #[test]
    fn reject_return_call_ref_non_funcref_externref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/return-call-ref-non-funcref-externref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "return_call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::ExternRef)
        ));
    }

    #[test]
    fn reject_return_call_ref_non_funcref_funcref_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/return-call-ref-non-funcref-funcref.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "return_call_ref"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::FuncRef)
        ));
    }

    #[test]
    fn reject_return_call_ref_multi_result_official_case() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/return-call-ref-multi-result.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(33));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch { expected, found }
                if expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && found == vec![
                        ValType::Num(crate::types::NumType::I32),
                        ValType::Num(crate::types::NumType::I32),
                    ]
        ));
    }

    #[test]
    fn reject_return_call_ref_result_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60, 0x00, 0x01,
            0x7E, 0x60, 0x00, 0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x01, 0x07, 0x05, 0x01, 0x01,
            0x66, 0x00, 0x00, 0x0A, 0x0D, 0x02, 0x04, 0x00, 0x42, 0x00, 0x0B, 0x06, 0x00, 0xD2,
            0x00, 0x15, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(43));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch {
                expected,
                found,
            } if expected == vec![ValType::Num(crate::types::NumType::I32)]
                && found == vec![ValType::Num(crate::types::NumType::I64)]
        ));
    }

    #[test]
    fn reject_call_ref_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7F, 0x03, 0x03, 0x02, 0x00, 0x01, 0x07, 0x05,
            0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x0F, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x08,
            0x00, 0x42, 0x00, 0xD2, 0x00, 0x14, 0x01, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(47));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "call_ref",
                expected: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                }),
                found: ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }),
            }
        ));
    }

    #[test]
    fn reject_typed_ref_global_init_from_imported_typed_global_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7E, 0x02, 0x0B, 0x01, 0x03, 0x65, 0x6E, 0x76,
            0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x06, 0x07, 0x01, 0x63, 0x01, 0x00, 0x23, 0x00,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(40));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::GlobalInitTypeMismatch {
                expected: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                }),
                found: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }),
            }
        ));
    }

    #[test]
    fn reject_typed_table_get_to_defined_mut_global_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-to-defined-mut-global-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(74));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "global.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_imported_typed_table_get_to_defined_mut_global_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/imported-typed-table-to-defined-mut-global-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(62));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "global.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_local_set_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-local-set-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(56));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "local.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_local_if_to_return_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-local-if-to-return-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(62));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, .. }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_global_set_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-global-set-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(62));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "global.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_table_init_shared_source_to_global_set_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-shared-source-to-global-set-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(98));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "global.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(1)),
                    })
        ));
    }

    #[test]
    fn reject_typed_global_if_to_return_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-global-if-to-return-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(68));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, .. }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_table_set_if_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-set-if-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(67));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "table.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_table_if_to_return_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-if-to-return-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(70));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, .. }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_passive_element_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-passive-element-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(32));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementExprTypeMismatch { expected, found }
                if expected == ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_table_init_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-table-init-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(76));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementTableTypeMismatch { expected, found }
                if expected == RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }
                    && found == RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    }
        ));
    }

    #[test]
    fn reject_typed_defined_global_passive_element_table_init_to_return_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-defined-global-passive-element-table-init-to-return-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(83));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, .. }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_imported_global_passive_element_table_init_to_return_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-imported-global-passive-element-table-init-to-return-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(74));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::FunctionResultTypeMismatch { expected, found, .. }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_typed_defined_global_passive_element_table_init_to_global_set_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-defined-global-passive-element-table-init-to-global-set-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(87));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch { op, expected, found }
                if op == "global.set"
                    && expected == ValType::Ref(RefType::Typed {
                        nullable: false,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_imported_global_passive_element_table_init_nullability_mismatch() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/typed-imported-global-passive-element-table-init-nullability-mismatch.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(79));
        // The table's element type is (ref $t1) (its init expr references
        // the module's only function, of type 1); the passive segment
        // provides (ref null $t0) values — still a mismatch.
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementTableTypeMismatch { expected, found }
                if expected == RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                }
                    && found == RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    }
        ));
    }

    #[test]
    fn reject_typed_element_expr_from_imported_typed_global_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0B, 0x02, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7E, 0x02, 0x0B, 0x01, 0x03, 0x65, 0x6E, 0x76,
            0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x04, 0x05, 0x01, 0x63, 0x01, 0x00, 0x01, 0x09,
            0x0C, 0x01, 0x06, 0x00, 0x41, 0x00, 0x0B, 0x63, 0x01, 0x01, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(52));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementExprTypeMismatch {
                expected: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                }),
                found: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }),
            }
        ));
    }

    #[test]
    fn reject_imported_typed_global_to_imported_table_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x10, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7E, 0x60, 0x00, 0x01, 0x63, 0x01, 0x02, 0x16,
            0x02, 0x03, 0x65, 0x6E, 0x76, 0x01, 0x67, 0x03, 0x63, 0x00, 0x00, 0x03, 0x65, 0x6E,
            0x76, 0x01, 0x74, 0x01, 0x63, 0x01, 0x00, 0x01, 0x03, 0x02, 0x01, 0x02, 0x0A, 0x0E,
            0x01, 0x0C, 0x00, 0x41, 0x00, 0x23, 0x00, 0x26, 0x00, 0x41, 0x00, 0x25, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(63));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "table.set",
                expected: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                }),
                found: ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                }),
            }
        ));
    }

    #[test]
    fn reject_typed_passive_element_from_defined_typed_global_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/defined-typed-global-passive-element-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(48));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementExprTypeMismatch { expected, found }
                if expected == ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_passive_element_from_imported_typed_global_with_wrong_concrete_type() {
        let bytes = include_bytes!(
            "../../../baedeker-testdata/spec/invalid-validate/imported-typed-global-passive-element-wrong-concrete-type.wasm",
        );
        let module = Module::decode(bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(41));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementExprTypeMismatch { expected, found }
                if expected == ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })
                    && found == ValType::Ref(RefType::Typed {
                        nullable: true,
                        heap: crate::types::HeapType::Type(TypeIdx(0)),
                    })
        ));
    }

    #[test]
    fn reject_typed_if_result_with_wrong_concrete_type() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7E, 0x60, 0x01, 0x7F, 0x01, 0x63, 0x01, 0x03,
            0x03, 0x02, 0x00, 0x02, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0A, 0x14, 0x02,
            0x04, 0x00, 0x20, 0x00, 0x0B, 0x0D, 0x00, 0x20, 0x00, 0x04, 0x63, 0x01, 0xD2, 0x00,
            0x05, 0xD0, 0x01, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(56));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected == vec![ValType::Ref(RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                })] && found == vec![ValType::Ref(RefType::Typed {
                    nullable: false,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                })]
        ));
    }

    #[test]
    fn reject_table_init_with_incompatible_typed_element_segment() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0E, 0x03, 0x60, 0x01, 0x7F,
            0x01, 0x7F, 0x60, 0x01, 0x7E, 0x01, 0x7E, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00,
            0x02, 0x04, 0x05, 0x01, 0x63, 0x01, 0x00, 0x04, 0x07, 0x0C, 0x02, 0x01, 0x66, 0x00,
            0x00, 0x04, 0x69, 0x6E, 0x69, 0x74, 0x00, 0x01, 0x09, 0x08, 0x01, 0x05, 0x63, 0x00,
            0x01, 0xD2, 0x00, 0x0B, 0x0A, 0x13, 0x02, 0x04, 0x00, 0x20, 0x00, 0x0B, 0x0C, 0x00,
            0x41, 0x00, 0x41, 0x00, 0x41, 0x01, 0xFC, 0x0C, 0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(76));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ElementTableTypeMismatch {
                expected: RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(1)),
                },
                found: RefType::Typed {
                    nullable: true,
                    heap: crate::types::HeapType::Type(TypeIdx(0)),
                },
            }
        ));
    }

    #[test]
    fn reject_return_call_indirect_result_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x01, 0x7E, 0x03, 0x02, 0x01, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00,
            0x01, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x41, 0x00, 0x13, 0x01, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(36));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ResultTypeMismatch {
                expected,
                found,
            } if expected == vec![ValType::Num(crate::types::NumType::I32)]
                && found == vec![ValType::Num(crate::types::NumType::I64)]
        ));
    }

    #[test]
    fn reject_call_indirect_with_non_funcref_table() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x02, 0x0D, 0x01, 0x03, b'e', b'n', b'v', 0x03, b't', b'a', b'b', 0x01, 0x6F, 0x00,
            0x01, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x41, 0x00, 0x11, 0x00,
            0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidCallIndirectTableType {
                expected: RefType::FuncRef,
                found: RefType::ExternRef,
            }
        ));
    }

    #[test]
    fn reject_return_call_indirect_with_non_funcref_table() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x04, 0x04, 0x01, 0x6F, 0x00, 0x01, 0x0A, 0x09, 0x01,
            0x07, 0x00, 0x41, 0x00, 0x13, 0x00, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(32));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidCallIndirectTableType {
                expected: RefType::FuncRef,
                found: RefType::ExternRef,
            }
        ));
    }

    #[test]
    fn validate_table_get_set_size_and_grow() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x04, 0x04, 0x01, 0x70, 0x00,
            0x01, 0x0A, 0x1D, 0x02, 0x0A, 0x00, 0x41, 0x00, 0x25, 0x00, 0x1A, 0xFC, 0x10, 0x00,
            0x0B, 0x10, 0x00, 0x41, 0x00, 0xD0, 0x70, 0x26, 0x00, 0xD0, 0x70, 0x41, 0x01, 0xFC,
            0x0F, 0x00, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_table_get_set_size_and_grow_on_nonzero_table() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x00, 0x03, 0x03, 0x02, 0x00, 0x01, 0x04, 0x07, 0x02, 0x70, 0x00,
            0x01, 0x70, 0x00, 0x01, 0x0A, 0x1D, 0x02, 0x0A, 0x00, 0x41, 0x00, 0x25, 0x01, 0x1A,
            0xFC, 0x10, 0x01, 0x0B, 0x10, 0x00, 0x41, 0x00, 0xD0, 0x70, 0x26, 0x01, 0xD0, 0x70,
            0x41, 0x01, 0xFC, 0x0F, 0x01, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn validate_broader_numeric_operators() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x04, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x01, 0x7E, 0x60, 0x00, 0x01, 0x7D, 0x60, 0x00, 0x01, 0x7C, 0x03,
            0x05, 0x04, 0x00, 0x01, 0x02, 0x03, 0x0A, 0x26, 0x04, 0x08, 0x00, 0x41, 0x03, 0x41,
            0x01, 0x6B, 0x45, 0x0B, 0x05, 0x00, 0x42, 0x05, 0x79, 0x0B, 0x08, 0x00, 0x43, 0x00,
            0x00, 0x80, 0x3F, 0x8B, 0x0B, 0x0C, 0x00, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xF0, 0x3F, 0x99, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_broader_numeric_operator_type_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x0A, 0x01, 0x08, 0x00, 0x43, 0x00, 0x00, 0x80,
            0x3F, 0x67, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "i32.unary",
                expected: ValType::Num(crate::types::NumType::I32),
                found: ValType::Num(crate::types::NumType::F32),
            }
        ));
    }

    #[test]
    fn validate_conversions_and_reinterpretations() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x14, 0x05, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x01, 0x7E, 0x60, 0x00, 0x01, 0x7D, 0x60, 0x00, 0x01, 0x7C, 0x60,
            0x00, 0x00, 0x03, 0x06, 0x05, 0x00, 0x01, 0x02, 0x03, 0x04, 0x0A, 0x2A, 0x05, 0x05,
            0x00, 0x42, 0x2A, 0xA7, 0x0B, 0x0C, 0x00, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xF0, 0x3F, 0xBD, 0x0B, 0x05, 0x00, 0x41, 0x7F, 0xBE, 0x0B, 0x08, 0x00, 0x43, 0x00,
            0x00, 0x40, 0x40, 0xBB, 0x0B, 0x06, 0x00, 0x42, 0x7F, 0xC4, 0x1A, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_conversion_type_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7F, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x07, 0x01, 0x05, 0x00, 0x41, 0x01, 0xA8, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "i32.trunc_f32",
                expected: ValType::Num(crate::types::NumType::F32),
                found: ValType::Num(crate::types::NumType::I32),
            }
        ));
    }

    #[test]
    fn validate_saturating_truncation_variants() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x04, 0x60, 0x00, 0x01,
            0x7F, 0x60, 0x00, 0x01, 0x7E, 0x60, 0x00, 0x01, 0x7D, 0x60, 0x00, 0x01, 0x7C, 0x03,
            0x05, 0x04, 0x00, 0x01, 0x01, 0x00, 0x0A, 0x31, 0x04, 0x09, 0x00, 0x43, 0x00, 0x00,
            0x80, 0x3F, 0xFC, 0x00, 0x0B, 0x0D, 0x00, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xF0, 0x3F, 0xFC, 0x07, 0x0B, 0x09, 0x00, 0x43, 0x00, 0x00, 0x80, 0x3F, 0xFC, 0x04,
            0x0B, 0x0D, 0x00, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF0, 0x3F, 0xFC, 0x02,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        module.validate().unwrap();
    }

    #[test]
    fn reject_saturating_truncation_type_mismatch() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7E, 0x03, 0x02, 0x01, 0x00, 0x0A, 0x08, 0x01, 0x06, 0x00, 0x42, 0x00, 0xFC, 0x04,
            0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(26));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::TypeMismatch {
                op: "i64.trunc_sat_f32",
                expected: ValType::Num(crate::types::NumType::F32),
                found: ValType::Num(crate::types::NumType::I64),
            }
        ));
    }
}
