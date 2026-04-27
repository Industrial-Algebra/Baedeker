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

use alloc::{collections::BTreeSet, vec, vec::Vec};

use crate::binary::instr::{DecodedInstr, Instr, decode_instr_sequence_with_offsets};
use crate::binary::module::Module;
use crate::error::ByteOffset;
use crate::types::{
    BlockType, DataIdx, DataMode, ElemIdx, ElementInit, ElementMode, ExportDesc, FuncIdx, FuncType,
    GlobalIdx, ImportDesc, LocalDecl, MemIdx, Mutability, RefType, TableIdx, TypeIdx, ValType,
};
use crate::validate::error::{ValidationError, ValidationErrorKind};
use crate::validate::state::{ControlKind::*, OperandType, Reachability, ValidationState};

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
    validate_tables(module)?;
    validate_globals(module)?;
    validate_data_segments(module)?;
    validate_bulk_memory(module)?;
    validate_exports(module)?;
    validate_elements(module)?;
    validate_start(module)?;

    for (func_idx, (type_idx, code)) in module.functions.iter().zip(module.codes()).enumerate() {
        let ty = &module.types[type_idx.0 as usize];
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

fn validate_type_definitions(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Type)
        .map(|section| section.offset)
        .unwrap_or(0);

    for ty in &module.types {
        validate_func_type_type_indices(module, ty, None, offset)?;
    }

    Ok(())
}

fn validate_imports(module: &Module<'_>) -> Result<(), ValidationError> {
    let offset = module
        .section(crate::binary::section::SectionId::Import)
        .map(|section| section.offset)
        .unwrap_or(0);

    for import in module.imports() {
        match import.desc {
            ImportDesc::Func(type_idx) => {
                if module.types.get(type_idx.0 as usize).is_none() {
                    return Err(ValidationError {
                        offset: ByteOffset(offset),
                        function: None,
                        kind: ValidationErrorKind::UnknownTypeIdx { idx: type_idx },
                    });
                }
            }
            ImportDesc::Table(table) => {
                validate_reftype_type_indices(module, table.elem, None, offset)?;
            }
            ImportDesc::Global(global) => {
                validate_valtype_type_indices(module, global.val_type, None, offset)?;
            }
            ImportDesc::Mem(_) => {}
        }
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

    let available_globals: Vec<_> = match scope {
        ConstExprGlobalScope::ImportedPlusDefined {
            defined_globals_available,
        } => imported_globals
            .chain(defined_globals.take(defined_globals_available))
            .collect(),
        ConstExprGlobalScope::All => imported_globals.chain(defined_globals).collect(),
    };

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
            resolve_memory_type(module, *memory, FuncIdx(0), *offset_offset)?;
            validate_const_i32_expr(module, offset_expr, *offset_offset)?;
        }
    }

    Ok(())
}

