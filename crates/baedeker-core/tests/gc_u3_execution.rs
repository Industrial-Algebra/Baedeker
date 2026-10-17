// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3: cast/conversion execution semantics.
//!
//! Full pipeline (WAT → encode → decode → lower → execute) against the
//! pinned `exec/eval.ml` rows: ref.test/ref.cast (+null forms), br_on_cast /
//! br_on_cast_fail, ref.eq, and the extern/any conversions.

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    ExceptionRecord, ExternPayload, Imports, LinkGroup, RuntimeError, RuntimeErrorKind, Store,
    Value, execute_export,
};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

fn run(source: &str) -> Result<Vec<Value>, RuntimeError> {
    run_args(source, &[])
}

fn run_args(source: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    execute_export(&module, &store, "f", args)
}

fn run_i32(source: &str) -> i32 {
    match run(source) {
        Ok(values) => match values.as_slice() {
            [Value::I32(value)] => *value,
            other => panic!("expected one i32, got {other:?}"),
        },
        Err(error) => panic!("execution failed: {error:?}"),
    }
}

fn run_i32_args(source: &str, args: &[Value]) -> i32 {
    match run_args(source, args) {
        Ok(values) => match values.as_slice() {
            [Value::I32(value)] => *value,
            other => panic!("expected one i32, got {other:?}"),
        },
        Err(error) => panic!("execution failed: {error:?}"),
    }
}

const SUBTYPES: &str = r#"
  (type $a (sub (struct (field i32))))
  (type $b (sub $a (struct (field i32) (field i32))))
  (type $c (sub $b (struct (field i32) (field i32) (field i32))))
  (type $other (struct (field i32)))
"#;

// ── ref.test ─────────────────────────────────────────────────────────

#[test]
fn ref_test_concrete_hit_and_miss() {
    let hit = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 i32.const 2 i32.const 3 struct.new $c
    ref.test (ref $c)))"#
    );
    assert_eq!(run_i32(&hit), 1);

    let miss = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 i32.const 2 i32.const 3 struct.new $c
    ref.test (ref $other)))"#
    );
    assert_eq!(run_i32(&miss), 0);
}

#[test]
fn ref_test_declared_supertype_chain() {
    // $c -> $b -> $a, distinct concrete ids: every ancestor matches.
    for target in ["$a", "$b", "$c"] {
        let source = format!(
            r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 i32.const 2 i32.const 3 struct.new $c
    ref.test (ref {target})))"#
        );
        assert_eq!(run_i32(&source), 1, "target {target} must match");
    }
}

#[test]
fn ref_test_abstract_targets() {
    let source = |target: &str| {
        format!(
            r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 i32.const 2 i32.const 3 struct.new $c
    ref.test (ref {target})))"#
        )
    };
    assert_eq!(run_i32(&source("struct")), 1);
    assert_eq!(run_i32(&source("eq")), 1);
    assert_eq!(run_i32(&source("any")), 1);
    assert_eq!(run_i32(&source("array")), 0);
    assert_eq!(run_i32(&source("i31")), 0);
    assert_eq!(run_i32(&source("none")), 0);

    let i31 = |target: &str| {
        format!(
            r#"(module
  (func (export "f") (result i32)
    i32.const 7 ref.i31
    ref.test (ref {target})))"#
        )
    };
    assert_eq!(run_i32(&i31("i31")), 1);
    assert_eq!(run_i32(&i31("eq")), 1);
    assert_eq!(run_i32(&i31("any")), 1);
    assert_eq!(run_i32(&i31("struct")), 0);

    let array = |target: &str| {
        format!(
            r#"(module
  (type $arr (array i32))
  (func (export "f") (result i32)
    i32.const 9 i32.const 2 array.new $arr
    ref.test (ref {target})))"#
        )
    };
    assert_eq!(run_i32(&array("array")), 1);
    assert_eq!(run_i32(&array("eq")), 1);
    assert_eq!(run_i32(&array("struct")), 0);
}

