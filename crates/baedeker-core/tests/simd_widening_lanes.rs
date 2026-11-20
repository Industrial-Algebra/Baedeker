// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! SIMD unit 1b: unary / extend / extmul / extadd_pairwise / narrow /
//! dot / bitselect / any_true / all_true / bitmask. Exact LE lane bytes.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{Store, Value, execute_export};

fn lower(wat: &str) -> RegModule {
    let buffer = wast::parser::ParseBuffer::new(wat).expect("wat parses");
    let mut parsed = wast::parser::parse::<wast::Wat>(&buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    module.lower().expect("module lowers")
}

fn run_v128(body: &str) -> [u8; 16] {
    let wat = format!("(module (func (export \"run\") (result v128) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[])
        .expect("executes")
        .as_slice()
    {
        [Value::V128(bytes)] => *bytes,
        other => panic!("expected one v128 result, got {other:?}"),
    }
}

fn run_i32(body: &str) -> i32 {
    let wat = format!("(module (func (export \"run\") (result i32) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[])
        .expect("executes")
        .as_slice()
    {
        [Value::I32(v)] => *v,
        other => panic!("expected one i32 result, got {other:?}"),
    }
}

#[allow(dead_code)]
fn run_i64(body: &str) -> i64 {
    let wat = format!("(module (func (export \"run\") (result i64) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[])
        .expect("executes")
        .as_slice()
    {
        [Value::I64(v)] => *v,
        other => panic!("expected one i64 result, got {other:?}"),
    }
}

fn rep4(pattern: [u8; 4]) -> [u8; 16] {
    [
        pattern[0], pattern[1], pattern[2], pattern[3], pattern[0], pattern[1], pattern[2],
        pattern[3], pattern[0], pattern[1], pattern[2], pattern[3], pattern[0], pattern[1],
        pattern[2], pattern[3],
    ]
}

fn rep2<const N: usize>(pattern: [u16; N]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (chunk, lane) in out.chunks_exact_mut(2).zip(pattern.iter().cycle()) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    out
}

fn rep1(pattern: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (chunk, lane) in out.chunks_exact_mut(4).zip(pattern.iter().cycle()) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    out
}

fn rep64(pattern: [u64; 2]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (chunk, lane) in out.chunks_exact_mut(8).zip(pattern.iter().cycle()) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    out
}

#[test]
fn i8x16_abs_neg_wrap_on_min() {
    let abs = run_v128(
        "(i8x16.abs (v128.const i8x16 0x80 0xff 0x01 0x7f 0x80 0xff 0x01 0x7f \
          0x80 0xff 0x01 0x7f 0x80 0xff 0x01 0x7f))",
    );
    assert_eq!(abs, rep4([0x80, 0x01, 0x01, 0x7f]));
    let neg = run_v128(
        "(i8x16.neg (v128.const i8x16 0x80 0x00 0x01 0xff 0x80 0x00 0x01 0xff \
          0x80 0x00 0x01 0xff 0x80 0x00 0x01 0xff))",
    );
    assert_eq!(neg, rep4([0x80, 0x00, 0xff, 0x01]));
}

#[test]
fn i8x16_popcnt_counts_bits() {
    let got = run_v128(
        "(i8x16.popcnt (v128.const i8x16 0xff 0x00 0xf0 0x0f 0xff 0x00 0xf0 0x0f \
          0xff 0x00 0xf0 0x0f 0xff 0x00 0xf0 0x0f))",
    );
    assert_eq!(got, rep4([0x08, 0x00, 0x04, 0x04]));
}

#[test]
fn wider_abs_neg_wrap_on_min() {
    let abs16 = run_v128(
        "(i16x8.abs (v128.const i16x8 0x8000 0xffff 0x0001 0x7fff \
          0x8000 0xffff 0x0001 0x7fff))",
    );
    assert_eq!(abs16, rep2([0x8000, 0x0001, 0x0001, 0x7fff]));
    let neg32 =
        run_v128("(i32x4.neg (v128.const i32x4 0x80000000 0x00000000 0x00000001 0xffffffff))");
    assert_eq!(
        neg32,
        rep1([0x80000000, 0x00000000, 0xffffffff, 0x00000001])
    );
    let abs64 = run_v128("(i64x2.abs (v128.const i64x2 0x8000000000000000 0xffffffffffffffff))");
    assert_eq!(abs64, rep64([0x8000000000000000, 0x0000000000000001]));
}

#[test]
fn any_true_and_all_true() {
    let any_set = run_i32("(v128.any_true (v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1))");
    assert_eq!(any_set, 1);
    let any_clear = run_i32("(v128.any_true (v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))");
    assert_eq!(any_clear, 0);
    let all_false = run_i32("(i16x8.all_true (v128.const i16x8 1 2 3 4 5 6 7 0))");
    assert_eq!(all_false, 0);
    let all_true = run_i32("(i16x8.all_true (v128.const i16x8 1 2 3 4 5 6 7 8))");
    assert_eq!(all_true, 1);
}

#[test]
fn bitmask_packs_lane_msbs() {
    let i8 = run_i32(
        "(i8x16.bitmask (v128.const i8x16 0x80 0x00 0xff 0x01 0x80 0x00 0xff 0x01 \
          0x80 0x00 0xff 0x01 0x80 0x00 0xff 0x01))",
    );
    assert_eq!(i8 as u32, 0x5555);
    let i32 =
        run_i32("(i32x4.bitmask (v128.const i32x4 0x80000000 0x7fffffff 0xffffffff 0x00000001))");
    assert_eq!(i32, 0b0101);
}

#[test]
fn narrow_i16x8_to_i8x16_saturates_signed_and_unsigned() {
    let s = run_v128(
        "(i8x16.narrow_i16x8_s (v128.const i16x8 0x0080 0xff00 0x0001 0xffff \
          0x0080 0xff00 0x0001 0xffff) \
         (v128.const i16x8 0x0180 0x0000 0x0100 0x0002 \
          0x0180 0x0000 0x0100 0x0002))",
    );
    let mut expected = [0u8; 16];
    expected[..8].copy_from_slice(&[0x7f, 0x80, 0x01, 0xff, 0x7f, 0x80, 0x01, 0xff]);
    expected[8..].copy_from_slice(&[0x7f, 0x00, 0x7f, 0x02, 0x7f, 0x00, 0x7f, 0x02]);
    assert_eq!(s, expected);

    let u = run_v128(
        "(i8x16.narrow_i16x8_u (v128.const i16x8 0x0100 0xffff 0x0001 0x0000 \
          0x0100 0xffff 0x0001 0x0000) \
         (v128.const i16x8 0xff00 0x0002 0x0100 0x0080 \
          0xff00 0x0002 0x0100 0x0080))",
    );
    let mut expected_u = [0u8; 16];
    expected_u[..8].copy_from_slice(&[0xff, 0x00, 0x01, 0x00, 0xff, 0x00, 0x01, 0x00]);
    expected_u[8..].copy_from_slice(&[0x00, 0x02, 0xff, 0x80, 0x00, 0x02, 0xff, 0x80]);
    assert_eq!(u, expected_u);
}

#[test]
fn extend_low_high_signed_unsigned() {
    let src = "(v128.const i8x16 0x80 0x01 0x02 0x03 0x04 0x05 0x06 0x07 \
                0x08 0x09 0x0a 0x0b 0x0c 0x0d 0x0e 0x0f)";
    let low_s = run_v128(&format!("(i16x8.extend_low_i8x16_s {src})"));
    assert_eq!(
        low_s,
        rep2([
            0xff80, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007
        ])
    );
    let high_u = run_v128(&format!("(i16x8.extend_high_i8x16_u {src})"));
    assert_eq!(
        high_u,
        rep2([
            0x0008, 0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x000e, 0x000f
        ])
    );
    let high_s32 = run_v128(
        "(i32x4.extend_high_i16x8_s (v128.const i16x8 0x8000 0x0001 0x7fff 0xffff \
          0x8000 0x0001 0x7fff 0xffff))",
    );
    assert_eq!(
        high_s32,
        rep1([0xffff8000, 0x00000001, 0x00007fff, 0xffffffff])
    );
    let low_s64 =
        run_v128("(i64x2.extend_low_i32x4_s (v128.const i32x4 0x80000000 0x00000001 0 0))");
    assert_eq!(low_s64, rep64([0xffffffff80000000, 0x0000000000000001]));
}

#[test]
fn extmul_low_high_signed_unsigned() {
    let low_s = run_v128(
        "(i16x8.extmul_low_i8x16_s \
         (v128.const i8x16 0x80 0x02 0x03 0x04 0x05 0x06 0x07 0x08 0 0 0 0 0 0 0 0) \
         (v128.const i8x16 0x02 0x03 0x04 0x05 0x06 0x07 0x08 0x09 0 0 0 0 0 0 0 0))",
    );
    assert_eq!(
        low_s,
        rep2([
            0xff00, 0x0006, 0x000c, 0x0014, 0x001e, 0x002a, 0x0038, 0x0048
        ])
    );
    let high_u = run_v128(
        "(i32x4.extmul_high_i16x8_u \
         (v128.const i16x8 0 0 0 0 0xffff 0x0002 0x00ff 0x8000) \
         (v128.const i16x8 0 0 0 0 0x0002 0x0003 0x0100 0x0002))",
    );
    assert_eq!(
        high_u,
        rep1([0x0001fffe, 0x00000006, 0x0000ff00, 0x00010000])
    );
    let high_s64 = run_v128(
        "(i64x2.extmul_high_i32x4_s \
         (v128.const i32x4 0 0 0xffffffff 0x00000002) \
         (v128.const i32x4 0 0 0x00000002 0x00000003))",
    );
    assert_eq!(high_s64, rep64([0xfffffffffffffffe, 0x0000000000000006]));
}

#[test]
fn extadd_pairwise_sums_adjacent_lanes() {
    let s = run_v128(
        "(i16x8.extadd_pairwise_i8x16_s (v128.const i8x16 0x80 0x01 0x7f 0x02 \
          0xff 0x00 0x02 0xff 0x80 0x01 0x7f 0x02 0xff 0x00 0x02 0xff))",
    );
    assert_eq!(s, rep2([0xff81, 0x0081, 0xffff, 0x0001]));
    let u = run_v128(
        "(i16x8.extadd_pairwise_i8x16_u (v128.const i8x16 0x80 0x01 0x7f 0x02 \
          0xff 0x00 0x02 0xff 0x80 0x01 0x7f 0x02 0xff 0x00 0x02 0xff))",
    );
    assert_eq!(u, rep2([0x0081, 0x0081, 0x00ff, 0x0101]));
    let s32 = run_v128(
        "(i32x4.extadd_pairwise_i16x8_s (v128.const i16x8 0x8000 0x0001 0x7fff 0xffff \
          0x8000 0x0001 0x7fff 0xffff))",
    );
    assert_eq!(s32, rep1([0xffff8001, 0x00007ffe, 0xffff8001, 0x00007ffe]));
}

#[test]
fn dot_i16x8_s_wraps_at_i32() {
    let normal = run_v128(
        "(i32x4.dot_i16x8_s (v128.const i16x8 0x8000 0x0001 0x0002 0x0003 \
          0x0004 0x0005 0x0006 0x0007) \
         (v128.const i16x8 0x8000 0x0002 0x0003 0x0004 \
          0x0005 0x0006 0x0007 0x0008))",
    );
    assert_eq!(
        normal,
        rep1([0x40000002, 0x00000012, 0x00000032, 0x00000062])
    );
    let wrapping = run_v128(
        "(i32x4.dot_i16x8_s (v128.const i16x8 0x8000 0x8000 0x8000 0x8000 \
          0x8000 0x8000 0x8000 0x8000) \
         (v128.const i16x8 0x8000 0x8000 0x8000 0x8000 \
          0x8000 0x8000 0x8000 0x8000))",
    );
    assert_eq!(
        wrapping,
        rep1([0x80000000, 0x80000000, 0x80000000, 0x80000000])
    );
}

#[test]
fn bitselect_picks_bits_by_mask() {
    let got = run_v128(
        "(v128.bitselect (v128.const i8x16 0xaa 0xaa 0xaa 0xaa 0xaa 0xaa 0xaa 0xaa \
          0xaa 0xaa 0xaa 0xaa 0xaa 0xaa 0xaa 0xaa) \
         (v128.const i8x16 0x55 0x55 0x55 0x55 0x55 0x55 0x55 0x55 \
          0x55 0x55 0x55 0x55 0x55 0x55 0x55 0x55) \
         (v128.const i8x16 0x0f 0x0f 0x0f 0x0f 0x0f 0x0f 0x0f 0x0f \
          0x0f 0x0f 0x0f 0x0f 0x0f 0x0f 0x0f 0x0f))",
    );
    assert_eq!(got, [0x5a; 16]);
}
