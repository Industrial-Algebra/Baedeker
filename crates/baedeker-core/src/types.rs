//! Core WebAssembly type definitions.
//!
//! These types mirror the WASM spec's abstract syntax for types.
//! See [Spec §2.3](https://webassembly.github.io/spec/core/syntax/types.html).

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Number types.
/// See [Spec §2.3.1](https://webassembly.github.io/spec/core/syntax/types.html#number-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumType {
    I32,
    I64,
    F32,
    F64,
}

/// Vector types.
/// See [Spec §2.3.2](https://webassembly.github.io/spec/core/syntax/types.html#vector-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VecType {
    V128,
}

/// Reference types.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#reference-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefType {
    FuncRef,
    ExternRef,
}

/// Value types — the union of number, vector, and reference types.
/// See [Spec §2.3.4](https://webassembly.github.io/spec/core/syntax/types.html#value-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValType {
    Num(NumType),
    Vec(VecType),
    Ref(RefType),
}

impl ValType {
    /// Binary encoding byte for this value type.
    /// See [Spec §5.3.1](https://webassembly.github.io/spec/core/binary/types.html#value-types).
    pub fn encoding(self) -> u8 {
        match self {
            ValType::Num(NumType::I32) => 0x7F,
            ValType::Num(NumType::I64) => 0x7E,
            ValType::Num(NumType::F32) => 0x7D,
            ValType::Num(NumType::F64) => 0x7C,
            ValType::Vec(VecType::V128) => 0x7B,
            ValType::Ref(RefType::FuncRef) => 0x70,
            ValType::Ref(RefType::ExternRef) => 0x6F,
        }
    }

    /// Decode a value type from its binary encoding byte.
    pub fn from_encoding(byte: u8) -> Option<Self> {
        match byte {
            0x7F => Some(ValType::Num(NumType::I32)),
            0x7E => Some(ValType::Num(NumType::I64)),
            0x7D => Some(ValType::Num(NumType::F32)),
            0x7C => Some(ValType::Num(NumType::F64)),
            0x7B => Some(ValType::Vec(VecType::V128)),
            0x70 => Some(ValType::Ref(RefType::FuncRef)),
            0x6F => Some(ValType::Ref(RefType::ExternRef)),
            _ => None,
        }
    }
}

/// Function types — parameter and result type vectors.
/// See [Spec §2.3.5](https://webassembly.github.io/spec/core/syntax/types.html#function-types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

/// Limits — used by memories and tables to specify size constraints.
/// See [Spec §2.3.7](https://webassembly.github.io/spec/core/syntax/types.html#limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub min: u32,
    pub max: Option<u32>,
}

/// Memory types.
/// See [Spec §2.3.8](https://webassembly.github.io/spec/core/syntax/types.html#memory-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemType {
    pub limits: Limits,
}

/// Table types.
/// See [Spec §2.3.9](https://webassembly.github.io/spec/core/syntax/types.html#table-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableType {
    pub elem: RefType,
    pub limits: Limits,
}

/// Mutability of a global variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    Const,
    Var,
}

/// Global types.
/// See [Spec §2.3.10](https://webassembly.github.io/spec/core/syntax/types.html#global-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalType {
    pub val_type: ValType,
    pub mutability: Mutability,
}

/// Block types for structured control instructions.
/// See [Spec §5.4.2](https://webassembly.github.io/spec/core/binary/instructions.html#control-instructions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    /// Block produces no value (encoded as 0x40).
    Empty,
    /// Block produces a single value of the given type.
    Val(ValType),
    /// Block has the function signature at the given type index (encoded as s33).
    TypeIdx(u32),
}

/// Newtype index wrappers to prevent mixing up different index spaces.
macro_rules! define_idx {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub u32);
    };
}

define_idx!(TypeIdx);
define_idx!(FuncIdx);
define_idx!(TableIdx);
define_idx!(MemIdx);
define_idx!(GlobalIdx);
define_idx!(ElemIdx);
define_idx!(DataIdx);
define_idx!(LocalIdx);
define_idx!(LabelIdx);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valtype_encoding_roundtrip() {
        let types = [
            ValType::Num(NumType::I32),
            ValType::Num(NumType::I64),
            ValType::Num(NumType::F32),
            ValType::Num(NumType::F64),
            ValType::Vec(VecType::V128),
            ValType::Ref(RefType::FuncRef),
            ValType::Ref(RefType::ExternRef),
        ];
        for ty in &types {
            assert_eq!(ValType::from_encoding(ty.encoding()), Some(*ty));
        }
    }

    #[test]
    fn valtype_invalid_encoding() {
        assert_eq!(ValType::from_encoding(0x00), None);
        assert_eq!(ValType::from_encoding(0xFF), None);
    }
}
