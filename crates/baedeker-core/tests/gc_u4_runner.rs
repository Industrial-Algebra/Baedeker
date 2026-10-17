// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: reference result patterns and host-handle result matching.
//!
//! The pinned rows are `script/runner.ml:464-483` (`assert_ref_pat`): the
//! `RefTypePat` wildcards, `NullPat`, and `RefPat`'s `eq_ref` identity — which
//! for host values means host-handle equality. `ref.host N` (an *any-side*
//! host reference, `script.ml:80`) and `ref.extern N` (the extern-side zone)
//! are distinct inputs, so a module that internalizes `(ref.extern 3)` must
//! return something a `(ref.host 3)` pattern accepts.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RefPattern, Store, Value, execute_export, ref_matches_pattern};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

fn run(source: &str, export: &str, args: &[Value]) -> Vec<Value> {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    execute_export(&module, &store, export, args).unwrap()
}

fn matches(value: Value, pattern: RefPattern) -> bool {
    ref_matches_pattern(&value, pattern, None)
}

const REFS: &str = r#"
(module
  (type $s (struct (field i32)))
  (type $a (array i32))
  (type $f (func))
  (elem declare func $noop)
  (func $noop)
  (func (export "echo_extern") (param externref) (result externref)
    local.get 0)
  (func (export "internalize") (param externref) (result anyref)
    local.get 0
    any.convert_extern)
  (func (export "externalize") (param anyref) (result externref)
    local.get 0
    extern.convert_any)
  (func (export "i31") (result (ref i31))
    i32.const 7
    ref.i31)
  (func (export "struct") (result (ref $s))
    i32.const 1
    struct.new $s)
  (func (export "array") (result (ref $a))
    i32.const 2
    i32.const 3
    array.new $a)
  (func (export "funcref") (result funcref)
    ref.func $noop)
  (func (export "null_any") (result anyref)
    ref.null any)
)"#;

/// `ref.extern N` in, `(ref.extern N)` out: exactly equal.
#[test]
fn module_returning_ref_extern_is_asserted_equal() {
    let values = run(REFS, "echo_extern", &[Value::ExternRef(Some(9))]);
    assert_eq!(values, vec![Value::ExternRef(Some(9))]);
    assert!(matches(values[0], RefPattern::Extern(Some(9))));
    assert!(!matches(values[0], RefPattern::Extern(Some(8))));
    // Bare `(ref.extern)`: any non-null extern reference.
    assert!(matches(values[0], RefPattern::Extern(None)));
    assert!(!matches(values[0], RefPattern::Null));
}

/// A host reference internalized by `any.convert_extern` keeps its handle, so
/// a `(ref.host N)` result pattern matches by identity (`eq_ref`).
#[test]
fn ref_host_result_matches_host_handle_identity() {
    let values = run(REFS, "internalize", &[Value::ExternRef(Some(3))]);
    assert_eq!(values, vec![Value::HostAny(Some(3))]);
    assert!(
        matches(values[0], RefPattern::Host(3)),
        "internalized host handle must match (ref.host 3)"
    );
    assert!(!matches(values[0], RefPattern::Host(4)));
    // The any-side handle is not the extern zone's exact value.
    assert!(!matches(values[0], RefPattern::Extern(Some(3))));
}

/// `ref.host N` as an INPUT is an any-side value: `externalize` accepts it
/// and re-tags it into the extern zone with the same handle.
#[test]
fn ref_host_input_flows_through_externalize() {
    let values = run(REFS, "externalize", &[Value::HostAny(Some(2))]);
    assert_eq!(values, vec![Value::ExternRef(Some(2))]);
    assert!(matches(values[0], RefPattern::Extern(Some(2))));
    // Identity holds across zones for the same host value.
    assert!(matches(values[0], RefPattern::Host(2)));
}