fn validate_const_i32_expr(
    module: &Module<'_>,
    expr: &[u8],
    offset: usize,
) -> Result<(), ValidationError> {
    validate_const_expr(
        module,
        expr,
        offset,
        ValType::Num(crate::types::NumType::I32),
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
    if !valtype_matches(found, expected) {
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
        Instr::I32Const(_) if !matches!(kind, ConstExprKind::ElementExpr) => {
            stack.push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I64Const(_) if !matches!(kind, ConstExprKind::ElementExpr) => {
            stack.push(ValType::Num(crate::types::NumType::I64));
        }
        Instr::F32Const(_) if !matches!(kind, ConstExprKind::ElementExpr) => {
            stack.push(ValType::Num(crate::types::NumType::F32));
        }
        Instr::F64Const(_) if !matches!(kind, ConstExprKind::ElementExpr) => {
            stack.push(ValType::Num(crate::types::NumType::F64));
        }
        Instr::RefNull(ref_type) => {
            validate_reftype_type_indices(module, ref_type, None, decoded.offset.0)?;
            stack.push(normalize_valtype(module, ValType::Ref(ref_type)));
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
        Instr::I32Add | Instr::I32Sub | Instr::I32Mul
            if matches!(kind, ConstExprKind::GlobalInit | ConstExprKind::I32Offset) =>
        {
            pop_const_expect(
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            pop_const_expect(
                stack,
                ValType::Num(crate::types::NumType::I32),
                decoded.offset,
                kind,
            )?;
            stack.push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I64Add | Instr::I64Sub | Instr::I64Mul
            if matches!(kind, ConstExprKind::GlobalInit) =>
        {
            pop_const_expect(
                stack,
                ValType::Num(crate::types::NumType::I64),
                decoded.offset,
                kind,
            )?;
            pop_const_expect(
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
    stack: &mut Vec<ValType>,
    expected: ValType,
    offset: ByteOffset,
    kind: ConstExprKind,
) -> Result<(), ValidationError> {
    let Some(found) = stack.pop() else {
        return Err(match kind {
            ConstExprKind::ElementExpr => ValidationError {
                offset,
                function: None,
                kind: ValidationErrorKind::ElementExprTypeMismatch {
                    expected,
                    found: expected,
                },
            },
            ConstExprKind::GlobalInit | ConstExprKind::I32Offset => ValidationError {
                offset,
                function: None,
                kind: ValidationErrorKind::GlobalInitTypeMismatch {
                    expected,
                    found: expected,
                },
            },
        });
    };

    if !valtype_matches(found, expected) {
        return Err(match kind {
            ConstExprKind::ElementExpr => ValidationError {
                offset,
                function: None,
                kind: ValidationErrorKind::ElementExprTypeMismatch { expected, found },
            },
            ConstExprKind::GlobalInit | ConstExprKind::I32Offset => ValidationError {
                offset,
                function: None,
                kind: ValidationErrorKind::GlobalInitTypeMismatch { expected, found },
            },
        });
    }

    Ok(())
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
    let mut names = BTreeSet::new();

    for export in module.exports() {
        if !names.insert(export.name.as_str()) {
            return Err(ValidationError {
                offset: ByteOffset(offset),
                function: None,
                kind: ValidationErrorKind::DuplicateExportName {
                    name: export.name.clone(),
                },
            });
        }

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
            if !reftype_matches(elem_type, table_type.elem) {
                return Err(ValidationError {
                    offset: ByteOffset(*offset_offset),
                    function: None,
                    kind: ValidationErrorKind::ElementTableTypeMismatch {
                        expected: table_type.elem,
                        found: elem_type,
                    },
                });
            }
            validate_const_i32_expr(module, offset_expr, *offset_offset)?;
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
    let ty = normalize_func_type(module, ty);
    let mut locals = ty.params.clone();
    expand_locals(module, function, &mut locals, local_decls, code.body_offset)?;

    let instrs = code
        .instructions_with_offsets()
        .map_err(|e| ValidationError {
            offset: e.offset,
            function: Some(function),
            kind: e.into(),
        })?;

    let local_inits = initial_local_inits(&locals, ty.params.len());
    let mut state = ValidationState::new(locals, local_inits, ty.results.clone());

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

fn initial_local_inits(locals: &[ValType], param_count: usize) -> Vec<bool> {
    locals
        .iter()
        .enumerate()
        .map(|(idx, ty)| idx < param_count || valtype_is_defaultable(*ty))
        .collect()
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
            let (outer_height, start_types, end_types, local_inits) = {
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
                    frame.start_types.clone(),
                    frame.end_types.clone(),
                    frame.local_inits.clone(),
                )
            };
            pop_control_result_types(function, state, &end_types, offset)?;
            state.operands.truncate(outer_height);
            state.local_inits = local_inits;
            for ty in start_types {
                state.operands.push(ty);
            }
            let floor = state.operands.len();
            let frame = state.current_frame_mut();
            frame.stack_floor = floor;
            state.reachability = Reachability::Reachable;
        }
        Instr::Block(block_type) => {
            let sig = resolve_block_type(module, function, *block_type, offset)?;
            pop_exact(function, state, &sig.params, offset)?;
            state.push_frame(Block, *block_type, sig.params, sig.results);
        }
        Instr::Loop(block_type) => {
            let sig = resolve_block_type(module, function, *block_type, offset)?;
            pop_exact(function, state, &sig.params, offset)?;
            state.push_frame(Loop, *block_type, sig.params, sig.results);
        }
        Instr::If(block_type) => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "if",
            )?;
            let sig = resolve_block_type(module, function, *block_type, offset)?;
            pop_exact(function, state, &sig.params, offset)?;
            state.push_frame(If, *block_type, sig.params, sig.results);
        }
        Instr::End => {
            if state.controls.len() > 1 {
                finish_frame(function, state, offset)?;
            }
        }
        Instr::Br(label) => {
            let label_types = validate_label(function, state, *label, offset)?.to_vec();
            pop_branch_types(function, state, *label, &label_types, offset)?;
            state.enter_unreachable();
        }
        Instr::BrIf(label) => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "br_if",
            )?;
            let label_types = validate_label(function, state, *label, offset)?.to_vec();
            pop_branch_types(function, state, *label, &label_types, offset)?;
            for ty in label_types {
                state.operands.push(ty);
            }
        }
        Instr::BrOnNull(label) => {
            let label_types = validate_label(function, state, *label, offset)?.to_vec();
            let found = pop_ref_type(function, state, offset, "br_on_null")?;
            pop_branch_types(function, state, *label, &label_types, offset)?;
            for ty in label_types {
                state.operands.push(ty);
            }
            match found {
                Some(found) => state.operands.push(ValType::Ref(found.as_non_null())),
                None => state.operands.push_bottom(),
            }
        }
        Instr::BrOnNonNull(label) => {
            let label_types = validate_label(function, state, *label, offset)?.to_vec();
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
                && !reftype_matches(found, expected_input)
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
            pop_branch_types(function, state, *label, rest, offset)?;
            for ty in rest {
                state.operands.push(*ty);
            }
        }
        Instr::BrTable { targets, default } => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "br_table",
            )?;
            let default_types = validate_label(function, state, *default, offset)?.to_vec();
            if state.reachability == Reachability::Reachable {
                for label in targets {
                    let label_types = validate_label(function, state, *label, offset)?;
                    if label_types != default_types.as_slice() {
                        return Err(ValidationError {
                            offset: ByteOffset(offset),
                            function: Some(function),
                            kind: ValidationErrorKind::InconsistentBranchTypes {
                                expected: default_types.clone(),
                                found: label_types.to_vec(),
                            },
                        });
                    }
                }
            } else {
                ensure_stack_types(state, &default_types).map_err(|found| ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::BranchTypeMismatch {
                        label: *default,
                        expected: default_types.clone(),
                        found,
                    },
                })?;
                for label in targets {
                    let label_types = validate_label(function, state, *label, offset)?;
                    ensure_stack_types(state, label_types).map_err(|found| ValidationError {
                        offset: ByteOffset(offset),
                        function: Some(function),
                        kind: ValidationErrorKind::BranchTypeMismatch {
                            label: *label,
                            expected: label_types.to_vec(),
                            found,
                        },
                    })?;
                }
            }
            pop_branch_types(function, state, *default, &default_types, offset)?;
            state.enter_unreachable();
        }
        Instr::Return => {
            let expected = state.controls[0].end_types.clone();
            pop_control_result_types(function, state, &expected, offset)?;
            state.enter_unreachable();
        }
        Instr::Call(idx) => {
            let ty = resolve_func_type(module, *idx, function, offset)?;
            pop_exact(function, state, &ty.params, offset)?;
            for result in &ty.results {
                state.operands.push(*result);
            }
        }
        Instr::ReturnCall(idx) => {
            let ty = resolve_func_type(module, *idx, function, offset)?;
            validate_tail_call_results(function, state, &ty.results, offset)?;
            pop_exact(function, state, &ty.params, offset)?;
            state.enter_unreachable();
        }
        Instr::CallRef(type_idx) => {
            let ty = resolve_type(module, *type_idx, Some(function), offset)?;
            pop_expect(
                function,
                state,
                ValType::Ref(RefType::concrete(
                    true,
                    canonicalize_func_type_idx(module, *type_idx),
                )),
                offset,
                "call_ref",
            )?;
            pop_exact(function, state, &ty.params, offset)?;
            for result in &ty.results {
                state.operands.push(*result);
            }
        }
        Instr::ReturnCallRef(type_idx) => {
            let ty = resolve_type(module, *type_idx, Some(function), offset)?;
            validate_tail_call_results(function, state, &ty.results, offset)?;
            pop_expect(
                function,
                state,
                ValType::Ref(RefType::concrete(
                    true,
                    canonicalize_func_type_idx(module, *type_idx),
                )),
                offset,
                "return_call_ref",
            )?;
            pop_exact(function, state, &ty.params, offset)?;
            state.enter_unreachable();
        }
        Instr::CallIndirect {
            type_idx,
            table_idx,
        } => {
            let table_type =
                resolve_table_type_with_context(module, *table_idx, Some(function), offset)?;
            if !reftype_matches(table_type.elem, RefType::FuncRef) {
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
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "call_indirect",
            )?;
            pop_exact(function, state, &ty.params, offset)?;
            for result in &ty.results {
                state.operands.push(*result);
            }
        }
        Instr::ReturnCallIndirect {
            type_idx,
            table_idx,
        } => {
            let table_type =
                resolve_table_type_with_context(module, *table_idx, Some(function), offset)?;
            if !reftype_matches(table_type.elem, RefType::FuncRef) {
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
            validate_tail_call_results(function, state, &ty.results, offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "return_call_indirect",
            )?;
            pop_exact(function, state, &ty.params, offset)?;
            state.enter_unreachable();
        }
        Instr::Drop => {
            pop_any(function, state, offset, "drop")?;
        }
        Instr::Select => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "select",
            )?;
            let rhs = pop_operand_type(function, state, offset, "select")?;
            let lhs = pop_operand_type(function, state, offset, "select")?;
            match (lhs, rhs) {
                (OperandType::Bottom, OperandType::Bottom) => state.operands.push_bottom(),
                (OperandType::Bottom, OperandType::Typed(rhs))
                | (OperandType::Typed(rhs), OperandType::Bottom) => state.operands.push(rhs),
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
                    state.operands.push(lhs);
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
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "select_typed",
            )?;
            let rhs = pop_operand_type(function, state, offset, "select_typed")?;
            let lhs = pop_operand_type(function, state, offset, "select_typed")?;
            if !operand_matches(lhs, expected) || !operand_matches(rhs, expected) {
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
            state.operands.push(expected);
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
            state.operands.push(ty);
        }
        Instr::LocalSet(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(function, state, ty, offset, "local.set")?;
            state.local_inits[idx.0 as usize] = true;
        }
        Instr::LocalTee(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(function, state, ty, offset, "local.tee")?;
            state.local_inits[idx.0 as usize] = true;
            state.operands.push(ty);
        }
        Instr::GlobalGet(idx) => {
            let ty = resolve_global_type(module, *idx, function, offset)?;
            state.operands.push(ty.val_type);
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
            pop_expect(function, state, ty.val_type, offset, "global.set")?;
        }
        Instr::TableGet(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.get",
            )?;
            state.operands.push(ValType::Ref(ty.elem));
        }
        Instr::TableSet(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            pop_expect(function, state, ValType::Ref(ty.elem), offset, "table.set")?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
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
                max_align: 2,
                stored: ValType::Num(crate::types::NumType::I64),
            },
        )?,
        Instr::MemoryInit(data_idx, mem_idx) => {
            resolve_data_segment(module, *data_idx, function, offset)?;
            resolve_memory_type(module, *mem_idx, function, offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.init",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.init",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.init",
            )?;
        }
        Instr::DataDrop(data_idx) => {
            resolve_data_segment(module, *data_idx, function, offset)?;
        }
        Instr::MemoryCopy { dst, src } => {
            resolve_memory_type(module, *dst, function, offset)?;
            resolve_memory_type(module, *src, function, offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.copy",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.copy",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.copy",
            )?;
        }
        Instr::MemoryFill(mem_idx) => {
            resolve_memory_type(module, *mem_idx, function, offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.fill",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.fill",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
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
            if !reftype_matches(elem_type, table.elem) {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::ElementTableTypeMismatch {
                        expected: table.elem,
                        found: elem_type,
                    },
                });
            }
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.init",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.init",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
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
            if !reftype_matches(src_ty.elem, dst_ty.elem)
                || !reftype_matches(dst_ty.elem, src_ty.elem)
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
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.copy",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.copy",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.copy",
            )?;
        }
        Instr::TableGrow(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.grow",
            )?;
            pop_expect(function, state, ValType::Ref(ty.elem), offset, "table.grow")?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::TableSize(idx) => {
            let _ = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::TableFill(idx) => {
            let ty = resolve_table_type_with_context(module, *idx, Some(function), offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.fill",
            )?;
            pop_expect(function, state, ValType::Ref(ty.elem), offset, "table.fill")?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "table.fill",
            )?;
        }
        Instr::MemorySize(idx) => {
            resolve_memory_type(module, *idx, function, offset)?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::MemoryGrow(idx) => {
            resolve_memory_type(module, *idx, function, offset)?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "memory.grow",
            )?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I32Const(_) => state
            .operands
            .push(ValType::Num(crate::types::NumType::I32)),
        Instr::I64Const(_) => state
            .operands
            .push(ValType::Num(crate::types::NumType::I64)),
        Instr::F32Const(_) => state
            .operands
            .push(ValType::Num(crate::types::NumType::F32)),
        Instr::F64Const(_) => state
            .operands
            .push(ValType::Num(crate::types::NumType::F64)),
        Instr::RefNull(ref_type) => {
            validate_reftype_type_indices(module, *ref_type, Some(function), offset)?;
            state
                .operands
                .push(normalize_valtype(module, ValType::Ref(*ref_type)));
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
            state
                .operands
                .push(ValType::Ref(RefType::concrete(false, type_idx)));
        }
        Instr::V128Const(_) => state
            .operands
            .push(ValType::Vec(crate::types::VecType::V128)),
        Instr::I32Eqz => validate_numeric_unary(
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
            function,
            state,
            offset,
            "i32.compare",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Eqz => validate_numeric_unary(
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
            function,
            state,
            offset,
            "i64.compare",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::F32Eq | Instr::F32Ne | Instr::F32Lt | Instr::F32Gt | Instr::F32Le | Instr::F32Ge => {
            validate_numeric_binary(
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
                function,
                state,
                offset,
                "f64.compare",
                ValType::Num(crate::types::NumType::F64),
                ValType::Num(crate::types::NumType::I32),
            )?
        }
        Instr::I32Clz | Instr::I32Ctz | Instr::I32Popcnt => validate_numeric_unary(
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
            function,
            state,
            offset,
            "i32.binary",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Clz | Instr::I64Ctz | Instr::I64Popcnt => validate_numeric_unary(
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
            function,
            state,
            offset,
            "f64.binary",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::I32WrapI64 => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.wrap_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32TruncF32S | Instr::I32TruncF32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.trunc_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32TruncF64S | Instr::I32TruncF64U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.trunc_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64ExtendI32S | Instr::I64ExtendI32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i64.extend_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64TruncF32S | Instr::I64TruncF32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i64.trunc_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64TruncF64S | Instr::I64TruncF64U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i64.trunc_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::F32ConvertI32S | Instr::F32ConvertI32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "f32.convert_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F32ConvertI64S | Instr::F32ConvertI64U => validate_numeric_conversion(
            function,
            state,
            offset,
            "f32.convert_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F32DemoteF64 => validate_numeric_conversion(
            function,
            state,
            offset,
            "f32.demote_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F64ConvertI32S | Instr::F64ConvertI32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "f64.convert_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::F64ConvertI64S | Instr::F64ConvertI64U => validate_numeric_conversion(
            function,
            state,
            offset,
            "f64.convert_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::F64PromoteF32 => validate_numeric_conversion(
            function,
            state,
            offset,
            "f64.promote_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::I32ReinterpretF32 => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.reinterpret_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64ReinterpretF64 => validate_numeric_conversion(
            function,
            state,
            offset,
            "i64.reinterpret_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::F32ReinterpretI32 => validate_numeric_conversion(
            function,
            state,
            offset,
            "f32.reinterpret_i32",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::F32),
        )?,
        Instr::F64ReinterpretI64 => validate_numeric_conversion(
            function,
            state,
            offset,
            "f64.reinterpret_i64",
            ValType::Num(crate::types::NumType::I64),
            ValType::Num(crate::types::NumType::F64),
        )?,
        Instr::I32Extend8S | Instr::I32Extend16S => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.sign_extend",
            ValType::Num(crate::types::NumType::I32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64Extend8S | Instr::I64Extend16S | Instr::I64Extend32S => {
            validate_numeric_conversion(
                function,
                state,
                offset,
                "i64.sign_extend",
                ValType::Num(crate::types::NumType::I64),
                ValType::Num(crate::types::NumType::I64),
            )?
        }
        Instr::I32TruncSatF32S | Instr::I32TruncSatF32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.trunc_sat_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I32TruncSatF64S | Instr::I32TruncSatF64U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i32.trunc_sat_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I32),
        )?,
        Instr::I64TruncSatF32S | Instr::I64TruncSatF32U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i64.trunc_sat_f32",
            ValType::Num(crate::types::NumType::F32),
            ValType::Num(crate::types::NumType::I64),
        )?,
        Instr::I64TruncSatF64S | Instr::I64TruncSatF64U => validate_numeric_conversion(
            function,
            state,
            offset,
            "i64.trunc_sat_f64",
            ValType::Num(crate::types::NumType::F64),
            ValType::Num(crate::types::NumType::I64),
        )?,
    }

    Ok(())
}

