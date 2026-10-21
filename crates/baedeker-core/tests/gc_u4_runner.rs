// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: reference result patterns and host-handle result matching.
//!
//! The pinned rows are `script/runner.ml:464-483` (`assert_ref_pat`): the
//! `RefTypePat` wildcards, `NullPat`, and `RefPat`'s `eq_ref` identity. Two
//! identity regimes must not be confused:
//!
//! * `RefPat` carries a VALUE and matches by `eq_ref` identity. `script.ml:93`
//!   equates `HostRef n` with `HostRef n` ONLY (and `runtime/extern.ml:13`
//!   equates `ExternRef r1` with `ExternRef r2` by their payloads), so a
//!   `(ref.host N)` pattern never matches an extern-zone value.
//! * `RefTypePat` is KIND-ONLY: `RefTypePat ExternHT, _ -> true` accepts every
//!   reference value, nulls included.
//!
//! Reference patterns also only apply to reference values: the pin reaches
//! `assert_ref_pat` from the `Ref` result case, so a numeric result never
//! matches one.

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
    // Bare `(ref.extern)` is the `RefTypePat ExternHT` row: EVERY reference
    // value, nulls included.
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
/// and re-tags it into the extern zone with the same handle. The two zones
/// are NOT interchangeable for `RefPat` identity: `script.ml:93` equates
/// `HostRef` with `HostRef` only, so the externalized extern-zone value is
/// matched by `(ref.extern N)`, never by `(ref.host N)`.
#[test]
fn ref_host_input_flows_through_externalize() {
    let values = run(REFS, "externalize", &[Value::HostAny(Some(2))]);
    assert_eq!(values, vec![Value::ExternRef(Some(2))]);
    assert!(matches(values[0], RefPattern::Extern(Some(2))));
    assert!(
        !matches(values[0], RefPattern::Host(2)),
        "host-identity patterns never cross zones (script.ml:93)"
    );
    // The internalized any-side value is what `(ref.host N)` matches.
    let internal = run(REFS, "internalize", &[Value::ExternRef(Some(2))]);
    assert_eq!(internal, vec![Value::HostAny(Some(2))]);
    assert!(matches(internal[0], RefPattern::Host(2)));
    assert!(!matches(internal[0], RefPattern::Extern(Some(2))));
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

/// `RefTypePat ExternHT, _ -> true` (runner.ml:475) has NO null exclusion: the
/// bare `(ref.extern)` pattern matches every reference value, null included.
#[test]
fn extern_wildcard_accepts_every_reference_value_including_null() {
    use baedeker_core::runtime::{ExternPayload, HeapId, ObjectId};

    let references = [
        Value::FuncRef(Some((0, 1))),
        Value::FuncRef(None),
        Value::ExternRef(Some(7)),
        Value::ExternRef(None),
        Value::HostAny(Some(7)),
        Value::HostAny(None),
        Value::ExternGc(Some(ExternPayload::I31(1))),
        Value::ExternGc(None),
        Value::ExnRef(Some(0)),
        Value::ExnRef(None),
        Value::GcRef {
            heap: HeapId(1),
            id: ObjectId(0),
        },
        Value::I31(5),
    ];
    for value in references {
        assert!(
            matches(value, RefPattern::Extern(None)),
            "(ref.extern) must match every reference value, got {value:?}"
        );
    }
    // ... and only reference values.
    assert!(!matches(Value::I32(7), RefPattern::Extern(None)));

    // `(ref.extern N)` stays exact extern-zone identity: the any-side
    // internalized handle and the externalized GC value are other values.
    assert!(matches(
        Value::ExternRef(Some(3)),
        RefPattern::Extern(Some(3))
    ));
    assert!(!matches(
        Value::HostAny(Some(3)),
        RefPattern::Extern(Some(3))
    ));
}

/// `(ref.host N)` is `HostRef` identity only (`script.ml:93`): the any-side
/// internalized handle matches by handle, the extern zone and every other
/// reference kind do not.
#[test]
fn host_pattern_is_any_zone_identity_only() {
    assert!(matches(Value::HostAny(Some(3)), RefPattern::Host(3)));
    assert!(!matches(Value::HostAny(Some(4)), RefPattern::Host(3)));
    assert!(!matches(Value::HostAny(None), RefPattern::Host(3)));
    assert!(
        !matches(Value::ExternRef(Some(3)), RefPattern::Host(3)),
        "the extern zone is not HostRef, so (ref.host 3) must not match"
    );
    assert!(!matches(Value::ExternRef(None), RefPattern::Host(3)));
    assert!(!matches(Value::ExternGc(None), RefPattern::Host(3)));
    assert!(!matches(Value::I32(3), RefPattern::Host(3)));
}

/// A reference pattern never matches a non-reference value: the pin dispatches
/// reference patterns from the `Ref` result case only.
#[test]
fn reference_patterns_reject_non_reference_values() {
    let patterns = [
        RefPattern::Null,
        RefPattern::Extern(None),
        RefPattern::Extern(Some(0)),
        RefPattern::Host(0),
        RefPattern::FuncAny,
        RefPattern::FuncIndex(0),
        RefPattern::Any,
        RefPattern::Eq,
        RefPattern::Struct,
        RefPattern::Array,
        RefPattern::I31,
    ];
    let non_references = [
        Value::I32(7),
        Value::I64(7),
        Value::F32(7.5),
        Value::F64(7.5),
        Value::V128([7; 16]),
    ];
    for value in non_references {
        for pattern in patterns {
            assert!(
                !matches(value, pattern),
                "{pattern:?} must not match the non-reference {value:?}"
            );
        }
    }
}
