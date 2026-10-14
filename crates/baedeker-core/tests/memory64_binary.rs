// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! memory64 unit 1: decode-side binary surface for 64-bit limits and
//! memarg offsets.
//!
//! Every byte string is computed twice: once from the section-framing table
//! (id, LEB size, vec count, entry) and once by hand-decoding each LEB field.
//! The contract lists section contents without the linking LEB size byte; the
//! size byte is restored here and the count is kept explicit.

use baedeker_core::binary::instr::{Instr, decode_instr_sequence};
use baedeker_core::binary::module::Module;
use baedeker_core::error::DecodeErrorKind;
use baedeker_core::types::{DataMode, ImportDesc, Limits};

#[test]
fn memory64_min_only_decodes() {
    // memory section (id 5) — contents: 01 count, 04 i64-min-only flag, 01 min.
    // Contents are 3 bytes, so the section size LEB is 03.
    let bytes: [u8; 13] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x05, 0x03, 0x01, 0x04, 0x01, // memory section
    ];
    let module = Module::decode(&bytes).unwrap();
    assert_eq!(module.memories().len(), 1);
    assert_eq!(
        module.memories()[0].limits,
        Limits {
            min: 1,
            max: None,
            is64: true,
        }
    );
}

#[test]
fn memory64_min_max_decodes() {
    // contents: 01 count, 05 i64 min+max flag, 01 min, 0a max. 4 bytes → size 04.
    let bytes: [u8; 14] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x05, 0x04, 0x01, 0x05, 0x01, 0x0a, // memory section
    ];
    let module = Module::decode(&bytes).unwrap();
    assert_eq!(module.memories().len(), 1);
    assert_eq!(
        module.memories()[0].limits,
        Limits {
            min: 1,
            max: Some(10),
            is64: true,
        }
    );
}

#[test]
fn table64_decodes() {
    // table section (id 4) — contents: 01 count, 70 funcref, 04 i64 flag,
    // 02 min. The table entry encodes the reference type *before* the limits,
    // so the bytes are 70 04 02 (the contract lists them as 04 02 70).
    // 4 bytes → size 04.
    let bytes: [u8; 14] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x04, 0x04, 0x01, 0x70, 0x04, 0x02, // table section
    ];
    let module = Module::decode(&bytes).unwrap();
    assert_eq!(module.tables().len(), 1);
    assert_eq!(
        module.tables()[0].limits,
        Limits {
            min: 2,
            max: None,
            is64: true,
        }
    );
}

#[test]
fn imported_memory64_decodes() {
    // import section (id 2) — contents: 01 count, 01 61 module "a",
    // 01 6d name "m", 02 memory kind, 04 i64 flag, 01 min.
    // 8 bytes → size 08.
    let bytes: [u8; 18] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x02, 0x08, 0x01, 0x01, 0x61, 0x01, 0x6d, 0x02, 0x04, 0x01, // import section
    ];
    let module = Module::decode(&bytes).unwrap();
    assert_eq!(module.imports().len(), 1);
    assert_eq!(module.imports()[0].module, "a");
    assert_eq!(module.imports()[0].name, "m");
    let ImportDesc::Mem(mem) = module.imports()[0].desc else {
        panic!("expected memory import");
    };
    assert_eq!(
        mem.limits,
        Limits {
            min: 1,
            max: None,
            is64: true,
        }
    );
}

#[test]
fn i64_data_offset_decodes() {
    // header
    // memory section (id 5, size 03): 01 count, 04 i64 flag, 01 min
    // data section (id 0x0b, size 07): 01 count, 00 active mem 0,
    //   offset expr 42 00 0b (i64.const 0; end), 01 init len, 78 one byte.
    let bytes: [u8; 22] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x05, 0x03, 0x01, 0x04, 0x01, // memory section: one i64 memory, min 1
        0x0b, 0x07, 0x01, 0x00, 0x42, 0x00, 0x0b, 0x01, 0x78, // data section
    ];
    let module = Module::decode(&bytes).unwrap();
    assert_eq!(module.data().len(), 1);
    let DataMode::Active {
        offset_expr,
        memory,
        ..
    } = module.data()[0].mode
    else {
        panic!("expected an active data segment");
    };
    assert_eq!(memory.0, 0);
    assert_eq!(offset_expr, &[0x42, 0x00, 0x0b]);
    let instrs = decode_instr_sequence(offset_expr, 0).unwrap();
    assert!(instrs.contains(&Instr::I64Const(0)));
}

#[test]
fn malformed_flag_0x06_is_rejected() {
    // memory section: 01 count, 06 unsupported flag, 01 min. 3 bytes → size 03.
    let bytes: [u8; 13] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x05, 0x03, 0x01, 0x06, 0x01, // memory section
    ];
    let err = Module::decode(&bytes).unwrap_err();
    assert!(matches!(
        err.kind,
        DecodeErrorKind::UnexpectedByte {
            expected: 0x00,
            found: 0x06,
        }
    ));
}

#[test]
fn malformed_eleven_byte_u64_leb_rejected() {
    // memory section: 01 count, 04 i64 flag, then an 11-byte LEB min
    // (ten 0x80 continuation bytes followed by 0x00). Contents are 13 bytes,
    // so the section size LEB is 0x0d.
    let bytes: [u8; 23] = [
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x05, 0x0d, 0x01, 0x04, // memory section header + vector/flag
        0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00, // LEB min
    ];
    let err = Module::decode(&bytes).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::Leb128Overflow);
}

#[test]
fn memarg_u64_offset_decodes() {
    // memory64 with a load offset of 2^32 — larger than a u32 can hold, so the
    // memarg offset must be decoded as a u64 LEB.
    let wat = r#"(module
        (memory i64 1)
        (func (export "load") (param i64) (result i64)
            (i64.load offset=4294967296 (local.get 0))))"#;
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = parsed.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    let instrs = module.codes()[0].instructions().unwrap();
    let offset = instrs
        .iter()
        .find_map(|instr| match instr {
            Instr::I64Load(memarg) => Some(memarg.offset),
            _ => None,
        })
        .expect("i64.load present");
    assert_eq!(offset, 0x1_0000_0000);
}
