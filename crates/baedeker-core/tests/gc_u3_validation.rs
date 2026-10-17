// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3: validation of the cast/conversion instruction family, pinned
//! to `interpreter/valid/valid.ml` rows 732-741 and 492-519.

use baedeker_core::binary::module::Module;
use baedeker_core::validate::error::ValidationErrorKind;

fn validate(source: &str) -> Result<(), ValidationErrorKind> {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.validate().map_err(|error| error.kind)
}

const TYPES: &str = r#"
  (type $a (sub (struct (field i32))))
  (type $b (sub $a (struct (field i32) (field i32))))
  (type $other (struct (field i32)))
"#;

// ── ref.test / ref.cast operand = (ref null top(ht)) ─────────────────

#[test]
fn ref_test_extern_target_accepts_externref_operand() {
    let source = r#"(module
  (func (export "f") (param externref) (result i32)
    local.get 0
    ref.test (ref extern)))"#;
    assert_eq!(validate(source), Ok(()));
}

#[test]
fn ref_test_extern_target_rejects_anyref_operand() {
    // `top(extern) = extern`, so an anyref operand (top any) is not a
    // valid operand.
    let source = r#"(module
  (func (export "f") (param anyref) (result i32)
    local.get 0
    ref.test (ref extern)))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::TypeMismatch { op: "ref.test", .. })
    ));
}

#[test]
fn ref_test_concrete_target_reads_any_top() {
    // `top($a) = any`: a `(ref null $b)` operand is accepted.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $b)) (result i32)
    local.get 0
    ref.test (ref $a)))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn ref_cast_result_is_target_nullness() {
    let nonnull = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result (ref $b))
    local.get 0
    ref.cast (ref $b)))"#
    );
    assert_eq!(validate(&nonnull), Ok(()));

    // Null form result is nullable, assignable to `(ref null $b)`.
    let nullable = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result (ref null $b))
    local.get 0
    ref.cast (ref null $b)))"#
    );
    assert_eq!(validate(&nullable), Ok(()));
}

#[test]
fn ref_cast_null_form_to_nonnull_result_rejected() {
    // A nullable cast cannot produce a non-null result.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result (ref $b))
    local.get 0
    ref.cast (ref null $b)))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::TypeMismatch { .. })
            | Err(ValidationErrorKind::ResultTypeMismatch { .. })
            | Err(ValidationErrorKind::FunctionResultTypeMismatch { .. })
            | Err(ValidationErrorKind::ControlResultTypeMismatch { .. })
    ));
}

// ── ref.eq operand domain ────────────────────────────────────────────

#[test]
fn ref_eq_accepts_eq_side_operands() {
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a) (ref null $a)) (result i32)
    local.get 0
    local.get 1
    ref.eq))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn ref_eq_rejects_externref_operand() {
    let source = r#"(module
  (func (export "f") (param externref externref) (result i32)
    local.get 0
    local.get 1
    ref.eq))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::TypeMismatch { op: "ref.eq", .. })
    ));
}

// ── br_on_cast / br_on_cast_fail ─────────────────────────────────────

#[test]
fn br_on_cast_requires_target_subtype_of_source() {
    // rt2 = (ref $other) is not a subtype of rt1 = (ref $a).
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result i32)
    (block $hit (result (ref null $other))
      local.get 0
      br_on_cast $hit (ref null $a) (ref null $other)
      drop
      ref.null $other)
    drop
    i32.const 0))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::TypeMismatch {
            op: "br_on_cast",
            ..
        })
    ));
}

#[test]
fn br_on_cast_fail_requires_target_subtype_of_source() {
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $other)) (result i32)
    (block $miss (result (ref null $other))
      local.get 0
      br_on_cast_fail $miss (ref null $other) (ref null $a)
      drop
      ref.null $other)
    drop
    i32.const 0))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::TypeMismatch {
            op: "br_on_cast",
            ..
        })
    ));
}

#[test]
fn br_on_cast_label_last_type_must_accept_target() {
    // The label's result is i32, not the cast reference type.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result i32)
    (block $hit (result i32)
      local.get 0
      br_on_cast $hit (ref null $a) (ref null $b)
      drop
      i32.const 0)))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::BranchTypeMismatch { .. })
    ));
}

#[test]
fn br_on_cast_label_must_be_non_empty() {
    // `br_on_cast` to the function's implicit return frame with no results.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a))
    local.get 0
    br_on_cast 0 (ref null $a) (ref null $b)))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::BranchTypeMismatch { .. })
    ));
}