#[test]
fn ref_test_null_forms() {
    let null_any = r#"(module
  (func (export "f") (result i32)
    ref.null any
    ref.test (ref null any)))"#;
    assert_eq!(run_i32(null_any), 1);

    let null_concrete = r#"(module
  (type $s (struct (field i32)))
  (func (export "f") (result i32)
    ref.null $s
    ref.test (ref null $s)))"#;
    assert_eq!(run_i32(null_concrete), 1);

    let null_nonnull = r#"(module
  (type $s (struct (field i32)))
  (func (export "f") (result i32)
    ref.null $s
    ref.test (ref $s)))"#;
    assert_eq!(run_i32(null_nonnull), 0);
}

#[test]
fn ref_test_funcref_value() {
    let func_target = r#"(module
  (func $g)
  (elem declare func $g)
  (func (export "f") (result i32)
    ref.func $g
    ref.test (ref func)))"#;
    assert_eq!(run_i32(func_target), 1);

    let nofunc_miss = r#"(module
  (func $g)
  (elem declare func $g)
  (func (export "f") (result i32)
    ref.func $g
    ref.test (ref nofunc)))"#;
    assert_eq!(run_i32(nofunc_miss), 0);
}

// ── ref.cast ─────────────────────────────────────────────────────────

#[test]
fn ref_test_and_cast_extern_target_on_externref() {
    let test = r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    ref.test (ref extern)))"#;
    assert_eq!(run_i32_args(test, &[Value::ExternRef(Some(1))]), 1);
    assert_eq!(run_i32_args(test, &[Value::ExternRef(None)]), 0);

    let cast = r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    ref.cast (ref null extern)
    ref.is_null))"#;
    assert_eq!(run_i32_args(cast, &[Value::ExternRef(Some(1))]), 0);
    assert_eq!(run_i32_args(cast, &[Value::ExternRef(None)]), 1);
}

#[test]
fn ref_cast_hit_passes_value_through() {
    let source = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 i32.const 2 i32.const 3 struct.new $c
    ref.cast (ref $a)
    struct.get $a 0))"#
    );
    assert_eq!(run_i32(&source), 1);
}

#[test]
fn ref_cast_miss_traps_with_message_prefix() {
    let source = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 i32.const 2 i32.const 3 struct.new $c
    ref.cast (ref $other)
    drop
    i32.const 0))"#
    );
    let error = run(&source).expect_err("cast must trap");
    match &error.kind {
        RuntimeErrorKind::CastFailure { .. } => {
            let message = format!("{}", error.kind);
            assert!(
                message.starts_with("cast failure, expected "),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected CastFailure, got {other:?}"),
    }
}

#[test]
fn ref_cast_null_forms() {
    let nonnull = r#"(module
  (type $s (struct (field i32)))
  (func (export "f") (result i32)
    ref.null $s
    ref.cast (ref $s)
    ref.is_null))"#;
    assert!(run(nonnull).is_err());

    let nullable = r#"(module
  (type $s (struct (field i32)))
  (func (export "f") (result i32)
    ref.null $s
    ref.cast (ref null $s)
    ref.is_null))"#;
    assert_eq!(run_i32(nullable), 1);
}

// ── br_on_cast / br_on_cast_fail ─────────────────────────────────────

/// A module with builders plus `f`, so the ref under test is created in the
/// same store/heap the tester executes in.
const BR_ON_CAST_MODULE: &str = r#"
  (type $a (sub (struct (field i32))))
  (type $b (sub $a (struct (field i32) (field i32))))
  (type $c (sub $b (struct (field i32) (field i32) (field i32))))
  (func (export "build_hit") (result (ref null $a))
    i32.const 1 i32.const 2 i32.const 3 struct.new $c)
  (func (export "build_plain") (result (ref null $a))
    i32.const 5 struct.new $a)
"#;

