// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 1: type-section binary grammar.
//!
//! Exercises every row of the reference grammar (`interpreter/binary/decode.ml`
//! at pin `dfa3f32`) plus the malformed variants the decoder must reject.

use baedeker_core::binary::module::Module;
use baedeker_core::types::{CompType, HeapType, RefType, StorageType, SubType, TypeIdx, ValType};

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

/// A minimal module whose only section is the given type-section payload.
fn module_with_type_section(type_data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
    bytes.push(0x01); // type section id
    push_leb_u32(type_data.len() as u32, &mut bytes);
    bytes.extend_from_slice(type_data);
    bytes
}

fn decode_ok(type_data: &[u8]) -> Vec<SubType> {
    let bytes = module_with_type_section(type_data);
    Module::decode(&bytes)
        .unwrap_or_else(|error| panic!("expected decode success: {error}"))
        .types()
        .to_vec()
}

fn decode_err(type_data: &[u8]) -> baedeker_core::error::DecodeError {
    let bytes = module_with_type_section(type_data);
    Module::decode(&bytes).expect_err("expected decode failure")
}

fn func(types: &[SubType], index: usize) -> &baedeker_core::types::FuncType {
    types[index].as_func().expect("expected a function type")
}

#[test]
fn decode_func_comptype() {
    let types = decode_ok(&[0x01, 0x60, 0x00, 0x00]);
    assert_eq!(types.len(), 1);
    assert!(types[0].is_final);
    assert!(types[0].supertypes.is_empty());
    assert!(func(&types, 0).params.is_empty());
}

#[test]
fn decode_struct_comptype() {
    let types = decode_ok(&[0x01, 0x5F, 0x01, 0x7F, 0x00]);
    match &types[0].comp {
        CompType::Struct(fields) => {
            assert_eq!(fields.len(), 1);
            assert!(!fields[0].mutable);
            assert_eq!(
                fields[0].storage,
                StorageType::Val(ValType::Num(baedeker_core::types::NumType::I32))
            );
        }
        other => panic!("expected struct, got {other:?}"),
    }
}