fn validate_numeric_unary(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    input: ValType,
    result: ValType,
) -> Result<(), ValidationError> {
    pop_expect(function, state, input, offset, op)?;
    state.operands.push(result);
    Ok(())
}

fn validate_numeric_binary(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    input: ValType,
    result: ValType,
) -> Result<(), ValidationError> {
    pop_expect(function, state, input, offset, op)?;
    pop_expect(function, state, input, offset, op)?;
    state.operands.push(result);
    Ok(())
}

fn validate_numeric_conversion(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    input: ValType,
    result: ValType,
) -> Result<(), ValidationError> {
    pop_expect(function, state, input, offset, op)?;
    state.operands.push(result);
    Ok(())
}

fn validate_ref_is_null(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    let _ = pop_ref_type(function, state, offset, "ref.is_null")?;
    state
        .operands
        .push(ValType::Num(crate::types::NumType::I32));
    Ok(())
}

fn validate_ref_as_non_null(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    let found = pop_ref_type(function, state, offset, "ref.as_non_null")?;
    match found {
        Some(found) => state.operands.push(ValType::Ref(found.as_non_null())),
        None => state.operands.push_bottom(),
    }
    Ok(())
}

struct MemLoadValidation {
    op: &'static str,
    memory: MemIdx,
    found_align: u32,
    max_align: u32,
    result: ValType,
}

