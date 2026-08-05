// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! The C ABI value representation and conversions to/from `baedeker_core::Value`.

use baedeker_core::runtime::Value;
use baedeker_core::types::{NumType, ValType, VecType};

/// Type tag for [`BaedekerValue`], and the value-type encoding used when
/// declaring host function signatures.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaedekerValueTag {
    I32 = 0,
    I64 = 1,
    F32 = 2,
    F64 = 3,
    V128 = 4,
}

impl BaedekerValueTag {
    pub(crate) fn from_u8(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::I32,
            1 => Self::I64,
            2 => Self::F32,
            3 => Self::F64,
            4 => Self::V128,
            _ => return None,
        })
    }

    pub(crate) fn val_type(self) -> ValType {
        match self {
            Self::I32 => ValType::Num(NumType::I32),
            Self::I64 => ValType::Num(NumType::I64),
            Self::F32 => ValType::Num(NumType::F32),
            Self::F64 => ValType::Num(NumType::F64),
            Self::V128 => ValType::Vec(VecType::V128),
        }
    }
}

/// Payload of a [`BaedekerValue`]; the active field is selected by the tag.
/// Floats are carried as C `float`/`double` values, vectors as 16 raw bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub union BaedekerValueData {
    pub i32_: i32,
    pub i64_: i64,
    pub f32_: f32,
    pub f64_: f64,
    pub v128: [u8; 16],
}

/// A WebAssembly value across the FFI boundary. Reference types (funcref /
/// externref) are not representable in this FFI version.
///
/// `tag` is a [`BaedekerValueTag`] value carried as a plain `uint8_t`:
/// fixed-underlying-type enums are C23-only, and a C-int enum here would
/// read 3 bytes of undefined struct padding against Rust's 1-byte write.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BaedekerValue {
    pub tag: u8,
    pub data: BaedekerValueData,
}

impl BaedekerValue {
    /// Convert a core value; `None` for reference values (unsupported here).
    pub(crate) fn from_core(value: Value) -> Option<Self> {
        Some(match value {
            Value::I32(v) => Self {
                tag: BaedekerValueTag::I32 as u8,
                data: BaedekerValueData { i32_: v },
            },
            Value::I64(v) => Self {
                tag: BaedekerValueTag::I64 as u8,
                data: BaedekerValueData { i64_: v },
            },
            Value::F32(v) => Self {
                tag: BaedekerValueTag::F32 as u8,
                data: BaedekerValueData { f32_: v },
            },
            Value::F64(v) => Self {
                tag: BaedekerValueTag::F64 as u8,
                data: BaedekerValueData { f64_: v },
            },
            Value::V128(bytes) => Self {
                tag: BaedekerValueTag::V128 as u8,
                data: BaedekerValueData { v128: bytes },
            },
            Value::FuncRef(_) | Value::ExternRef(_) => return None,
        })
    }

    pub(crate) fn to_core(self) -> Value {
        unsafe {
            match self.tag {
                t if t == BaedekerValueTag::I32 as u8 => Value::I32(self.data.i32_),
                t if t == BaedekerValueTag::I64 as u8 => Value::I64(self.data.i64_),
                t if t == BaedekerValueTag::F32 as u8 => Value::F32(self.data.f32_),
                t if t == BaedekerValueTag::F64 as u8 => Value::F64(self.data.f64_),
                t if t == BaedekerValueTag::V128 as u8 => Value::V128(self.data.v128),
                _ => unreachable!("BaedekerValue tags are constructed from BaedekerValueTag"),
            }
        }
    }
}
