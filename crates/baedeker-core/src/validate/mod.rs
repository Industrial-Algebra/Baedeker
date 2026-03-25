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
use crate::types::{
    BlockType, FuncIdx, FuncType, GlobalIdx, ImportDesc, LocalDecl, MemIdx, ValType,
};
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

    let full_stack = state.operands.as_slice().to_vec();
    if full_stack != ty.results {
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
            pop_control_result_types(function, state, &end_types, offset)?;
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
        Instr::BrTable { targets, default } => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "br_table",
            )?;
            let default_types = validate_label(function, state, *default, offset)?.to_vec();
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
        Instr::SelectTyped(types) => {
            if types.len() != 1 {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::InvalidSelectResultArity { found: types.len() },
                });
            }
            let expected = types[0];
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "select_typed",
            )?;
            let rhs = pop_operand_type(function, state, offset, "select_typed")?;
            let lhs = pop_operand_type(function, state, offset, "select_typed")?;
            if lhs != expected || rhs != expected {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::SelectOperandTypeMismatch {
                        expected,
                        found: vec![lhs, rhs],
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
            state.operands.push(ty);
        }
        Instr::LocalSet(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(function, state, ty, offset, "local.set")?;
        }
        Instr::LocalTee(idx) => {
            let ty = *state.locals.get(idx.0 as usize).ok_or(ValidationError {
                offset: ByteOffset(offset),
                function: Some(function),
                kind: ValidationErrorKind::UnknownLocalIdx { idx: *idx },
            })?;
            pop_expect(function, state, ty, offset, "local.tee")?;
            state.operands.push(ty);
        }
        Instr::GlobalGet(idx) => {
            let ty = resolve_global_type(module, *idx, function, offset)?;
            state.operands.push(ty.val_type);
        }
        Instr::GlobalSet(idx) => {
            let ty = resolve_global_type(module, *idx, function, offset)?;
            pop_expect(function, state, ty.val_type, offset, "global.set")?;
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
        Instr::I32Eqz => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "i32.eqz",
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
                "i32.compare",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "i32.compare",
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
                "i32.add",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I32),
                offset,
                "i32.add",
            )?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I32));
        }
        Instr::I64Add => {
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I64),
                offset,
                "i64.add",
            )?;
            pop_expect(
                function,
                state,
                ValType::Num(crate::types::NumType::I64),
                offset,
                "i64.add",
            )?;
            state
                .operands
                .push(ValType::Num(crate::types::NumType::I64));
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

fn resolve_global_type(
    module: &Module<'_>,
    idx: GlobalIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<crate::types::GlobalType, ValidationError> {
    module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Global(global) => Some(global),
            _ => None,
        })
        .nth(idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownGlobalIdx { idx },
        })
}

fn resolve_memory_type(
    module: &Module<'_>,
    idx: MemIdx,
    function: FuncIdx,
    offset: usize,
) -> Result<crate::types::MemType, ValidationError> {
    module
        .imports
        .iter()
        .filter_map(|import| match import.desc {
            ImportDesc::Mem(memory) => Some(memory),
            _ => None,
        })
        .nth(idx.0 as usize)
        .ok_or(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::UnknownMemIdx { idx },
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

    pop_control_result_types(function, state, &frame.end_types, offset)?;
    state.operands.truncate(frame.outer_height);
    for ty in frame.end_types {
        state.operands.push(ty);
    }
    state.reachability = Reachability::Reachable;
    Ok(())
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
            available: state.operands.as_slice().to_vec(),
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
    if state.reachability == Reachability::Unreachable
        && state.operands.len() == state.current_frame().stack_floor
    {
        return Ok(());
    }

    let found = state
        .operands
        .pop()
        .ok_or_else(|| underflow_error(function, state, op, &[expected], offset))?;
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
        pop_expect(function, state, *expected_ty, offset, "stack")?;
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
    if state.reachability == Reachability::Unreachable
        && state.operands.len() == state.current_frame().stack_floor
    {
        return Ok(());
    }

    let operands = state.operands.as_slice();
    let found_len = core::cmp::min(operands.len(), expected.len());
    let found = operands[operands.len().saturating_sub(found_len)..].to_vec();

    if operands.len() < expected.len() || found != expected[expected.len() - found_len..] {
        return Err(found);
    }

    Ok(())
}

fn pop_operand_type(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
) -> Result<ValType, ValidationError> {
    state
        .operands
        .pop()
        .ok_or_else(|| underflow_error(function, state, op, &[], offset))
}

fn pop_any(
    function: FuncIdx,
    state: &mut ValidationState,
    offset: usize,
    op: &'static str,
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
        .ok_or_else(|| underflow_error(function, state, op, &[], offset))
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
                if op == "i32.add"
                    && expected == vec![ValType::Num(crate::types::NumType::I32)]
                    && available.is_empty()
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
    fn reject_unknown_imported_global_index() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x0A, 0x06, 0x01, 0x04, 0x00, 0x23, 0x00, 0x0B,
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.validate().unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::UnknownGlobalIdx { .. }
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
            ValidationErrorKind::UnknownMemIdx { .. }
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
}
