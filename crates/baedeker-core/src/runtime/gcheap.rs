// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! The GC heap object model (slice 5: collector-shaped, never-free).
//!
//! The heap is an append-only `ObjectId`-indexed table of struct/array
//! objects. Slice 5 never frees; the real collector (slice 5b) swaps the
//! strategy behind this interface without touching AOT or FFI. Every
//! allocation path reserves its capacity *before* building the payload, so
//! a huge-but-valid size is a clean [`RuntimeErrorKind::ResourceLimitExceeded`]
//! refusal rather than an abort, and a refused allocation leaves no partial
//! object behind.
//!
//! Each heap carries a process-unique [`HeapId`]; heap references
//! (`Value::GcRef`) carry it so a reference created in one store's heap can
//! be resolved against a registered foreign heap in a linked store rather
//! than misinterpreted as a local `ObjectId`.
//!
//! Aggregate layout ids are interned by their **cross-module closed form**
//! (U1's [`ClosedDefinedType`]) when allocated from a module, so two
//! modules that declare structurally identical struct/array types share a
//! layout id even when their local type indices differ.
//!
//! `live_bytes` is **logical** accounting, frozen by the design's formula:
//! struct = 8-byte header + Σ field widths; array = 12-byte header +
//! len × elem width; widths i8=1 i16=2 i32/f32=4 i64/f64=8 v128=16 and a
//! reference handle is 4. `i31` values are immediates and contribute 0.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{RuntimeError, RuntimeErrorKind, Value};
use crate::types::{ClosedDefinedType, FieldType, StorageType, ValType};

/// A process-unique heap identity. `Value::GcRef` carries one so object
/// ids from different stores never alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HeapId(pub u64);

/// Source of fresh heap ids. The first heap gets 1 so 0 is never a valid
/// heap identity (mirrors instance id 0 being reserved).
static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);

/// An index into the heap's object table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub u32);

/// A heap-interned type layout identifier.
///
/// The heap interns [`GcTypeKind`] by structural equality, so two modules
/// that declare structurally identical struct/array types share a layout id.
/// (U3 casts use the validator's declared-supertype chains, not this table.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GcTypeId(pub u32);

/// The layout of a GC aggregate type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum GcTypeKind {
    Struct(Vec<FieldType>),
    Array(FieldType),
}

/// The interning key. `Raw` is used by the direct heap API (no module
/// context); `Closed` is the cross-module structural identity produced by
/// the runtime, so a nested concrete reference at a different local type
/// index still interns equal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeKey {
    Closed(ClosedDefinedType),
    Raw(GcTypeKind),
}

/// A GC heap object. Never freed in slice 5.
#[derive(Debug, Clone, PartialEq)]
pub enum GcObject {
    Struct {
        type_id: GcTypeId,
        fields: Vec<Value>,
    },
    Array {
        type_id: GcTypeId,
        len: u32,
        elems: Vec<Value>,
    },
}

/// A packed-field read extension marker (`struct.get_s` / `get_u`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext {
    S,
    U,
}

/// A point-in-time allocation counter for [`GcHeap::allocations_since`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcSnapshot {
    allocations: u64,
}

/// The heap's public accounting view. All fields are **logical**: reserved
/// capacity is exposed separately from live bytes and is explicitly not
/// allocator overhead or process RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcStats {
    pub live_objects: u32,
    pub live_bytes: u64,
    pub reserved_slots: u32,
    pub reserved_bytes: u64,
}

/// Bytes occupied by a single storage slot (the logical width).
pub fn storage_width(storage: StorageType) -> u64 {
    match storage {
        StorageType::I8 => 1,
        StorageType::I16 => 2,
        StorageType::Val(val) => val_width(val),
    }
}

/// Logical bytes for a value type (reference handles count as 4).
pub fn val_width(val: ValType) -> u64 {
    match val {
        ValType::Num(crate::types::NumType::I32 | crate::types::NumType::F32) => 4,
        ValType::Num(crate::types::NumType::I64 | crate::types::NumType::F64) => 8,
        ValType::Vec(_) => 16,
        ValType::Ref(_) => 4,
    }
}

