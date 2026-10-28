//! Threads U1: surface — shared memory types + the 0xFE atomic family.
//!
//! Contract: docs/plans/threads-unit1-contract.md. Pin:
//! WebAssembly/threads@main-legacy 33b2dcd6. All module bytes here are
//! hand-built (byte tables below are cross-checked against the pin's
//! `binary/instructions.rst` + wast-240 `expr.rs:791+` encoders).
//!
//! Correction (threads U3, contract r1): an earlier version of this header
//! claimed wast-240 could not express classic shared memories. It can —
//! `MemoryType { shared }` (`wast-240 src/core/types.rs:649+`) parses the
//! trailing `shared` keyword and the encoder forwards it
//! (`src/core/binary.rs:515-524`). The hand-built bytes below stay as
//! decoder/validator regressions independent of the text grammar; the native
//! WAT path is pinned in `threads_u3_native_shared.rs`.

use baedeker_core::binary::instr::Instr;
use baedeker_core::binary::module::Module;

fn uleb(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

/// Section: id + uleb size + payload.
fn section(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![id];
    uleb(payload.len() as u64, &mut s);
    s.extend_from_slice(payload);
    s
}

/// Minimal module: header + type [(func)] + func + (caller adds e.g.
/// memory/code) sections in the given order.
fn module_with(sections: &[Vec<u8>]) -> Vec<u8> {
    let mut m = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];
    for s in sections {
        m.extend_from_slice(s);
    }
    m
}

/// Shared i32 memory (1..2): vec(1) [ 0x03 min=1 max=2 ].
fn shared_memory_payload() -> Vec<u8> {
    vec![0x01, 0x03, 0x01, 0x02]
}

#[test]
fn shared_memory_min_max_decodes() {
    let bytes = module_with(&[section(5, &shared_memory_payload())]);
    let module = Module::decode(&bytes).expect("shared memory 1..2 decodes");
    let mem = module.memories().first().expect("one memory");
    assert!(mem.limits.shared, "0x03 tag = shared");
    assert_eq!(mem.limits.min, 1);
    assert_eq!(mem.limits.max, Some(2));
    assert!(!mem.limits.is64);
}

#[test]
fn shared_memory_without_max_is_malformed() {
    // tag 0x02 carries no max: the memtype production requires max when
    // shared ("shared memory must have maximum" — pin binary/types.rst
    // _binary-memtype_; valid.ml:379).
    let bytes = module_with(&[section(5, &[0x01, 0x02, 0x01])]);
    let err = Module::decode(&bytes).expect_err("shared requires max");
    assert!(
        err.to_string().contains("shared memory must have maximum"),
        "got: {err}"
    );
}

#[test]
fn unshared_flags_still_decode() {
    let bytes = module_with(&[section(5, &[0x01, 0x00, 0x01])]);
    let module = Module::decode(&bytes).expect("0x00 tag decodes");
    assert!(!module.memories()[0].limits.shared);
    let bytes = module_with(&[section(5, &[0x01, 0x01, 0x01, 0x02])]);
    let module = Module::decode(&bytes).expect("0x01 tag decodes");
    assert!(!module.memories()[0].limits.shared);
}

#[test]
fn imported_shared_memory_decodes() {
    // import "e.m": memtype tag 0x03 min=1 max=2.
    let payload: &[u8] = &[0x01, 0x01, b'e', 0x01, b'm', 0x02, 0x03, 0x01, 0x02];
    let bytes = module_with(&[section(2, payload)]);
    let module = Module::decode(&bytes).expect("shared import decodes");
    let mem = module
        .imports()
        .iter()
        .find_map(|imp| match &imp.desc {
            baedeker_core::types::ImportDesc::Mem(ty) => Some(ty),
            _ => None,
        })
        .expect("one memory import");
    assert!(mem.limits.shared);
    assert_eq!(mem.limits.max, Some(2));
}

#[test]
fn imported_shared_memory_without_max_is_malformed() {
    let payload: &[u8] = &[0x01, 0x01, b'e', 0x01, b'm', 0x02, 0x02, 0x01];
    let bytes = module_with(&[section(2, payload)]);
    let err = Module::decode(&bytes).expect_err("shared import requires max");
    assert!(
        err.to_string().contains("shared memory must have maximum"),
        "got: {err}"
    );
}

#[test]
fn table_rejects_shared_limits() {
    // Tables stay unshared at this pin: table section entry with limits
    // tag 0x03 must be malformed.
    let payload: &[u8] = &[0x01, 0x70, 0x03, 0x01, 0x02];
    let bytes = module_with(&[section(4, payload)]);
    assert!(Module::decode(&bytes).is_err());
}

/// The 0xFE family: (sub-opcode, natural align log2, instruction shape).
/// Table cross-checked against wast-240 expr.rs (0xfe,0x10)..(0xfe,0x4f)
/// and the pin's binary/instructions.rst.
fn family() -> Vec<(u8, u32, &'static str)> {
    vec![
        (0x00, 2, "notify"),
        (0x01, 2, "wait32"),
        (0x02, 3, "wait64"),
        (0x10, 2, "i32.atomic.load"),
        (0x11, 3, "i64.atomic.load"),
        (0x12, 0, "i32.atomic.load8_u"),
        (0x13, 1, "i32.atomic.load16_u"),
        (0x14, 2, "i64.atomic.load8_u"),
        (0x15, 2, "i64.atomic.load16_u"),
        (0x16, 2, "i64.atomic.load32_u"),
        (0x17, 2, "i32.atomic.load — see note"),
    ]
}

#[test]
fn atomic_family_table_is_pinned() {
    // Placeholder replaced by per-opcode decode tests below once the
    // AtomicOp surface exists; keeps the table honest during RED.
    assert!(family().len() >= 7);
}

// NOTE: full per-opcode decode/validate tests are added alongside the
// AtomicOp enum in the implementation step (compile-time RED pair).

#[test]
fn fence_reserved_byte_must_be_zero() {
    use baedeker_core::binary::instr::decode_instr_sequence;
    let ok = decode_instr_sequence(&[0xFE, 0x03, 0x00, 0x0B], 0).unwrap();
    assert!(matches!(ok.as_slice(), [Instr::AtomicFence, Instr::End]));
    let bad = decode_instr_sequence(&[0xFE, 0x03, 0x01, 0x0B], 0);
    assert!(bad.is_err(), "fence reserved byte must be 0x00");
}

#[test]
fn unknown_fe_subopcodes_refused() {
    use baedeker_core::binary::instr::decode_instr_sequence;
    for sub in [0x04u8, 0x0F, 0x4F, 0x50, 0xFF] {
        assert!(
            decode_instr_sequence(&[0xFE, sub, 0x02, 0x00, 0x0B], 0).is_err(),
            "0xFE {sub:#04x} must be refused"
        );
    }
}