#[test]
fn br_on_cast_branches_on_hit_and_falls_through_on_miss() {
    let source = format!(
        r#"(module {BR_ON_CAST_MODULE}
  (func (export "f") (param (ref null $a)) (result i32)
    (block $hit (result (ref null $b))
      local.get 0
      br_on_cast $hit (ref null $a) (ref null $b)
      drop
      ref.null $b)
    ref.is_null
    i32.eqz))"#
    );
    let module = wat_module(&source);
    let store = Store::instantiate(&module).unwrap();
    let hit = execute_export(&module, &store, "build_hit", &[]).unwrap()[0];
    assert_eq!(
        execute_export(&module, &store, "f", &[hit]).unwrap(),
        vec![Value::I32(1)]
    );
    let plain = execute_export(&module, &store, "build_plain", &[]).unwrap()[0];
    assert_eq!(
        execute_export(&module, &store, "f", &[plain]).unwrap(),
        vec![Value::I32(0)]
    );
}

#[test]
fn br_on_cast_fail_branches_on_miss_and_falls_through_on_hit() {
    let source = format!(
        r#"(module {BR_ON_CAST_MODULE}
  (func (export "f") (param (ref null $a)) (result i32)
    (local $ok i32)
    (block $miss (result (ref null $a))
      local.get 0
      br_on_cast_fail $miss (ref null $a) (ref null $b)
      drop
      i32.const 1
      local.set $ok
      ref.null $a)
    drop
    local.get $ok))"#
    );
    let module = wat_module(&source);
    let store = Store::instantiate(&module).unwrap();
    let hit = execute_export(&module, &store, "build_hit", &[]).unwrap()[0];
    assert_eq!(
        execute_export(&module, &store, "f", &[hit]).unwrap(),
        vec![Value::I32(1)]
    );
    let plain = execute_export(&module, &store, "build_plain", &[]).unwrap()[0];
    assert_eq!(
        execute_export(&module, &store, "f", &[plain]).unwrap(),
        vec![Value::I32(0)]
    );
}

// ── ref.eq ───────────────────────────────────────────────────────────

#[test]
fn ref_eq_matrix() {
    let i31_eq = r#"(module
  (func (export "f") (result i32)
    i32.const 42 ref.i31
    i32.const 42 ref.i31
    ref.eq))"#;
    assert_eq!(run_i32(i31_eq), 1);
    let i31_ne = r#"(module
  (func (export "f") (result i32)
    i32.const 42 ref.i31
    i32.const 43 ref.i31
    ref.eq))"#;
    assert_eq!(run_i32(i31_ne), 0);

    let obj_eq = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    (local $r (ref null $a))
    i32.const 1 struct.new $a
    local.tee $r
    local.get $r
    ref.eq))"#
    );
    assert_eq!(run_i32(&obj_eq), 1);

    let obj_ne = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 struct.new $a
    i32.const 1 struct.new $a
    ref.eq))"#
    );
    assert_eq!(run_i32(&obj_ne), 0);

    let null_eq = r#"(module
  (func (export "f") (result i32)
    ref.null eq
    ref.null eq
    ref.eq))"#;
    assert_eq!(run_i32(null_eq), 1);

    let cross = format!(
        r#"(module {SUBTYPES}
  (func (export "f") (result i32)
    i32.const 1 struct.new $a
    i32.const 1 ref.i31
    ref.eq))"#
    );
    assert_eq!(run_i32(&cross), 0);
}

// ── conversions ──────────────────────────────────────────────────────

#[test]
fn convert_roundtrip_identity() {
    let source = r#"(module
  (func (export "f") (param externref) (result externref)
    local.get 0
    any.convert_extern
    extern.convert_any))"#;
    let result = run_args(source, &[Value::ExternRef(Some(7))]).unwrap();
    assert_eq!(result, vec![Value::ExternRef(Some(7))]);
}

#[test]
fn convert_null_roundtrip() {
    let source = r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    any.convert_extern
    ref.is_null))"#;
    assert_eq!(run_i32_args(source, &[Value::ExternRef(None)]), 1);
}

