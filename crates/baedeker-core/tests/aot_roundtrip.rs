// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! AOT roundtrip soak: serialize every lowerable fixture module and verify
//! the deserialized module is identical and executes identically.

#![cfg(feature = "aot")]

use baedeker_core::aot::{deserialize, serialize};
use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::{Store, Value, execute_export};
use baedeker_core::types::{HeapType, RefType, ValType};

fn lower_fixture(bytes: &[u8]) -> Option<baedeker_core::lower::RegModule> {
    let module = Module::decode(bytes).ok()?;
    module.validate().ok()?;
    lower_module(&module).ok()
}

#[test]
fn roundtrip_all_lowerable_fixtures() {
    let mut total = 0usize;
    let mut lowered = 0usize;
    for subdir in ["valid", "invalid-validate"] {
        for path in baedeker_testdata::spec_cases(subdir) {
            let bytes = baedeker_testdata::spec_case_bytes(&path);
            total += 1;
            let Some(module) = lower_fixture(&bytes) else {
                continue;
            };
            lowered += 1;
            let artifact = serialize(&module);
            let restored = deserialize(&artifact).unwrap_or_else(|e| {
                panic!("{}: deserialize failed: {e}", path.display());
            });
            assert_eq!(
                restored,
                module,
                "{}: roundtrip changed the IR",
                path.display()
            );
        }
    }
    assert!(total > 200, "fixture inventory moved? (found {total})");
    // A meaningful fraction of the corpus must actually roundtrip.
    assert!(lowered > 150, "only {lowered}/{total} fixtures lowered");
    println!("roundtripped {lowered}/{total} fixtures");
}

#[test]
fn roundtrip_all_gc_heap_types_in_global_types() {
    // Every new GC heap type in a serialized ref position: a global's value
    // type is carried in the artifact. RegModule value types must roundtrip
    // each new `HeapType` variant, not just the legacy func/extern/exn ones.
    let wat = r#"
(module
  (global (ref null any) (ref.null any))
  (global (ref null none) (ref.null none))
  (global (ref null eq) (ref.null eq))
  (global (ref null i31) (ref.null i31))
  (global (ref null struct) (ref.null struct))
  (global (ref null array) (ref.null array))
  (global (ref null nofunc) (ref.null nofunc))
  (global (ref null noextern) (ref.null noextern))
  (global (ref null noexn) (ref.null noexn))
)"#;
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = parsed.encode().unwrap();
    let module = lower_fixture(&bytes).expect("globals with GC heap types lower");

    let expected = [
        HeapType::Any,
        HeapType::None,
        HeapType::Eq,
        HeapType::I31,
        HeapType::Struct,
        HeapType::Array,
        HeapType::NoFunc,
        HeapType::NoExtern,
        HeapType::NoExn,
    ];
    assert_eq!(module.globals.len(), expected.len());
    for (global, heap) in module.globals.iter().zip(expected) {
        assert_eq!(global.ty, ValType::Ref(RefType::from_parts(true, heap)));
    }

    let restored = deserialize(&serialize(&module)).expect("artifact roundtrips");
    assert_eq!(restored, module, "roundtrip changed GC heap types");
    for (global, heap) in restored.globals.iter().zip(expected) {
        assert_eq!(global.ty, ValType::Ref(RefType::from_parts(true, heap)));
    }
}

#[test]
fn execution_is_identical_after_roundtrip() {
    let wat = r#"
(module
  (memory (export "memory") 1)
  (global $g (mut i32) (i32.const 100))
  (func (export "accumulate") (param i32) (result i32)
    (local $i i32) (local $acc i32)
    (loop $l
      (local.set $acc (i32.add (local.get $acc) (local.get $i)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_s (local.get $i) (local.get 0))))
    (i32.store (i32.const 0) (local.get $acc))
    (i32.add (local.get $acc) (global.get $g)))
)"#;
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = parsed.encode().unwrap();
    let module = lower_fixture(&bytes).expect("fixture lowers");

    let run = |m: &baedeker_core::lower::RegModule| -> (Vec<Value>, i32) {
        let store = Store::instantiate(m).unwrap();
        let results = execute_export(m, &store, "accumulate", &[Value::I32(10)]).unwrap();
        let cell = store.with_memory(0, |mem| i32::from_le_bytes(mem[0..4].try_into().unwrap()));
        (results, cell.unwrap())
    };

    let before = run(&module);
    let restored = deserialize(&serialize(&module)).unwrap();
    let after = run(&restored);
    assert_eq!(before, after);
    assert_eq!(before.0, [Value::I32(145)]); // sum(0..10) + global 100
    assert_eq!(before.1, 45);
}

#[test]
fn cast_conversion_opcodes_roundtrip_and_execute() {
    // GC U3: the new cast/conversion RegOps must serialize, deserialize,
    // and execute identically (AOT v10).
    let wat = r#"
(module
  (type $a (sub (struct (field i32))))
  (type $b (sub $a (struct (field i32) (field i32))))
  (func (export "build") (result (ref $a))
    i32.const 1 i32.const 2 struct.new $b)
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
    i32.or)
  (func (export "convert") (param externref) (result externref)
    local.get 0
    any.convert_extern
    extern.convert_any)
  (func (export "eq") (result i32)
    i32.const 7 ref.i31
    i32.const 7 ref.i31
    ref.eq))"#;
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = parsed.encode().unwrap();
    let module = lower_fixture(&bytes).expect("cast fixture lowers");

    let restored = deserialize(&serialize(&module)).unwrap();
    assert_eq!(restored, module, "roundtrip changed the cast IR");

    let run = |m: &baedeker_core::lower::RegModule| {
        let store = Store::instantiate(m).unwrap();
        let object = execute_export(m, &store, "build", &[]).unwrap();
        let query = execute_export(m, &store, "query", &object).unwrap();
        let converted = execute_export(m, &store, "convert", &[Value::ExternRef(Some(9))]).unwrap();
        let eq = execute_export(m, &store, "eq", &[]).unwrap();
        (query, converted, eq)
    };
    assert_eq!(run(&module), run(&restored));
}
