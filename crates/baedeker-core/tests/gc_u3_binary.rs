// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3: `ref.test`/`ref.cast`/`br_on_cast`/`ref.eq`/conversion binary
//! decoding, pinned to `interpreter/binary/decode.ml`.
//!
//! Covers all six instruction shapes × null forms, the `br_on_cast` flags
//! byte (`flags & 0xfc == 0` required), the two-heaptype operand order, and
//! the core-space `ref.eq` opcode `0xd3`.

use baedeker_core::binary::instr::{Instr, decode_instr, decode_instr_sequence};
use baedeker_core::binary::leb128::Cursor;
use baedeker_core::error::DecodeErrorKind;
use baedeker_core::types::{BlockType, HeapType, LabelIdx, RefType, TypeIdx, ValType};

fn decode(bytes: &[u8]) -> Result<Instr, baedeker_core::error::DecodeError> {
    let mut cursor = Cursor::new(bytes);
    decode_instr(&mut cursor, 0)
}

#[test]
fn decode_ref_test_null_forms() {
    assert_eq!(
        decode(&[0xFB, 0x14, 0x70]).unwrap(),
        Instr::RefTest {
            nullable: false,
            ht: HeapType::Func
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x15, 0x70]).unwrap(),
        Instr::RefTest {
            nullable: true,
            ht: HeapType::Func
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x14, 0x03]).unwrap(),
        Instr::RefTest {
            nullable: false,
            ht: HeapType::Type(TypeIdx(3))
        }
    );
}

#[test]
fn decode_ref_cast_null_forms() {
    assert_eq!(
        decode(&[0xFB, 0x16, 0x6E]).unwrap(),
        Instr::RefCast {
            nullable: false,
            ht: HeapType::Any
        }
    );
    assert_eq!(
        decode(&[0xFB, 0x17, 0x6E]).unwrap(),
        Instr::RefCast {
            nullable: true,
            ht: HeapType::Any
        }
    );
}

#[test]
fn decode_ref_eq_core_opcode() {
    // ref.eq lives in the core reference space, not behind 0xFB.
    assert_eq!(decode(&[0xD3]).unwrap(), Instr::RefEq);
}

#[test]
fn decode_conversions() {
    assert_eq!(decode(&[0xFB, 0x1A]).unwrap(), Instr::AnyConvertExtern);
    assert_eq!(decode(&[0xFB, 0x1B]).unwrap(), Instr::ExternConvertAny);
}

#[test]
fn decode_br_on_cast_flags_bit0_bit1() {
    // flags 0b00: src NoNull, tgt NoNull.
    assert_eq!(
        decode(&[0xFB, 0x18, 0x00, 0x00, 0x70, 0x6F]).unwrap(),
        Instr::BrOnCast {
            fail: false,
            src: RefType::from_parts(false, HeapType::Func),
            tgt: RefType::from_parts(false, HeapType::Extern),
            label: LabelIdx(0),
        }
    );
    // flags 0b01: src Null.
    assert_eq!(
        decode(&[0xFB, 0x18, 0x01, 0x00, 0x70, 0x6F]).unwrap(),
        Instr::BrOnCast {
            fail: false,
            src: RefType::FuncRef,
            tgt: RefType::from_parts(false, HeapType::Extern),
            label: LabelIdx(0),
        }
    );
    // flags 0b10: tgt Null.
    assert_eq!(
        decode(&[0xFB, 0x18, 0x02, 0x00, 0x70, 0x6F]).unwrap(),
        Instr::BrOnCast {
            fail: false,
            src: RefType::from_parts(false, HeapType::Func),
            tgt: RefType::ExternRef,
            label: LabelIdx(0),
        }
    );
    // flags 0b11: both Null.
    assert_eq!(
        decode(&[0xFB, 0x18, 0x03, 0x00, 0x70, 0x6F]).unwrap(),
        Instr::BrOnCast {
            fail: false,
            src: RefType::FuncRef,
            tgt: RefType::ExternRef,
            label: LabelIdx(0),
        }
    );
}