#[test]
fn converted_extern_matches_any_only() {
    let source = |target: &str| {
        format!(
            r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    any.convert_extern
    ref.test (ref {target})))"#
        )
    };
    let arg = [Value::ExternRef(Some(3))];
    assert_eq!(run_i32_args(&source("any"), &arg), 1);
    assert_eq!(run_i32_args(&source("eq"), &arg), 0);
    assert_eq!(run_i32_args(&source("struct"), &arg), 0);
}

#[test]
fn raw_externref_rejected_by_any_boundary() {
    // A raw externref is not an anyref: passing one where `anyref` is
    // declared is a clean boundary rejection.
    let source = r#"(module
  (func (export "f") (param anyref) (result i32)
    i32.const 1))"#;
    let error = run_args(source, &[Value::ExternRef(Some(3))]).expect_err("rejected");
    assert!(
        matches!(error.kind, RuntimeErrorKind::TypeMismatch { .. }),
        "expected TypeMismatch, got {:?}",
        error.kind
    );
}

#[test]
fn any_boundary_accepts_converted_extern_eq_boundary_rejects() {
    let any = r#"(module
  (func (export "f") (param anyref) (result i32)
    i32.const 1))"#;
    assert_eq!(run_i32_args(any, &[Value::HostAny(Some(5))]), 1);

    // A converted extern is an any but not an eq value.
    let eq = r#"(module
  (func (export "f") (param eqref) (result i32)
    i32.const 1))"#;
    assert!(run_args(eq, &[Value::HostAny(Some(5))]).is_err());
}

#[test]
fn cross_domain_boundary_rejections() {
    // An internalized host handle (any domain) is refused by an extern-typed
    // parameter.
    let extern_param = r#"(module
  (func (export "f") (param externref) (result i32)
    i32.const 1))"#;
    assert!(matches!(
        run_args(extern_param, &[Value::HostAny(Some(3))])
            .expect_err("rejected")
            .kind,
        RuntimeErrorKind::TypeMismatch { .. }
    ));

    // An externalized GC value (extern domain) is refused by an any-typed
    // parameter.
    let any_param = r#"(module
  (func (export "f") (param anyref) (result i32)
    i32.const 1))"#;
    assert!(matches!(
        run_args(any_param, &[Value::ExternGc(Some(ExternPayload::I31(3)))])
            .expect_err("rejected")
            .kind,
        RuntimeErrorKind::TypeMismatch { .. }
    ));

    // A raw GC immediate is refused by an extern-typed parameter.
    assert!(matches!(
        run_args(extern_param, &[Value::I31(3)])
            .expect_err("rejected")
            .kind,
        RuntimeErrorKind::TypeMismatch { .. }
    ));
}

#[test]
fn host_any_extern_ref_roundtrip() {
    // HostAny -> ExternRef -> HostAny is the exact identity.
    let source = r#"(module
  (func (export "f") (param externref) (result anyref)
    local.get 0
    any.convert_extern
    extern.convert_any
    any.convert_extern))"#;
    let result = run_args(source, &[Value::ExternRef(Some(9))]).unwrap();
    assert_eq!(result, vec![Value::HostAny(Some(9))]);
}

#[test]
fn externalized_object_and_i31_roundtrip_identity() {
    let source = r#"(module
  (type $s (struct (field i32)))
  (func (export "f_obj") (result i32)
    (local $o (ref null $s))
    i32.const 5 struct.new $s
    local.tee $o
    extern.convert_any
    any.convert_extern
    ref.cast (ref $s)
    local.get $o
    ref.eq)
  (func (export "f_i31") (result i32)
    i32.const 5 ref.i31
    extern.convert_any
    any.convert_extern
    ref.cast (ref i31)
    i32.const 5 ref.i31
    ref.eq))"#;
    assert_eq!(run_i32_of(source, "f_obj"), 1);
    assert_eq!(run_i32_of(source, "f_i31"), 1);
}

