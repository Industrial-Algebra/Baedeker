// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3: the extern/any conversions never allocate.
//!
//! Conversions are immediate value-level re-tags (`exec/eval.ml:929-939`), so
//! a conversion storm leaves the heap stats and allocation counter untouched.

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
fn extern_any_conversions_allocate_zero_bytes() {
    let source = r#"(module
  (func (export "f") (param externref) (result externref)
    local.get 0
    any.convert_extern
    extern.convert_any))"#;
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();

    let before_stats = store.gc_stats();
    let before_alloc = store.gc_snapshot();

    for i in 0..1_000u32 {
        let result = execute_export(&module, &store, "f", &[Value::ExternRef(Some(i))]).unwrap();
        assert_eq!(result, vec![Value::ExternRef(Some(i))]);
    }

    assert_eq!(
        store.gc_stats(),
        before_stats,
        "conversions must not change logical heap stats"
    );
    assert_eq!(
        store.gc_allocations_since(before_alloc),
        0,
        "conversions must not allocate heap objects"
    );
}

#[test]
fn null_conversions_allocate_zero_bytes() {
    let source = r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    any.convert_extern
    extern.convert_any
    ref.is_null))"#;
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    let before_stats = store.gc_stats();
    let before_alloc = store.gc_snapshot();

    for _ in 0..1_000 {
        assert_eq!(
            execute_export(&module, &store, "f", &[Value::ExternRef(None)]).unwrap(),
            vec![Value::I32(1)]
        );
    }

    assert_eq!(store.gc_stats(), before_stats);
    assert_eq!(store.gc_allocations_since(before_alloc), 0);
}