#[test]
fn decode_br_on_cast_fail() {
    assert_eq!(
        decode(&[0xFB, 0x19, 0x00, 0x00, 0x70, 0x6F]).unwrap(),
        Instr::BrOnCast {
            fail: true,
            src: RefType::from_parts(false, HeapType::Func),
            tgt: RefType::from_parts(false, HeapType::Extern),
            label: LabelIdx(0),
        }
    );
}

#[test]
fn decode_br_on_cast_two_heaptype_operand_order() {
    // Source heaptype comes first, target second, with a non-zero label.
    assert_eq!(
        decode(&[0xFB, 0x18, 0x00, 0x07, 0x01, 0x6F]).unwrap(),
        Instr::BrOnCast {
            fail: false,
            src: RefType::concrete(false, TypeIdx(1)),
            tgt: RefType::from_parts(false, HeapType::Extern),
            label: LabelIdx(7),
        }
    );
}

#[test]
fn decode_br_on_cast_reserved_flags_rejected() {
    // Bit 2 and any high bits are malformed (`flags & 0xfc == 0` required).
    for flags in [0x04u8, 0x08, 0x10, 0x20, 0x40, 0x80, 0xFF] {
        let err = decode(&[0xFB, 0x18, flags, 0x00, 0x70, 0x6F]).unwrap_err();
        assert_eq!(
            err.kind,
            DecodeErrorKind::MalformedCastFlags { flags },
            "flags {flags:#04x} must be rejected"
        );
        let err = decode(&[0xFB, 0x19, flags, 0x00, 0x70, 0x6F]).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::MalformedCastFlags { flags });
    }
}

#[test]
fn decode_br_on_cast_missing_heaptype_is_eof() {
    let err = decode(&[0xFB, 0x18, 0x00, 0x00, 0x70]).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnexpectedEof);
}

#[test]
fn new_opcodes_decode_in_instruction_sequence() {
    let instrs = decode_instr_sequence(
        &[
            0xFB, 0x14, 0x70, // ref.test (ref func)
            0xFB, 0x15, 0x6E, // ref.test (ref null any)
            0xFB, 0x16, 0x6A, // ref.cast (ref array)
            0xFB, 0x17, 0x6B, // ref.cast (ref null struct)
            0xFB, 0x18, 0x03, 0x00, 0x70, 0x6F, // br_on_cast
            0xFB, 0x19, 0x01, 0x00, 0x70, 0x6F, // br_on_cast_fail
            0xFB, 0x1A, // any.convert_extern
            0xFB, 0x1B, // extern.convert_any
            0xD3, // ref.eq
            0x0B, // end
        ],
        0,
    )
    .expect("sequence decodes");
    assert_eq!(instrs.len(), 10);
    assert_eq!(
        instrs[0],
        Instr::RefTest {
            nullable: false,
            ht: HeapType::Func
        }
    );
    assert_eq!(instrs[8], Instr::RefEq);
    assert_eq!(instrs[9], Instr::End);
}

#[test]
fn new_opcodes_roundtrip_through_wasm_encoding() {
    // A real module exercises the same opcodes end-to-end: WAT → wast
    // encode → our decode.
    let wat = r#"
(module
  (type $s (sub (struct (field i32))))
  (type $t (sub $s (struct (field i32) (field i32))))
  (func (param (ref null $s)) (result i32)
    (block $hit (result (ref null $t))
      local.get 0
      br_on_cast $hit (ref null $s) (ref null $t)
      drop
      ref.null $t)
    ref.test (ref $s))
  (func (result i32)
    i32.const 1 ref.i31
    i32.const 1 ref.i31
    ref.eq)
  (func (param externref) (result externref)
    local.get 0
    any.convert_extern
    extern.convert_any))"#;
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = parsed.encode().unwrap();
    let module = baedeker_core::binary::module::Module::decode(&bytes).expect("module decodes");
    // Every function body decodes without error and the new shapes appear.
    let mut saw_cast = false;
    let mut saw_br = false;
    let mut saw_eq = false;
    let mut saw_conversions = 0;
    for code in module.codes() {
        for instr in code.instructions().unwrap() {
            match instr {
                Instr::RefTest { .. } | Instr::RefCast { .. } => saw_cast = true,
                Instr::BrOnCast { .. } => saw_br = true,
                Instr::RefEq => saw_eq = true,
                Instr::AnyConvertExtern | Instr::ExternConvertAny => saw_conversions += 1,
                _ => {}
            }
        }
    }
    assert!(saw_cast, "ref.test/ref.cast present");
    assert!(saw_br, "br_on_cast present");
    assert!(saw_eq, "ref.eq present");
    assert_eq!(saw_conversions, 2, "both conversions present");
}

