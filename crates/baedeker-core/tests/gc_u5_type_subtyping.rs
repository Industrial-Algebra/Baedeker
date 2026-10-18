// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 5: runtime concrete-funcref type tests (`type-subtyping.wast`).
//!
//! `ref.test`/`ref.cast` on a function reference compare the function's
//! DECLARED defined type against the target heap type with `match_deftype`
//! (`interpreter/runtime/instance.ml:48` gives `type_of_ref (FuncRef f) =
//! UseHT (Def (Func.type_of f))`; `interpreter/valid/match.ml:92` routes
//! concrete-vs-concrete to `match_deftype`, `match.ml:151-155`:
//! `dt1 == dt2 || subst_deftype … || declared-supertype chain reaches dt2`).

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeError, RuntimeErrorKind, Store, Value, execute_export};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

fn run(source: &str, export: &str) -> Result<Vec<Value>, RuntimeError> {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    execute_export(&module, &store, export, &[])
}

fn run_i32(source: &str, export: &str) -> i32 {
    match run(source, export) {
        Ok(values) => match values.as_slice() {
            [Value::I32(value)] => *value,
            other => panic!("expected one i32, got {other:?}"),
        },
        Err(error) => panic!("execution failed: {error:?}"),
    }
}

/// `ref.test` with a concrete target matches the function's own type.
#[test]
fn ref_test_matches_own_declared_type() {
    let source = r#"
        (module
          (type $f (sub (func)))
          (func $g (type $f))
          (elem declare func $g)
          (func (export "run") (result i32) (ref.test (ref $f) (ref.func $g)))
        )
    "#;
    assert_eq!(run_i32(source, "run"), 1);
}

/// A declared subtype matches its supertype's concrete reference.
#[test]
fn ref_test_matches_declared_supertype() {
    let source = r#"
        (module
          (type $sup (sub (func)))
          (type $sub (sub $sup (func)))
          (func $g (type $sub))
          (elem declare func $g)
          (func (export "run") (result i32) (ref.test (ref $sup) (ref.func $g)))
        )
    "#;
    assert_eq!(run_i32(source, "run"), 1);
}

/// An unrelated function type does not match (the supertype chain never
/// reaches it).
#[test]
fn ref_test_rejects_unrelated_type() {
    let source = r#"
        (module
          (type $other (sub (func (param i32))))
          (type $sup (sub (func)))
          (type $sub (sub $sup (func)))
          (func $g (type $sub))
          (elem declare func $g)
          (func (export "run") (result i32) (ref.test (ref $other) (ref.func $g)))
        )
    "#;
    assert_eq!(run_i32(source, "run"), 0);
}

/// Structurally equivalent types in DIFFERENT rec groups are one canonical
/// type (`match_deftype` closed-form identity), so a `ref.test` across them
/// succeeds — the `type-subtyping.wast` "distinct declared type" family.
#[test]
fn ref_test_matches_structurally_equal_distinct_type_entries() {
    let source = r#"
        (module
          (rec (type $f1 (sub (func))) (type (struct (field (ref $f1)))))
          (rec (type $f2 (sub (func))) (type (struct (field (ref $f2)))))
          (rec (type $g1 (sub $f1 (func))) (type (struct)))
          (rec (type $g2 (sub $f2 (func))) (type (struct)))
          (func $g (type $g2))
          (elem declare func $g)
          (func (export "run") (result i32) (ref.test (ref $g1) (ref.func $g)))
        )
    "#;
    assert_eq!(run_i32(source, "run"), 1);
}

/// A distinct declared type whose REC GROUP SHAPE differs is not equivalent:
/// the corpus module where `$g2`'s struct field order/type differs from
/// `$g1`'s expects 0.
#[test]
fn ref_test_rejects_distinct_rec_group_shape() {
    let source = r#"
        (module
          (rec (type $f1 (sub (func))) (type (struct (field (ref $f1)))))
          (rec (type $f2 (sub (func))) (type (struct (field (ref $f1)))))
          (rec (type $g1 (sub $f1 (func))) (type (struct)))
          (rec (type $g2 (sub $f2 (func))) (type (struct)))
          (func $g (type $g2))
          (elem declare func $g)
          (func (export "run") (result i32) (ref.test (ref $g1) (ref.func $g)))
        )
    "#;
    assert_eq!(run_i32(source, "run"), 0);
}

/// `ref.cast` to a concrete function type succeeds for a matching function
/// and yields the same (non-null) reference.
#[test]
fn ref_cast_accepts_matching_function_type() {
    let source = r#"
        (module
          (type $t0 (sub (func (result (ref null func)))))
          (func $f0 (type $t0) (ref.null func))
          (table funcref (elem $f0))
          (func (export "run") (result i32)
            (ref.cast (ref $t0) (table.get (i32.const 0)))
            (ref.is_null))
        )
    "#;
    // The table holds the function reference `$f0` itself (not its result),
    // so a successful cast leaves a non-null funcref.
    assert_eq!(run_i32(source, "run"), 0);
}

