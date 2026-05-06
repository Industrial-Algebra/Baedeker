//! Register-based lowering skeleton.
//!
//! Phase 2 starts by turning validated stack-machine functions into an
//! inspectable register-oriented IR. This module is deliberately small: it
//! establishes the execution-side vocabulary and lowers straight-line functions
//! before broader control-flow/runtime semantics are added.

use alloc::{string::String, vec::Vec};

use crate::binary::instr::{DecodedInstr, Instr};
use crate::binary::module::Module;
use crate::error::{ByteOffset, DecodeContext, DecodeErrorKind};
use crate::types::{
    CodeBody, ExportDesc, FuncIdx, FuncType, LocalDecl, LocalIdx, NumType, TypeIdx, ValType,
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
    LocalSet { local: LocalIdx, value: Reg },
    LocalTee { local: LocalIdx, value: Reg },
    Drop { value: Reg },
    I32Const { dst: Reg, value: i32 },
    I64Const { dst: Reg, value: i64 },
    F32Const { dst: Reg, value: f32 },
    F64Const { dst: Reg, value: f64 },
    I32Add { dst: Reg, lhs: Reg, rhs: Reg },
    I32Sub { dst: Reg, lhs: Reg, rhs: Reg },
    I32Mul { dst: Reg, lhs: Reg, rhs: Reg },
    I64Add { dst: Reg, lhs: Reg, rhs: Reg },
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

    Ok(RegModule { funcs, exports })
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
            Instr::I32Add => self.lower_binary(
                offset,
                "i32.add",
                ValType::Num(NumType::I32),
                |dst, lhs, rhs| RegOp::I32Add { dst, lhs, rhs },
            )?,
            Instr::I32Sub => self.lower_binary(
                offset,
                "i32.sub",
                ValType::Num(NumType::I32),
                |dst, lhs, rhs| RegOp::I32Sub { dst, lhs, rhs },
            )?,
            Instr::I32Mul => self.lower_binary(
                offset,
                "i32.mul",
                ValType::Num(NumType::I32),
                |dst, lhs, rhs| RegOp::I32Mul { dst, lhs, rhs },
            )?,
            Instr::I64Add => self.lower_binary(
                offset,
                "i64.add",
                ValType::Num(NumType::I64),
                |dst, lhs, rhs| RegOp::I64Add { dst, lhs, rhs },
            )?,
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

    fn lower_binary(
        &mut self,
        offset: ByteOffset,
        op: &'static str,
        ty: ValType,
        make_op: impl FnOnce(Reg, Reg, Reg) -> RegOp,
    ) -> Result<(), LowerError> {
        let rhs = self.pop_expect(offset, op, ty)?;
        let lhs = self.pop_expect(offset, op, ty)?;
        let dst = self.alloc_reg(ty);
        self.stack.push(RegValue { reg: dst, ty });
        self.emit(offset, make_op(dst, lhs.reg, rhs.reg));
        Ok(())
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

    #[test]
    fn lower_local_set_temp_storage() {
        let module = Module::decode(local_set_temp_module()).unwrap();
        let reg_module = module.lower().unwrap();
        let func = &reg_module.funcs[0];

        assert_eq!(func.reg_types, vec![ValType::Num(NumType::I32); 4]);
        assert_eq!(
            func.instrs
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
                &RegOp::I32Add {
                    dst: Reg(3),
                    lhs: Reg(1),
                    rhs: Reg(2),
                },
                &RegOp::Return {
                    values: vec![Reg(3)],
                },
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
            func.instrs
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
            func.instrs
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
                &RegOp::I32Sub {
                    dst: Reg(2),
                    lhs: Reg(0),
                    rhs: Reg(1),
                },
                &RegOp::I32Const {
                    dst: Reg(3),
                    value: 3,
                },
                &RegOp::I32Mul {
                    dst: Reg(4),
                    lhs: Reg(2),
                    rhs: Reg(3),
                },
                &RegOp::Return {
                    values: vec![Reg(4)],
                },
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
            func.instrs
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
                &RegOp::I64Add {
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
}
