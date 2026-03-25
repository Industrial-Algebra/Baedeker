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

use crate::binary::instr::{DecodedInstr, Instr};
use crate::binary::module::Module;
use crate::error::ByteOffset;
use crate::types::{BlockType, FuncIdx, FuncType, ImportDesc, LocalDecl, ValType};
use crate::validate::error::{ValidationError, ValidationErrorKind};
use crate::validate::state::{ControlKind::*, Reachability, ValidationState};

pub use error::{ValidationError as Error, ValidationErrorKind as ErrorKind};
pub use state::{
    ControlFrame, Reachability as ValidationReachability, TypeStack,
    ValidationState as FunctionValidationState,
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

fn validate_function(
    module: &Module<'_>,
    function: FuncIdx,
    ty: &FuncType,
    local_decls: &[LocalDecl],
    code: &crate::types::CodeBody<'_>,
) -> Result<(), ValidationError> {
    let mut locals = ty.params.clone();
    expand_locals(&mut locals, local_decls);

    let instrs = code
        .instructions_with_offsets()
        .map_err(|e| ValidationError {
            offset: e.offset,
            function: Some(function),
            kind: e.into(),
        })?;

    let mut state = ValidationState::new(locals, ty.results.clone());

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

    let found = state.operands.as_slice().to_vec();
    if found != ty.results {
        return Err(ValidationError {
            offset: ByteOffset(code.body_offset),
            function: Some(function),
            kind: ValidationErrorKind::ResultTypeMismatch {
                expected: ty.results.clone(),
                found,
            },
        });
    }

    Ok(())
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
                    frame.start_types.clone(),
                    frame.end_types.clone(),
                )
            };
            pop_exact(function, state, &end_types, offset)?;
            state.operands.truncate(outer_height);
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
            pop_exact(function, state, &label_types, offset)?;
            state.enter_unreachable();
        }
        Instr::BrIf(label) => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            let label_types = validate_label(function, state, *label, offset)?.to_vec();
            pop_exact(function, state, &label_types, offset)?;
            for ty in label_types {
                state.operands.push(ty);
            }
        }
        Instr::Return => {
            let expected = state.controls[0].end_types.clone();
            pop_exact(function, state, &expected, offset)?;
            state.enter_unreachable();
        }
        Instr::Call(idx) => {
            let ty = resolve_func_type(module, *idx, function, offset)?;
            pop_exact(function, state, &ty.params, offset)?;
            for result in &ty.results {
                state.operands.push(*result);
            }
        }
        Instr::Drop => {
            pop_any(function, state, offset)?;
        }
        Instr::Select => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            let rhs = state.operands.pop().map_err(|kind| ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind,
            })?;
            let lhs = state.operands.pop().map_err(|kind| ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind,
            })?;
            if lhs != rhs {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::TypeMismatch {
                        expected: lhs,
                        found: rhs,
                    },
                });
            }
            state.operands.push(lhs);
        }
        Instr::LocalGet(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            state.operands.push(ty);
        }
        Instr::LocalSet(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(function, state, ty, offset)?;
        }
        Instr::LocalTee(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(function, state, ty, offset)?;
            state.operands.push(ty);
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
        Instr::I32Eqz => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I32Eq
        | Instr::I32Ne
        | Instr::I32LtS
        | Instr::I32LtU
        | Instr::I32GtS
        | Instr::I32GtU
        | Instr::I32LeS
        | Instr::I32LeU
        | Instr::I32GeS
        | Instr::I32GeU => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I32Add => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
            )?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
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
        BlockType::Val(val) => Ok(FuncType {
            params: Vec::new(),
            results: vec![val],
        }),
        BlockType::TypeIdx(idx) => module
            .types
            .get(idx as usize)
            .cloned()
            .ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::InvalidBlockType { block_type },
            }),
    }
}

fn resolve_func_type<'m>(
    module: &'m Module<'_>,
    idx: FuncIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<&'m FuncType, ValidationError> {
    let imported_funcs = module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Func(type_idx) => Some(type_idx),
            _ => None,
        });
    let defined_funcs = module.functions.iter().copied();

    let type_idx = imported_funcs
        .chain(defined_funcs)
        .nth(idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownFuncIdx { idx },
        })?;

    module
        .types
        .get(type_idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownTypeIdx { idx: type_idx },
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

    pop_exact(function, state, &frame.end_types, offset)?;
    state.operands.truncate(frame.outer_height);
    for ty in frame.end_types {
        state.operands.push(ty);
    }
    state.reachability = Reachability::Reachable;
    Ok(())
}

fn pop_expect(
    function: FuncIdx,
    state: &mut ValidationState,
    expected: ValType,
    offset: usize,
) -> Result<(), ValidationError> {
    if state.reachability == Reachability::Unreachable
        && state.operands.len() == state.current_frame().stack_floor
    {
        return Ok(());
    }

    let found = state.operands.pop().map_err(|kind| ValidationError {
        offset: ByteOffset(offset),
        function: Some(function),
        kind,
    })?;
    if found != expected {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::TypeMismatch { expected, found },
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
        pop_expect(function, state, *expected_ty, offset)?;
    }
    Ok(())
}

fn pop_any(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
) -> Result<(), ValidationError> {
    if state.reachability == Reachability::Unreachable
        && state.operands.len() == state.current_frame().stack_floor
    {
        return Ok(());
    }

    state
        .operands
        .pop()
        .map(|_| ())
        .map_err(|kind| ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind,
        })
}

fn expand_locals(locals: &mut Vec<ValType>, local_decls: &[LocalDecl]) {
    for decl in local_decls {
        for _ in 0..decl.count {
            locals.push(decl.val_type);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::module::Module;

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
}