/// A null function reference casts cleanly to a nullable concrete type.
#[test]
fn ref_cast_accepts_null_against_nullable_target() {
    let source = r#"
        (module
          (type $t0 (sub (func)))
          (func (export "run") (result i32)
            (ref.cast (ref null $t0) (ref.null func))
            (ref.is_null))
        )
    "#;
    assert_eq!(run_i32(source, "run"), 1);
}

/// A failed concrete `ref.cast` is `CastFailure` (trap-shaped), not a
/// non-trap error.
#[test]
fn ref_cast_failure_is_cast_failure_kind() {
    let source = r#"
        (module
          (type $t0 (sub (func (result (ref null func)))))
          (type $t1 (sub $t0 (func (result (ref null $t1)))))
          (func $f0 (type $t0) (ref.null func))
          (table funcref (elem $f0))
          (func (export "run")
            (ref.cast (ref $t1) (table.get (i32.const 0)))
            (drop))
        )
    "#;
    match run(source, "run") {
        Err(error) => assert!(
            matches!(error.kind, RuntimeErrorKind::CastFailure { .. }),
            "expected CastFailure, got {error:?}"
        ),
        Ok(values) => panic!("expected a cast failure, got {values:?}"),
    }
}

// ---------------------------------------------------------------------------
// Imported function references: the type comes from the DEFINING function
// ---------------------------------------------------------------------------
//
// `runtime/instance.ml:48` derives a funcref's type from the ACTUAL function
// instance: `| FuncRef f -> UseHT (Def (Func.type_of f))`, and
// `exec/eval.ml:632-634` resolves `ref.func x` to that function instance
// (`let f = func c.frame.inst x in Ref (FuncRef f)`). An import declaration is
// only a LINK constraint, so `ref.test` on an imported funcref must see the
// PROVIDER's defining type (a subtype of the declaration), never the
// declaration itself.

mod linking {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use baedeker_core::runtime::{ExceptionRecord, Imports, LinkGroup, link_func};

    /// One link group plus the arena its members share.
    struct Group {
        group: Rc<RefCell<LinkGroup>>,
        arena: Rc<RefCell<Vec<ExceptionRecord>>>,
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

        fn add(&mut self, module: &Rc<RegModule>) -> Rc<RefCell<Store>> {
            let id = self.next_id;
            self.next_id += 1;
            Store::instantiate_linked(module, &Imports::new(), &self.group, id, &self.arena)
                .expect("module instantiates")
        }

        /// Bind every function import of `consumer` from `provider`, using the
        /// runner's per-declaration `link_func` + `bind_host_func` wiring.
        fn link(
            &mut self,
            consumer: &Rc<RegModule>,
            consumer_store: &Rc<RefCell<Store>>,
            provider: &Rc<RegModule>,
            provider_store: &Rc<RefCell<Store>>,
            module_name: &str,
        ) {
            for (idx, declared) in consumer.imported_funcs.iter().enumerate() {
                if declared.module != module_name {
                    continue;
                }
                let func_idx = provider_store
                    .borrow()
                    .export_func(&declared.name)
                    .unwrap_or_else(|| {
                        panic!("provider has no function export {:?}", declared.name)
                    });
                let host = link_func(
                    provider.clone(),
                    provider_store.clone(),
                    func_idx,
                    declared.ty.clone(),
                );
                consumer_store
                    .borrow_mut()
                    .bind_host_func(idx as u32, host)
                    .expect("import binds");
            }
        }
    }

    fn export_i32(module: &RegModule, store: &Store, name: &str) -> i32 {
        match execute_export(module, store, name, &[])
            .expect("export runs")
            .as_slice()
        {
            [Value::I32(value)] => *value,
            other => panic!("expected one i32 from {name}, got {other:?}"),
        }
    }

    const PROVIDER: &str = r#"
        (module
          (type $sup (sub (func (result i32))))
          (type $sub (sub $sup (func (result i32))))
          (func (export "g") (type $sub) (i32.const 7)))
    "#;

    /// The consumer imports `g` declared as `$sup`; its defining type is `$sub`.
    const CONSUMER: &str = r#"
        (module
          (type $sup (sub (func (result i32))))
          (type $sub (sub $sup (func (result i32))))
          (import "a" "g" (func $g (type $sup)))
          (elem declare func $g)
          (func (export "to_sub") (result i32) (ref.test (ref $sub) (ref.func $g)))
          (func (export "to_sup") (result i32) (ref.test (ref $sup) (ref.func $g)))
          (func (export "call") (result i32) (call $g)))
    "#;

    /// `ref.test` on an imported funcref matches the PROVIDER's defining type
    /// (a declared subtype of the import declaration), not the declaration.
    #[test]
    fn imported_funcref_keeps_defining_type() {
        let provider = Rc::new(wat_module(PROVIDER));
        let consumer = Rc::new(wat_module(CONSUMER));
        let mut group = Group::new();
        let provider_store = group.add(&provider);
        let consumer_store = group.add(&consumer);
        group.link(&consumer, &consumer_store, &provider, &provider_store, "a");

        let store = consumer_store.borrow();
        assert_eq!(
            export_i32(&consumer, &store, "to_sub"),
            1,
            "the imported function's defining type ($sub) must match"
        );
        assert_eq!(
            export_i32(&consumer, &store, "to_sup"),
            1,
            "the requested supertype still matches"
        );
        assert_eq!(export_i32(&consumer, &store, "call"), 7);
    }