#[test]
fn br_on_cast_nullable_target_fallthrough_is_non_null() {
    // diff((ref null $a), (ref null $b)) = (ref $a): the fall-through value
    // is non-null, so returning it from a `(ref $a)` function is valid.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result (ref $a))
    (block $hit (result (ref null $b))
      local.get 0
      br_on_cast $hit (ref null $a) (ref null $b)
      return)
    unreachable))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn br_on_cast_non_null_target_fallthrough_keeps_source_nullness() {
    // diff((ref null $a), (ref $b)) = (ref null $a): the fall-through value
    // is nullable, so returning it from a `(ref $a)` function is rejected.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result (ref $a))
    (block $hit (result (ref null $b))
      local.get 0
      br_on_cast $hit (ref null $a) (ref $b)
      return)
    unreachable))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::FunctionResultTypeMismatch { .. })
            | Err(ValidationErrorKind::ResultTypeMismatch { .. })
            | Err(ValidationErrorKind::ControlResultTypeMismatch { .. })
    ));
}

#[test]
fn br_on_cast_fail_fallthrough_is_target() {
    // Fall-through is `rt2` = `(ref $b)` non-null, so returning it from a
    // `(ref $b)` function is valid.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result (ref $b))
    (block $miss (result (ref null $a))
      local.get 0
      br_on_cast_fail $miss (ref null $a) (ref $b)
      return)
    unreachable))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn br_on_cast_label_diff_refinement_requires_null_matching() {
    // `br_on_cast_fail` with a non-null target: the label's last type must
    // accept `rt1' = diff((ref null $a), (ref $b)) = (ref null $a)`.
    // Declaring the label result as the non-null `(ref $a)` rejects.
    let source = format!(
        r#"(module {TYPES}
  (func (export "f") (param (ref null $a)) (result i32)
    (block $miss (result (ref $a))
      local.get 0
      br_on_cast_fail $miss (ref null $a) (ref $b)
      drop
      ref.null $a)
    drop
    i32.const 0))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::BranchTypeMismatch { .. })
    ));
}

// ── conversions ──────────────────────────────────────────────────────

#[test]
fn any_convert_extern_type_effects() {
    let source = r#"(module
  (func (export "f") (param externref) (result anyref)
    local.get 0
    any.convert_extern))"#;
    assert_eq!(validate(source), Ok(()));
}

#[test]
fn any_convert_extern_rejects_anyref_operand() {
    let source = r#"(module
  (func (export "f") (param anyref) (result anyref)
    local.get 0
    any.convert_extern))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::TypeMismatch {
            op: "any.convert_extern",
            ..
        })
    ));
}

#[test]
fn extern_convert_any_type_effects() {
    let source = r#"(module
  (func (export "f") (param anyref) (result externref)
    local.get 0
    extern.convert_any))"#;
    assert_eq!(validate(source), Ok(()));
}

#[test]
fn extern_convert_any_rejects_externref_operand() {
    let source = r#"(module
  (func (export "f") (param externref) (result externref)
    local.get 0
    extern.convert_any))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::TypeMismatch {
            op: "extern.convert_any",
            ..
        })
    ));
}

#[test]
fn conversion_preserves_non_null_nullness() {
    // A non-null externref converts to a non-null anyref.
    let source = r#"(module
  (func (export "f") (param (ref extern)) (result (ref any))
    local.get 0
    any.convert_extern))"#;
    assert_eq!(validate(source), Ok(()));

    // And back.
    let source = r#"(module
  (func (export "f") (param (ref any)) (result (ref extern))
    local.get 0
    extern.convert_any))"#;
    assert_eq!(validate(source), Ok(()));
}

#[test]
fn converted_extern_is_refused_by_eq_typed_consumers() {
    // The conversion produces an anyref, which is not an eq value.
    let source = r#"(module
  (func (export "f") (param externref externref) (result i32)
    local.get 0
    any.convert_extern
    local.get 1
    any.convert_extern
    ref.eq))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::TypeMismatch { op: "ref.eq", .. })
    ));
}

// ── FIX 2: block/loop GC shorthand result types ──────────────────────

/// Every GC reference shorthand the U1 grammar supports is accepted as a
/// `block`/`loop` result type (WAT → encode → decode → validate). Before the
/// fix the block-type discriminator only admitted `0x69`/`0x6f`/`0x70` plus
/// `ref`/`ref null`, so the other shorthand bytes misdecoded as type indices.
#[test]
fn block_and_loop_accept_every_gc_shorthand_result() {
    for shorthand in [
        "anyref",
        "eqref",
        "i31ref",
        "structref",
        "arrayref",
        "nullref",
        "nullfuncref",
        "nullexternref",
        "exnref",
        "nullexnref",
        "funcref",
        "externref",
    ] {
        for form in ["block", "loop"] {
            let source = format!(
                r#"(module (func (result {shorthand}) ({form} (result {shorthand}) unreachable)))"#
            );
            assert_eq!(
                validate(&source),
                Ok(()),
                "{form} result shorthand `{shorthand}` must validate"
            );
        }
    }
}

/// The reviewer's exact shorthand, spelled `(ref null any)`, is accepted too.
#[test]
fn block_accepts_ref_null_any_result() {
    let source = r#"(module
  (func (param (ref null any)) (result i32 (ref null any))
    (block $b (result i32 (ref null any))
      i32.const 7
      local.get 0
      br_on_cast $b (ref null any) (ref struct))))"#;
    assert_eq!(validate(source), Ok(()));
}