struct MemStoreValidation {
    op: &'static str,
    memory: MemIdx,
    found_align: u32,
    max_align: u32,
    stored: ValType,
}

struct SimdLaneValidation {
    op: &'static str,
    memory: MemIdx,
    found_align: u32,
    max_align: u32,
    lane: u8,
    max_lane: u8,
}

fn validate_load(
    module: &Module<'_>,
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    load: MemLoadValidation,
) -> Result<(), ValidationError> {
    validate_memarg_align(function, offset, load.op, load.found_align, load.max_align)?;
    resolve_memory_type(module, load.memory, function, offset)?;
    pop_expect(
        function,
        state,
        ValType::Num(crate::types::NumType::I32),
        offset,
        load.op,
    )?;
    state.operands.push(load.result);
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
    resolve_memory_type(module, store.memory, function, offset)?;
    pop_expect(function, state, store.stored, offset, store.op)?;
    pop_expect(
        function,
        state,
        ValType::Num(crate::types::NumType::I32),
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
    validate_simd_lane_idx(function, offset, lane.op, lane.lane, lane.max_lane)?;
    resolve_memory_type(module, lane.memory, function, offset)?;
    pop_expect(
        function,
        state,
        ValType::Vec(crate::types::VecType::V128),
        offset,
        lane.op,
    )?;
    pop_expect(
        function,
        state,
        ValType::Num(crate::types::NumType::I32),
        offset,
        lane.op,
    )?;
    state
        .operands
        .push(ValType::Vec(crate::types::VecType::V128));
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
    validate_simd_lane_idx(function, offset, lane.op, lane.lane, lane.max_lane)?;
    resolve_memory_type(module, lane.memory, function, offset)?;
    pop_expect(
        function,
        state,
        ValType::Vec(crate::types::VecType::V128),
        offset,
        lane.op,
    )?;
    pop_expect(
        function,
        state,
        ValType::Num(crate::types::NumType::I32),
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
                results: vec![normalize_valtype(module, val)],
            })
        }
        BlockType::TypeIdx(idx) => module
            .types
            .get(idx as usize)
            .map(|ty| normalize_func_type(module, ty))
            .ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::InvalidBlockType { block_type },
            }),
    }
}

