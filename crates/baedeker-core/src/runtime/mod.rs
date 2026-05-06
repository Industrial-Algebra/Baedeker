//! Minimal register-IR execution core.
//!
//! This is the first Phase 2 runtime slice: execute straight-line lowered IR
//! independently from validation. Broader control flow, calls, memory, tables,
//! traps, and host integration are added in later checkpoints.

use alloc::vec::Vec;

use crate::lower::{Reg, RegFunc, RegOp};
use crate::types::{NumType, ValType};

/// A runtime WebAssembly value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl Value {
    fn val_type(self) -> ValType {
        match self {
            Value::I32(_) => ValType::Num(NumType::I32),
            Value::I64(_) => ValType::Num(NumType::I64),
            Value::F32(_) => ValType::Num(NumType::F32),
            Value::F64(_) => ValType::Num(NumType::F64),
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
    UninitializedLocal { local: u32 },
    UninitializedRegister { reg: Reg },
    UnknownRegister { reg: Reg },
    MissingReturn,
}

/// Execute a single lowered function with positional arguments.
pub fn execute_func(func: &RegFunc, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
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

    let mut registers = alloc::vec![None; func.reg_types.len()];

    for instr in &func.instrs {
        match &instr.op {
            RegOp::LocalGet { dst, local } => {
                let value = locals
                    .get(local.0 as usize)
                    .and_then(|value| *value)
                    .ok_or(RuntimeError {
                        kind: RuntimeErrorKind::UninitializedLocal { local: local.0 },
                    })?;
                set_reg(&mut registers, *dst, value)?;
            }
            RegOp::LocalSet { local, value } | RegOp::LocalTee { local, value } => {
                let value = get_reg(&registers, *value)?;
                let slot = locals.get_mut(local.0 as usize).ok_or(RuntimeError {
                    kind: RuntimeErrorKind::UninitializedLocal { local: local.0 },
                })?;
                *slot = Some(value);
            }
            RegOp::Drop { value } => {
                get_reg(&registers, *value)?;
            }
            RegOp::I32Const { dst, value } => {
                set_reg(&mut registers, *dst, Value::I32(*value))?;
            }
            RegOp::I64Const { dst, value } => {
                set_reg(&mut registers, *dst, Value::I64(*value))?;
            }
            RegOp::F32Const { dst, value } => {
                set_reg(&mut registers, *dst, Value::F32(*value))?;
            }
            RegOp::F64Const { dst, value } => {
                set_reg(&mut registers, *dst, Value::F64(*value))?;
            }
            RegOp::I32Add { dst, lhs, rhs } => {
                let lhs = expect_i32(get_reg(&registers, *lhs)?)?;
                let rhs = expect_i32(get_reg(&registers, *rhs)?)?;
                set_reg(&mut registers, *dst, Value::I32(lhs.wrapping_add(rhs)))?;
            }
            RegOp::Return { values } => {
                let mut results = Vec::with_capacity(values.len());
                for &reg in values {
                    results.push(get_reg(&registers, reg)?);
                }
                return Ok(results);
            }
        }
    }

    Err(RuntimeError {
        kind: RuntimeErrorKind::MissingReturn,
    })
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
