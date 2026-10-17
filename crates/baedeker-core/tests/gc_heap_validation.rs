// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2: validation of the struct/array/i31 instruction families.
//!
//! Each test drives the full pipeline (WAT → encode → decode) and then
//! checks `Module::validate`, mirroring `valid/valid.ml`'s per-row rules.

use baedeker_core::binary::module::Module;
use baedeker_core::validate::error::ValidationErrorKind;

fn validate(source: &str) -> Result<(), ValidationErrorKind> {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.validate().map_err(|error| error.kind)
}

const STRUCT_TYPES: &str = r#"
  (type $s (struct (field i32) (field (mut i64)) (field (mut i8))))
  (type $packed (struct (field i8) (field (mut i16))))
  (type $imm (struct (field i32)))
  (type $def (struct (field i32) (field (ref null func))))
  (type $nondef (struct (field (ref func))))
"#;

#[test]
fn struct_new_valid() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (result (ref $s))
    i32.const 1 i64.const 2 i32.const 3 struct.new $s))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn struct_new_wrong_field_type_rejected() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (result (ref $s))
    i32.const 1 i64.const 2 f32.const 3 struct.new $s))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::TypeMismatch { .. })
    ));
}

#[test]
fn struct_new_default_valid() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (result (ref $def)) struct.new_default $def))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn struct_new_default_non_defaultable_rejected() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (result (ref $nondef)) struct.new_default $nondef))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::NonDefaultable { .. })
    ));
}

#[test]
fn struct_new_on_non_struct_rejected() {
    let source = r#"(module
  (type $v (func))
  (func (export "f") (result (ref 0)) struct.new 1))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::UnknownStruct { .. })
    ));
}

#[test]
fn struct_get_valid() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $s)) (result i64)
    local.get 0 struct.get $s 1))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn struct_get_packed_rejected() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $packed)) (result i32)
    local.get 0 struct.get $packed 0))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::PackedFieldMismatch { packed: true, .. })
    ));
}

#[test]
fn struct_get_s_unpacked_rejected() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $s)) (result i32)
    local.get 0 struct.get_s $s 0))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::PackedFieldMismatch { packed: false, .. })
    ));
}

#[test]
fn struct_get_s_packed_valid() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $packed)) (result i32)
    local.get 0 struct.get_s $packed 0))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn struct_get_unknown_field_rejected() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $imm)) (result i32)
    local.get 0 struct.get $imm 1))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::UnknownField { .. })
    ));
}

#[test]
fn struct_set_valid() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $s))
    local.get 0 i64.const 5 struct.set $s 1))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn struct_set_immutable_rejected() {
    let source = format!(
        r#"(module {STRUCT_TYPES}
  (func (export "f") (param (ref null $imm))
    local.get 0 i32.const 5 struct.set $imm 0))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::ImmutableField { .. })
    ));
}

const ARRAY_TYPES: &str = r#"
  (type $a (array (mut i32)))
  (type $packed (array (mut i8)))
  (type $imm (array i32))
  (type $def (array (mut (ref null func))))
  (type $nondef (array (ref func)))
  (type $v (array (mut (ref null func))))
"#;

#[test]
fn array_new_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (result (ref $a))
    i32.const 7 i32.const 9 array.new $a))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_new_default_non_defaultable_rejected() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (result (ref $nondef))
    i32.const 7 array.new_default $nondef))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::NonDefaultable { .. })
    ));
}

#[test]
fn array_new_default_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (result (ref $def))
    i32.const 7 array.new_default $def))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_new_fixed_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (result (ref $a))
    i32.const 1 i32.const 2 i32.const 3 array.new_fixed $a 3))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_get_and_set_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (param (ref null $a)) (result i32)
    local.get 0 i32.const 0 i32.const 5 array.set $a
    local.get 0 i32.const 0 array.get $a))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_get_packed_rejected() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (param (ref null $packed)) (result i32)
    local.get 0 i32.const 0 array.get $packed))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::PackedFieldMismatch { packed: true, .. })
    ));
}

