// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! SIMD unit 1a: saturating / min-max / avgr / q15 / andnot and the
//! extract/replace lane gaps. Every v128 result is compared as exact
//! little-endian lane bytes.

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

fn run_f64(body: &str) -> f64 {
    let wat = format!("(module (func (export \"run\") (result f64) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[])
        .expect("executes")
        .as_slice()
    {
        [Value::F64(v)] => *v,
        other => panic!("expected one f64 result, got {other:?}"),
    }
}

fn rep4(pattern: [u8; 4]) -> [u8; 16] {
    [
        pattern[0], pattern[1], pattern[2], pattern[3], pattern[0], pattern[1], pattern[2],
        pattern[3], pattern[0], pattern[1], pattern[2], pattern[3], pattern[0], pattern[1],
        pattern[2], pattern[3],
    ]
}

fn rep2(pattern: [u16; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (chunk, lane) in out.chunks_exact_mut(2).zip(pattern.iter().cycle()) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    out
}

#[test]
fn i8x16_add_sat_s_saturates_at_signed_bounds() {
    let got = run_v128(
        "(i8x16.add_sat_s (v128.const i8x16 0x7f 0x80 0x01 0xff 0x7f 0x80 0x01 0xff \
          0x7f 0x80 0x01 0xff 0x7f 0x80 0x01 0xff) \
         (v128.const i8x16 0x01 0xff 0x7f 0x01 0x01 0xff 0x7f 0x01 \
          0x01 0xff 0x7f 0x01 0x01 0xff 0x7f 0x01))",
    );
    assert_eq!(got, rep4([0x7f, 0x80, 0x7f, 0x00]));
}

#[test]
fn i8x16_add_sat_u_saturates_at_unsigned_bounds() {
    let got = run_v128(
        "(i8x16.add_sat_u (v128.const i8x16 0xff 0x01 0xfe 0x7f 0xff 0x01 0xfe 0x7f \
          0xff 0x01 0xfe 0x7f 0xff 0x01 0xfe 0x7f) \
         (v128.const i8x16 0x01 0xff 0x02 0x81 0x01 0xff 0x02 0x81 \
          0x01 0xff 0x02 0x81 0x01 0xff 0x02 0x81))",
    );
    assert_eq!(got, [0xff; 16]);
}

#[test]
fn i8x16_sub_sat_s_saturates_at_signed_bounds() {
    let got = run_v128(
        "(i8x16.sub_sat_s (v128.const i8x16 0x80 0x7f 0x00 0x01 0x80 0x7f 0x00 0x01 \
          0x80 0x7f 0x00 0x01 0x80 0x7f 0x00 0x01) \
         (v128.const i8x16 0x01 0xff 0x01 0xff 0x01 0xff 0x01 0xff \
          0x01 0xff 0x01 0xff 0x01 0xff 0x01 0xff))",
    );
    assert_eq!(got, rep4([0x80, 0x7f, 0xff, 0x02]));
}

#[test]
fn i8x16_sub_sat_u_clamps_at_zero() {
    let got = run_v128(
        "(i8x16.sub_sat_u (v128.const i8x16 0x00 0x01 0xff 0x80 0x00 0x01 0xff 0x80 \
          0x00 0x01 0xff 0x80 0x00 0x01 0xff 0x80) \
         (v128.const i8x16 0x01 0xff 0x01 0x7f 0x01 0xff 0x01 0x7f \
          0x01 0xff 0x01 0x7f 0x01 0xff 0x01 0x7f))",
    );
    assert_eq!(got, rep4([0x00, 0x00, 0xfe, 0x01]));
}

#[test]
fn i16x8_add_sat_s_saturates_at_signed_bounds() {
    let got = run_v128(
        "(i16x8.add_sat_s (v128.const i16x8 0x7fff 0x8000 0x0001 0xffff \
          0x7fff 0x8000 0x0001 0xffff) \
         (v128.const i16x8 0x0001 0xffff 0x7fff 0x0001 \
          0x0001 0xffff 0x7fff 0x0001))",
    );
    assert_eq!(got, rep2([0x7fff, 0x8000, 0x7fff, 0x0000]));
}

#[test]
fn i16x8_min_max_respect_signedness() {
    let min_s = run_v128(
        "(i16x8.min_s (v128.const i16x8 0x0001 0xffff 0x8000 0x7fff \
          0x0001 0xffff 0x8000 0x7fff) \
         (v128.const i16x8 0x0002 0x0001 0x7fff 0x8000 \
          0x0002 0x0001 0x7fff 0x8000))",
    );
    assert_eq!(min_s, rep2([0x0001, 0xffff, 0x8000, 0x8000]));
    let min_u = run_v128(
        "(i16x8.min_u (v128.const i16x8 0x0001 0xffff 0x8000 0x0000 \
          0x0001 0xffff 0x8000 0x0000) \
         (v128.const i16x8 0x0002 0x0000 0x7fff 0xffff \
          0x0002 0x0000 0x7fff 0xffff))",
    );
    assert_eq!(min_u, rep2([0x0001, 0x0000, 0x7fff, 0x0000]));
    let max_s = run_v128(
        "(i16x8.max_s (v128.const i16x8 0x0001 0x8000 0xffff 0x0000 \
          0x0001 0x8000 0xffff 0x0000) \
         (v128.const i16x8 0x0002 0x7fff 0x0001 0xffff \
          0x0002 0x7fff 0x0001 0xffff))",
    );
    assert_eq!(max_s, rep2([0x0002, 0x7fff, 0x0001, 0x0000]));
    let max_u = run_v128(
        "(i16x8.max_u (v128.const i16x8 0x0001 0x8000 0x00ff 0x0000 \
          0x0001 0x8000 0x00ff 0x0000) \
         (v128.const i16x8 0x0002 0x7fff 0xffff 0xffff \
          0x0002 0x7fff 0xffff 0xffff))",
    );
    assert_eq!(max_u, rep2([0x0002, 0x8000, 0xffff, 0xffff]));
}

#[test]
fn i32x4_min_u_smoke() {
    let got = run_v128(
        "(i32x4.min_u (v128.const i32x4 0xffffffff 0x00000000 0x00000005 0x00000007) \
         (v128.const i32x4 0x00000001 0xffffffff 0x00000006 0x00000007))",
    );
    let mut expected = [0u8; 16];
    for (chunk, lane) in expected.chunks_exact_mut(4).zip([1u32, 0, 5, 7].iter()) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    assert_eq!(got, expected);
}

#[test]
fn avgr_u_rounds_up_without_overflow() {
    let i8 = run_v128(
        "(i8x16.avgr_u (v128.const i8x16 0xff 0x01 0xfe 0x00 0xff 0x01 0xfe 0x00 \
          0xff 0x01 0xfe 0x00 0xff 0x01 0xfe 0x00) \
         (v128.const i8x16 0xfe 0x02 0x01 0x00 0xfe 0x02 0x01 0x00 \
          0xfe 0x02 0x01 0x00 0xfe 0x02 0x01 0x00))",
    );
    assert_eq!(i8, rep4([0xff, 0x02, 0x80, 0x00]));
    let i16 = run_v128(
        "(i16x8.avgr_u (v128.const i16x8 0xffff 0x0001 0xffff 0x0001 \
          0xffff 0x0001 0xffff 0x0001) \
         (v128.const i16x8 0xfffe 0x0002 0xfffe 0x0002 \
          0xfffe 0x0002 0xfffe 0x0002))",
    );
    assert_eq!(i16, rep2([0xffff, 0x0002, 0xffff, 0x0002]));
}

#[test]
fn q15mulr_sat_s_rounds_and_saturates() {
    let got = run_v128(
        "(i16x8.q15mulr_sat_s (v128.const i16x8 0x8000 0x4000 0xc000 0x0001 \
          0x8000 0x4000 0xc000 0x0001) \
         (v128.const i16x8 0x8000 0x4000 0x4000 0x7fff \
          0x8000 0x4000 0x4000 0x7fff))",
    );
    assert_eq!(got, rep2([0x7fff, 0x2000, 0xe000, 0x0001]));
}

#[test]
fn v128_andnot_masks() {
    let got = run_v128(
        "(v128.andnot (v128.const i8x16 0xf0 0xff 0x0f 0xaa 0xf0 0xff 0x0f 0xaa \
          0xf0 0xff 0x0f 0xaa 0xf0 0xff 0x0f 0xaa) \
         (v128.const i8x16 0x0f 0xff 0xff 0x55 0x0f 0xff 0xff 0x55 \
          0x0f 0xff 0xff 0x55 0x0f 0xff 0xff 0x55))",
    );
    assert_eq!(got, rep4([0xf0, 0x00, 0x00, 0xaa]));
}

#[test]
fn i8x16_extract_lane_extends_signedness() {
    let s =
        run_i32("(i8x16.extract_lane_s 0 (v128.const i8x16 0x80 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))");
    assert_eq!(s, -128);
    let u =
        run_i32("(i8x16.extract_lane_u 0 (v128.const i8x16 0x80 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))");
    assert_eq!(u, 128);
    let i16s = run_i32("(i16x8.extract_lane_s 0 (v128.const i16x8 0x8000 0 0 0 0 0 0 0))");
    assert_eq!(i16s, -32768);
    let i16u = run_i32("(i16x8.extract_lane_u 0 (v128.const i16x8 0x8000 0 0 0 0 0 0 0))");
    assert_eq!(i16u, 32768);
}

#[test]
fn i8x16_replace_lane_truncates() {
    let got = run_v128(
        "(i8x16.replace_lane 2 (v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0) \
         (i32.const 0xffffff02))",
    );
    let mut expected = [0u8; 16];
    expected[2] = 0x02;
    assert_eq!(got, expected);
}

#[test]
fn i64x2_extract_replace_roundtrip() {
    let v =
        run_i64("(i64x2.extract_lane 0 (v128.const i64x2 0x0123456789abcdef 0xfffffffffffffffe))");
    assert_eq!(v, 0x0123_4567_89ab_cdef);
    let got = run_v128("(i64x2.replace_lane 1 (v128.const i64x2 0 0) (i64.const -1))");
    let mut expected = [0u8; 16];
    expected[8..].fill(0xff);
    assert_eq!(got, expected);
}

#[test]
fn f64x2_extract_replace() {
    let v = run_f64("(f64x2.extract_lane 1 (v128.const f64x2 1.5 -0.5))");
    assert_eq!(v, -0.5);
    let w = run_f64(
        "(f64x2.extract_lane 0 (f64x2.replace_lane 0 (v128.const f64x2 1.5 -0.5) (f64.const 3.25)))",
    );
    assert_eq!(w, 3.25);
}
