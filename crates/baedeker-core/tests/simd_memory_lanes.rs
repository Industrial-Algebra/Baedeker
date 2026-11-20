// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! SIMD unit 4b: v128 memory ops — load_extend, load_splat, load_zero,
//! load_lane, store_lane. Memory is initialized via a data segment;
//! every result is compared as exact LE lane bytes.

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

fn run_v128(wat: &str) -> [u8; 16] {
    let reg = lower(wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[])
        .expect("executes")
        .as_slice()
    {
        [Value::V128(bytes)] => *bytes,
        other => panic!("expected one v128 result, got {other:?}"),
    }
}

const DATA: &str = "(memory 1) (data (i32.const 0) \"\\80\\01\\fe\\ff\\00\\11\\22\\33\\44\\55\\66\\77\\88\\99\\aa\\bb\")";

#[test]
fn load_extend_signs_and_zeroes() {
    let s8 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load8x8_s (i32.const 0))))"
    ));
    // bytes 80 01 fe ff 00 11 22 33 sign-extended to i16 lanes
    assert_eq!(
        s8,
        [
            0x80, 0xFF, 0x01, 0x00, 0xFE, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x11, 0x00, 0x22, 0x00,
            0x33, 0x00,
        ]
    );
    let u8_ = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load8x8_u (i32.const 0))))"
    ));
    assert_eq!(
        u8_,
        [
            0x80, 0x00, 0x01, 0x00, 0xFE, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x11, 0x00, 0x22, 0x00,
            0x33, 0x00,
        ]
    );
    let s16 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load16x4_s (i32.const 0))))"
    ));
    // i16 lanes 0x0180, 0xfffe, 0x1100, 0x3322 sign-extended to i32
    assert_eq!(
        s16,
        [
            0x80, 0x01, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF, 0x00, 0x11, 0x00, 0x00, 0x22, 0x33,
            0x00, 0x00,
        ]
    );
    let s32 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load32x2_s (i32.const 0))))"
    ));
    // i32 lanes 0xfffe0180, 0x33221100 sign-extended to i64
    assert_eq!(
        s32,
        [
            0x80, 0x01, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x11, 0x22, 0x33, 0x00, 0x00,
            0x00, 0x00,
        ]
    );
}

#[test]
fn load_splat_replicates() {
    let b = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load8_splat (i32.const 3))))"
    ));
    assert_eq!(b, [0xFF; 16]);
    let w = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load16_splat (i32.const 4))))"
    ));
    assert_eq!(
        w,
        [
            0x00, 0x11, 0x00, 0x11, 0x00, 0x11, 0x00, 0x11, 0x00, 0x11, 0x00, 0x11, 0x00, 0x11,
            0x00, 0x11,
        ]
    );
    let d = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load64_splat (i32.const 8))))"
    ));
    assert_eq!(
        d,
        [
            0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
            0xAA, 0xBB,
        ]
    );
}

#[test]
fn load_zero_zero_fills() {
    let z32 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load32_zero (i32.const 0))))"
    ));
    assert_eq!(
        z32,
        [0x80, 0x01, 0xFE, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,]
    );
    let z64 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load64_zero (i32.const 8))))"
    ));
    assert_eq!(
        z64,
        [
            0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    );
}

#[test]
fn load_lane_replaces_one_lane() {
    let l8 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load8_lane 3 (i32.const 0) (v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))))"
    ));
    let mut expected = [0u8; 16];
    expected[3] = 0x80;
    assert_eq!(l8, expected);
    let l64 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load64_lane 1 (i32.const 8) (v128.const i64x2 0 0))))"
    ));
    let mut expected64 = [0u8; 16];
    expected64[8..].copy_from_slice(&[0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB]);
    assert_eq!(l64, expected64);
}

#[test]
fn store_lane_writes_one_lane() {
    let wat = format!(
        "(module {DATA} \
          (func (export \"run\") (result v128) \
           (v128.store8_lane 5 (i32.const 32) (v128.const i8x16 0 0 0 0 0 0xEE 0 0 0 0 0 0 0 0 0 0)) \
           (v128.load (i32.const 32))))"
    );
    let got = run_v128(&wat);
    assert_eq!(got, [0xEE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
}
