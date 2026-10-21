// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3: runtime type queries allocate nothing.
//!
//! The declared-supertype ancestry chains are precomputed at intern time, so
//! repeated `ref.test`/`ref.cast`/`br_on_cast` executions walk read-only data
//! and leave the heap stats and allocation counter unchanged.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{Store, Value, execute_export};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

#[test]
fn repeated_test_cast_and_br_on_cast_allocate_nothing() {
    let source = r#"
(module
  (type $a (sub (struct (field i32))))
  (type $b (sub $a (struct (field i32) (field i32))))
  (type $c (sub $b (struct (field i32) (field i32) (field i32))))
  (func (export "build") (result (ref $a))
    i32.const 1 i32.const 2 i32.const 3 struct.new $c)
  (func (export "query") (param (ref null $a)) (result i32)
    (local $t i32)
    local.get 0
    ref.test (ref $b)
    local.set $t
    local.get 0
    ref.cast (ref $a)
    drop
    (block $hit (result (ref null $b))
      local.get 0
      br_on_cast $hit (ref null $a) (ref null $b)
      drop
      ref.null $b)
    ref.is_null
    i32.eqz
    local.get $t
    i32.or))"#;
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    let object = execute_export(&module, &store, "build", &[]).unwrap()[0];

    // Warm up any lazy one-time state before snapshotting.
    let _ = execute_export(&module, &store, "query", &[object]).unwrap();

    let before_stats = store.gc_stats();
    let before_alloc = store.gc_snapshot();
    for _ in 0..1_000 {
        let _ = execute_export(&module, &store, "query", &[object]).unwrap();
    }
    assert_eq!(
        store.gc_stats(),
        before_stats,
        "type queries must not change heap stats"
    );
    assert_eq!(
        store.gc_allocations_since(before_alloc),
        0,
        "type queries must not allocate heap objects"
    );
}

#[test]
fn repeat_query_is_result_stable() {
    let source = r#"
(module
  (type $a (sub (struct (field i32))))
  (type $b (sub $a (struct (field i32) (field i32))))
  (func (export "build") (result (ref null $a))
    i32.const 1 i32.const 2 struct.new $b)
  (func (export "f") (param (ref null $a)) (result i32)
    local.get 0
    ref.test (ref $b)))"#;
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    let object = execute_export(&module, &store, "build", &[]).unwrap()[0];
    let hit = execute_export(&module, &store, "f", &[object]).unwrap();
    assert_eq!(hit, vec![Value::I32(1)]);
    for _ in 0..100 {
        let again = execute_export(&module, &store, "f", &[object]).unwrap();
        assert_eq!(again, hit);
    }
}