impl GcTypeKind {
    /// Logical bytes for an aggregate of this type with `len` elements
    /// (structs ignore `len`).
    fn logical_bytes(&self, len: u32) -> u64 {
        match self {
            GcTypeKind::Struct(fields) => {
                8 + fields
                    .iter()
                    .map(|field| storage_width(field.storage))
                    .sum::<u64>()
            }
            GcTypeKind::Array(field) => 12 + storage_width(field.storage) * len as u64,
        }
    }
}

fn alloc_failed() -> RuntimeError {
    RuntimeError {
        kind: RuntimeErrorKind::ResourceLimitExceeded { what: "gc heap" },
    }
}

/// A never-free GC heap.
#[derive(Debug)]
pub struct GcHeap {
    id: HeapId,
    objects: Vec<GcObject>,
    types: Vec<GcTypeKind>,
    type_ids: Vec<(TypeKey, GcTypeId)>,
    allocations: u64,
}

impl GcHeap {
    /// An empty heap with a fresh process-unique identity.
    pub fn new() -> Self {
        Self {
            id: HeapId(NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed)),
            objects: Vec::new(),
            types: Vec::new(),
            type_ids: Vec::new(),
            allocations: 0,
        }
    }

    /// This heap's process-unique identity.
    pub fn heap_id(&self) -> HeapId {
        self.id
    }

    /// Number of live objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the heap holds no objects.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Intern a layout, returning its stable id. Fallible end-to-end: the
    /// key clone is try-reserved, then the table slots, so a refusal (OOM)
    /// reports [`RuntimeErrorKind::ResourceLimitExceeded`] and leaves the
    /// type table untouched.
    pub fn intern_type(&mut self, kind: GcTypeKind) -> Result<GcTypeId, RuntimeError> {
        let key = TypeKey::Raw(try_clone_kind(&kind)?);
        self.intern_owned(key, kind)
    }

    /// Intern a layout by its cross-module closed form, so structurally
    /// identical aggregate types at different module-local indices share an
    /// id. The closed form is moved, not cloned. Fallible as
    /// [`GcHeap::intern_type`].
    pub fn intern_closed(
        &mut self,
        closed: ClosedDefinedType,
        kind: GcTypeKind,
    ) -> Result<GcTypeId, RuntimeError> {
        self.intern_owned(TypeKey::Closed(closed), kind)
    }

    fn intern_owned(&mut self, key: TypeKey, layout: GcTypeKind) -> Result<GcTypeId, RuntimeError> {
        if let Some((_, id)) = self.type_ids.iter().find(|(known, _)| *known == key) {
            return Ok(*id);
        }
        // Reserve every table slot before committing either push.
        self.types.try_reserve(1).map_err(|_| alloc_failed())?;
        self.type_ids.try_reserve(1).map_err(|_| alloc_failed())?;
        let id = GcTypeId(self.types.len() as u32);
        self.types.push(layout);
        self.type_ids.push((key, id));
        Ok(id)
    }

    /// The layout of an interned type.
    pub fn type_kind(&self, id: GcTypeId) -> Option<&GcTypeKind> {
        self.types.get(id.0 as usize)
    }

    /// The cross-module closed form of an interned type, when it was
    /// interned from a module type (the direct-API `intern_type` path has
    /// no closed form).
    pub fn closed_type(&self, id: GcTypeId) -> Option<&ClosedDefinedType> {
        self.type_ids.iter().find_map(|(key, tid)| {
            if *tid == id {
                match key {
                    TypeKey::Closed(closed) => Some(closed),
                    TypeKey::Raw(_) => None,
                }
            } else {
                None
            }
        })
    }

    /// The object at `id`, if any.
    pub fn object(&self, id: ObjectId) -> Option<&GcObject> {
        self.objects.get(id.0 as usize)
    }

    /// Mutable access to the object at `id`, if any.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut GcObject> {
        self.objects.get_mut(id.0 as usize)
    }

    /// The field values of a struct object.
    pub fn struct_fields(&self, id: ObjectId) -> Option<&[Value]> {
        match self.object(id)? {
            GcObject::Struct { fields, .. } => Some(fields),
            GcObject::Array { .. } => None,
        }
    }

    /// The element values of an array object.
    pub fn array_values(&self, id: ObjectId) -> Option<&[Value]> {
        match self.object(id)? {
            GcObject::Array { elems, .. } => Some(elems),
            GcObject::Struct { .. } => None,
        }
    }

    /// The length of an array object.
    pub fn array_len(&self, id: ObjectId) -> Option<u32> {
        match self.object(id)? {
            GcObject::Array { len, .. } => Some(*len),
            GcObject::Struct { .. } => None,
        }
    }

    /// The packed-read extension for an array's element storage: `Some(U)`
    /// for packed i8/i16, `None` for an unpacked storage type.
    pub fn array_ext(&self, id: ObjectId) -> Option<Ext> {
        match self.object_type(id)? {
            GcTypeKind::Array(field) => match field.storage {
                StorageType::I8 | StorageType::I16 => Some(Ext::U),
                StorageType::Val(_) => None,
            },
            GcTypeKind::Struct(_) => None,
        }
    }

    /// The layout of an object.
    pub fn object_type(&self, id: ObjectId) -> Option<&GcTypeKind> {
        match self.object(id)? {
            GcObject::Struct { type_id, .. } | GcObject::Array { type_id, .. } => {
                self.type_kind(*type_id)
            }
        }
    }

    /// The interned type id of an object.
    pub fn object_type_id(&self, id: ObjectId) -> Option<GcTypeId> {
        match self.object(id)? {
            GcObject::Struct { type_id, .. } | GcObject::Array { type_id, .. } => Some(*type_id),
        }
    }

    /// Allocate a struct. `fields` must already hold the (unpacked) field
    /// values in declaration order; packed storages are wrapped here.
    ///
    /// Fallible end-to-end: the payload vector is reserved before any field
    /// is written, then the object-table slot, then the layout is interned;
    /// a refusal at any stage leaves the heap untouched.
    pub fn alloc_struct(
        &mut self,
        kind: GcTypeKind,
        fields: Vec<Value>,
    ) -> Result<ObjectId, RuntimeError> {
        self.alloc_struct_inner(None, kind, fields)
    }

    /// Allocate a struct, interning by cross-module closed form.
    pub fn alloc_struct_closed(
        &mut self,
        closed: ClosedDefinedType,
        kind: GcTypeKind,
        fields: Vec<Value>,
    ) -> Result<ObjectId, RuntimeError> {
        self.alloc_struct_inner(Some(closed), kind, fields)
    }

    fn alloc_struct_inner(
        &mut self,
        closed: Option<ClosedDefinedType>,
        kind: GcTypeKind,
        fields: Vec<Value>,
    ) -> Result<ObjectId, RuntimeError> {
        // Try-reserve the payload BEFORE building, then fill. Borrow the
        // layout only for the duration of the fill so `kind` can move into
        // interning afterwards (no infallible clone).
        let mut stored: Vec<Value> = Vec::new();
        match &kind {
            GcTypeKind::Struct(field_types) => {
                stored
                    .try_reserve(field_types.len())
                    .map_err(|_| alloc_failed())?;
                for (field, value) in field_types.iter().zip(fields) {
                    stored.push(pack_field(field.storage, value));
                }
            }
            GcTypeKind::Array(_) => return Err(alloc_failed()),
        }
        self.objects.try_reserve(1).map_err(|_| alloc_failed())?;
        let type_id = match closed {
            Some(closed) => self.intern_closed(closed, kind)?,
            None => self.intern_type(kind)?,
        };
        let id = ObjectId(self.objects.len() as u32);
        self.objects.push(GcObject::Struct {
            type_id,
            fields: stored,
        });
        self.allocations += 1;
        Ok(id)
    }

    /// Allocate an array of `len` copies of `fill` (`array.new` /
    /// `array.new_default`). Reserves the payload before filling, so a huge
    /// length refuses cleanly with no partial object.
    pub fn alloc_array(
        &mut self,
        kind: GcTypeKind,
        len: u32,
        fill: Value,
    ) -> Result<ObjectId, RuntimeError> {
        self.alloc_array_inner(None, kind, len, fill)
    }

    /// Allocate an array, interning by cross-module closed form.
    pub fn alloc_array_closed(
        &mut self,
        closed: ClosedDefinedType,
        kind: GcTypeKind,
        len: u32,
        fill: Value,
    ) -> Result<ObjectId, RuntimeError> {
        self.alloc_array_inner(Some(closed), kind, len, fill)
    }

    fn alloc_array_inner(
        &mut self,
        closed: Option<ClosedDefinedType>,
        kind: GcTypeKind,
        len: u32,
        fill: Value,
    ) -> Result<ObjectId, RuntimeError> {
        let field = match &kind {
            GcTypeKind::Array(field) => *field,
            GcTypeKind::Struct(_) => return Err(alloc_failed()),
        };
        let count = len as usize;
        // Count-first: payload capacity, then the table slot; only then
        // intern and materialize. A refusal leaves both untouched.
        let mut elems: Vec<Value> = Vec::new();
        elems.try_reserve(count).map_err(|_| alloc_failed())?;
        self.objects.try_reserve(1).map_err(|_| alloc_failed())?;
        let type_id = match closed {
            Some(closed) => self.intern_closed(closed, kind)?,
            None => self.intern_type(kind)?,
        };
        let packed = pack_field(field.storage, fill);
        elems.resize(count, packed);
        let id = ObjectId(self.objects.len() as u32);
        self.objects.push(GcObject::Array {
            type_id,
            len,
            elems,
        });
        self.allocations += 1;
        Ok(id)
    }

    /// Allocate an array from explicit element values (`array.new_fixed`,
    /// `array.new_data`, `array.new_elem`). Packed storages are wrapped.
    pub fn alloc_fixed(
        &mut self,
        kind: GcTypeKind,
        elems: Vec<Value>,
    ) -> Result<ObjectId, RuntimeError> {
        self.alloc_fixed_inner(None, kind, elems)
    }

    /// Allocate a fixed array, interning by cross-module closed form.
    pub fn alloc_fixed_closed(
        &mut self,
        closed: ClosedDefinedType,
        kind: GcTypeKind,
        elems: Vec<Value>,
    ) -> Result<ObjectId, RuntimeError> {
        self.alloc_fixed_inner(Some(closed), kind, elems)
    }

    fn alloc_fixed_inner(
        &mut self,
        closed: Option<ClosedDefinedType>,
        kind: GcTypeKind,
        elems: Vec<Value>,
    ) -> Result<ObjectId, RuntimeError> {
        let field = match &kind {
            GcTypeKind::Array(field) => *field,
            GcTypeKind::Struct(_) => return Err(alloc_failed()),
        };
        let mut stored: Vec<Value> = Vec::new();
        stored
            .try_reserve(elems.len())
            .map_err(|_| alloc_failed())?;
        for value in elems {
            stored.push(pack_field(field.storage, value));
        }
        self.objects.try_reserve(1).map_err(|_| alloc_failed())?;
        let type_id = match closed {
            Some(closed) => self.intern_closed(closed, kind)?,
            None => self.intern_type(kind)?,
        };
        let len = stored.len() as u32;
        let id = ObjectId(self.objects.len() as u32);
        self.objects.push(GcObject::Array {
            type_id,
            len,
            elems: stored,
        });
        self.allocations += 1;
        Ok(id)
    }

    /// Read a struct field (packed fields are zero/sign-extended per `ext`).
    pub fn struct_get(
        &self,
        id: ObjectId,
        field: u32,
        ext: Option<Ext>,
    ) -> Result<Value, RuntimeError> {
        let kind = self.object_type(id).ok_or_else(alloc_failed)?;
        let storage = match kind {
            GcTypeKind::Struct(fields) => fields
                .get(field as usize)
                .map(|field| field.storage)
                .ok_or_else(alloc_failed)?,
            GcTypeKind::Array(_) => return Err(alloc_failed()),
        };
        let value = match self.object(id) {
            Some(GcObject::Struct { fields, .. }) => fields
                .get(field as usize)
                .copied()
                .ok_or_else(alloc_failed)?,
            _ => return Err(alloc_failed()),
        };
        Ok(read_field(value, storage, ext))
    }

    /// Write a struct field (packed storages are wrapped).
    pub fn struct_set(
        &mut self,
        id: ObjectId,
        field: u32,
        value: Value,
    ) -> Result<(), RuntimeError> {
        let storage = match self.object_type(id) {
            Some(GcTypeKind::Struct(fields)) => {
                fields.get(field as usize).map(|field| field.storage)
            }
            _ => None,
        }
        .ok_or_else(alloc_failed)?;
        match self.object_mut(id) {
            Some(GcObject::Struct { fields, .. }) => {
                let slot = fields.get_mut(field as usize).ok_or_else(alloc_failed)?;
                *slot = pack_field(storage, value);
                Ok(())
            }
            _ => Err(alloc_failed()),
        }
    }

    /// Read an array element (packed fields are zero/sign-extended per `ext`).
    pub fn array_get(
        &self,
        id: ObjectId,
        index: u32,
        ext: Option<Ext>,
    ) -> Result<Value, RuntimeError> {
        let storage = match self.object_type(id) {
            Some(GcTypeKind::Array(field)) => field.storage,
            _ => return Err(alloc_failed()),
        };
        let value = match self.object(id) {
            Some(GcObject::Array { elems, .. }) => elems
                .get(index as usize)
                .copied()
                .ok_or_else(alloc_failed)?,
            _ => return Err(alloc_failed()),
        };
        Ok(read_field(value, storage, ext))
    }

    /// Write an array element (packed storages are wrapped).
    pub fn array_set(
        &mut self,
        id: ObjectId,
        index: u32,
        value: Value,
    ) -> Result<(), RuntimeError> {
        let storage = match self.object_type(id) {
            Some(GcTypeKind::Array(field)) => field.storage,
            _ => return Err(alloc_failed()),
        };
        match self.object_mut(id) {
            Some(GcObject::Array { elems, .. }) => {
                let slot = elems.get_mut(index as usize).ok_or_else(alloc_failed)?;
                *slot = pack_field(storage, value);
                Ok(())
            }
            _ => Err(alloc_failed()),
        }
    }

    /// A snapshot for [`GcHeap::allocations_since`].
    pub fn snapshot(&self) -> GcSnapshot {
        GcSnapshot {
            allocations: self.allocations,
        }
    }

    /// Object-table insertions since `snapshot`.
    pub fn allocations_since(&self, snapshot: GcSnapshot) -> u64 {
        self.allocations - snapshot.allocations
    }

    /// Logical accounting plus reserved capacity.
    pub fn stats(&self) -> GcStats {
        let mut live_bytes: u64 = 0;
        for object in &self.objects {
            match object {
                GcObject::Struct { type_id, .. } => {
                    if let Some(kind) = self.type_kind(*type_id) {
                        live_bytes += kind.logical_bytes(0);
                    }
                }
                GcObject::Array { type_id, len, .. } => {
                    if let Some(kind) = self.type_kind(*type_id) {
                        live_bytes += kind.logical_bytes(*len);
                    }
                }
            }
        }
        let reserved_bytes: u64 = self
            .objects
            .iter()
            .map(|object| match object {
                GcObject::Struct { fields, .. } => {
                    (fields.capacity() * core::mem::size_of::<Value>()) as u64
                }
                GcObject::Array { elems, .. } => {
                    (elems.capacity() * core::mem::size_of::<Value>()) as u64
                }
            })
            .sum::<u64>()
            + (self.objects.capacity() * core::mem::size_of::<GcObject>()) as u64;
        GcStats {
            live_objects: self.objects.len() as u32,
            live_bytes,
            reserved_slots: self.objects.capacity() as u32,
            reserved_bytes,
        }
    }
}