fn resolve_type(
    module: &Module<'_>,
    idx: TypeIdx,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<FuncType, ValidationError> {
    let idx = canonicalize_func_type_idx(module, idx);
    module
        .types
        .get(idx.0 as usize)
        .map(|ty| normalize_func_type(module, ty))
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function,
            kind: ValidationErrorKind::UnknownTypeIdx { idx },
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

    module
        .types
        .get(type_idx.0 as usize)
        .ok_or(ValidationError {
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
        .filter_map(|import| match import.desc {
            ImportDesc::Table(table) => Some(table),
            _ => None,
        });
    let defined = module.tables.iter().copied();

    imported
        .chain(defined)
        .nth(idx.0 as usize)
        .map(|table| normalize_table_type(module, table))
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function,
            kind: ValidationErrorKind::UnknownTableIdx { idx, available },
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

fn finish_frame(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    let frame = state.pop_frame().ok_or(ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::UnexpectedEnd,
    })?;

    if frame.kind == If && !frame.has_else && !frame.end_types.is_empty() {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::MissingElseForResult,
        });
    }

    if let Err(found) = ensure_frame_end_types(state, frame.outer_height, &frame.end_types) {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::ControlResultTypeMismatch {
                expected: frame.end_types.clone(),
                found,
            },
        });
    }

    pop_control_result_types(function, state, &frame.end_types, offset)?;
    state.operands.truncate(frame.outer_height);
    state.local_inits = frame.local_inits;
    for ty in frame.end_types {
        state.operands.push(ty);
    }
    state.reachability = Reachability::Reachable;
    Ok(())
}

