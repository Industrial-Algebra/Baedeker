// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2: resource discipline for the never-free heap.
//!
//! Every allocation path reserves before building, so a huge-but-valid size
//! is a clean `ResourceLimitExceeded` refusal with no partial object and no
//! abort. These tests double as the try_reserve-before-build audit: a
//! refusal must leave `live_objects`/`live_bytes` untouched and the heap
//! still usable.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{GcHeap, GcTypeKind, RuntimeErrorKind, Store, execute_export};
use baedeker_core::types::{FieldType, NumType, StorageType, ValType};

fn field(storage: StorageType) -> FieldType {
    FieldType {
        mutable: true,
        storage,
    }
}

fn i32_array() -> GcTypeKind {
    GcTypeKind::Array(field(StorageType::Val(ValType::Num(NumType::I32))))
}

#[test]
fn huge_array_new_refuses_atomically() {
    let mut heap = GcHeap::new();
    let before = heap.stats();
    let result = heap.alloc_array(i32_array(), u32::MAX, baedeker_core::runtime::Value::I32(0));
    assert!(matches!(
        result,
        Err(error) if matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
    ));
    assert_eq!(heap.stats(), before);
    assert_eq!(heap.len(), 0);
}

#[test]
fn heap_remains_usable_after_a_refusal() {
    let mut heap = GcHeap::new();
    assert!(
        heap.alloc_array(i32_array(), u32::MAX, baedeker_core::runtime::Value::I32(0))
            .is_err()
    );
    // A normal allocation after the refusal succeeds and is the only object.
    let id = heap
        .alloc_array(i32_array(), 3, baedeker_core::runtime::Value::I32(7))
        .expect("post-refusal allocation succeeds");
    assert_eq!(heap.len(), 1);
    assert_eq!(heap.stats().live_objects, 1);
    assert_eq!(heap.stats().live_bytes, 12 + 3 * 4);
    assert_eq!(
        heap.array_get(id, 2, None).unwrap(),
        baedeker_core::runtime::Value::I32(7)
    );
}

#[test]
fn huge_new_fixed_count_underflows_before_building() {
    // `array.new_fixed` with a huge count on a shallow operand stack is a
    // validation underflow, not a `count`-sized allocation. (The runtime
    // term is only reachable with a matching operand stack, which a real
    // function body cannot build.)
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result (ref $a))
    i32.const 1
    array.new_fixed $a 2000000000))"#;
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    let error = module.validate().expect_err("huge count underflows");
    assert!(matches!(
        error.kind,
        baedeker_core::validate::error::ValidationErrorKind::StackUnderflow { .. }
    ));
}

#[test]
fn huge_array_new_default_from_module_refuses_cleanly() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f")
    i32.const -1
    array.new_default $a
    drop))"#;
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module: RegModule = Module::decode(&bytes).unwrap().lower().unwrap();
    let store = Store::instantiate(&module).unwrap();
    let before = store.gc_stats();
    let error = execute_export(&module, &store, "f", &[]).expect_err("huge array refuses");
    assert!(
        matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. }),
        "expected ResourceLimitExceeded, got {error:?}"
    );
    assert_eq!(store.gc_stats(), before);
    assert_eq!(store.gc_stats().live_objects, 0);
}

#[test]
fn array_new_data_checks_segment_before_payload() {
    let source = r#"(module
  (type $a (array (mut i8)))
  (data $d "\01\02")
  (func (export "f")
    i32.const 0 i32.const -1 array.new_data $a $d
    drop))"#;
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module: RegModule = Module::decode(&bytes).unwrap().lower().unwrap();
    let store = Store::instantiate(&module).unwrap();
    let snapshot = store.gc_snapshot();
    let error = execute_export(&module, &store, "f", &[]).expect_err("segment bounds trap");
    assert!(matches!(
        error.kind,
        RuntimeErrorKind::Trap(baedeker_core::runtime::RuntimeTrap::OutOfBoundsMemoryAccess)
    ));
    assert_eq!(store.gc_allocations_since(snapshot), 0);
    assert_eq!(store.gc_stats().live_objects, 0);
}
