// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Core WebAssembly type definitions.
//!
//! These types mirror the WASM spec's abstract syntax for types.
//! See [Spec §2.3](https://webassembly.github.io/spec/core/syntax/types.html).

use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};

/// Newtype index wrappers to prevent mixing up different index spaces.
macro_rules! define_idx {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
define_idx!(TagIdx);

/// Number types.
/// See [Spec §2.3.1](https://webassembly.github.io/spec/core/syntax/types.html#number-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NumType {
    I32,
    I64,
    F32,
    F64,
}

/// Vector types.
/// See [Spec §2.3.2](https://webassembly.github.io/spec/core/syntax/types.html#vector-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VecType {
    V128,
}

/// Heap types used by reference types.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#reference-types).
///
/// The abstract hierarchy is grouped by side:
/// - any-side: `Any`, `Eq`, `I31`, `Struct`, `Array`, `None`
/// - func-side: `Func`, `NoFunc`
/// - extern-side: `Extern`, `NoExtern`
/// - exn-side: `Exn`, `NoExn`
///
/// `Type` carries a concrete (defined) type index. NOTE: the existing
/// variants `Func`, `Extern`, `Exn`, `Type` keep their declaration order so
/// that serialized artifacts (AOT [`crate::lower::RegModule`]) are
/// byte-compatible; new variants are appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HeapType {
    Func,
    Extern,
    /// The `exn` abstract heap type (exception references).
    /// Part of the exception-handling proposal (wasm 3.0).
    Exn,
    Type(TypeIdx),
    /// `any`: the top of the internal (GC) hierarchy.
    Any,
    /// `none`: the bottom of the internal (GC) hierarchy.
    None,
    /// `eq`: the equality-comparable sub-hierarchy of `any`.
    Eq,
    /// `i31`: 31-bit immediate integers.
    I31,
    /// `struct`: struct references.
    Struct,
    /// `array`: array references.
    Array,
    /// `nofunc`: the bottom of the func hierarchy.
    NoFunc,
    /// `noextern`: the bottom of the extern hierarchy.
    NoExtern,
    /// `noexn`: the bottom of the exn hierarchy.
    NoExn,
}

impl HeapType {
    /// Context-free portion of the reference heap-type lattice
    /// (`valid/match.ml:77-104`). Concrete-to-abstract matching that needs
    /// the module's type table is handled by the validator's
    /// module-aware lattice.
    pub fn is_subtype_of(self, expected: Self) -> bool {
        use HeapType::*;
        if self == expected {
            return true;
        }
        match (self, expected) {
            // Internal hierarchy: i31/struct/array <= eq <= any.
            (Eq | Struct | Array | I31, Any) => true,
            (I31 | Struct | Array, Eq) => true,
            // bottoms match every non-bottom of their hierarchy top.
            (None, Any | Eq | Struct | Array | I31) => true,
            (NoFunc, Func) => true,
            (NoExtern, Extern) => true,
            (NoExn, Exn) => true,
            // Concrete function types are function references.
            (Type(_), Func) => true,
            _ => false,
        }
    }
}

/// Storage types: unpacked value types, or packed 8/16-bit integers.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#storage-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StorageType {
    Val(ValType),
    I8,
    I16,
}

/// A struct/array field: storage type plus mutability.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#field-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FieldType {
    pub mutable: bool,
    pub storage: StorageType,
}

/// A composite type: struct, array, or function.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#composite-types).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CompType {
    Struct(Vec<FieldType>),
    Array(FieldType),
    Func(FuncType),
}

/// A single subtype definition: finality, declared supertypes, and the
/// composite type.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#subtypes).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SubType {
    pub is_final: bool,
    pub supertypes: Vec<u32>,
    pub comp: CompType,
}

impl SubType {
    /// The function signature of this type, if it is a function type.
    pub fn as_func(&self) -> Option<&FuncType> {
        match &self.comp {
            CompType::Func(func) => Some(func),
            _ => None,
        }
    }
}

/// Reference types.
/// See [Spec §2.3.3](https://webassembly.github.io/spec/core/syntax/types.html#reference-types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
            heap => Self::Typed { nullable, heap },
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    pub min: u64,
    pub max: Option<u64>,
    pub is64: bool,
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
    pub offset: u64,
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
    /// Imported exception tag; the index is the tag's function type.
    Tag(TypeIdx),
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
    /// Exported exception tag.
    Tag(TagIdx),
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

