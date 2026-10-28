//! Threads U1: the 0xFE atomic family — decode + validation shapes.
//!
//! Compile-time RED pair with the implementation: references `AtomicOp`,
//! `AtomicWidth`, `AtomicRmw`, and `Limits.shared`, none of which exist
//! yet. Opcode table cross-checked against wast-240 expr.rs:786-855
//! (encoder) and the pin's binary/instructions.rst (grammar); natural
//! alignment from the MemArg<N> bit-widths.

use baedeker_core::binary::instr::{
    AtomicOp, AtomicRmw, AtomicWidth, Instr, decode_instr_sequence,
};
use baedeker_core::binary::module::Module;
use baedeker_core::types::MemArg;
use baedeker_core::validate::validate_module;

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

fn section(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![id];
    uleb(payload.len() as u64, &mut s);
    s.extend_from_slice(payload);
    s
}

fn module_with(sections: &[Vec<u8>]) -> Vec<u8> {
    let mut m = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];
    for s in sections {
        m.extend_from_slice(s);
    }
    m
}

/// Build a module whose one function pushes `consts` (i32/i64 as noted),
/// executes `body`, and ends. Memory section optional.
fn module_with_func(body: &[u8], consts: &[u8], memory: Option<&[u8]>) -> Vec<u8> {
    let mut sections: Vec<Vec<u8>> = vec![section(1, &[0x01, 0x60, 0x00, 0x00])];
    sections.push(section(3, &[0x01, 0x00]));
    if let Some(mem) = memory {
        sections.push(section(5, mem));
    }
    let mut full = vec![0x00]; // zero locals
    full.extend_from_slice(consts);
    full.extend_from_slice(body);
    full.push(0x0B);
    let mut code = vec![0x01];
    uleb(full.len() as u64, &mut code);
    code.extend_from_slice(&full);
    sections.push(section(10, &code));
    module_with(&sections)
}

