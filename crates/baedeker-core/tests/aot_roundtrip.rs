// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! AOT roundtrip soak: serialize every lowerable fixture module and verify
//! the deserialized module is identical and executes identically.

#![cfg(feature = "aot")]

use baedeker_core::aot::{deserialize, serialize};
use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::{Store, Value, execute_export};

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