/// Hash-consed canonicalizer for the module's type section.
///
/// Two type indices are *canonically equivalent* exactly when their rolled
/// closed forms are structurally equal (`syntax/types.ml` `roll_deftype` /
/// OCaml structural equality). Each equivalence class is represented by the
/// lowest type index in the class, so [`TypeCanon::canonical_id`] returns the
/// earliest equivalent index (matching the validator's normalization).
///
/// A bare subtype is treated as a singleton `rec` group, which makes
/// `bare ≡ rec (type ...)` automatic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCanon {
    canonical: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CanonHeap {
    Abstract(HeapType),
    InGroup(u32),
    External(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CanonVal {
    Num(NumType),
    Vec(VecType),
    Ref(bool, CanonHeap),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CanonStorage {
    Val(CanonVal),
    I8,
    I16,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CanonField {
    mutable: bool,
    storage: CanonStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CanonComp {
    Struct(Vec<CanonField>),
    Array(CanonField),
    Func(Vec<CanonVal>, Vec<CanonVal>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CanonMember {
    is_final: bool,
    supers: Vec<u32>,
    comp: CanonComp,
}

impl TypeCanon {
    /// Canonicalize a type section. `rec_groups` are the explicit `rec`
    /// group spans (start, length); every other index is its own singleton
    /// group.
    pub fn new(types: &[SubType], rec_groups: &[(u32, u32)]) -> Self {
        let mut group_len: BTreeMap<u32, u32> = BTreeMap::new();
        for &(start, len) in rec_groups {
            // Empty rec groups contribute no type indices, so they never
            // establish a group span; the following bare type stays its own
            // singleton group.
            if len > 0 {
                group_len.insert(start, len);
            }
        }

        let mut canonical = vec![0u32; types.len()];
        let mut group_ids: BTreeMap<Vec<CanonMember>, u32> = BTreeMap::new();
        let mut member_classes: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        let mut next_group: u32 = 0;

        let mut start: u32 = 0;
        while (start as usize) < types.len() {
            let len = group_len.get(&start).copied().unwrap_or(1);
            let members: Vec<CanonMember> = (0..len)
                .map(|k| canon_member(&types[(start + k) as usize], start, len, &canonical))
                .collect();
            let gid = match group_ids.get(&members) {
                Some(&gid) => gid,
                None => {
                    let gid = next_group;
                    next_group += 1;
                    group_ids.insert(members, gid);
                    gid
                }
            };
            for k in 0..len {
                let idx = start + k;
                let class = *member_classes.entry((gid, k)).or_insert(idx);
                canonical[idx as usize] = class;
            }
            start += len;
        }

        Self { canonical }
    }

    /// The canonical class representative for `idx`, if it exists.
    pub fn canonical_id(&self, idx: TypeIdx) -> Option<u32> {
        self.canonical.get(idx.0 as usize).copied()
    }

    /// Whether two type indices are canonically equivalent.
    pub fn equivalent(&self, lhs: TypeIdx, rhs: TypeIdx) -> bool {
        match (self.canonical_id(lhs), self.canonical_id(rhs)) {
            (Some(lhs), Some(rhs)) => lhs == rhs,
            _ => false,
        }
    }
}

/// A canonical token for a value type within one module's type section.
///
/// Two value types are canonically equal (`match_deftype`'s substitution
/// equality) exactly when their tokens are equal: concrete references use the
/// type's canonical class id, abstract heap types their own tag, and
/// nullability is part of the token.
fn canon_val_token(canon: &TypeCanon, val: ValType) -> u64 {
    fn heap_token(canon: &TypeCanon, heap: HeapType) -> u64 {
        match heap {
            HeapType::Type(idx) => canon
                .canonical_id(idx)
                .map(|id| 0x1000_0000 + u64::from(id))
                .unwrap_or(u64::MAX),
            // Abstract heap types are field-less variants; a stable tag per
            // variant keeps the token space collision-free.
            other => {
                use HeapType::*;
                0x0100_0000
                    + match other {
                        Func => 1,
                        Extern => 2,
                        Exn => 3,
                        Any => 4,
                        None => 5,
                        Eq => 6,
                        I31 => 7,
                        Struct => 8,
                        Array => 9,
                        NoFunc => 10,
                        NoExtern => 11,
                        NoExn => 12,
                        Type(_) => unreachable!("concrete heap types use canonical ids"),
                    }
            }
        }
    }
    match val {
        ValType::Num(num) => 0x0100_0000 + num as u64,
        ValType::Vec(_) => 0x0200_0000,
        ValType::Ref(ref_type) => {
            let nullability = if ref_type.is_nullable() { 1 << 32 } else { 0 };
            0x0300_0000 | nullability | heap_token(canon, ref_type.heap_type())
        }
    }
}

/// A canonical token for a function type within one module's type section.
/// Token equality is exactly the `match_deftype` equivalence of the two
/// function types (`valid/match.ml:145-150`: rolled closed forms compared
/// after substitution), which is the relation `call_indirect`/`call_ref` and
/// host-function registration need — *not* the covariant/contravariant
/// import-matching relation.
pub fn canonical_func_token(canon: &TypeCanon, ty: &FuncType) -> Vec<u64> {
    let mut out = Vec::with_capacity(ty.params.len() + ty.results.len() + 2);
    out.push(ty.params.len() as u64);
    for param in &ty.params {
        out.push(canon_val_token(canon, *param));
    }
    out.push(u64::MAX);
    out.push(ty.results.len() as u64);
    for result in &ty.results {
        out.push(canon_val_token(canon, *result));
    }
    out
}

/// The kind of a concrete (defined) composite type, independent of any
/// module's index space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcreteKind {
    Struct,
    Array,
    Func,
}

/// The kind of the defined type at `idx`, if it exists.
pub fn concrete_kind(types: &[SubType], idx: TypeIdx) -> Option<ConcreteKind> {
    Some(match &types.get(idx.0 as usize)?.comp {
        CompType::Struct(_) => ConcreteKind::Struct,
        CompType::Array(_) => ConcreteKind::Array,
        CompType::Func(_) => ConcreteKind::Func,
    })
}

/// A module-independent structural form of a defined type: the rolled
/// closed form of the rec group it belongs to, plus its position in that
/// group.
///
/// Per `syntax/types.ml` two defined types are canonically equal exactly
/// when `rolled1 = rolled2 ∧ i1 = i2`, so structural equality of these
/// values is cross-module concrete-type equivalence. In-group references
/// stay relative (`InGroup`); external references are recursively replaced
/// by their own closed forms, so no module-local index leaks into the
/// comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedDefinedType {
    members: Vec<ClosedSubType>,
    position: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClosedSubType {
    is_final: bool,
    supers: Vec<ClosedHeapRef>,
    comp: ClosedCompType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClosedHeapRef {
    Abstract(HeapType),
    InGroup(u32),
    External(Box<ClosedDefinedType>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClosedCompType {
    Struct(Vec<ClosedFieldType>),
    Array(ClosedFieldType),
    Func(Vec<ClosedValType>, Vec<ClosedValType>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClosedFieldType {
    mutable: bool,
    storage: ClosedStorageType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClosedStorageType {
    Val(ClosedValType),
    I8,
    I16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClosedValType {
    Num(NumType),
    Vec(VecType),
    Ref(bool, ClosedHeapRef),
}

impl ClosedDefinedType {
    /// Build the closed form of the defined type at `idx`. Returns `None`
    /// for an unknown index or a reference cycle (impossible for validated
    /// modules, which only reference earlier groups).
    pub fn of(types: &[SubType], rec_groups: &[(u32, u32)], idx: TypeIdx) -> Option<Self> {
        let mut path = Vec::new();
        Self::resolve(types, rec_groups, idx, &mut path)
    }

    /// The closed form of a BARE function type: one final member with no
    /// declared supertypes and no rec-group neighbours.
    ///
    /// This is how an embedder-provided host function reaches the runtime —
    /// the pin allocates every host function's type that way
    /// (`interpreter/host/spectest.ml:35`: `DefT (RecT [SubT (Final, [],
    /// FuncT (ts1, ts2))], 0l)`), so `runtime/instance.ml:48`'s
    /// `type_of_ref (FuncRef f) = UseHT (Def (Func.type_of f))` compares a
    /// host funcref against a concrete target through exactly this shape.
    ///
    /// Returns `None` when the signature mentions a concrete heap type:
    /// those carry module-local indices, which a bare `FuncType` cannot
    /// express.
    pub fn of_func_type(ty: &FuncType) -> Option<Self> {
        Some(Self {
            members: vec![ClosedSubType {
                is_final: true,
                supers: Vec::new(),
                comp: ClosedCompType::Func(
                    ty.params
                        .iter()
                        .map(closed_val_no_context)
                        .collect::<Option<_>>()?,
                    ty.results
                        .iter()
                        .map(closed_val_no_context)
                        .collect::<Option<_>>()?,
                ),
            }],
            position: 0,
        })
    }

    /// Structural subtype test between two cross-module closed forms:
    /// equality, or a declared-supertype chain rooted at `self` that reaches
    /// `expected`. Mirrors the validator's `deftype_matches`, but works
    /// across independent type sections (no module-local index is involved).
    pub fn is_subtype_of(&self, expected: &Self) -> bool {
        if self == expected {
            return true;
        }
        let Some(member) = self.members.get(self.position as usize) else {
            return false;
        };
        member.supers.iter().any(|sup| {
            self.resolve_super(sup)
                .is_some_and(|resolved| resolved.is_subtype_of(expected))
        })
    }

    /// Resolve a declared supertype reference against `self`'s group.
    fn resolve_super(&self, sup: &ClosedHeapRef) -> Option<Self> {
        match sup {
            ClosedHeapRef::Abstract(_) => None,
            ClosedHeapRef::InGroup(position) => Some(Self {
                members: self.members.clone(),
                position: *position,
            }),
            ClosedHeapRef::External(closed) => Some((**closed).clone()),
        }
    }

    fn resolve(
        types: &[SubType],
        rec_groups: &[(u32, u32)],
        idx: TypeIdx,
        path: &mut Vec<u32>,
    ) -> Option<Self> {
        if path.contains(&idx.0) {
            return None;
        }
        path.push(idx.0);
        let (start, len) = group_span(rec_groups, idx.0);
        let mut members = Vec::with_capacity(len as usize);
        for k in 0..len {
            let sub = types.get((start + k) as usize)?;
            members.push(closed_sub(sub, types, rec_groups, start, len, path)?);
        }
        path.pop();
        Some(Self {
            members,
            position: idx.0 - start,
        })
    }
}

fn group_span(rec_groups: &[(u32, u32)], idx: u32) -> (u32, u32) {
    rec_groups
        .iter()
        .find(|&&(start, len)| idx >= start && idx < start + len)
        .copied()
        .unwrap_or((idx, 1))
}

fn closed_sub(
    sub: &SubType,
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    start: u32,
    len: u32,
    path: &mut Vec<u32>,
) -> Option<ClosedSubType> {
    Some(ClosedSubType {
        is_final: sub.is_final,
        supers: sub
            .supertypes
            .iter()
            .map(|&s| {
                closed_heap(
                    HeapType::Type(TypeIdx(s)),
                    types,
                    rec_groups,
                    start,
                    len,
                    path,
                )
            })
            .collect::<Option<_>>()?,
        comp: closed_comp(&sub.comp, types, rec_groups, start, len, path)?,
    })
}

/// The closed form of a value type with NO enclosing type section: only
/// abstract heap types are expressible, so a concrete (module-local) heap
/// reference has no closed form here.
fn closed_val_no_context(val: &ValType) -> Option<ClosedValType> {
    Some(match *val {
        ValType::Num(num) => ClosedValType::Num(num),
        ValType::Vec(vec) => ClosedValType::Vec(vec),
        ValType::Ref(RefType::FuncRef) => {
            ClosedValType::Ref(true, ClosedHeapRef::Abstract(HeapType::Func))
        }
        ValType::Ref(RefType::ExternRef) => {
            ClosedValType::Ref(true, ClosedHeapRef::Abstract(HeapType::Extern))
        }
        ValType::Ref(RefType::Typed { nullable, heap }) => match heap {
            HeapType::Type(_) => return None,
            other => ClosedValType::Ref(nullable, ClosedHeapRef::Abstract(other)),
        },
    })
}

fn closed_heap(
    heap: HeapType,
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    start: u32,
    len: u32,
    path: &mut Vec<u32>,
) -> Option<ClosedHeapRef> {
    match heap {
        HeapType::Type(idx) => {
            if idx.0 >= start && idx.0 < start + len {
                Some(ClosedHeapRef::InGroup(idx.0 - start))
            } else {
                Some(ClosedHeapRef::External(Box::new(
                    ClosedDefinedType::resolve(types, rec_groups, idx, path)?,
                )))
            }
        }
        other => Some(ClosedHeapRef::Abstract(other)),
    }
}

fn closed_comp(
    comp: &CompType,
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    start: u32,
    len: u32,
    path: &mut Vec<u32>,
) -> Option<ClosedCompType> {
    Some(match comp {
        CompType::Struct(fields) => ClosedCompType::Struct(
            fields
                .iter()
                .map(|f| closed_field(*f, types, rec_groups, start, len, path))
                .collect::<Option<_>>()?,
        ),
        CompType::Array(field) => {
            ClosedCompType::Array(closed_field(*field, types, rec_groups, start, len, path)?)
        }
        CompType::Func(func) => ClosedCompType::Func(
            func.params
                .iter()
                .map(|v| closed_val(*v, types, rec_groups, start, len, path))
                .collect::<Option<_>>()?,
            func.results
                .iter()
                .map(|v| closed_val(*v, types, rec_groups, start, len, path))
                .collect::<Option<_>>()?,
        ),
    })
}

fn closed_field(
    field: FieldType,
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    start: u32,
    len: u32,
    path: &mut Vec<u32>,
) -> Option<ClosedFieldType> {
    Some(ClosedFieldType {
        mutable: field.mutable,
        storage: match field.storage {
            StorageType::Val(v) => {
                ClosedStorageType::Val(closed_val(v, types, rec_groups, start, len, path)?)
            }
            StorageType::I8 => ClosedStorageType::I8,
            StorageType::I16 => ClosedStorageType::I16,
        },
    })
}

fn closed_val(
    val: ValType,
    types: &[SubType],
    rec_groups: &[(u32, u32)],
    start: u32,
    len: u32,
    path: &mut Vec<u32>,
) -> Option<ClosedValType> {
    Some(match val {
        ValType::Num(num) => ClosedValType::Num(num),
        ValType::Vec(vec) => ClosedValType::Vec(vec),
        ValType::Ref(RefType::FuncRef) => {
            ClosedValType::Ref(true, ClosedHeapRef::Abstract(HeapType::Func))
        }
        ValType::Ref(RefType::ExternRef) => {
            ClosedValType::Ref(true, ClosedHeapRef::Abstract(HeapType::Extern))
        }
        ValType::Ref(RefType::Typed { nullable, heap }) => ClosedValType::Ref(
            nullable,
            closed_heap(heap, types, rec_groups, start, len, path)?,
        ),
    })
}

/// Structural (module-agnostic) equality of the defined types at `lhs` and
/// `rhs` — the spec's rolled-closed-form canonical equality, computed
/// across two independent type sections.
pub fn defined_types_equivalent(
    lhs_types: &[SubType],
    lhs_rec_groups: &[(u32, u32)],
    lhs: TypeIdx,
    rhs_types: &[SubType],
    rhs_rec_groups: &[(u32, u32)],
    rhs: TypeIdx,
) -> bool {
    match (
        ClosedDefinedType::of(lhs_types, lhs_rec_groups, lhs),
        ClosedDefinedType::of(rhs_types, rhs_rec_groups, rhs),
    ) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        _ => false,
    }
}

/// `match_deftype` across two type sections (`valid/match.ml:145-150`): the
/// PROVIDED type entry must be canonically identical to the declared one
/// (`dt1 == dt2`, i.e. equal rolled closed forms — iso-recursive groups
/// included, so group shape and position participate), or the provided entry
/// must declare a supertype chain reaching the declared one.
///
/// This is the relation import/export matching uses for functions
/// (`match_externtype` calls `match_deftype` on the two type entries), and the
/// relation `call_indirect`/`call_ref` use at run time. It is **not** the
/// covariant/contravariant `match_comptype` relation, which belongs to
/// subtype declarations.
pub fn defined_type_matches(
    provided_types: &[SubType],
    provided_recs: &[(u32, u32)],
    provided: TypeIdx,
    declared_types: &[SubType],
    declared_recs: &[(u32, u32)],
    declared: TypeIdx,
) -> bool {
    match (
        ClosedDefinedType::of(provided_types, provided_recs, provided),
        ClosedDefinedType::of(declared_types, declared_recs, declared),
    ) {
        (Some(provided), Some(declared)) => {
            provided == declared || provided.is_subtype_of(&declared)
        }
        _ => false,
    }
}

/// A canonical token for a value type within one module's type section.
///
/// Two value types are canonically equal (`match_deftype`'s substitution
/// equality) exactly when their tokens are equal: concrete references use the
/// type's canonical class id, abstract heap types their own tag, and
/// nullability is part of the token.
fn canon_ref_token(idx: u32, start: u32, len: u32, canonical: &[u32]) -> u32 {
    if idx >= start && idx < start + len {
        // In-group: relative rec reference.
        0x8000_0000 | (idx - start)
    } else {
        canonical.get(idx as usize).copied().unwrap_or(idx)
    }
}

fn canon_heap(heap: HeapType, start: u32, len: u32, canonical: &[u32]) -> CanonHeap {
    match heap {
        HeapType::Type(idx) => {
            if idx.0 >= start && idx.0 < start + len {
                CanonHeap::InGroup(idx.0 - start)
            } else {
                CanonHeap::External(canon_ref_token(idx.0, start, len, canonical))
            }
        }
        other => CanonHeap::Abstract(other),
    }
}

fn canon_val(val: ValType, start: u32, len: u32, canonical: &[u32]) -> CanonVal {
    match val {
        ValType::Num(num) => CanonVal::Num(num),
        ValType::Vec(vec) => CanonVal::Vec(vec),
        ValType::Ref(RefType::FuncRef) => CanonVal::Ref(true, CanonHeap::Abstract(HeapType::Func)),
        ValType::Ref(RefType::ExternRef) => {
            CanonVal::Ref(true, CanonHeap::Abstract(HeapType::Extern))
        }
        ValType::Ref(RefType::Typed { nullable, heap }) => {
            CanonVal::Ref(nullable, canon_heap(heap, start, len, canonical))
        }
    }
}

fn canon_storage(storage: StorageType, start: u32, len: u32, canonical: &[u32]) -> CanonStorage {
    match storage {
        StorageType::Val(val) => CanonStorage::Val(canon_val(val, start, len, canonical)),
        StorageType::I8 => CanonStorage::I8,
        StorageType::I16 => CanonStorage::I16,
    }
}

fn canon_field(field: FieldType, start: u32, len: u32, canonical: &[u32]) -> CanonField {
    CanonField {
        mutable: field.mutable,
        storage: canon_storage(field.storage, start, len, canonical),
    }
}

fn canon_comp(comp: &CompType, start: u32, len: u32, canonical: &[u32]) -> CanonComp {
    match comp {
        CompType::Struct(fields) => CanonComp::Struct(
            fields
                .iter()
                .map(|field| canon_field(*field, start, len, canonical))
                .collect(),
        ),
        CompType::Array(field) => CanonComp::Array(canon_field(*field, start, len, canonical)),
        CompType::Func(func) => CanonComp::Func(
            func.params
                .iter()
                .map(|val| canon_val(*val, start, len, canonical))
                .collect(),
            func.results
                .iter()
                .map(|val| canon_val(*val, start, len, canonical))
                .collect(),
        ),
    }
}

fn canon_member(sub: &SubType, start: u32, len: u32, canonical: &[u32]) -> CanonMember {
    CanonMember {
        is_final: sub.is_final,
        supers: sub
            .supertypes
            .iter()
            .map(|&s| canon_ref_token(s, start, len, canonical))
            .collect(),
        comp: canon_comp(&sub.comp, start, len, canonical),
    }
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
