// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! EH unit 3: `try_table` / `throw` / `throw_ref` execution.
//!
//! Each fixture goes through the full pipeline (WAT → encode → decode →
//! lower) and then executes the exported `"f"` against a fresh `Store`.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    RuntimeError, RuntimeErrorKind, RuntimeTrap, Store, Value, execute_export, execute_func,
};

/// Parse WAT, encode it to a binary, decode, and lower it.
fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

/// Instantiate `module` and invoke export `name` with `args`.
fn run_module(module: &RegModule, name: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    let store = Store::instantiate(module).unwrap();
    execute_export(module, &store, name, args)
}

/// Lower `source` and invoke its exported `"f"`.
fn run(source: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    run_module(&wat_module(source), "f", args)
}

#[test]
fn throw_caught_with_payload() {
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch 0 $h)
        i32.const 42
        throw 0)
      i32.const 7)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(42)]));
}

#[test]
fn catch_all_catches() {
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $h
      (try_table (catch_all $h)
        i32.const 5
        throw 0))
    i32.const 7))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(7)]));
}

#[test]
fn uncaught_exception_errors() {
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    i32.const 1
    throw 0))
"#;
    let error = run(source, &[]).unwrap_err();
    assert!(
        matches!(
            error.kind,
            RuntimeErrorKind::UncaughtException { exn: Some(_) }
        ),
        "expected UncaughtException with an arena index, got {error:?}"
    );
}

#[test]
fn nested_innermost_region_wins() {
    // Both regions have a catch_all; the inner one must win (result 1), the
    // outer one would yield 2.
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $final (result i32)
      (block $outer_land
        (try_table (catch_all $outer_land)
          (block $inner_land
            (try_table (catch_all $inner_land)
              i32.const 1
              throw 0))
          i32.const 1
          br $final))
      i32.const 2
      br $final)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(1)]));
}

#[test]
fn first_matching_clause_wins() {
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $final (result i32)
      (block $second (result i32)
        (block $first (result i32)
          (try_table (catch 0 $first) (catch 0 $second)
            i32.const 5
            throw 0)
          i32.const 0)
        br $final)
      drop
      i32.const 22
      br $final)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(5)]));
}

#[test]
fn cross_function_propagation() {
    let source = r#"
(module
  (tag (param i32))
  (func $callee (param i32)
    local.get 0
    throw 0)
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch 0 $h)
        i32.const 42
        call $callee)
      i32.const 7)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(42)]));
}

#[test]
fn catch_ref_and_rethrow() {
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $h
      (try_table (catch_all $h)
        (block $l (result i32 (ref exn))
          (try_table (result i32 (ref exn)) (catch_ref 0 $l)
            i32.const 99
            throw 0))
        throw_ref))
    i32.const 42))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(42)]));
}

#[test]
fn throw_ref_null_traps() {
    let source = r#"
(module
  (func (export "f") (result i32)
    (local exnref)
    local.get 0
    throw_ref))
"#;
    let error = run(source, &[]).unwrap_err();
    assert_eq!(
        error.kind,
        RuntimeErrorKind::Trap(RuntimeTrap::NullReference)
    );
}

#[test]
fn tag_mismatch_propagates() {
    // The inner region only catches tag 0; the thrown tag 1 must propagate
    // to the outer catch_all (result 2, not 1).
    let source = r#"
(module
  (tag (param i32))
  (tag (param i32))
  (func (export "f") (result i32)
    (block $final (result i32)
      (block $outer_land
        (try_table (catch_all $outer_land)
          (block $inner_land (result i32)
            (try_table (catch 0 $inner_land)
              i32.const 9
              throw 1)
            i32.const 0)
          drop
          i32.const 1
          br $final))
      i32.const 2
      br $final)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(2)]));
}

#[test]
fn multi_value_payload_order() {
    let source = r#"
(module
  (tag (param i32 f64))
  (func (export "f") (result i32 f64)
    (block $h (result i32 f64)
      (try_table (catch 0 $h)
        i32.const 5
        f64.const 6.5
        throw 0)
      i32.const 0
      f64.const 0)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(5), Value::F64(6.5)]));
}

#[test]
fn try_body_completes_normally() {
    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch 0 $h))
      i32.const 7)))
"#;
    assert_eq!(run(source, &[]), Ok(vec![Value::I32(7)]));
}

#[cfg(feature = "aot")]
#[test]
fn aot_roundtrip_preserves_eh() {
    use baedeker_core::aot::{deserialize, serialize};

    let source = r#"
(module
  (tag (param i32))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch 0 $h)
        i32.const 1
        throw 0)
      i32.const 7)))
"#;
    let module = wat_module(source);
    let before = run_module(&module, "f", &[]);
    let restored = deserialize(&serialize(&module)).unwrap();
    let after = run_module(&restored, "f", &[]);
    assert_eq!(before, Ok(vec![Value::I32(1)]));
    assert_eq!(before, after);
}

#[test]
fn storeless_throw_is_uncaught() {
    let source = r#"
(module
  (tag (param i32))
  (func (result i32)
    i32.const 1
    throw 0))
"#;
    let module = wat_module(source);
    let error = execute_func(&module.funcs[0], &[]).unwrap_err();
    assert_eq!(
        error.kind,
        RuntimeErrorKind::UncaughtException { exn: None }
    );
}
