//! Register-based lowering skeleton.
//!
//! Phase 2 starts by turning validated stack-machine functions into an
//! inspectable register-oriented IR. This module is deliberately small: it
//! establishes the execution-side vocabulary and lowers straight-line functions
//! before broader control-flow/runtime semantics are added.

use alloc::vec::Vec;

use crate::binary::instr::{DecodedInstr, Instr};
use crate::binary::module::Module;
use crate::error::{ByteOffset, DecodeContext, DecodeErrorKind};
use crate::types::{CodeBody, FuncIdx, FuncType, LocalDecl, LocalIdx, NumType, TypeIdx, ValType};
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
    pub instrs: Vec<RegInstr>,
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
    LocalGet { dst: Reg, local: LocalIdx },
    Drop { value: Reg },
    I32Const { dst: Reg, value: i32 },
    I64Const { dst: Reg, value: i64 },
    F32Const { dst: Reg, value: f32 },
    F64Const { dst: Reg, value: f64 },
    I32Add { dst: Reg, lhs: Reg, rhs: Reg },
    Return { values: Vec<Reg> },
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
    let mut funcs = Vec::new();

    for (defined_idx, (type_idx, code)) in module.functions().iter().zip(module.codes()).enumerate()
    {
        let func_idx = FuncIdx(imported_func_count + defined_idx as u32);
        let ty = &module.types()[type_idx.0 as usize];
        funcs.push(lower_function(func_idx, *type_idx, ty, code)?);
    }

    Ok(RegModule { funcs })
}

fn lower_function(
    func_idx: FuncIdx,
    type_idx: TypeIdx,
    ty: &FuncType,
    code: &CodeBody<'_>,
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
    let mut builder = FuncBuilder::new(func_idx, type_idx, ty, locals);

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

struct FuncBuilder {
    func_idx: FuncIdx,
    type_idx: TypeIdx,
    params: Vec<ValType>,
    results: Vec<ValType>,
    locals: Vec<ValType>,
    stack: Vec<RegValue>,
    reg_types: Vec<ValType>,
    instrs: Vec<RegInstr>,
}

impl FuncBuilder {
    fn new(func_idx: FuncIdx, type_idx: TypeIdx, ty: &FuncType, locals: Vec<ValType>) -> Self {
        Self {
            func_idx,
            type_idx,
            params: ty.params.clone(),
            results: ty.results.clone(),
            locals,
            stack: Vec::new(),
            reg_types: Vec::new(),
            instrs: Vec::new(),
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
            Instr::Drop => {
                let value = self.pop_any(offset, "drop")?;
                self.emit(offset, RegOp::Drop { value: value.reg });
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
            Instr::I32Add => {
                let rhs = self.pop_expect(offset, "i32.add", ValType::Num(NumType::I32))?;
                let lhs = self.pop_expect(offset, "i32.add", ValType::Num(NumType::I32))?;
                let dst = self.alloc_reg(ValType::Num(NumType::I32));
                self.stack.push(RegValue {
                    reg: dst,
                    ty: ValType::Num(NumType::I32),
                });
                self.emit(
                    offset,
                    RegOp::I32Add {
                        dst,
                        lhs: lhs.reg,
                        rhs: rhs.reg,
                    },
                );
            }
            Instr::End => {
                let values = self.pop_results(offset)?;
                self.emit(offset, RegOp::Return { values });
                return Ok(true);
            }
            instr => {
                return Err(LowerError {
                    offset,
                    function: Some(self.func_idx),
                    kind: LowerErrorKind::UnsupportedInstr {
                        op: instr_name(&instr),
                    },
                });
            }
        }

        Ok(false)
    }

    fn finish(self) -> RegFunc {
        RegFunc {
            idx: self.func_idx,
            type_idx: self.type_idx,
            params: self.params,
            results: self.results,
            locals: self.locals,
            reg_types: self.reg_types,
            instrs: self.instrs,
        }
    }

    fn alloc_reg(&mut self, ty: ValType) -> Reg {
        let reg = Reg(self.reg_types.len() as u32);
        self.reg_types.push(ty);
        reg
    }

    fn emit(&mut self, offset: ByteOffset, op: RegOp) {
        self.instrs.push(RegInstr { offset, op });
    }

    fn pop_any(&mut self, offset: ByteOffset, op: &'static str) -> Result<RegValue, LowerError> {
        self.stack.pop().ok_or(LowerError {
            offset,
            function: Some(self.func_idx),
            kind: LowerErrorKind::StackUnderflow {
                op,
                expected: ValType::Num(NumType::I32),
            },
        })
    }

    fn pop_expect(
        &mut self,
        offset: ByteOffset,
        op: &'static str,
        expected: ValType,
    ) -> Result<RegValue, LowerError> {
        let found = self.stack.pop().ok_or(LowerError {
            offset,
            function: Some(self.func_idx),
            kind: LowerErrorKind::StackUnderflow { op, expected },
        })?;

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
            func.instrs
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
                &RegOp::I32Add {
                    dst: Reg(2),
                    lhs: Reg(0),
                    rhs: Reg(1),
                },
                &RegOp::Return {
                    values: vec![Reg(2)],
                },
            ]
        );
    }

    #[test]
    fn lower_rejects_unsupported_control_for_now() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: [] -> []
            0x03, 0x02, 0x01, 0x00, // function type 0
            0x0a, 0x07, 0x01, 0x05, 0x00, // code body header
            0x02, 0x40, 0x0b, 0x0b, // block end end
        ];
        let module = Module::decode(&bytes).unwrap();
        let err = module.lower().unwrap_err();
        assert!(matches!(
            err.kind,
            LowerErrorKind::UnsupportedInstr { op: "block" }
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
}
