// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Core WebAssembly type definitions.
//!
//! These types mirror the WASM spec's abstract syntax for types.
//! See [Spec §2.3](https://webassembly.github.io/spec/core/syntax/types.html).

use alloc::{string::String, vec::Vec};

/// Newtype index wrappers to prevent mixing up different index spaces.
macro_rules! define_idx {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

/// Number types.
/// See [Spec §2.3.1](https://webassembly.github.io/spec/core/syntax/types.html#number-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NumType {
    I32,
    I64,
    F32,
    F64,
}

/// Vector types.
/// See [Spec §2.3.2](https://webassembly.github.io/spec/core/syntax/types.html#vector-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VecType {
    V128,
}

/// Heap types used by reference types.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#reference-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HeapType {
    Func,
    Extern,
    Type(TypeIdx),
}

impl HeapType {
    pub fn is_subtype_of(self, expected: Self) -> bool {
        match (self, expected) {
            (found, expected) if found == expected => true,
            (HeapType::Type(_), HeapType::Func) => true,
            _ => false,
        }
    }
}

/// Reference types.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#reference-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RefType {
    FuncRef,
    ExternRef,
    Typed { nullable: bool, heap: HeapType },
}

impl RefType {
    pub fn func(nullable: bool) -> Self {
        if nullable {
            Self::FuncRef
        } else {
            Self::Typed {
                nullable: false,
                heap: HeapType::Func,
            }
        }
    }

    pub fn extern_(nullable: bool) -> Self {
        if nullable {
            Self::ExternRef
        } else {
            Self::Typed {
                nullable: false,
                heap: HeapType::Extern,
            }
        }
    }

    pub fn concrete(nullable: bool, type_idx: TypeIdx) -> Self {
        Self::Typed {
            nullable,
            heap: HeapType::Type(type_idx),
        }
    }

    pub fn from_parts(nullable: bool, heap: HeapType) -> Self {
        match heap {
            HeapType::Func => Self::func(nullable),
            HeapType::Extern => Self::extern_(nullable),
            HeapType::Type(type_idx) => Self::concrete(nullable, type_idx),
        }
    }

    pub fn is_nullable(self) -> bool {
        match self {
            Self::FuncRef | Self::ExternRef => true,
            Self::Typed { nullable, .. } => nullable,
        }
    }

    pub fn heap_type(self) -> HeapType {
        match self {
            Self::FuncRef => HeapType::Func,
            Self::ExternRef => HeapType::Extern,
            Self::Typed { heap, .. } => heap,
        }
    }

    pub fn with_nullability(self, nullable: bool) -> Self {
        Self::from_parts(nullable, self.heap_type())
    }

    pub fn as_non_null(self) -> Self {
        self.with_nullability(false)
    }

    pub fn as_nullable(self) -> Self {
        self.with_nullability(true)
    }

    pub fn is_subtype_of(self, expected: Self) -> bool {
        if self == expected {
            return true;
        }

        if self.is_nullable() && !expected.is_nullable() {
            return false;
        }

        self.heap_type().is_subtype_of(expected.heap_type())
    }
}

/// Value types — the union of number, vector, and reference types.
/// See [Spec §2.3.4](https://webassembly.github.io/spec/core/syntax/types.html#value-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ValType {
    Num(NumType),
    Vec(VecType),
    Ref(RefType),
}

impl ValType {
    /// Decode a single-byte value type encoding.
    ///
    /// This only covers number/vector types plus the legacy short reference-type
    /// encodings for nullable `funcref` / `externref`. Multi-byte typed-reference
    /// encodings are handled by the binary parsers.
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

    pub fn is_subtype_of(self, expected: Self) -> bool {
        match (self, expected) {
            (ValType::Ref(found), ValType::Ref(expected)) => found.is_subtype_of(expected),
            _ => self == expected,
        }
    }
}

/// Function types — parameter and result type vectors.
/// See [Spec §2.3.5](https://webassembly.github.io/spec/core/syntax/types.html#function-types).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

/// A local declaration in a function body.
/// See [Spec §5.5.13](https://webassembly.github.io/spec/core/binary/modules.html#code-section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDecl {
    pub count: u32,
    pub val_type: ValType,
}

/// A function body from the code section.
/// The instruction stream is still stored as raw bytes until instruction decoding is implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBody<'a> {
    pub locals: Vec<LocalDecl>,
    pub body: &'a [u8],
    pub body_offset: usize,
}

/// A defined global from the global section.
/// See [Spec §5.5.11](https://webassembly.github.io/spec/core/binary/modules.html#global-section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global<'a> {
    pub global_type: GlobalType,
    pub init_expr: &'a [u8],
    pub init_offset: usize,
}

