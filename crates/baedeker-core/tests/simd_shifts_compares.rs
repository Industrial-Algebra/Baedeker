// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! SIMD unit 2: shifts (amount mod lane width) and compares (full-lane
//! masks). Exact LE lane bytes.

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

fn rep4(pattern: [u8; 4]) -> [u8; 16] {
    [
        pattern[0], pattern[1], pattern[2], pattern[3], pattern[0], pattern[1], pattern[2],
        pattern[3], pattern[0], pattern[1], pattern[2], pattern[3], pattern[0], pattern[1],
        pattern[2], pattern[3],
    ]
}

#[test]
fn i8x16_lt_signed_vs_unsigned_views() {
    let lt_s = run_v128(
        "(i8x16.lt_s (v128.const i8x16 0x80 0x01 0x02 0x02 0x80 0x01 0x02 0x02 \
          0x80 0x01 0x02 0x02 0x80 0x01 0x02 0x02) \
         (v128.const i8x16 0x01 0x01 0x03 0x02 0x01 0x01 0x03 0x02 \
          0x01 0x01 0x03 0x02 0x01 0x01 0x03 0x02))",
    );
    assert_eq!(lt_s, rep4([0xff, 0x00, 0xff, 0x00]));
    let lt_u = run_v128(
        "(i8x16.lt_u (v128.const i8x16 0x01 0x7f 0x80 0xff 0x01 0x7f 0x80 0xff \
          0x01 0x7f 0x80 0xff 0x01 0x7f 0x80 0xff) \
         (v128.const i8x16 0x02 0x80 0x7f 0x00 0x02 0x80 0x7f 0x00 \
          0x02 0x80 0x7f 0x00 0x02 0x80 0x7f 0x00))",
    );
    assert_eq!(lt_u, rep4([0xff, 0xff, 0x00, 0x00]));
}

#[test]
fn i16x8_ge_s_and_i32x4_gt_u() {
    let ge_s = run_v128(
        "(i16x8.ge_s (v128.const i16x8 0x0002 0x8000 0xffff 0x0000 \
          0x0002 0x8000 0xffff 0x0000) \
         (v128.const i16x8 0x0001 0x8001 0x0000 0x0000 \
          0x0001 0x8001 0x0000 0x0000))",
    );
    assert_eq!(
        ge_s,
        [
            0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00,
            0xff, 0xff,
        ]
    );
    let gt_u = run_v128(
        "(i32x4.gt_u (v128.const i32x4 0xffffffff 0x00000001 5 7) \
         (v128.const i32x4 0x00000001 0xffffffff 6 7))",
    );
    assert_eq!(
        gt_u,
        [
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]
    );
}

#[test]
fn i64x2_lt_s_eq_ne() {
    let lt_s = run_v128("(i64x2.lt_s (v128.const i64x2 -2 5) (v128.const i64x2 -1 5))");
    assert_eq!(
        lt_s,
        [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]
    );
    let ne = run_v128("(i64x2.ne (v128.const i64x2 -2 5) (v128.const i64x2 -2 6))");
    assert_eq!(
        ne,
        [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ]
    );
}

#[test]
fn f32x4_eq_treats_neg_zero_equal_and_nan_unequal() {
    let got = run_v128(
        "(f32x4.eq (v128.const f32x4 1.5 -0.0 2.0 nan) \
         (v128.const f32x4 1.5 0.0 2.5 nan))",
    );
    assert_eq!(
        got,
        [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]
    );
}

#[test]
fn f32x4_le_is_false_on_nan() {
    let got = run_v128(
        "(f32x4.le (v128.const f32x4 1.0 nan 2.0 3.0) \
         (v128.const f32x4 2.0 1.0 2.0 4.0))",
    );
    assert_eq!(
        got,
        [
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ]
    );
}

#[test]
fn f64x2_lt_ge_zero_signs() {
    let lt = run_v128("(f64x2.lt (v128.const f64x2 1.0 -0.0) (v128.const f64x2 2.0 0.0))");
    assert_eq!(
        lt,
        [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]
    );
    let ge = run_v128("(f64x2.ge (v128.const f64x2 1.0 -0.0) (v128.const f64x2 2.0 0.0))");
    assert_eq!(
        ge,
        [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ]
    );
}

#[test]
fn shifts_mod_lane_width_and_drop_bits() {
    let shl = run_v128(
        "(i8x16.shl (v128.const i8x16 0x01 0x80 0x40 0x02 0x01 0x80 0x40 0x02 \
          0x01 0x80 0x40 0x02 0x01 0x80 0x40 0x02) \
         (i32.const 9))",
    );
    assert_eq!(shl, rep4([0x02, 0x00, 0x80, 0x04]));
    let shr_s = run_v128(
        "(i8x16.shr_s (v128.const i8x16 0x80 0xff 0x40 0x08 0x80 0xff 0x40 0x08 \
          0x80 0xff 0x40 0x08 0x80 0xff 0x40 0x08) \
         (i32.const 1))",
    );
    assert_eq!(shr_s, rep4([0xc0, 0xff, 0x20, 0x04]));
    let shr_u = run_v128(
        "(i8x16.shr_u (v128.const i8x16 0x80 0xff 0x01 0x00 0x80 0xff 0x01 0x00 \
          0x80 0xff 0x01 0x00 0x80 0xff 0x01 0x00) \
         (i32.const 4))",
    );
    assert_eq!(shr_u, rep4([0x08, 0x0f, 0x00, 0x00]));
}

#[test]
fn wider_shifts_wrap_and_sign_extend() {
    let shl16 = run_v128("(i16x8.shl (v128.const i16x8 0x8000 0x0001 0 0 0 0 0 0) (i32.const 1))");
    assert_eq!(
        shl16,
        [0x00, 0x00, 0x02, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,]
    );
    let shr64 = run_v128("(i64x2.shr_s (v128.const i64x2 0x8000000000000000 -8) (i32.const 1))");
    assert_eq!(
        shr64,
        [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0xfc, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ]
    );
}
