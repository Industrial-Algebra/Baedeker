// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 1: type-section validation.
//!
//! Pins the reference supertype rules (`check_subtype_sub`, valid.ml:161-177)
//! and the composite/heap-type match lattice (`valid/match.ml`) through the
//! public decode + validate pipeline.

use baedeker_core::binary::module::Module;
use baedeker_core::validate::error::ValidationError;

fn wat_bytes(text: &str) -> Vec<u8> {
    let buf = wast::parser::ParseBuffer::new(text)
        .unwrap_or_else(|error| panic!("failed to parse wat buffer: {error}"));
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf)
        .unwrap_or_else(|error| panic!("failed to parse wat: {error}"));
    wat.encode()
        .unwrap_or_else(|error| panic!("failed to encode wat: {error}"))
}

fn validate(text: &str) -> Result<(), ValidationError> {
    let bytes = wat_bytes(text);
    let module =
        Module::decode(&bytes).unwrap_or_else(|error| panic!("expected module to decode: {error}"));
    module.validate()
}

fn assert_invalid(text: &str, expected_kind: &str) {
    let error = match validate(text) {
        Ok(()) => panic!("expected invalid module, but it validated:\n{text}"),
        Err(error) => error,
    };
    let rendered = format!("{:?}", error.kind);
    assert!(
        rendered.contains(expected_kind),
        "expected {expected_kind}, got {rendered} for:\n{text}"
    );
}

fn assert_valid(text: &str) {
    if let Err(error) = validate(text) {
        panic!("expected valid module, got {error:?} for:\n{text}");
    }
}

fn push_leb_u32(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn module_with_type_section(type_data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
    bytes.push(0x01);
    push_leb_u32(type_data.len() as u32, &mut bytes);
    bytes.extend_from_slice(type_data);
    bytes
}

#[test]
fn reject_supertype_from_later_group() {
    // A bare subtype whose declared supertype is a later (not yet installed)
    // group is an unknown-index error.
    assert_invalid(
        "(module (type (sub 1 (func))) (type (func)))",
        "UnknownTypeIdx",
    );
}

#[test]
fn reject_in_group_supertype() {
    // Supertypes must strictly precede the rec group start, so an in-group
    // supertype is a forward use.
    assert_invalid(
        "(module (rec (type (sub 1 (func))) (type (func))))",
        "ForwardUseOfTypeInSubType",
    );
}

#[test]
fn reject_final_supertype() {
    // A bare type is final; declaring it as a supertype is invalid.
    assert_invalid(
        "(module (type (func)) (type (sub 0 (func))))",
        "FinalSupertype",
    );
}

#[test]
fn reject_struct_width_violation() {
    // The subtype has fewer fields than its declared supertype.
    assert_invalid(
        "(module
           (type $s (sub (struct (field i32) (field i32))))
           (type (sub $s (struct (field i32)))))",
        "SubtypeComptypeMismatch",
    );
}

#[test]
fn reject_mutable_field_variance() {
    // Mutable fields are invariant: (mut i32) does not match (mut i64).
    assert_invalid(
        "(module
           (type $s (sub (struct (field (mut i32)))))
           (type (sub $s (struct (field (mut i64))))))",
        "SubtypeComptypeMismatch",
    );
}

#[test]
fn reject_param_covariance() {
    // Function params are contravariant: a subtype may not widen a param.
    assert_invalid(
        "(module
           (type $s (sub (func (param i32))))
           (type (sub $s (func (param i64)))))",
        "SubtypeComptypeMismatch",
    );
}

#[test]
fn reject_nullability_mismatch() {
    // A nullable field does not match a non-nullable one.
    assert_invalid(
        "(module
           (type $s (sub (struct (field (ref func)))))
           (type (sub $s (struct (field (ref null func))))))",
        "SubtypeComptypeMismatch",
    );
}

#[test]
fn reject_unknown_type_index_in_field() {
    // `(ref 1)` inside a struct field that is never defined is invalid.
    assert_invalid("(module (type (struct (field (ref 1)))))", "UnknownTypeIdx");
}

#[test]
fn reject_bottom_of_wrong_hierarchy() {
    // `(ref none)` is the bottom of the any hierarchy and does not match a
    // function-referencing field.
    assert_invalid(
        "(module
           (type $s (sub (struct (field (ref func)))))
           (type (sub $s (struct (field (ref none))))))",
        "SubtypeComptypeMismatch",
    );
}

#[test]
fn accept_valid_multi_supertype_subtype() {
    // Two non-final supertypes, matching composite type: valid at this pin.
    // Built from raw bytes because the bundled text parser supports only a
    // single declared parent.
    let type_data = [
        0x03, // three types
        0x50, 0x00, 0x60, 0x00, 0x00, // (sub (func))
        0x50, 0x00, 0x60, 0x00, 0x00, // (sub (func))
        0x50, 0x02, 0x00, 0x01, 0x60, 0x00, 0x00, // (sub 0 1 (func))
    ];
    let bytes = module_with_type_section(&type_data);
    let module = Module::decode(&bytes).expect("multi-supertype module must decode");
    module
        .validate()
        .expect("multi-supertype module must validate");
}

#[test]
fn accept_valid_struct_width_subtype() {
    // A struct subtype may add trailing fields (width subtyping).
    assert_valid(
        "(module
           (type $s (sub (struct (field i32))))
           (type (sub $s (struct (field i32) (field i64)))))",
    );
}

#[test]
fn accept_valid_function_subtype() {
    // Functions: params contravariant (identity here), results covariant.
    assert_valid(
        "(module
           (type $s (sub (func (param i32) (result i32))))
           (type (sub $s (func (param i32) (result i32)))))",
    );
}
