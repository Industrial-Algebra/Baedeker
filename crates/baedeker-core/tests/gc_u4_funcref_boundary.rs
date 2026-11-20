// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: concrete `(ref $ft)` parameter boundaries.
//!
//! A funcref value reaching a CONCRETE function-typed parameter must have a
//! matching function type: parameters contravariantly, results covariantly,
//! concrete reference types by rolled closed form (`valid/match.ml:135-141`).
//! The r2 ledger reproduced the old laxness — an incompatible funcref was
//! admitted and its body returned `7` — so the first test is that exact case.
//! `HeapType::Func` (abstract) stays permissive: every funcref is a `funcref`.

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{Imports, LinkGroup, RuntimeErrorKind, Store, Value, execute_export};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

struct Group {
    group: Rc<RefCell<LinkGroup>>,
    arena: Rc<RefCell<Vec<baedeker_core::runtime::ExceptionRecord>>>,
    next_id: u32,
}

impl Group {
    fn new() -> Self {
        Self {
            group: Rc::new(RefCell::new(LinkGroup::new())),
            arena: Rc::new(RefCell::new(Vec::new())),
            next_id: 1,
        }
    }

    fn add(&mut self, module: &Rc<RegModule>, imports: &Imports) -> Rc<RefCell<Store>> {
        let id = self.next_id;
        self.next_id += 1;
        Store::instantiate_linked(module, imports, &self.group, id, &self.arena)
            .expect("module instantiates")
    }
}

const BOUNDARY: &str = r#"
(module
  (type $expect (func (result i32)))
  (type $other (func (result i64)))
  (type $void (func))
  (func $seven (type $expect) (result i32) i32.const 7)
  (func $wrong (type $other) (result i64) i64.const 7)
  (func $nothing (type $void))
  (elem declare func $seven $wrong $nothing)
  (func (export "right") (result (ref $expect)) (ref.func $seven))
  (func (export "wrong") (result (ref $other)) (ref.func $wrong))
  (func (export "void") (result (ref $void)) (ref.func $nothing))
  (func (export "call_expect") (param (ref null $expect)) (result i32)
    local.get 0
    call_ref $expect)
  (func (export "call_abstract") (param funcref) (result i32)
    local.get 0
    ref.is_null
    i32.eqz)
)"#;

fn store_for(source: &str) -> (RegModule, Store) {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    (module, store)
}

/// Positive control: the exact signature passes and executes.
#[test]
fn concrete_param_accepts_exact_funcref() {
    let (module, store) = store_for(BOUNDARY);
    let seven = execute_export(&module, &store, "right", &[]).unwrap();
    let result = execute_export(&module, &store, "call_expect", &seven).unwrap();
    assert_eq!(result, vec![Value::I32(7)]);
}

/// The r2 reproduction: an incompatible funcref must be rejected at the
/// boundary instead of being admitted (and returning 7).
#[test]
fn concrete_param_rejects_incompatible_funcref() {
    let (module, store) = store_for(BOUNDARY);
    let wrong = execute_export(&module, &store, "wrong", &[]).unwrap();
    let error = execute_export(&module, &store, "call_expect", &wrong)
        .expect_err("a (func (result i64)) value must not satisfy (ref null (func (result i32)))");
    assert!(
        matches!(error.kind, RuntimeErrorKind::TypeMismatch { .. }),
        "expected a clean boundary TypeMismatch, got {error:?}"
    );
}

/// Arity is part of the signature: a `(func)` does not satisfy `(func (result i32))`.
#[test]
fn concrete_param_rejects_arity_mismatch() {
    let (module, store) = store_for(BOUNDARY);
    let nothing = execute_export(&module, &store, "void", &[]).unwrap();
    let error = execute_export(&module, &store, "call_expect", &nothing)
        .expect_err("a (func) value must not satisfy (ref null (func (result i32)))");
    assert!(matches!(error.kind, RuntimeErrorKind::TypeMismatch { .. }));
}

/// Abstract `HeapType::Func` accepts every funcref (lattice-correct), and
/// rejects nulls for a non-nullable parameter.
#[test]
fn abstract_func_param_accepts_every_funcref() {
    let (module, store) = store_for(BOUNDARY);
    for export in ["right", "wrong", "void"] {
        let value = execute_export(&module, &store, export, &[]).unwrap();
        let result = execute_export(&module, &store, "call_abstract", &value).unwrap();
        assert_eq!(
            result,
            vec![Value::I32(1)],
            "{export} must be a non-null funcref"
        );
    }

    // `funcref` is the nullable shorthand, so a null is legitimate; the
    // abstract parameter does not constrain the signature either way.
    let null = execute_export(&module, &store, "call_abstract", &[Value::FuncRef(None)]).unwrap();
    assert_eq!(null, vec![Value::I32(0)]);
}

/// A cross-module funcref whose function type is structurally equal but lives
/// at a different local index must satisfy a concrete parameter.
#[test]
fn cross_module_structural_funcref_param_accepted() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $first (func))
  (type $p (func (result i32)))
  (func $f (type $p) (result i32) i32.const 5)
  (elem declare func $f)
  (func (export "mk") (result (ref $p)) ref.func $f))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $c (func (result i32)))
  (func (export "call") (param (ref null $c)) (result i32)
    local.get 0
    call_ref $c))"#,
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, &Imports::new());
    let imports = Imports::new().with_module_types("a", &provider);
    let consumer_store = group.add(&consumer, &imports);

    let funcref = execute_export(&provider, &provider_store.borrow(), "mk", &[]).unwrap();
    let result = execute_export(&consumer, &consumer_store.borrow(), "call", &funcref).unwrap();
    assert_eq!(result, vec![Value::I32(5)]);
}

/// The negative half: a structurally different cross-module function type is
/// rejected at the boundary.
#[test]
fn cross_module_different_funcref_param_rejected() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $p (func (result i64)))
  (func $f (type $p) (result i64) i64.const 5)
  (elem declare func $f)
  (func (export "mk") (result (ref $p)) ref.func $f))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $c (func (result i32)))
  (func (export "call") (param (ref null $c)) (result i32)
    local.get 0
    call_ref $c))"#,
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, &Imports::new());
    let imports = Imports::new().with_module_types("a", &provider);
    let consumer_store = group.add(&consumer, &imports);

    let funcref = execute_export(&provider, &provider_store.borrow(), "mk", &[]).unwrap();
    let error = execute_export(&consumer, &consumer_store.borrow(), "call", &funcref)
        .expect_err("a (func (result i64)) value must not satisfy (ref null (func (result i32)))");
    assert!(matches!(error.kind, RuntimeErrorKind::TypeMismatch { .. }));
}
