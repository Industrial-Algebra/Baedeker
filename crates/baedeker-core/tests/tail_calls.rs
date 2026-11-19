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
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    HostFunction, RuntimeError, RuntimeErrorKind, RuntimeTrap, Store, Value, execute_export,
};
use baedeker_core::types::{FuncType, NumType, ValType};

const DEPTH: i32 = 1_000_000;

fn lower_wat(wat: &str) -> std::rc::Rc<RegModule> {
    let buffer = wast::parser::ParseBuffer::new(wat).expect("wat parses");
    let mut parsed = wast::parser::parse::<wast::Wat>(&buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    std::rc::Rc::new(module.lower().expect("module lowers"))
}

fn run_export(wat: &str, export: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    let reg = lower_wat(wat);
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

#[test]
fn cross_instance_tail_call_type_check_uses_callers_declared_type() {
    // Review round 1, P1: the cross-instance branch of `execute_funcref`
    // resolved the declared type index against the REMOTE module's type
    // table (the `module` binding was shadowed before the lookup), so
    // divergent type numbering could wrongly allow a mismatched callee or
    // wrongly reject a valid one. Pin both directions through a shared
    // table linking two instances.
    use std::cell::RefCell;
    use std::rc::Rc;

    use baedeker_core::runtime::{Imports, LinkGroup, Table};
    use baedeker_core::types::{Limits, RefType, TableType};

    // Callee instance: type[0] = (result i64), type[1] = (result i32) —
    // numbering deliberately divergent from the caller's.
    let callee = lower_wat(
        r#"(module
             (type $t0 (func (result i64)))
             (type $t1 (func (result i32)))
             (func $h (type $t0) (i64.const 42))
             (func $g (type $t1) (i32.const 42)))"#,
    );
    // Caller instance: type[0] = (result i32).
    let caller = lower_wat(
        r#"(module
             (type $a0 (func (result i32)))
             (import "shared" "t" (table $t 2 funcref))
             (func (export "run") (param $slot i32) (result i32)
               (return_call_indirect $t (type $a0) (local.get $slot))))"#,
    );

    let group: Rc<RefCell<LinkGroup>> = Rc::new(RefCell::new(LinkGroup::new()));
    let table_ty = TableType {
        elem: RefType::FuncRef,
        limits: Limits {
            min: 2,
            max: None,
            is64: false,
            shared: false,
        },
        init: None,
    };
    let shared = Rc::new(RefCell::new(
        Table::new(table_ty.clone(), Value::FuncRef(None)).expect("table allocates"),
    ));
    let arena = Rc::new(RefCell::new(Vec::new()));

    let _callee_store = Store::instantiate_linked(
        &callee,
        &Imports::new().shared_table("shared", "t", table_ty.clone(), shared.clone()),
        &group,
        1,
        &arena,
    )
    .expect("callee instantiates");
    let caller_store = Store::instantiate_linked(
        &caller,
        &Imports::new().shared_table("shared", "t", table_ty, shared.clone()),
        &group,
        0,
        &arena,
    )
    .expect("caller instantiates");

    // Slot 0 -> callee $h (result i64): mismatched against the caller's
    // declared (result i32); the old bug allowed it (returned I64(42)).
    assert!(shared.borrow_mut().set(0, Value::FuncRef(Some((1, 0)))));
    // Slot 1 -> callee $g (result i32): valid; the old bug rejected it.
    assert!(shared.borrow_mut().set(1, Value::FuncRef(Some((1, 1)))));

    let store = caller_store.borrow();
    let err = execute_export(&caller, &store, "run", &[Value::I32(0)])
        .expect_err("mismatched cross-instance callee must trap");
    assert!(
        matches!(
            err.kind,
            RuntimeErrorKind::Trap(RuntimeTrap::IndirectCallTypeMismatch)
        ),
        "got {err:?}"
    );
    let ok = execute_export(&caller, &store, "run", &[Value::I32(1)])
        .expect("valid cross-instance callee must run");
    assert_eq!(ok, vec![Value::I32(42)]);
}

#[test]
fn fueled_infinite_tail_loop_exhausts_fuel_not_backstop() {
    // Review round 1, P2: with a fuel budget set, the coarse 10M-iteration
    // backstop used to fire first (MissingReturn with fuel remaining).
    // Whenever a budget exists, fuel exhaustion is the honest bound.
    let reg = lower_wat(
        r#"(module
             (func $a (return_call $b))
             (func $b (return_call $a))
             (func (export "run") (return_call $a)))"#,
    );
    let store = Store::instantiate(&reg).expect("module instantiates");
    store.set_fuel(Some(10_000_001));
    let err =
        execute_export(&reg, &store, "run", &[]).expect_err("infinite loop must not complete");
    assert!(
        matches!(err.kind, RuntimeErrorKind::FuelExhausted),
        "got {err:?}"
    );
    assert_eq!(store.fuel(), Some(0));
}

#[test]
fn unfueled_infinite_tail_loop_hits_coarse_backstop() {
    // Without a budget, the coarse backstop still bounds infinite tail
    // recursion (the trampoline itself never grows the stack).
    let reg = lower_wat(
        r#"(module
             (func $a (return_call $b))
             (func $b (return_call $a))
             (func (export "run") (return_call $a)))"#,
    );
    let store = Store::instantiate(&reg).expect("module instantiates");
    let err =
        execute_export(&reg, &store, "run", &[]).expect_err("infinite loop must not complete");
    assert!(
        matches!(err.kind, RuntimeErrorKind::MissingReturn),
        "got {err:?}"
    );
}