/// (sub-opcode, natural align log2, expected decoded op). 67 entries:
/// 3 sync + 7 loads + 7 stores + 42 rmw + 7 cmpxchg.
fn family() -> Vec<(u8, u32, AtomicOp)> {
    use AtomicOp::*;
    use AtomicRmw::*;
    use AtomicWidth::*;
    let m = |align: u32| MemArg {
        align,
        offset: 0x42,
        memory: baedeker_core::types::MemIdx(0),
    };
    vec![
        (0x00, 2, Notify(m(2))),
        (0x01, 2, Wait32(m(2))),
        (0x02, 3, Wait64(m(3))),
        (0x10, 2, Load(I32, m(2))),
        (0x11, 3, Load(I64, m(3))),
        (0x12, 0, Load(I32N8, m(0))),
        (0x13, 1, Load(I32N16, m(1))),
        (0x14, 0, Load(I64N8, m(0))),
        (0x15, 1, Load(I64N16, m(1))),
        (0x16, 2, Load(I64N32, m(2))),
        (0x17, 2, Store(I32, m(2))),
        (0x18, 3, Store(I64, m(3))),
        (0x19, 0, Store(I32N8, m(0))),
        (0x1a, 1, Store(I32N16, m(1))),
        (0x1b, 0, Store(I64N8, m(0))),
        (0x1c, 1, Store(I64N16, m(1))),
        (0x1d, 2, Store(I64N32, m(2))),
        (0x1e, 2, Rmw(Add, I32, m(2))),
        (0x1f, 3, Rmw(Add, I64, m(3))),
        (0x20, 0, Rmw(Add, I32N8, m(0))),
        (0x21, 1, Rmw(Add, I32N16, m(1))),
        (0x22, 0, Rmw(Add, I64N8, m(0))),
        (0x23, 1, Rmw(Add, I64N16, m(1))),
        (0x24, 2, Rmw(Add, I64N32, m(2))),
        (0x25, 2, Rmw(Sub, I32, m(2))),
        (0x26, 3, Rmw(Sub, I64, m(3))),
        (0x27, 0, Rmw(Sub, I32N8, m(0))),
        (0x28, 1, Rmw(Sub, I32N16, m(1))),
        (0x29, 0, Rmw(Sub, I64N8, m(0))),
        (0x2a, 1, Rmw(Sub, I64N16, m(1))),
        (0x2b, 2, Rmw(Sub, I64N32, m(2))),
        (0x2c, 2, Rmw(And, I32, m(2))),
        (0x2d, 3, Rmw(And, I64, m(3))),
        (0x2e, 0, Rmw(And, I32N8, m(0))),
        (0x2f, 1, Rmw(And, I32N16, m(1))),
        (0x30, 0, Rmw(And, I64N8, m(0))),
        (0x31, 1, Rmw(And, I64N16, m(1))),
        (0x32, 2, Rmw(And, I64N32, m(2))),
        (0x33, 2, Rmw(Or, I32, m(2))),
        (0x34, 3, Rmw(Or, I64, m(3))),
        (0x35, 0, Rmw(Or, I32N8, m(0))),
        (0x36, 1, Rmw(Or, I32N16, m(1))),
        (0x37, 0, Rmw(Or, I64N8, m(0))),
        (0x38, 1, Rmw(Or, I64N16, m(1))),
        (0x39, 2, Rmw(Or, I64N32, m(2))),
        (0x3a, 2, Rmw(Xor, I32, m(2))),
        (0x3b, 3, Rmw(Xor, I64, m(3))),
        (0x3c, 0, Rmw(Xor, I32N8, m(0))),
        (0x3d, 1, Rmw(Xor, I32N16, m(1))),
        (0x3e, 0, Rmw(Xor, I64N8, m(0))),
        (0x3f, 1, Rmw(Xor, I64N16, m(1))),
        (0x40, 2, Rmw(Xor, I64N32, m(2))),
        (0x41, 2, Rmw(Xchg, I32, m(2))),
        (0x42, 3, Rmw(Xchg, I64, m(3))),
        (0x43, 0, Rmw(Xchg, I32N8, m(0))),
        (0x44, 1, Rmw(Xchg, I32N16, m(1))),
        (0x45, 0, Rmw(Xchg, I64N8, m(0))),
        (0x46, 1, Rmw(Xchg, I64N16, m(1))),
        (0x47, 2, Rmw(Xchg, I64N32, m(2))),
        (0x48, 2, Cmpxchg(I32, m(2))),
        (0x49, 3, Cmpxchg(I64, m(3))),
        (0x4a, 0, Cmpxchg(I32N8, m(0))),
        (0x4b, 1, Cmpxchg(I32N16, m(1))),
        (0x4c, 0, Cmpxchg(I64N8, m(0))),
        (0x4d, 1, Cmpxchg(I64N16, m(1))),
        (0x4e, 2, Cmpxchg(I64N32, m(2))),
    ]
}

#[test]
fn full_family_decodes_with_natural_alignment() {
    for (sub, natural, expected) in family() {
        let bytes = [0xFE, sub, natural as u8, 0x42, 0x0B];
        let decoded = decode_instr_sequence(&bytes, 0)
            .unwrap_or_else(|e| panic!("0xFE {sub:#04x} must decode: {e}"));
        assert_eq!(
            decoded.as_slice(),
            [Instr::Atomic(expected), Instr::End],
            "0xFE {sub:#04x}"
        );
    }
}

#[test]
fn align_above_natural_is_invalid() {
    // Shared memory 1..2 + i32.atomic.load with align=3 (8 > 4 natural):
    // per check_memop (valid.ml:151-160) "alignment must not be larger
    // than natural" — validation, not trap.
    let bytes = module_with_func(
        &[0xFE, 0x10, 0x03, 0x00],
        &[0x41, 0x00],
        Some(&[0x01, 0x03, 0x01, 0x02]),
    );
    let module = Module::decode(&bytes).expect("decodes");
    let err = validate_module(&module).expect_err("align 3 > natural 2");
    assert!(
        err.to_string().contains("invalid memarg alignment"),
        "got: {err}"
    );
}