    /// The defining type survives an IMPORT CHAIN: C imports from B, whose
    /// export is itself an import of A's function.
    #[test]
    fn imported_funcref_keeps_defining_type_through_chain() {
        let provider = Rc::new(wat_module(PROVIDER));
        let middle = Rc::new(wat_module(
            r#"
            (module
              (type $sup (sub (func (result i32))))
              (import "a" "g" (func $g (type $sup)))
              (export "g" (func $g)))
            "#,
        ));
        let consumer = Rc::new(wat_module(CONSUMER));
        let mut group = Group::new();
        let provider_store = group.add(&provider);
        let middle_store = group.add(&middle);
        group.link(&middle, &middle_store, &provider, &provider_store, "a");
        let consumer_store = group.add(&consumer);
        group.link(&consumer, &consumer_store, &middle, &middle_store, "a");

        let store = consumer_store.borrow();
        assert_eq!(
            export_i32(&consumer, &store, "to_sub"),
            1,
            "the provider's defining type must survive the import chain"
        );
        assert_eq!(export_i32(&consumer, &store, "call"), 7);
    }

    /// A STANDALONE store's `link_func` binding carries the provider's
    /// defining type too (no link group involved).
    #[test]
    fn standalone_link_func_binding_keeps_defining_type() {
        let provider = Rc::new(wat_module(PROVIDER));
        let consumer = Rc::new(wat_module(CONSUMER));
        let provider_store = Rc::new(RefCell::new(
            Store::instantiate(&provider).expect("provider instantiates"),
        ));
        let mut consumer_store =
            Store::instantiate_with_imports(&consumer, &Imports::new()).expect("consumer");

        for (idx, declared) in consumer.imported_funcs.iter().enumerate() {
            let func_idx = provider_store
                .borrow()
                .export_func(&declared.name)
                .expect("provider export");
            let host = link_func(
                provider.clone(),
                provider_store.clone(),
                func_idx,
                declared.ty.clone(),
            );
            consumer_store
                .bind_host_func(idx as u32, host)
                .expect("import binds");
        }

        assert_eq!(
            export_i32(&consumer, &consumer_store, "to_sub"),
            1,
            "a standalone link_func binding must carry the defining type"
        );
        assert_eq!(export_i32(&consumer, &consumer_store, "call"), 7);
    }
}

/// A NATIVE host function has no defining module, so its funcref type is its
/// own function type: the pin allocates host functions with the type entry
/// `DefT (RecT [SubT (Final, [], FuncT (ts1, ts2))], 0l)`
/// (`interpreter/host/spectest.ml:35`) — FINAL and supertype-free.
///
/// `register_host_func` matches signatures at the `FuncType` level (more
/// permissive than the spec's `match_deftype` link check, which would also
/// compare finality), so a non-final declaration can be bound here. The
/// funcref's type still comes from the HOST function, not the declaration:
/// the non-final `$open` does not match, while the final `$final` — the shape
/// the pin allocates — does.
#[test]
fn host_function_funcref_is_typed_by_its_own_signature() {
    use baedeker_core::runtime::{HostFunction, Imports};
    use baedeker_core::types::{FuncType, NumType, ValType};

    let source = r#"
        (module
          (type $final (sub final (func (result i32))))
          (type $open (sub (func (result i32))))
          (import "env" "f" (func $f (type $open)))
          (elem declare func $f)
          (func (export "to_open") (result i32) (ref.test (ref $open) (ref.func $f)))
          (func (export "to_final") (result i32) (ref.test (ref $final) (ref.func $f)))
          (func (export "call") (result i32) (call $f)))
    "#;
    let module = wat_module(source);
    let mut store = Store::instantiate_with_imports(&module, &Imports::new()).expect("instantiate");
    store
        .register_host_func(
            "env",
            "f",
            HostFunction::new(
                FuncType {
                    params: vec![],
                    results: vec![ValType::Num(NumType::I32)],
                },
                |_args| Ok(vec![Value::I32(9)]),
            ),
        )
        .expect("host function registers");

    let run = |export: &str| match execute_export(&module, &store, export, &[]) {
        Ok(values) => match values.as_slice() {
            [Value::I32(value)] => *value,
            other => panic!("expected one i32 from {export}, got {other:?}"),
        },
        Err(error) => panic!("{export} failed: {error:?}"),
    };
    assert_eq!(
        run("to_open"),
        0,
        "the host's own type entry is FINAL, so a non-final declaration does not type it"
    );
    assert_eq!(
        run("to_final"),
        1,
        "the pin's host allocation (one final supertype-free member) matches $final"
    );
    assert_eq!(run("call"), 9);
}
