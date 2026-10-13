// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! SIMD unit 3: float unary, wasm min/max (canonical NaN), pmin/pmax
//! (operand-selecting), conversions (trunc_sat via Rust `as`
//! semantics), swizzle, shuffle. Exact LE lane bytes / exact bit
//! patterns.

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

fn f32x4_lanes(bytes: [u8; 16]) -> [u32; 4] {
    let mut out = [0u32; 4];
    for (lane, chunk) in out.iter_mut().zip(bytes.chunks_exact(4)) {
        *lane = u32::from_le_bytes(chunk.try_into().expect("width"));
    }
    out
}

fn f64x2_lanes(bytes: [u8; 16]) -> [u64; 2] {
    let mut out = [0u64; 2];
    for (lane, chunk) in out.iter_mut().zip(bytes.chunks_exact(8)) {
        *lane = u64::from_le_bytes(chunk.try_into().expect("width"));
    }
    out
}

fn i32x4_lanes(bytes: [u8; 16]) -> [i32; 4] {
    let mut out = [0i32; 4];
    for (lane, chunk) in out.iter_mut().zip(bytes.chunks_exact(4)) {
        *lane = i32::from_le_bytes(chunk.try_into().expect("width"));
    }
    out
}

#[test]
fn f32x4_min_max_canonicalize_nan_and_order_zeros() {
    let min = run_v128(
        "(f32x4.min (v128.const f32x4 1.0 -0.0 2.0 nan) \
         (v128.const f32x4 2.0 0.0 nan 3.0))",
    );
    assert_eq!(
        f32x4_lanes(min),
        [
            1.0f32.to_bits(),
            (-0.0f32).to_bits(),
            0x7FC0_0000,
            0x7FC0_0000
        ]
    );
    let max = run_v128(
        "(f32x4.max (v128.const f32x4 1.0 -0.0 2.0 nan) \
         (v128.const f32x4 2.0 0.0 nan 3.0))",
    );
    assert_eq!(
        f32x4_lanes(max),
        [2.0f32.to_bits(), 0.0f32.to_bits(), 0x7FC0_0000, 0x7FC0_0000]
    );
}

#[test]
fn pmin_pmax_select_operands_without_canonicalizing() {
    let pmin = run_v128("(f64x2.pmin (v128.const f64x2 1.0 nan) (v128.const f64x2 2.0 3.0))");
    assert_eq!(f64x2_lanes(pmin), [1.0f64.to_bits(), 0x7FF8_0000_0000_0000]);
    let pmax = run_v128("(f64x2.pmax (v128.const f64x2 1.0 nan) (v128.const f64x2 2.0 3.0))");
    assert_eq!(f64x2_lanes(pmax), [2.0f64.to_bits(), 0x7FF8_0000_0000_0000]);
    let pmin_zero =
        run_v128("(f32x4.pmin (v128.const f32x4 -0.0 0.0 0 0) (v128.const f32x4 0.0 -0.0 0 0))");
    assert_eq!(
        f32x4_lanes(pmin_zero),
        [
            (-0.0f32).to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits()
        ]
    );
}

#[test]
fn float_unary_bits() {
    let abs = run_v128("(f32x4.abs (v128.const f32x4 -1.5 2.0 -0.0 -nan))");
    assert_eq!(
        f32x4_lanes(abs),
        [
            1.5f32.to_bits(),
            2.0f32.to_bits(),
            0.0f32.to_bits(),
            0x7FC0_0000
        ]
    );
    let sqrt = run_v128("(f64x2.sqrt (v128.const f64x2 4.0 2.25))");
    assert_eq!(f64x2_lanes(sqrt), [2.0f64.to_bits(), 1.5f64.to_bits()]);
    let nearest = run_v128("(f32x4.nearest (v128.const f32x4 0.5 1.5 2.5 -0.5))");
    assert_eq!(
        f32x4_lanes(nearest),
        [
            0.0f32.to_bits(),
            2.0f32.to_bits(),
            2.0f32.to_bits(),
            (-0.0f32).to_bits()
        ]
    );
    let ceil = run_v128("(f64x2.ceil (v128.const f64x2 -1.5 1.5))");
    assert_eq!(f64x2_lanes(ceil), [(-1.0f64).to_bits(), 2.0f64.to_bits()]);
    let floor = run_v128("(f64x2.floor (v128.const f64x2 -1.5 1.5))");
    assert_eq!(f64x2_lanes(floor), [(-2.0f64).to_bits(), 1.0f64.to_bits()]);
    let trunc = run_v128("(f64x2.trunc (v128.const f64x2 -1.5 1.5))");
    assert_eq!(f64x2_lanes(trunc), [(-1.0f64).to_bits(), 1.0f64.to_bits()]);
}