fn canonicalize_func_type_idx(module: &Module<'_>, idx: TypeIdx) -> TypeIdx {
    if module.types.get(idx.0 as usize).is_none() {
        return idx;
    }

    for candidate in 0..idx.0 {
        let candidate = TypeIdx(candidate);
        if func_type_indices_equivalent(module, candidate, idx) {
            return candidate;
        }
    }

    idx
}

fn func_type_indices_equivalent(module: &Module<'_>, lhs: TypeIdx, rhs: TypeIdx) -> bool {
    let mut seen = BTreeSet::new();
    func_type_indices_equivalent_inner(module, lhs, rhs, &mut seen)
}

fn func_type_indices_equivalent_inner(
    module: &Module<'_>,
    lhs: TypeIdx,
    rhs: TypeIdx,
    seen: &mut BTreeSet<(u32, u32)>,
) -> bool {
    let key = if lhs.0 <= rhs.0 {
        (lhs.0, rhs.0)
    } else {
        (rhs.0, lhs.0)
    };
    if !seen.insert(key) {
        return true;
    }

    let Some(lhs_ty) = module.types.get(lhs.0 as usize) else {
        return false;
    };
    let Some(rhs_ty) = module.types.get(rhs.0 as usize) else {
        return false;
    };

    lhs_ty.params.len() == rhs_ty.params.len()
        && lhs_ty.results.len() == rhs_ty.results.len()
        && lhs_ty
            .params
            .iter()
            .zip(&rhs_ty.params)
            .all(|(lhs, rhs)| valtype_equivalent_inner(module, *lhs, *rhs, seen))
        && lhs_ty
            .results
            .iter()
            .zip(&rhs_ty.results)
            .all(|(lhs, rhs)| valtype_equivalent_inner(module, *lhs, *rhs, seen))
}