impl Default for GcHeap {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap a value into a packed storage slot (low 8/16 bits).
/// Fallibly clone a layout for use as a `Raw` interning key.
fn try_clone_kind(kind: &GcTypeKind) -> Result<GcTypeKind, RuntimeError> {
    match kind {
        GcTypeKind::Struct(fields) => {
            let mut out: Vec<FieldType> = Vec::new();
            out.try_reserve(fields.len()).map_err(|_| alloc_failed())?;
            out.extend_from_slice(fields);
            Ok(GcTypeKind::Struct(out))
        }
        GcTypeKind::Array(field) => Ok(GcTypeKind::Array(*field)),
    }
}

/// Wrap a value into a packed storage slot (low 8/16 bits).
fn pack_field(storage: StorageType, value: Value) -> Value {
    match storage {
        StorageType::Val(_) => value,
        StorageType::I8 => Value::I32(value_i32(value) & 0xFF),
        StorageType::I16 => Value::I32(value_i32(value) & 0xFFFF),
    }
}

/// Zero/sign-extend a packed storage slot to the unpacked `i32`.
fn read_field(value: Value, storage: StorageType, ext: Option<Ext>) -> Value {
    match storage {
        StorageType::Val(_) => value,
        StorageType::I8 => Value::I32(extend(value_i32(value), 8, ext)),
        StorageType::I16 => Value::I32(extend(value_i32(value), 16, ext)),
    }
}

fn value_i32(value: Value) -> i32 {
    match value {
        Value::I32(value) => value,
        _ => 0,
    }
}

fn extend(raw: i32, bits: u32, ext: Option<Ext>) -> i32 {
    let shift = 32 - bits;
    match ext {
        Some(Ext::S) => (raw << shift) >> shift,
        // Validation only admits `_s`/`_u` for packed fields; treat a
        // missing marker as the unsigned read.
        Some(Ext::U) | None => raw & (i32::MAX >> (31 - bits)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NumType, RefType};
    use alloc::vec;

    fn f(storage: StorageType) -> FieldType {
        FieldType {
            mutable: true,
            storage,
        }
    }

    #[test]
    fn struct_stats_exact() {
        let mut heap = GcHeap::new();
        let kind = GcTypeKind::Struct(vec![
            f(StorageType::Val(ValType::Num(NumType::I32))),
            f(StorageType::Val(ValType::Vec(crate::types::VecType::V128))),
        ]);
        heap.alloc_struct(kind, vec![Value::I32(0), Value::V128([0; 16])])
            .unwrap();
        // 8 header + 4 + 16 = 28
        assert_eq!(heap.stats().live_bytes, 28);
        assert_eq!(heap.stats().live_objects, 1);
    }

    #[test]
    fn array_stats_exact() {
        let mut heap = GcHeap::new();
        let kind = GcTypeKind::Array(f(StorageType::Val(ValType::Vec(
            crate::types::VecType::V128,
        ))));
        heap.alloc_array(kind, 3, Value::V128([0; 16])).unwrap();
        // 12 header + 3 * 16 = 60
        assert_eq!(heap.stats().live_bytes, 60);
    }

    #[test]
    fn packed_roundtrip() {
        let mut heap = GcHeap::new();
        let kind = GcTypeKind::Array(f(StorageType::I8));
        let id = heap.alloc_fixed(kind, vec![Value::I32(0x1FF)]).unwrap();
        assert_eq!(
            heap.array_get(id, 0, Some(Ext::U)).unwrap(),
            Value::I32(0xFF)
        );
        assert_eq!(heap.array_get(id, 0, Some(Ext::S)).unwrap(), Value::I32(-1));
    }

    #[test]
    fn huge_array_refused_atomically() {
        let mut heap = GcHeap::new();
        let before = heap.stats();
        let kind = GcTypeKind::Array(f(StorageType::Val(ValType::Ref(RefType::FuncRef))));
        assert!(
            heap.alloc_array(kind, u32::MAX, Value::FuncRef(None))
                .is_err()
        );
        assert_eq!(heap.stats(), before);
    }

    #[test]
    fn allocations_since_counts_insertions() {
        let mut heap = GcHeap::new();
        let snap = heap.snapshot();
        let kind = GcTypeKind::Array(f(StorageType::Val(ValType::Num(NumType::I32))));
        heap.alloc_array(kind.clone(), 2, Value::I32(0)).unwrap();
        heap.alloc_array(kind, 0, Value::I32(0)).unwrap();
        assert_eq!(heap.allocations_since(snap), 2);
    }

    #[test]
    fn heap_ids_are_unique() {
        let a = GcHeap::new();
        let b = GcHeap::new();
        assert_ne!(a.heap_id(), b.heap_id());
        assert_ne!(a.heap_id().0, 0);
    }
}
