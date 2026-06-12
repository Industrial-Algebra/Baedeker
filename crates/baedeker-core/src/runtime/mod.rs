//! Minimal register-IR execution core.
//!
//! This is the first Phase 2 runtime slice: execute straight-line lowered IR
//! independently from validation. Broader control flow, calls, memory, tables,
//! traps, and host integration are added in later checkpoints.

use alloc::{string::String, vec::Vec};

use crate::lower::{BinaryOp, Reg, RegFunc, RegInstr, RegModule, RegOp, RegTerm, UnaryOp};
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
    InvalidConversionToInteger,
}

impl RuntimeTrap {
    /// The canonical WAST assertion message for this trap.
    pub fn wast_message(self) -> &'static str {
        match self {
            RuntimeTrap::IntegerDivideByZero => "integer divide by zero",
            RuntimeTrap::IntegerOverflow => "integer overflow",
            RuntimeTrap::InvalidConversionToInteger => "invalid conversion to integer",
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

    if func.blocks.is_empty() {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::MissingReturn,
        });
    }

    let mut block_idx: u32 = 0;
    let max_iterations = func.blocks.len() * 100;
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
            execute_reg_op(&mut registers, &mut locals, instr)?;
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
            RegTerm::Br { target_block, .. } => {
                block_idx = *target_block;
            }
            RegTerm::Fallthrough => {
                block_idx += 1;
                if block_idx as usize >= func.blocks.len() {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::MissingReturn,
                    });
                }
            }
        }
    }
}

fn execute_reg_op(
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
    }
    Ok(())
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