/// `RefTypePat AnyHT` matches everything except a non-null funcref.
#[test]
fn any_pattern_excludes_funcrefs() {
    assert!(matches(run(REFS, "i31", &[])[0], RefPattern::Any));
    assert!(matches(run(REFS, "struct", &[])[0], RefPattern::Any));
    assert!(matches(run(REFS, "array", &[])[0], RefPattern::Any));
    assert!(matches(
        run(REFS, "internalize", &[Value::ExternRef(Some(1))])[0],
        RefPattern::Any
    ));
    // A null also matches the `AnyHT, _` wildcard row.
    assert!(matches(run(REFS, "null_any", &[])[0], RefPattern::Any));
    assert!(
        !matches(run(REFS, "funcref", &[])[0], RefPattern::Any),
        "funcrefs are explicitly excluded from anyref"
    );
}

/// `RefTypePat EqHT` matches i31/struct/array only — not host handles.
#[test]
fn eq_pattern_matches_i31_and_aggregates_only() {
    assert!(matches(run(REFS, "i31", &[])[0], RefPattern::Eq));
    assert!(matches(run(REFS, "struct", &[])[0], RefPattern::Eq));
    assert!(matches(run(REFS, "array", &[])[0], RefPattern::Eq));
    assert!(!matches(
        run(REFS, "internalize", &[Value::ExternRef(Some(1))])[0],
        RefPattern::Eq
    ));
    assert!(!matches(run(REFS, "null_any", &[])[0], RefPattern::Eq));
}

/// `RefTypePat StructHT`/`ArrayHT` are kind-aware: the heap knows the object's
/// kind, the value alone does not.
#[test]
fn struct_and_array_patterns_are_kind_aware() {
    let module = wat_module(REFS);
    let store = Store::instantiate(&module).unwrap();
    let i31 = execute_export(&module, &store, "i31", &[]).unwrap();
    let structure = execute_export(&module, &store, "struct", &[]).unwrap();
    let array = execute_export(&module, &store, "array", &[]).unwrap();

    assert!(ref_matches_pattern(
        &structure[0],
        RefPattern::Struct,
        Some(&store)
    ));
    assert!(!ref_matches_pattern(
        &structure[0],
        RefPattern::Array,
        Some(&store)
    ));
    assert!(ref_matches_pattern(
        &array[0],
        RefPattern::Array,
        Some(&store)
    ));
    assert!(!ref_matches_pattern(
        &array[0],
        RefPattern::Struct,
        Some(&store)
    ));
    assert!(!ref_matches_pattern(
        &i31[0],
        RefPattern::Struct,
        Some(&store)
    ));
    assert!(!ref_matches_pattern(
        &i31[0],
        RefPattern::Array,
        Some(&store)
    ));
}

/// `NullPat` matches only null references.
#[test]
fn null_pattern_matches_only_nulls() {
    assert!(matches(Value::FuncRef(None), RefPattern::Null));
    assert!(matches(Value::ExternRef(None), RefPattern::Null));
    assert!(matches(Value::HostAny(None), RefPattern::Null));
    assert!(matches(Value::ExternGc(None), RefPattern::Null));
    assert!(matches(run(REFS, "null_any", &[])[0], RefPattern::Null));
    assert!(!matches(Value::I31(0), RefPattern::Null));
    assert!(!matches(run(REFS, "struct", &[])[0], RefPattern::Null));
}

/// An externalized i31 comes back as `extern.convert_any`'s extern-side value:
/// `(ref.extern)` matches it, and the extern object keeps i31 identity.
#[test]
fn extern_pattern_matches_externalized_gc_values() {
    let values = run(REFS, "externalize", &[Value::I31(5)]);
    assert_eq!(
        values,
        vec![Value::ExternGc(Some(
            baedeker_core::runtime::ExternPayload::I31(5)
        ))]
    );
    assert!(matches(values[0], RefPattern::Extern(None)));
    // Round trip back into the any zone restores the i31.
    let back = run(REFS, "internalize", &values);
    assert_eq!(back, vec![Value::I31(5)]);
    assert!(matches(back[0], RefPattern::I31));
}