#[test]
fn trunc_sat_saturates_and_nans_to_zero() {
    let s = run_v128("(i32x4.trunc_sat_f32x4_s (v128.const f32x4 1.5 -1.5 3.0e10 nan))");
    assert_eq!(i32x4_lanes(s), [1, -1, i32::MAX, 0]);
    let u = run_v128("(i32x4.trunc_sat_f32x4_u (v128.const f32x4 -1.0 1.5 4294967296.0 nan))");
    assert_eq!(
        u,
        [0, 0, 0, 0, 1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0,]
    );
    let s_zero = run_v128("(i32x4.trunc_sat_f64x2_s_zero (v128.const f64x2 1.9 -1.9))");
    assert_eq!(i32x4_lanes(s_zero), [1, -1, 0, 0]);
}

#[test]
fn int_float_conversions_round() {
    let s = run_v128("(f32x4.convert_i32x4_s (v128.const i32x4 -1 1 0x7fffffff 0x80000000))");
    assert_eq!(
        f32x4_lanes(s),
        [
            (-1.0f32).to_bits(),
            1.0f32.to_bits(),
            2147483648.0f32.to_bits(),
            (-2147483648.0f32).to_bits()
        ]
    );
    let u = run_v128("(f32x4.convert_i32x4_u (v128.const i32x4 0xffffffff 1 2 3))");
    assert_eq!(
        f32x4_lanes(u),
        [
            4294967296.0f32.to_bits(),
            1.0f32.to_bits(),
            2.0f32.to_bits(),
            3.0f32.to_bits()
        ]
    );
    let low_s = run_v128("(f64x2.convert_low_i32x4_s (v128.const i32x4 5 -6 0 0))");
    assert_eq!(f64x2_lanes(low_s), [5.0f64.to_bits(), (-6.0f64).to_bits()]);
}

#[test]
fn demote_and_promote_with_zero_fill() {
    let demote = run_v128("(f32x4.demote_f64x2_zero (v128.const f64x2 1.5 -0.25))");
    assert_eq!(
        f32x4_lanes(demote),
        [
            1.5f32.to_bits(),
            (-0.25f32).to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits()
        ]
    );
    let promote = run_v128("(f64x2.promote_low_f32x4 (v128.const f32x4 1.5 -0.25 99.0 99.0))");
    assert_eq!(
        f64x2_lanes(promote),
        [1.5f64.to_bits(), (-0.25f64).to_bits()]
    );
}

#[test]
fn swizzle_out_of_range_indices_yield_zero() {
    let got = run_v128(
        "(i8x16.swizzle (v128.const i8x16 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15) \
         (v128.const i8x16 15 14 1 0 16 0 31 7 2 3 4 5 6 7 8 9))",
    );
    assert_eq!(got, [15, 14, 1, 0, 0, 0, 0, 7, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn shuffle_selects_from_both_operands() {
    // Non-identity operand bytes: an implementation that echoes the
    // immediate indices (instead of indexing the operands) must fail.
    let got = run_v128(
        "(i8x16.shuffle 0 15 16 31 3 4 17 18 5 6 7 8 19 20 21 22 \
         (v128.const i8x16 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25) \
         (v128.const i8x16 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45))",
    );
    assert_eq!(
        got,
        [
            10, 25, 30, 45, 13, 14, 31, 32, 15, 16, 17, 18, 33, 34, 35, 36
        ]
    );
}