fn valtype_equivalent_inner(
    module: &Module<'_>,
    lhs: ValType,
    rhs: ValType,
    seen: &mut BTreeSet<(u32, u32)>,
) -> bool {
    match (lhs, rhs) {
        (ValType::Ref(lhs), ValType::Ref(rhs)) => reftype_equivalent_inner(module, lhs, rhs, seen),
        _ => lhs == rhs,
    }
}

fn reftype_equivalent_inner(
    module: &Module<'_>,
    lhs: RefType,
    rhs: RefType,
    seen: &mut BTreeSet<(u32, u32)>,
) -> bool {
    lhs.is_nullable() == rhs.is_nullable()
        && match (lhs.heap_type(), rhs.heap_type()) {
            (crate::types::HeapType::Type(lhs), crate::types::HeapType::Type(rhs)) => {
                func_type_indices_equivalent_inner(module, lhs, rhs, seen)
            }
            (lhs, rhs) => lhs == rhs,
        }
}

fn validate_func_type_type_indices(
    module: &Module<'_>,
    ty: &FuncType,
    function: Option<FuncIdx>,
    offset: usize,
) -> Result<(), ValidationError> {
    for &param in &ty.params {
        validate_valtype_type_indices(module, param, function, offset)?;
    }
    for &result in &ty.results {
        validate_valtype_type_indices(module, result, function, offset)?;
    }
    Ok(())
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

fn normalize_func_type(module: &Module<'_>, ty: &FuncType) -> FuncType {
    FuncType {
        params: ty
            .params
            .iter()
            .copied()
            .map(|ty| normalize_valtype(module, ty))
            .collect(),
        results: ty
            .results
            .iter()
            .copied()
            .map(|ty| normalize_valtype(module, ty))
            .collect(),
    }
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
    }
}

fn reftype_matches(found: RefType, expected: RefType) -> bool {
    found.is_subtype_of(expected)
}

fn valtype_matches(found: ValType, expected: ValType) -> bool {
    found.is_subtype_of(expected)
}

fn valtype_vec_matches(found: &[ValType], expected: &[ValType]) -> bool {
    found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|(found, expected)| valtype_matches(*found, *expected))
}

fn is_stack_polymorphic(state: &ValidationState) -> bool {
    state.reachability == Reachability::Unreachable
        && state.operands.len() == state.current_frame().stack_floor
}