fn run_i32_of(source: &str, export: &str) -> i32 {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    match execute_export(&module, &store, export, &[])
        .unwrap()
        .as_slice()
    {
        [Value::I32(value)] => *value,
        other => panic!("expected one i32, got {other:?}"),
    }
}

#[test]
fn externalized_i31_payload_survives_roundtrip() {
    let source = r#"(module
  (func (export "f") (result i32)
    i32.const -3 ref.i31
    extern.convert_any
    any.convert_extern
    ref.cast (ref i31)
    i31.get_s))"#;
    assert_eq!(run_i32(source), -3);
}

#[test]
fn externalized_object_in_extern_table_keeps_identity() {
    let source = r#"(module
  (type $s (struct (field i32)))
  (table $t 1 externref)
  (func (export "build") (result (ref $s))
    i32.const 5 struct.new $s)
  (func (export "store") (param (ref null $s))
    i32.const 0
    local.get 0
    extern.convert_any
    table.set $t)
  (func (export "load") (result (ref $s))
    i32.const 0
    table.get $t
    any.convert_extern
    ref.cast (ref $s)))"#;
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    let object = execute_export(&module, &store, "build", &[]).unwrap()[0];
    execute_export(&module, &store, "store", &[object]).unwrap();
    let back = execute_export(&module, &store, "load", &[]).unwrap()[0];
    assert_eq!(
        back, object,
        "externalized table roundtrip must keep identity"
    );
}

#[test]
fn converted_extern_null_satisfies_any_boundary() {
    let source = r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    any.convert_extern
    extern.convert_any
    ref.is_null))"#;
    assert_eq!(run_i32_args(source, &[Value::ExternRef(None)]), 1);
}

// ── A5: two-module distinct-ID subtype ───────────────────────────────

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

    fn add(&mut self, module: &Rc<RegModule>, imports: &Imports) -> Rc<RefCell<Store>> {
        let id = self.next_id;
        self.next_id += 1;
        Store::instantiate_linked(module, imports, &self.group, id, &self.arena)
            .expect("module instantiates")
    }
}

/// Two modules declare a structurally identical supertype (so it interns to
/// the same layout id in both heaps), while the provider allocates a
/// distinct concrete subtype. The consumer's `ref.test` against the shared
/// supertype must succeed through the subtype's precomputed ancestry chain.
#[test]
fn a5_distinct_id_subtype_across_modules() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $super (sub (struct (field i32))))
  (type $sub (sub $super (struct (field i32) (field i32))))
  (table (export "t") 1 (ref null $super))
  (func (export "store")
    i32.const 0
    i32.const 1 i32.const 2 struct.new $sub
    table.set 0))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $super (sub (struct (field i32))))
  (import "a" "t" (table 1 (ref null $super)))
  (func (export "f") (result i32)
    i32.const 0
    table.get 0
    ref.test (ref $super)))"#,
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, &Imports::new());
    let (ty, shared) = provider_store
        .borrow()
        .export_table("t")
        .expect("table export");
    let imports = Imports::new()
        .with_module_types("a", &provider)
        .shared_table("a", "t", ty, shared);
    let consumer_store = group.add(&consumer, &imports);

    execute_export(&provider, &provider_store.borrow(), "store", &[]).unwrap();
    let results = execute_export(&consumer, &consumer_store.borrow(), "f", &[]).unwrap();
    assert_eq!(
        results,
        vec![Value::I32(1)],
        "distinct-ID subtype must match across modules"
    );
}