#[test]
fn decode_array_comptype() {
    let types = decode_ok(&[0x01, 0x5E, 0x7F, 0x01]);
    match &types[0].comp {
        CompType::Array(field) => {
            assert!(field.mutable);
            assert_eq!(
                field.storage,
                StorageType::Val(ValType::Num(baedeker_core::types::NumType::I32))
            );
        }
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn decode_packed_field_storage() {
    let types = decode_ok(&[0x01, 0x5F, 0x02, 0x78, 0x00, 0x77, 0x01]);
    match &types[0].comp {
        CompType::Struct(fields) => {
            assert_eq!(fields[0].storage, StorageType::I8);
            assert!(!fields[0].mutable);
            assert_eq!(fields[1].storage, StorageType::I16);
            assert!(fields[1].mutable);
        }
        other => panic!("expected struct, got {other:?}"),
    }
}

#[test]
fn decode_sub_non_final() {
    let types = decode_ok(&[0x01, 0x50, 0x00, 0x60, 0x00, 0x00]);
    assert!(!types[0].is_final);
    assert!(types[0].supertypes.is_empty());
}

#[test]
fn decode_sub_final_with_supertype() {
    let types = decode_ok(&[0x02, 0x60, 0x00, 0x00, 0x4F, 0x01, 0x00, 0x60, 0x00, 0x00]);
    assert!(types[0].is_final);
    assert!(types[1].is_final);
    assert_eq!(types[1].supertypes, vec![0]);
}

#[test]
fn decode_sub_with_multiple_supertypes() {
    let types = decode_ok(&[
        0x03, 0x60, 0x00, 0x00, 0x60, 0x00, 0x00, 0x4F, 0x02, 0x00, 0x01, 0x60, 0x00, 0x00,
    ]);
    assert_eq!(types[2].supertypes, vec![0, 1]);
}

#[test]
fn decode_rec_group() {
    let bytes = module_with_type_section(&[0x01, 0x4E, 0x02, 0x60, 0x00, 0x00, 0x5F, 0x00]);
    let module = Module::decode(&bytes).unwrap();
    assert_eq!(module.types().len(), 2);
    assert_eq!(module.rec_groups, vec![(0, 2)]);
    assert!(matches!(module.types()[1].comp, CompType::Struct(_)));
}

#[test]
fn decode_empty_rec_group_is_valid() {
    let bytes = module_with_type_section(&[0x01, 0x4E, 0x00]);
    let module = Module::decode(&bytes).expect("empty rec group must be valid");
    assert!(module.types().is_empty());
    assert_eq!(module.rec_groups, vec![(0, 0)]);
}

#[test]
fn decode_all_abstract_heaptypes() {
    let types = decode_ok(&[
        0x01, 0x60, 0x0C, 0x6E, 0x6D, 0x6C, 0x6B, 0x6A, 0x71, 0x70, 0x73, 0x6F, 0x72, 0x69, 0x74,
        0x00,
    ]);
    let expected = [
        HeapType::Any,
        HeapType::Eq,
        HeapType::I31,
        HeapType::Struct,
        HeapType::Array,
        HeapType::None,
        HeapType::Func,
        HeapType::NoFunc,
        HeapType::Extern,
        HeapType::NoExtern,
        HeapType::Exn,
        HeapType::NoExn,
    ];
    let params = &func(&types, 0).params;
    assert_eq!(params.len(), expected.len());
    for (param, expected) in params.iter().zip(expected) {
        let ValType::Ref(ref_type) = param else {
            panic!("expected reference type, got {param:?}");
        };
        assert_eq!(ref_type.heap_type(), expected);
    }
}

#[test]
fn decode_typed_null_and_non_null_refs() {
    let types = decode_ok(&[0x01, 0x60, 0x02, 0x63, 0x00, 0x64, 0x00, 0x00]);
    let params = &func(&types, 0).params;
    assert_eq!(params[0], ValType::Ref(RefType::concrete(true, TypeIdx(0))));
    assert_eq!(
        params[1],
        ValType::Ref(RefType::concrete(false, TypeIdx(0)))
    );
}

#[test]
fn decode_in_group_forward_reference() {
    let types = decode_ok(&[
        0x01, 0x4E, 0x02, 0x60, 0x01, 0x63, 0x01, 0x00, 0x60, 0x00, 0x00,
    ]);
    let params = &func(&types, 0).params;
    assert_eq!(params[0], ValType::Ref(RefType::concrete(true, TypeIdx(1))));
}

#[test]
fn reject_invalid_mutability_byte() {
    let err = decode_err(&[0x01, 0x5F, 0x01, 0x7F, 0x02]);
    assert!(
        format!("{:?}", err.kind).contains("InvalidMutability"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn reject_unknown_heaptype_byte() {
    let err = decode_err(&[0x01, 0x60, 0x01, 0x63, 0x68, 0x00]);
    assert!(
        format!("{:?}", err.kind).contains("UnknownRefType"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn reject_bad_packtype_byte() {
    let err = decode_err(&[0x01, 0x5F, 0x01, 0x79, 0x00]);
    assert!(
        format!("{:?}", err.kind).contains("UnknownValType"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn reject_rec_group_trailing_garbage() {
    let err = decode_err(&[0x01, 0x4E, 0x00, 0xFF]);
    assert!(
        format!("{:?}", err.kind).contains("SectionSizeMismatch"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn reject_sub_vec_then_bad_comptype() {
    let err = decode_err(&[0x01, 0x50, 0x00, 0x61]);
    assert!(
        format!("{:?}", err.kind).contains("UnexpectedByte"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn reject_negative_concrete_heaptype_index() {
    // Reviewer reproduction: `63 f0 7f` is `(ref null <s33 -16>)`. A negative
    // concrete index is malformed; it must not be silently widened to 16368.
    let err = decode_err(&[0x01, 0x60, 0x01, 0x63, 0xf0, 0x7f, 0x00]);
    assert!(
        format!("{:?}", err.kind).contains("UnknownRefType"),
        "unexpected error: {:?}",
        err.kind
    );
}

#[test]
fn decode_multi_byte_concrete_heaptype_index() {
    // A non-negative multi-byte s33 concrete index decodes normally: `80 01`
    // encodes 128 (0x80 low 7 bits = 0, next byte 0x01 shifted << 7 = 128).
    let types = decode_ok(&[0x01, 0x60, 0x01, 0x63, 0x80, 0x01, 0x00]);
    let params = &func(&types, 0).params;
    assert_eq!(
        params[0],
        ValType::Ref(RefType::concrete(true, TypeIdx(128)))
    );
}

/// Review r3b P1: the pinned `s33` reader wraps `-2^32` to 0, so the
/// 5-byte encoding `80 80 80 80 70` is a concrete reference to type index 0.
#[test]
fn decode_wrapped_wide_heaptype_index_zero() {
    let types = decode_ok(&[0x01, 0x60, 0x01, 0x63, 0x80, 0x80, 0x80, 0x80, 0x70, 0x00]);
    let params = &func(&types, 0).params;
    assert_eq!(params[0], ValType::Ref(RefType::concrete(true, TypeIdx(0))));
}

/// Review r3b P1: a func param `(ref null 0)` built from the wide wrapping
/// encoding `63 80 80 80 80 70` must VALIDATE as referencing type index 0 when
/// a type 0 exists (it must not be rejected as malformed).
#[test]
fn validate_wrapped_wide_heaptype_index_zero() {
    let bytes = module_with_type_section(&[
        0x02, // two types
        0x5F, 0x01, 0x7F, 0x00, // type 0: (struct (field i32))
        0x60, 0x01, 0x63, 0x80, 0x80, 0x80, 0x80, 0x70,
        0x00, // type 1: (func (param (ref null 0)))
    ]);
    let module = Module::decode(&bytes).expect("wide wrapped index must decode");
    module.validate().expect("wide wrapped index must validate");
    let ty = module.func_type(TypeIdx(1)).expect("type 1 is a func type");
    assert_eq!(
        ty.params[0],
        ValType::Ref(RefType::concrete(true, TypeIdx(0)))
    );
}