fn operand_matches(found: OperandType, expected: ValType) -> bool {
    match found {
        OperandType::Typed(found) => valtype_matches(found, expected),
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
    state
        .operands
        .as_slice()
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
    state: &ValidationState,
    outer_height: usize,
    expected: &[ValType],
) -> Result<(), Vec<ValType>> {
    let operands = state.operands.as_slice();
    if operands.len() < outer_height {
        return Err(Vec::new());
    }
    let actual = &operands[outer_height..];
    let found = stack_found(actual, expected);

    if actual.len() > expected.len() {
        return Err(found);
    }

    let expected_suffix = &expected[expected.len().saturating_sub(actual.len())..];
    if actual
        .iter()
        .zip(expected_suffix)
        .all(|(found, expected)| operand_matches(*found, *expected))
        && (state.reachability == Reachability::Unreachable || actual.len() == expected.len())
    {
        Ok(())
    } else {
        Err(found)
    }
}

fn pop_operand(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
    expected: &[ValType],
) -> Result<OperandType, ValidationError> {
    match state.operands.pop() {
        Some(found) => Ok(found),
        None if is_stack_polymorphic(state) => Ok(OperandType::Bottom),
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
    ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::StackUnderflow {
            op,
            expected: expected.to_vec(),
            available: concrete_stack(state),
        },
    }
}

fn pop_expect(
    function: FuncIdx,
    state: &mut ValidationState,
    expected: ValType,
    offset: usize,
    op: &'static str,
) -> Result<(), ValidationError> {
    let found = pop_operand(function, state, offset, op, &[expected])?;
    if !operand_matches(found, expected) {
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
    function: FuncIdx,
    state: &mut ValidationState,
    expected: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    for expected_ty in expected.iter().rev() {
        pop_expect(function, state, *expected_ty, offset, "stack")?;
    }
    Ok(())
}

fn validate_tail_call_results(
    function: FuncIdx,
    state: &ValidationState,
    found: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    let expected = &state.controls[0].end_types;
    if !valtype_vec_matches(found, expected) {
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
    function: FuncIdx,
    state: &mut ValidationState,
    label: crate::types::LabelIdx,
    expected: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    ensure_stack_types(state, expected).map_err(|found| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::BranchTypeMismatch {
            label,
            expected: expected.to_vec(),
            found,
        },
    })?;
    pop_exact(function, state, expected, offset)
}

fn pop_control_result_types(
    function: FuncIdx,
    state: &mut ValidationState,
    expected: &[ValType],
    offset: usize,
) -> Result<(), ValidationError> {
    ensure_stack_types(state, expected).map_err(|found| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind: ValidationErrorKind::ControlResultTypeMismatch {
            expected: expected.to_vec(),
            found,
        },
    })?;
    pop_exact(function, state, expected, offset)
}

fn ensure_stack_types(state: &ValidationState, expected: &[ValType]) -> Result<(), Vec<ValType>> {
    if is_stack_polymorphic(state) {
        return Ok(());
    }

    let operands = state.operands.as_slice();
    let found_len = core::cmp::min(operands.len(), expected.len());
    let found = &operands[operands.len().saturating_sub(found_len)..];
    let expected_suffix = &expected[expected.len() - found_len..];

    if (state.reachability == Reachability::Reachable && operands.len() < expected.len())
        || !found
            .iter()
            .zip(expected_suffix)
            .all(|(found, expected)| operand_matches(*found, *expected))
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
            0x03, 0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x0A, 0x0E, 0x01, 0x0C, 0x00,
            0x41, 0x00, 0x41, 0x00, 0x41, 0x01, 0xFC, 0x08, 0x00, 0x00, 0x0B, 0x0C, 0x01, 0x00,
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
        assert_eq!(err.offset, ByteOffset(36));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::ControlResultTypeMismatch { expected, found }
                if expected.is_empty() && found.is_empty()
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
    fn reject_br_table_with_inconsistent_target_types() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x10, 0x01, 0x0E, 0x00, 0x02, 0x7F, 0x02, 0x7E, 0x41,
            0x00, 0x0E, 0x01, 0x00, 0x01, 0x0B, 0x0B, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert_eq!(err.offset, ByteOffset(29));
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InconsistentBranchTypes { expected, found }
                if expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && found == vec![ValType::Num(crate::types::NumType::I64)]
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
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InconsistentBranchTypes { expected, found }
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
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InconsistentBranchTypes { expected, found }
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
    fn reject_non_constant_element_expr() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x01,
            0x09, 0x07, 0x01, 0x05, 0x70, 0x01, 0x41, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::NonConstantElementExpr
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