/// Limits — used by memories and tables to specify size constraints.
/// See [Spec §2.3.7](https://webassembly.github.io/spec/core/syntax/types.html#limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Limits {
    pub min: u32,
    pub max: Option<u32>,
}

/// Memory types.
/// See [Spec §2.3.8](https://webassembly.github.io/spec/core/syntax/types.html#memory-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MemType {
    pub limits: Limits,
}

/// Memory instruction immediate.
/// See [Spec §2.4.5](https://webassembly.github.io/spec/core/syntax/instructions.html#memory-instructions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MemArg {
    pub align: u32,
    pub offset: u32,
    pub memory: MemIdx,
}

/// Mode of a data segment.
/// See [Spec §2.5.8](https://webassembly.github.io/spec/core/syntax/modules.html#data-segments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataMode<'a> {
    Passive,
    Active {
        memory: MemIdx,
        offset_expr: &'a [u8],
        offset_offset: usize,
    },
}

/// A defined data segment from the data section.
/// See [Spec §5.5.14](https://webassembly.github.io/spec/core/binary/modules.html#data-section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSegment<'a> {
    pub mode: DataMode<'a>,
    pub init: &'a [u8],
    pub init_offset: usize,
}

/// Mode of an element segment.
/// See [Spec §2.5.7](https://webassembly.github.io/spec/core/syntax/modules.html#element-segments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementMode<'a> {
    Passive,
    Declarative,
    Active {
        table: TableIdx,
        offset_expr: &'a [u8],
        offset_offset: usize,
    },
}

/// A reference-valued initializer expression within an element segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementExpr<'a> {
    pub expr: &'a [u8],
    pub offset: usize,
}

/// Initialization payload for an element segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementInit<'a> {
    FuncIndices(Vec<FuncIdx>),
    Expressions(Vec<ElementExpr<'a>>),
}

/// A defined element segment from the element section.
/// See [Spec §5.5.12](https://webassembly.github.io/spec/core/binary/modules.html#element-section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementSegment<'a> {
    pub mode: ElementMode<'a>,
    pub elem_type: RefType,
    pub init: ElementInit<'a>,
}

/// An imported item declaration.
/// See [Spec §2.5.11](https://webassembly.github.io/spec/core/syntax/modules.html#imports).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub desc: ImportDesc,
}

/// Import descriptor specifying what kind of item is imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportDesc {
    Func(TypeIdx),
    Table(TableType),
    Mem(MemType),
    Global(GlobalType),
}

/// An exported item declaration.
/// See [Spec §2.5.10](https://webassembly.github.io/spec/core/syntax/modules.html#exports).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub desc: ExportDesc,
}

/// Export descriptor specifying what kind of item is exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportDesc {
    Func(FuncIdx),
    Table(TableIdx),
    Mem(MemIdx),
    Global(GlobalIdx),
}

/// Table types.
/// See [Spec §2.3.9](https://webassembly.github.io/spec/core/syntax/types.html#table-types).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TableType {
    pub elem: RefType,
    pub limits: Limits,
    /// Optional initializer expression filling the table at instantiation
    /// (encoded with the `0x40` table marker).
    pub init: Option<Vec<u8>>,
}

/// Mutability of a global variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Mutability {
    Const,
    Var,
}

/// Global types.
/// See [Spec §2.3.10](https://webassembly.github.io/spec/core/syntax/types.html#global-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valtype_single_byte_encoding_roundtrip() {
        let types = [
            ValType::Num(NumType::I32),
            ValType::Num(NumType::I64),
            ValType::Num(NumType::F32),
            ValType::Num(NumType::F64),
            ValType::Vec(VecType::V128),
            ValType::Ref(RefType::FuncRef),
            ValType::Ref(RefType::ExternRef),
        ];
        let encodings = [0x7F, 0x7E, 0x7D, 0x7C, 0x7B, 0x70, 0x6F];
        for (ty, encoding) in types.iter().zip(encodings) {
            assert_eq!(ValType::from_encoding(encoding), Some(*ty));
        }
    }

    #[test]
    fn valtype_invalid_encoding() {
        assert_eq!(ValType::from_encoding(0x00), None);
        assert_eq!(ValType::from_encoding(0x63), None);
        assert_eq!(ValType::from_encoding(0x64), None);
        assert_eq!(ValType::from_encoding(0xFF), None);
    }

    #[test]
    fn typed_reference_subtyping() {
        let concrete = RefType::concrete(false, TypeIdx(3));
        assert!(concrete.is_subtype_of(RefType::func(false)));
        assert!(concrete.is_subtype_of(RefType::FuncRef));
        assert!(!RefType::FuncRef.is_subtype_of(RefType::func(false)));
        assert!(!RefType::ExternRef.is_subtype_of(RefType::FuncRef));
    }
}
