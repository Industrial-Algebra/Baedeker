// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! EH unit 1: binary + types surface for wasm 3.0 exception handling.
//!
//! Hand-built byte vectors covering the tag section, the `exnref` value type,
//! and the `try_table` / `throw` / `throw_ref` instructions.

use baedeker_core::binary::instr::{
    Instr, TryTable, TryTableCatch, TryTableCatchKind, decode_instr,
};
use baedeker_core::binary::leb128::Cursor;
use baedeker_core::binary::module::Module;
use baedeker_core::binary::typeparser::parse_val_type;
use baedeker_core::error::{DecodeContext, DecodeError, DecodeErrorKind};
use baedeker_core::types::{
    BlockType, ExportDesc, HeapType, ImportDesc, RefType, TagIdx, TypeIdx, ValType,
};
use baedeker_core::validate::error::ValidationErrorKind;

/// Decode a WASM binary into a module — the same entry the other integration
/// tests use.
fn decode<'a>(bytes: &'a [u8]) -> Result<Module<'a>, DecodeError> {
    Module::decode(bytes)
}

#[test]
fn tag_section_decodes() {
    let bytes = [
        0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x05, 0x01, 0x60, 0x01, 0x7F, 0x00, // type: (func (param i32))
        0x0D, 0x03, 0x01, 0x00, 0x00, // tag: 1 entry, attr 0, typeidx 0
    ];
    let module = decode(&bytes).unwrap();
    assert_eq!(module.tags, vec![TypeIdx(0)]);
}

#[test]
fn tag_section_rejects_nonzero_attribute() {
    let bytes = [
        0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x05, 0x01, 0x60, 0x01, 0x7F, 0x00, // type: (func (param i32))
        0x0D, 0x03, 0x01, 0x01, 0x00, // tag: attr 0x01 (invalid)
    ];
    let err = decode(&bytes).unwrap_err();
    assert_eq!(
        err.kind,
        DecodeErrorKind::InvalidTagAttribute { byte: 0x01 }
    );
}

#[test]
fn tag_section_order_enforced() {
    let bytes = [
        0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
        0x06, 0x06, 0x01, 0x7F, 0x00, 0x41, 0x00, 0x0B, // global: i32 const 0
        0x0D, 0x03, 0x01, 0x00, 0x00, // tag — must precede global
    ];
    let err = decode(&bytes).unwrap_err();
    assert_eq!(
        err.kind,
        DecodeErrorKind::SectionOutOfOrder {
            prev: 6,
            current: 13,
        }
    );
}

#[test]
fn exnref_valtype_parses() {
    let nullable = ValType::Ref(RefType::Typed {
        nullable: true,
        heap: HeapType::Exn,
    });
    let non_null = ValType::Ref(RefType::Typed {
        nullable: false,
        heap: HeapType::Exn,
    });

    let mut cursor = Cursor::new(&[0x69]);
    assert_eq!(
        parse_val_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap(),
        nullable
    );

    let mut cursor = Cursor::new(&[0x63, 0x69]);
    assert_eq!(
        parse_val_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap(),
        nullable
    );

    let mut cursor = Cursor::new(&[0x64, 0x69]);
    assert_eq!(
        parse_val_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap(),
        non_null
    );
}

#[test]
fn throw_and_try_table_decode() {
    let mut cursor = Cursor::new(&[0x08, 0x00]);
    assert_eq!(
        decode_instr(&mut cursor, 0).unwrap(),
        Instr::Throw(TagIdx(0))
    );

    let mut cursor = Cursor::new(&[0x0A]);
    assert_eq!(decode_instr(&mut cursor, 0).unwrap(), Instr::ThrowRef);

    let mut cursor = Cursor::new(&[0x1F, 0x40, 0x02, 0x00, 0x00, 0x01, 0x03, 0x02]);
    assert_eq!(
        decode_instr(&mut cursor, 0).unwrap(),
        Instr::TryTable(TryTable {
            block_type: BlockType::Empty,
            catches: vec![
                TryTableCatch {
                    kind: TryTableCatchKind::Catch,
                    tag: Some(TagIdx(0)),
                    label: 1,
                },
                TryTableCatch {
                    kind: TryTableCatchKind::CatchAllRef,
                    tag: None,
                    label: 2,
                },
            ],
        })
    );

    let mut cursor = Cursor::new(&[0x1F, 0x40, 0x01, 0x04, 0x00, 0x00]);
    let err = decode_instr(&mut cursor, 0).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnknownOpcode { byte: 0x04 });
}

#[test]
fn tag_import_export_decode() {
    let bytes = [
        0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
        0x02, 0x06, 0x01, 0x00, 0x00, 0x04, 0x00, 0x00, // import "" "" tag type 0
        0x07, 0x05, 0x01, 0x01, 0x74, 0x04, 0x00, // export "t" tag 0
    ];
    let module = decode(&bytes).unwrap();
    assert_eq!(module.imports[0].desc, ImportDesc::Tag(TypeIdx(0)));
    assert_eq!(module.exports[0].desc, ExportDesc::Tag(TagIdx(0)));
}

#[test]
fn legacy_eh_opcodes_stay_unknown() {
    for byte in [0x06u8, 0x07, 0x09, 0x18, 0x19] {
        let data = [byte];
        let mut cursor = Cursor::new(&data);
        let err = decode_instr(&mut cursor, 0).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnknownOpcode { byte });
    }
}

#[test]
fn validation_rejects_eh_for_now() {
    let bytes = [
        0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
        0x03, 0x02, 0x01, 0x00, // function 0 references type 0
        0x0D, 0x03, 0x01, 0x00, 0x00, // tag 0
        0x0A, 0x06, 0x01, 0x04, 0x00, 0x08, 0x00, 0x0B, // code: throw 0; end
    ];
    let module = decode(&bytes).unwrap();
    let err = module.validate().unwrap_err();
    assert_eq!(
        err.kind,
        ValidationErrorKind::Unsupported {
            feature: "exception handling",
        }
    );
}
