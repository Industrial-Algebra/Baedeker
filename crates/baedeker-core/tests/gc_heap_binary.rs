// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2: `0xFB`-prefixed instruction decoding.
//!
//! Covers every row of the reference decoder's GC opcode table
//! (`interpreter/binary/decode.ml`) plus the malformed variants the decoder
//! must reject. Cast/conversion sub-opcodes are U3's scope and decode as
//! unknown opcodes here.

use baedeker_core::binary::instr::{Instr, decode_instr};
use baedeker_core::binary::leb128::Cursor;
use baedeker_core::error::DecodeErrorKind;
use baedeker_core::types::{DataIdx, ElemIdx, TypeIdx};

fn decode(bytes: &[u8]) -> Result<Instr, baedeker_core::error::DecodeError> {
    let mut cursor = Cursor::new(bytes);
    decode_instr(&mut cursor, 0)
}

#[test]
fn decode_struct_ops() {
    assert_eq!(
        decode(&[0xFB, 0x00, 0x03]).unwrap(),
        Instr::StructNew(TypeIdx(3))
    );
    assert_eq!(
        decode(&[0xFB, 0x01, 0x03]).unwrap(),
        Instr::StructNewDefault(TypeIdx(3))
    );
    assert_eq!(
        decode(&[0xFB, 0x02, 0x03, 0x01]).unwrap(),
        Instr::StructGet {
            type_idx: TypeIdx(3),
            field: 1
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x03, 0x03, 0x01]).unwrap(),
        Instr::StructGetS {
            type_idx: TypeIdx(3),
            field: 1
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x04, 0x03, 0x01]).unwrap(),
        Instr::StructGetU {
            type_idx: TypeIdx(3),
            field: 1
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x05, 0x03, 0x02]).unwrap(),
        Instr::StructSet {
            type_idx: TypeIdx(3),
            field: 2
        }
    );
}

#[test]
fn decode_array_new_ops() {
    assert_eq!(
        decode(&[0xFB, 0x06, 0x07]).unwrap(),
        Instr::ArrayNew(TypeIdx(7))
    );
    assert_eq!(
        decode(&[0xFB, 0x07, 0x07]).unwrap(),
        Instr::ArrayNewDefault(TypeIdx(7))
    );
    assert_eq!(
        decode(&[0xFB, 0x08, 0x07, 0x04]).unwrap(),
        Instr::ArrayNewFixed {
            type_idx: TypeIdx(7),
            count: 4
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x09, 0x07, 0x02]).unwrap(),
        Instr::ArrayNewData {
            type_idx: TypeIdx(7),
            data: DataIdx(2)
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x0A, 0x07, 0x02]).unwrap(),
        Instr::ArrayNewElem {
            type_idx: TypeIdx(7),
            elem: ElemIdx(2)
        }
    );
}

#[test]
fn decode_array_access_ops() {
    assert_eq!(
        decode(&[0xFB, 0x0B, 0x07]).unwrap(),
        Instr::ArrayGet(TypeIdx(7))
    );
    assert_eq!(
        decode(&[0xFB, 0x0C, 0x07]).unwrap(),
        Instr::ArrayGetS(TypeIdx(7))
    );
    assert_eq!(
        decode(&[0xFB, 0x0D, 0x07]).unwrap(),
        Instr::ArrayGetU(TypeIdx(7))
    );
    assert_eq!(
        decode(&[0xFB, 0x0E, 0x07]).unwrap(),
        Instr::ArraySet(TypeIdx(7))
    );
    assert_eq!(decode(&[0xFB, 0x0F]).unwrap(), Instr::ArrayLen);
    assert_eq!(
        decode(&[0xFB, 0x10, 0x07]).unwrap(),
        Instr::ArrayFill(TypeIdx(7))
    );
    assert_eq!(
        decode(&[0xFB, 0x11, 0x07, 0x08]).unwrap(),
        Instr::ArrayCopy {
            dst: TypeIdx(7),
            src: TypeIdx(8)
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x12, 0x07, 0x02]).unwrap(),
        Instr::ArrayInitData {
            type_idx: TypeIdx(7),
            data: DataIdx(2)
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x13, 0x07, 0x02]).unwrap(),
        Instr::ArrayInitElem {
            type_idx: TypeIdx(7),
            elem: ElemIdx(2)
        }
    );
}

#[test]
fn decode_i31_ops() {
    assert_eq!(decode(&[0xFB, 0x1C]).unwrap(), Instr::RefI31);
    assert_eq!(decode(&[0xFB, 0x1D]).unwrap(), Instr::I31GetS);
    assert_eq!(decode(&[0xFB, 0x1E]).unwrap(), Instr::I31GetU);
}

#[test]
fn decode_multibyte_type_index_leb() {
    // typeidx 300 = 0xAC 0x02.
    assert_eq!(
        decode(&[0xFB, 0x00, 0xAC, 0x02]).unwrap(),
        Instr::StructNew(TypeIdx(300))
    );
}

#[test]
fn struct_set_missing_immediates_is_eof() {
    // 0xFB 0x05 needs a typeidx and a field index.
    let err = decode(&[0xFB, 0x05, 0x00]).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnexpectedEof);
}

#[test]
fn struct_get_missing_field_index_is_eof() {
    let err = decode(&[0xFB, 0x02, 0x00]).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnexpectedEof);
}

#[test]
fn reserved_cast_subopcodes_are_unknown() {
    // 0x14–0x1B are casts/conversions (U3).
    for sub in [0x14u8, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B] {
        let err = decode(&[0xFB, sub]).unwrap_err();
        assert_eq!(
            err.kind,
            DecodeErrorKind::UnknownOpcode { byte: 0xFB },
            "sub-opcode {sub:#04x} must be an unknown opcode"
        );
    }
}

#[test]
fn unknown_gc_subopcode_is_unknown() {
    let err = decode(&[0xFB, 0x1F]).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnknownOpcode { byte: 0xFB });
}

#[test]
fn bad_leb_type_index_rejected() {
    // A 5-byte LEB with continuation bits set overflows.
    let err = decode(&[0xFB, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]).unwrap_err();
    assert!(matches!(
        err.kind,
        DecodeErrorKind::Leb128TooLong | DecodeErrorKind::Leb128Overflow
    ));
}

#[test]
fn array_new_fixed_count_is_u32_leb() {
    // count 0x80 0x01 = 128.
    assert_eq!(
        decode(&[0xFB, 0x08, 0x00, 0x80, 0x01]).unwrap(),
        Instr::ArrayNewFixed {
            type_idx: TypeIdx(0),
            count: 128
        }
    );
}

#[test]
fn gc_opcode_decodes_through_module_instruction_sequence() {
    // A whole function body: `ref.i31; end` decodes as a sequence.
    let instrs = baedeker_core::binary::instr::decode_instr_sequence(&[0xFB, 0x1C, 0x0B], 0)
        .expect("sequence decodes");
    assert_eq!(instrs, vec![Instr::RefI31, Instr::End]);
}
