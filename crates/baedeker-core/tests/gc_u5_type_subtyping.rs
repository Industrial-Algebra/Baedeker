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
