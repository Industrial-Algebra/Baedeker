// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2: exact logical heap accounting (`GcStats`).
//!
//! `live_bytes` is the frozen logical formula: struct = 8 + Σ widths;
//! array = 12 + len × width; widths i8=1 i16=2 i32/f32=4 i64/f64=8
//! v128=16, reference handle 4; i31 contributes 0.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{GcHeap, GcTypeKind, RuntimeErrorKind, Store, Value, execute_export};
use baedeker_core::types::{FieldType, NumType, RefType, StorageType, ValType, VecType};

fn field(storage: StorageType) -> FieldType {
    FieldType {
        mutable: true,
        storage,
    }
}

fn v128() -> StorageType {
    StorageType::Val(ValType::Vec(VecType::V128))
}

#[test]
fn v128_struct_is_24_bytes() {
    let mut heap = GcHeap::new();
    let kind = GcTypeKind::Struct(vec![field(v128())]);
    heap.alloc_struct(kind, vec![Value::V128([0; 16])]).unwrap();
    let stats = heap.stats();
    assert_eq!(stats.live_objects, 1);
    assert_eq!(stats.live_bytes, 8 + 16);
}

#[test]
fn three_v128_array_is_60_bytes() {
    let mut heap = GcHeap::new();
    let kind = GcTypeKind::Array(field(v128()));
    heap.alloc_array(kind, 3, Value::V128([0; 16])).unwrap();
    let stats = heap.stats();
    assert_eq!(stats.live_objects, 1);
    assert_eq!(stats.live_bytes, 12 + 3 * 16);
}

#[test]
fn mixed_width_struct_accounting() {
    let mut heap = GcHeap::new();
    let kind = GcTypeKind::Struct(vec![
        field(StorageType::I8),
        field(StorageType::I16),
        field(StorageType::Val(ValType::Num(NumType::I32))),
        field(StorageType::Val(ValType::Num(NumType::I64))),
        field(StorageType::Val(ValType::Ref(RefType::FuncRef))),
    ]);
    heap.alloc_struct(kind, vec![Value::I32(0); 5]).unwrap();
    // 8 + 1 + 2 + 4 + 8 + 4 = 27
    assert_eq!(heap.stats().live_bytes, 27);
}

#[test]
fn allocations_since_counts_only_insertions() {
    let mut heap = GcHeap::new();
    let snapshot = heap.snapshot();
    let kind = GcTypeKind::Array(field(StorageType::Val(ValType::Num(NumType::I32))));
    heap.alloc_array(kind.clone(), 2, Value::I32(0)).unwrap();
    assert_eq!(heap.allocations_since(snapshot), 1);
    heap.alloc_array(kind, 0, Value::I32(0)).unwrap();
    assert_eq!(heap.allocations_since(snapshot), 2);
}

#[test]
fn refused_allocation_leaves_stats_unchanged() {
    let mut heap = GcHeap::new();
    let before = heap.stats();
    let kind = GcTypeKind::Array(field(v128()));
    let result = heap.alloc_array(kind, u32::MAX, Value::V128([0; 16]));
    assert!(matches!(
        result,
        Err(error) if matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
    ));
    assert_eq!(heap.stats(), before);
}

#[test]
fn store_i31_operations_allocate_nothing() {
    let source = r#"(module
  (func (export "f") (result i32)
    i32.const 123 ref.i31 i31.get_u))"#;
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module: RegModule = Module::decode(&bytes).unwrap().lower().unwrap();
    let store = Store::instantiate(&module).unwrap();
    let before = store.gc_stats();
    let snapshot = store.gc_snapshot();
    assert_eq!(
        execute_export(&module, &store, "f", &[]),
        Ok(vec![Value::I32(123)])
    );
    assert_eq!(store.gc_allocations_since(snapshot), 0);
    assert_eq!(store.gc_stats(), before);
}

#[test]
fn store_stats_track_a_struct_allocation() {
    let source = r#"(module
  (type $s (struct (field v128)))
  (func (export "f") (result i32)
    v128.const i32x4 0 0 0 0 struct.new $s
    drop
    i32.const 0))"#;
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module: RegModule = Module::decode(&bytes).unwrap().lower().unwrap();
    let store = Store::instantiate(&module).unwrap();
    let snapshot = store.gc_snapshot();
    assert_eq!(
        execute_export(&module, &store, "f", &[]),
        Ok(vec![Value::I32(0)])
    );
    assert_eq!(store.gc_allocations_since(snapshot), 1);
    assert_eq!(store.gc_stats().live_objects, 1);
    assert_eq!(store.gc_stats().live_bytes, 24);
}