/// A provider whose type section carries EXTRA leading types, so the shared
/// supertype interns at a DIFFERENT position than in the consumer's heap.
/// The consumer's concrete-target queries must resolve the foreign object by
/// the target's CLOSED FORM in the object heap, not by comparing the
/// store-side positional id.
#[test]
fn foreign_heap_non_aligned_ids_resolve_by_closed_form() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $pad1 (struct (field f64)))
  (type $pad2 (struct (field i64)))
  (type $super (sub (struct (field i32))))
  (type $sub (sub $super (struct (field i32) (field i32))))
  (table (export "t") 1 (ref null $super))
  (func (export "store")
    i32.const 0
    i32.const 1 i32.const 2 struct.new $sub
    table.set 0))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $pad (struct (field f32)))
  (type $super (sub (struct (field i32))))
  (import "a" "t" (table 1 (ref null $super)))
  (func (export "test_super") (result i32)
    i32.const 0
    table.get 0
    ref.test (ref $super))
  (func (export "cast_super") (result (ref $super))
    i32.const 0
    table.get 0
    ref.cast (ref $super))
  (func (export "br_super") (result i32)
    (block $hit (result (ref null $super))
      i32.const 0
      table.get 0
      br_on_cast $hit (ref null $super) (ref null $super)
      drop
      ref.null $super)
    ref.is_null
    i32.eqz))"#,
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, &Imports::new());
    let (ty, shared) = provider_store
        .borrow()
        .export_table("t")
        .expect("table export");
    let imports = Imports::new()
        .with_module_types("a", &provider)
        .shared_table("a", "t", ty, shared);
    let consumer_store = group.add(&consumer, &imports);

    execute_export(&provider, &provider_store.borrow(), "store", &[]).unwrap();

    // The provider interns $super at position 2; the consumer at position 1.
    // Only a closed-form lookup in the object heap finds the ancestor.
    let test = execute_export(&consumer, &consumer_store.borrow(), "test_super", &[]).unwrap();
    assert_eq!(test, vec![Value::I32(1)], "ref.test must match");

    let cast = execute_export(&consumer, &consumer_store.borrow(), "cast_super", &[]).unwrap();
    let _ = cast;

    let branch = execute_export(&consumer, &consumer_store.borrow(), "br_super", &[]).unwrap();
    assert_eq!(branch, vec![Value::I32(1)], "br_on_cast must branch on hit");
}

/// A target type with no closed-form match anywhere in the object heap must
/// miss `ref.test` and trap `ref.cast` when applied to the foreign object.
#[test]
fn foreign_heap_structurally_different_target_misses() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $pad1 (struct (field f64)))
  (type $pad2 (struct (field i64)))
  (type $super (sub (struct (field i32))))
  (type $sub (sub $super (struct (field i32) (field i32))))
  (table (export "t") 1 (ref null $super))
  (func (export "store")
    i32.const 0
    i32.const 1 i32.const 2 struct.new $sub
    table.set 0))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $pad (struct (field f32)))
  (type $super (sub (struct (field i32))))
  (type $other (struct (field i32) (field i64)))
  (import "a" "t" (table 1 (ref null $super)))
  (func (export "test_other") (result i32)
    i32.const 0
    table.get 0
    ref.test (ref $other))
  (func (export "cast_other") (result i32)
    i32.const 0
    table.get 0
    ref.cast (ref $other)
    drop
    i32.const 1))"#,
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, &Imports::new());
    let (ty, shared) = provider_store
        .borrow()
        .export_table("t")
        .expect("table export");
    let imports = Imports::new()
        .with_module_types("a", &provider)
        .shared_table("a", "t", ty, shared);
    let consumer_store = group.add(&consumer, &imports);

    execute_export(&provider, &provider_store.borrow(), "store", &[]).unwrap();

    let test = execute_export(&consumer, &consumer_store.borrow(), "test_other", &[]).unwrap();
    assert_eq!(test, vec![Value::I32(0)], "no closed-form match must miss");

    let error = execute_export(&consumer, &consumer_store.borrow(), "cast_other", &[])
        .expect_err("cast must trap");
    assert!(
        matches!(error.kind, RuntimeErrorKind::CastFailure { .. }),
        "expected CastFailure, got {:?}",
        error.kind
    );
}