// ── FIX 2: block-type shorthand accepts every GC reference type ──────
//
// `parse_block_type` must accept the same single-byte reftype shorthands the
// U1 valtype grammar accepts (`typeparser.rs` `parse_val_type_with_first_byte`):
// `0x63`/`0x64` (`ref`/`ref null`) plus every abstract heap-type byte
// `0x69..=0x74`. Before the fix the discriminator only admitted `0x69` and
// `0x6f`/`0x70`, so e.g. `(block (result (ref null any))` — encoded by the
// toolchain as the `0x6e` shorthand — was misread as a type-index LEB and
// failed with `Leb128Overflow`.

/// The pinned abstract heap-type byte table (`typeparser.rs`).
const ABSTRACT_HEAP_BYTES: &[(u8, HeapType)] = &[
    (0x74, HeapType::NoExn),
    (0x73, HeapType::NoFunc),
    (0x72, HeapType::NoExtern),
    (0x71, HeapType::None),
    (0x70, HeapType::Func),
    (0x6F, HeapType::Extern),
    (0x6E, HeapType::Any),
    (0x6D, HeapType::Eq),
    (0x6C, HeapType::I31),
    (0x6B, HeapType::Struct),
    (0x6A, HeapType::Array),
    (0x69, HeapType::Exn),
];

#[test]
fn block_accepts_every_gc_reftype_shorthand() {
    for &(byte, heap) in ABSTRACT_HEAP_BYTES {
        let expected = BlockType::Val(ValType::Ref(RefType::from_parts(true, heap)));
        assert_eq!(
            decode(&[0x02, byte]).unwrap(),
            Instr::Block(expected),
            "block result shorthand byte {byte:#04x}"
        );
    }
}

#[test]
fn loop_accepts_every_gc_reftype_shorthand() {
    for &(byte, heap) in ABSTRACT_HEAP_BYTES {
        let expected = BlockType::Val(ValType::Ref(RefType::from_parts(true, heap)));
        assert_eq!(
            decode(&[0x03, byte]).unwrap(),
            Instr::Loop(expected),
            "loop result shorthand byte {byte:#04x}"
        );
    }
}

#[test]
fn block_accepts_ref_and_ref_null_heaptype_forms() {
    // `0x63 ht` = `(ref null ht)`, `0x64 ht` = `(ref ht)`.
    assert_eq!(
        decode(&[0x02, 0x63, 0x6E]).unwrap(),
        Instr::Block(BlockType::Val(ValType::Ref(RefType::from_parts(
            true,
            HeapType::Any
        ))))
    );
    assert_eq!(
        decode(&[0x02, 0x64, 0x6B]).unwrap(),
        Instr::Block(BlockType::Val(ValType::Ref(RefType::from_parts(
            false,
            HeapType::Struct
        ))))
    );
}

#[test]
fn block_result_anyref_does_not_decode_as_type_index() {
    // The reviewer's failure byte: `0x6e` must be the anyref shorthand, not
    // the start of a concrete type index.
    let decoded = decode(&[0x02, 0x6E]).unwrap();
    assert_eq!(
        decoded,
        Instr::Block(BlockType::Val(ValType::Ref(RefType::from_parts(
            true,
            HeapType::Any
        ))))
    );
    // A genuinely encoded concrete type index still decodes as an index
    // (not mistaken for a shorthand byte).
    assert_eq!(
        decode(&[0x02, 0x00]).unwrap(),
        Instr::Block(BlockType::TypeIdx(0))
    );
    assert_eq!(
        decode(&[0x02, 0x05]).unwrap(),
        Instr::Block(BlockType::TypeIdx(5))
    );
}