#[test]
fn array_set_immutable_rejected() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (param (ref null $imm))
    local.get 0 i32.const 0 i32.const 5 array.set $imm))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::ImmutableArray { .. })
    ));
}

#[test]
fn array_len_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (param (ref null $a)) (result i32)
    local.get 0 array.len))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_fill_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (param (ref null $a))
    local.get 0 i32.const 0 i32.const 5 i32.const 3 array.fill $a))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_copy_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (param (ref null $a) (ref null $a))
    local.get 0 i32.const 0 local.get 1 i32.const 0 i32.const 2 array.copy $a $a))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_copy_storage_mismatch_rejected() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (type $b (array (mut i64)))
  (func (export "f") (param (ref null $a) (ref null $b))
    local.get 0 i32.const 0 local.get 1 i32.const 0 i32.const 2 array.copy $a $b))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::ArrayStorageMismatch { .. })
    ));
}

#[test]
fn array_new_data_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (data $d "abcd")
  (func (export "f") (result (ref $a))
    i32.const 0 i32.const 1 array.new_data $a $d))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_new_data_non_numeric_rejected() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (data $d "abcd")
  (func (export "f") (result (ref $v))
    i32.const 0 i32.const 1 array.new_data $v $d))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::ArrayNotNumOrVec { .. })
    ));
}

#[test]
fn array_new_elem_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (elem $e func $g)
  (func $g)
  (func (export "f") (result (ref $v))
    i32.const 0 i32.const 1 array.new_elem $v $e))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_new_elem_type_mismatch_rejected() {
    let source = r#"(module
  (type $v (array (mut externref)))
  (elem $e func $g)
  (func $g)
  (func (export "f") (result (ref $v))
    i32.const 0 i32.const 1 array.new_elem $v $e))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::ElemFieldTypeMismatch { .. })
    ));
}

#[test]
fn array_init_data_and_elem_valid() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (data $d "abcd")
  (elem $e func $g)
  (func $g)
  (func (export "f") (param (ref null $a) (ref null $v))
    local.get 0 i32.const 0 i32.const 0 i32.const 1 array.init_data $a $d
    local.get 1 i32.const 0 i32.const 0 i32.const 1 array.init_elem $v $e))"#
    );
    assert_eq!(validate(&source), Ok(()));
}

#[test]
fn array_init_data_immutable_rejected() {
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (data $d "abcd")
  (func (export "f") (param (ref null $imm))
    local.get 0 i32.const 0 i32.const 0 i32.const 1 array.init_data $imm $d))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::ImmutableArray { .. })
    ));
}

#[test]
fn ref_i31_and_gets_valid() {
    let source = r#"(module
  (func (export "f") (result i32)
    i32.const -1 ref.i31 i31.get_u
    i32.const 7 ref.i31 i31.get_s
    i32.add))"#;
    assert_eq!(validate(source), Ok(()));
}

#[test]
fn i31_get_on_non_i31_rejected() {
    let source = r#"(module
  (func (export "f") (param (ref null func)) (result i32)
    local.get 0 i31.get_s))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::TypeMismatch { .. })
    ));
}

#[test]
fn array_on_non_array_type_rejected() {
    let source = r#"(module
  (type $s (struct (field i32)))
  (func (export "f") (result (ref $s))
    i32.const 1 i32.const 0 array.new 0))"#;
    assert!(matches!(
        validate(source),
        Err(ValidationErrorKind::UnknownArray { .. })
    ));
}

#[test]
fn array_new_fixed_huge_count_underflows_cleanly() {
    // A huge count on a shallow stack must be a validation error, not an
    // abort or a huge allocation. The operand stack cannot hold the values.
    let source = format!(
        r#"(module {ARRAY_TYPES}
  (func (export "f") (result (ref $a))
    i32.const 1
    array.new_fixed $a 2147483647))"#
    );
    assert!(matches!(
        validate(&source),
        Err(ValidationErrorKind::StackUnderflow { .. })
    ));
}
