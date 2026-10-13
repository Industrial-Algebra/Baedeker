// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Tail-call execution: spec-mandated stack safety.
//!
//! WASM guarantees `return_call*` does not grow the call stack. These tests
//! drive tail recursion far beyond `MAX_CALL_DEPTH` (512) and assert normal
//! completion. Where a call below mirrors an existing test's API usage
//! (wat encoding, lowering, instantiation, host registration), copy the
//! exact form used in `crates/baedeker-core/tests/differential.rs` /
//! `runtime_wast.rs`.

use std::cell::Cell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::runtime::{HostFunction, RuntimeError, Store, Value, execute_export};
use baedeker_core::types::{FuncType, NumType, ValType};

const DEPTH: i32 = 1_000_000;

fn run_export(wat: &str, export: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    let buffer = wast::parser::ParseBuffer::new(wat).expect("wat parses");
    let mut parsed = wast::parser::parse::<wast::Wat>(&buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    let reg = module.lower().expect("module lowers");
    let store = Store::instantiate(&reg).expect("module instantiates");
    execute_export(&reg, &store, export, args)
}

#[test]
fn direct_tail_recursion_is_stack_safe() {
    let wat = r#"
        (module
          (func $run (export "run") (param $n i32) (result i32)
            (if (result i32) (i32.eqz (local.get $n))
              (then (i32.const 42))
              (else (return_call $run
                (i32.sub (local.get $n) (i32.const 1)))))))
    "#;
    let result = run_export(wat, "run", &[Value::I32(DEPTH)]).expect("no stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
}

#[test]
fn indirect_tail_recursion_is_stack_safe() {
    let wat = r#"
        (module
          (type $t (func (param i32) (result i32)))
          (table $tbl 1 funcref)
          (elem (i32.const 0) $run)
          (func $run (export "run") (param $n i32) (result i32)
            (if (result i32) (i32.eqz (local.get $n))
              (then (i32.const 42))
              (else (return_call_indirect $tbl (type $t)
                (i32.sub (local.get $n) (i32.const 1)) (i32.const 0))))))
    "#;
    let result = run_export(wat, "run", &[Value::I32(DEPTH)]).expect("no stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
}

#[test]
fn ref_tail_recursion_is_stack_safe() {
    let wat = r#"
        (module
          (type $t (func (param i32) (result i32)))
          (elem declare func $run)
          (func $run (export "run") (param $n i32) (result i32)
            (if (result i32) (i32.eqz (local.get $n))
              (then (i32.const 42))
              (else (return_call_ref $t
                (i32.sub (local.get $n) (i32.const 1)) (ref.func $run))))))
    "#;
    let result = run_export(wat, "run", &[Value::I32(DEPTH)]).expect("no stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
}

#[test]
fn tail_call_to_host_returns_host_results() {
    let wat = r#"
        (module
          (func $inc (import "env" "inc") (param i32) (result i32))
          (func (export "run") (param i32) (result i32)
            (return_call $inc (local.get 0))))
    "#;
    let buffer = wast::parser::ParseBuffer::new(wat).expect("wat parses");
    let mut parsed = wast::parser::parse::<wast::Wat>(&buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    let reg = module.lower().expect("module lowers");
    let mut store = Store::instantiate(&reg).expect("module instantiates");

    let observed: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let recorded = Rc::clone(&observed);
    let host = HostFunction::new(
        FuncType {
            params: vec![ValType::Num(NumType::I32)],
            results: vec![ValType::Num(NumType::I32)],
        },
        move |args| {
            let Value::I32(value) = args[0] else {
                unreachable!("host function receives an i32 argument");
            };
            recorded.set(Some(value));
            Ok(vec![Value::I32(value + 1)])
        },
    );
    store
        .register_host_func("env", "inc", host)
        .expect("host function registers");

    let result = execute_export(&reg, &store, "run", &[Value::I32(7)]).expect("executes");
    assert_eq!(result, vec![Value::I32(8)]);
    assert_eq!(observed.get(), Some(7));
}
