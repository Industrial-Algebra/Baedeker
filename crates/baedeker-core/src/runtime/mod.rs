//! Minimal register-IR execution core.
//!
//! This is the first Phase 2 runtime slice: execute straight-line lowered IR
//! independently from validation. Broader control flow, calls, memory, tables,
//! traps, and host integration are added in later checkpoints.

use alloc::{string::String, vec::Vec};

use crate::lower::{BinaryOp, Reg, RegFunc, RegModule, RegOp, UnaryOp};
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
    Trap(RuntimeTrap),
    UninitializedLocal { local: u32 },
    UninitializedRegister { reg: Reg },
    UnknownRegister { reg: Reg },
    UnknownExport { name: String },
    ExportedFunctionNotLowered { func: u32 },
    MissingReturn,
}

/// WebAssembly runtime traps surfaced by the interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTrap {
    IntegerDivideByZero,
    IntegerOverflow,
}

impl RuntimeTrap {
    /// The canonical WAST assertion message for this trap.
    pub fn wast_message(self) -> &'static str {
        match self {
            RuntimeTrap::IntegerDivideByZero => "integer divide by zero",
            RuntimeTrap::IntegerOverflow => "integer overflow",
        }
    }
}

/// Execute an exported lowered function by name.
pub fn execute_export(
    module: &RegModule,
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

    execute_func(func, args)
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
            RegOp::Unary { op, dst, value } => execute_unary_op(&mut registers, *op, *dst, *value)?,
            RegOp::Binary { op, dst, lhs, rhs } => {
                execute_binary_op(&mut registers, *op, *dst, *lhs, *rhs)?
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::module::Module;

    #[test]
    fn execute_exported_add_by_name() {
        let reg_module = lowered_add_module();

        let result = execute_export(&reg_module, "add", &[Value::I32(20), Value::I32(22)]).unwrap();

        assert_eq!(result, alloc::vec![Value::I32(42)]);
    }

    #[test]
    fn reject_unknown_export_name() {
        let reg_module = lowered_add_module();

        let err = execute_export(&reg_module, "missing", &[]).unwrap_err();

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

        let err = execute_export(&reg_module, "add", &[Value::I32(20)]).unwrap_err();

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

        let err = execute_export(
            &reg_module,
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

        let err =
            execute_export(&reg_module, "add", &[Value::I64(20), Value::I32(22)]).unwrap_err();

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