#[test]
fn atomics_validate_against_unshared_memory() {
    // The pin's own interpreter puts NO shared-context requirement on
    // atomic instructions (valid.ml:151 checks only mems[0]); the rst's
    // stricter text is the recorded ambiguity in the contract. Corpus rule
    // wins: this module validates.
    let bytes = module_with_func(
        &[0xFE, 0x10, 0x02, 0x00, 0x1A],
        &[0x41, 0x00],
        Some(&[0x01, 0x00, 0x01]),
    );
    let module = Module::decode(&bytes).expect("decodes");
    validate_module(&module).expect("unshared memory is fine for atomics");
}

#[test]
fn wait32_shape_validates_and_mismatch_rejected() {
    // wait32: [i32 i32 i64] -> [i32]; result dropped.
    let good = module_with_func(
        &[0xFE, 0x01, 0x02, 0x00, 0x1A],
        &[0x41, 0x00, 0x41, 0x00, 0x42, 0x00],
        Some(&[0x01, 0x03, 0x01, 0x02]),
    );
    validate_module(&Module::decode(&good).unwrap()).expect("wait32 shape ok");
    // wait64 expects an i64: feeding it i32 constants must be invalid.
    let bad = module_with_func(
        &[0xFE, 0x02, 0x03, 0x00, 0x1A],
        &[0x41, 0x00, 0x41, 0x00, 0x41, 0x00],
        Some(&[0x01, 0x03, 0x01, 0x02]),
    );
    let err = validate_module(&Module::decode(&bad).unwrap()).expect_err("wait64 needs i64");
    assert!(err.to_string().contains("type mismatch"), "got: {err}");
}

#[test]
fn notify_and_fence_shapes_validate() {
    // notify: [i32 i32] -> [i32], dropped; fence: [] -> [].
    let bytes = module_with_func(
        &[0xFE, 0x00, 0x02, 0x00, 0x1A, 0xFE, 0x03, 0x00],
        &[0x41, 0x00, 0x41, 0x00],
        Some(&[0x01, 0x03, 0x01, 0x02]),
    );
    validate_module(&Module::decode(&bytes).unwrap()).expect("notify + fence ok");
}

#[test]
fn store_and_cmpxchg_shapes_validate() {
    // store: [i32 i32] -> []; cmpxchg: [i32 t t] -> [t], dropped.
    let bytes = module_with_func(
        &[
            0xFE, 0x17, 0x02, 0x00, 0x41, 0x00, 0x41, 0x00, 0x41, 0x00, 0xFE, 0x48, 0x02, 0x00,
            0x1A,
        ],
        &[0x41, 0x00, 0x41, 0x00],
        Some(&[0x01, 0x03, 0x01, 0x02]),
    );
    validate_module(&Module::decode(&bytes).unwrap()).expect("store + cmpxchg ok");
    // Cmpxchg missing its replacement operand: invalid.
    let bad = module_with_func(
        &[0xFE, 0x48, 0x02, 0x00, 0x1A],
        &[0x41, 0x00, 0x41, 0x00],
        Some(&[0x01, 0x03, 0x01, 0x02]),
    );
    assert!(
        validate_module(&Module::decode(&bad).unwrap()).is_err(),
        "cmpxchg needs [i32 i32 i32]"
    );
}

#[test]
fn atomic_without_memory_is_invalid() {
    // check_memop requires C.mems[0]: no memory section, no atomics.
    let bytes = module_with_func(&[0xFE, 0x10, 0x02, 0x00, 0x1A], &[0x41, 0x00], None);
    let module = Module::decode(&bytes).unwrap();
    assert!(validate_module(&module).is_err());
}
