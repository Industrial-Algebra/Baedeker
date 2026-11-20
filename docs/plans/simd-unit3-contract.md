# SIMD Unit 3 — Float Unary / Min-Max-PMin-PMax / Conversions / Swizzle / Shuffle (Plan Contract)

**Unit:** simd-3 · **Depends on:** 1a+1b+2 (merged into this branch) · **Followed by:** 4 (vendor + un-defer)
**Ladder plan:** `docs/plans/simd-completeness.md` (read its Semantics risk register first).

## Scope

34 instructions (opcodes verified against the wast-240 encoder table):

| Family | Ops | Opcodes (0xFD-prefixed) |
|---|---|---|
| float unary | f32x4/f64x2 abs, neg, sqrt, ceil, floor, trunc, nearest | f32x4: 224, 225, 227, 103–106; f64x2: 236, 237, 239, 116, 117, 122, 148 |
| min/max/pmin/pmax | f32x4: 232–235; f64x2: 244–247 | 8 ops |
| conversions | i32x4.trunc_sat_f32x4_s/u (248, 249); f32x4.convert_i32x4_s/u (250, 251); i32x4.trunc_sat_f64x2_s_zero/u_zero (252, 253); f32x4.demote_f64x2_zero (94); f64x2.promote_low_f32x4 (95); f64x2.convert_low_i32x4_s/u (254, 255) | 10 ops |
| swizzle / shuffle | i8x16.swizzle (14); i8x16.shuffle (13, 16 immediate bytes) | 2 ops |

NO relaxed_* ops, NO f64x2.convert_i64x2_* (GC-era) — out of scope.

## Files

1. **Modify** `crates/baedeker-core/src/binary/instr.rs` — 34 `Instr` variants + decode arms (shuffle reads 16 immediates).
2. **Modify** `crates/baedeker-core/src/lower/mod.rs` — 2 enums + 3 new `RegOp` variants + `V128BinaryKind` extension + helpers + 34 arms.
3. **Modify** `crates/baedeker-core/src/validate/mod.rs` — groups + shuffle-immediate bound check.
4. **Modify** `crates/baedeker-core/src/runtime/mod.rs` — wasm min/max helpers, float-unary/convert/swizzle/shuffle helpers + arms + `lane_float_op!`-adjacent dispatch.
5. **Create** `crates/baedeker-core/tests/simd_float_conversions.rs` — verbatim in C6.
6. **Conditionally modify** `crates/baedeker-core/src/validate/error.rs` — ONLY if a new error variant is needed for the shuffle-lane bound; reuse an existing variant if one fits.

## Contracts

### C1 — `Instr` variants (after the unit-2 group)

```rust
    F32x4Abs,
    F32x4Neg,
    F32x4Sqrt,
    F32x4Ceil,
    F32x4Floor,
    F32x4Trunc,
    F32x4Nearest,
    F64x2Abs,
    F64x2Neg,
    F64x2Sqrt,
    F64x2Ceil,
    F64x2Floor,
    F64x2Trunc,
    F64x2Nearest,
    F32x4Min,
    F32x4Max,
    F32x4PMin,
    F32x4PMax,
    F64x2Min,
    F64x2Max,
    F64x2PMin,
    F64x2PMax,
    I32x4TruncSatF32x4S,
    I32x4TruncSatF32x4U,
    F32x4ConvertI32x4S,
    F32x4ConvertI32x4U,
    I32x4TruncSatF64x2SZero,
    I32x4TruncSatF64x2UZero,
    F32x4DemoteF64x2Zero,
    F64x2PromoteLowF32x4,
    F64x2ConvertLowI32x4S,
    F64x2ConvertLowI32x4U,
    I8x16Swizzle,
    I8x16Shuffle([u8; 16]),
```

### C2 — Decode arms (`decode_simd_instr`)

```rust
        13 => {
            let mut lanes = [0u8; 16];
            for lane in lanes.iter_mut() {
                *lane = cursor.read_u8()?;
            }
            Ok(Instr::I8x16Shuffle(lanes))
        }
        14 => Ok(Instr::I8x16Swizzle),
        94 => Ok(Instr::F32x4DemoteF64x2Zero),
        95 => Ok(Instr::F64x2PromoteLowF32x4),
        103 => Ok(Instr::F32x4Ceil),
        104 => Ok(Instr::F32x4Floor),
        105 => Ok(Instr::F32x4Trunc),
        106 => Ok(Instr::F32x4Nearest),
        116 => Ok(Instr::F64x2Ceil),
        117 => Ok(Instr::F64x2Floor),
        122 => Ok(Instr::F64x2Trunc),
        148 => Ok(Instr::F64x2Nearest),
        224 => Ok(Instr::F32x4Abs),
        225 => Ok(Instr::F32x4Neg),
        227 => Ok(Instr::F32x4Sqrt),
        232 => Ok(Instr::F32x4Min),
        233 => Ok(Instr::F32x4Max),
        234 => Ok(Instr::F32x4PMin),
        235 => Ok(Instr::F32x4PMax),
        236 => Ok(Instr::F64x2Abs),
        237 => Ok(Instr::F64x2Neg),
        239 => Ok(Instr::F64x2Sqrt),
        244 => Ok(Instr::F64x2Min),
        245 => Ok(Instr::F64x2Max),
        246 => Ok(Instr::F64x2PMin),
        247 => Ok(Instr::F64x2PMax),
        248 => Ok(Instr::I32x4TruncSatF32x4S),
        249 => Ok(Instr::I32x4TruncSatF32x4U),
        250 => Ok(Instr::F32x4ConvertI32x4S),
        251 => Ok(Instr::F32x4ConvertI32x4U),
        252 => Ok(Instr::I32x4TruncSatF64x2SZero),
        253 => Ok(Instr::I32x4TruncSatF64x2UZero),
        254 => Ok(Instr::F64x2ConvertLowI32x4S),
        255 => Ok(Instr::F64x2ConvertLowI32x4U),
```

(The shuffle immediate read must follow the cursor idiom the decoder
uses for `parse_lane_idx` — `cursor.read_u8()` is the presumed form; the
compiler is the arbiter. If the existing decoder for lane immediates
uses a different method, match it.)

### C3 — IR (`lower/mod.rs`)

**(a)** New enums:

```rust
/// Lane-wise float unary kinds. `nearest` is round-half-to-EVEN (NOT
/// `f64::round`). abs/neg are bit operations on the sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V128FloatUnaryKind {
    Abs,
    Neg,
    Sqrt,
    Ceil,
    Floor,
    Trunc,
    Nearest,
}

/// Shape-changing lane conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V128ConvertKind {
    TruncSatF32x4S,
    TruncSatF32x4U,
    ConvertI32x4S,
    ConvertI32x4U,
    TruncSatF64x2SZero,
    TruncSatF64x2UZero,
    DemoteF64x2Zero,
    PromoteLowF32x4,
    ConvertLowI32x4S,
    ConvertLowI32x4U,
}
```

**(b)** Extend `V128BinaryKind` with four float-only kinds (after
`Q15MulrSatS`):

```rust
    FMin,
    FMax,
    PMin,
    PMax,
```

**(c)** New `RegOp` variants (after the unit-2 variants):

```rust
    /// Lane-wise float unary (abs/neg/sqrt/ceil/floor/trunc/nearest).
    V128FloatUnary {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
        kind: V128FloatUnaryKind,
    },
    /// Shape-changing conversion per `kind`.
    V128Convert {
        dst: Reg,
        src: Reg,
        kind: V128ConvertKind,
    },
    /// result[i] = b[i] < 16 ? a[b[i]] : 0.
    V128Swizzle {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// result[i] = lanes[i] < 16 ? a[lanes[i]] : b[lanes[i] - 16].
    V128Shuffle {
        dst: Reg,
        a: Reg,
        b: Reg,
        lanes: [u8; 16],
    },
```

**(d)** Lowering helpers:

```rust
    fn lower_v128_float_unary(&mut self, offset: ByteOffset, shape: LaneShape, kind: V128FloatUnaryKind) -> Result<(), LowerError>;
    fn lower_v128_convert(&mut self, offset: ByteOffset, kind: V128ConvertKind) -> Result<(), LowerError>;
    fn lower_v128_swizzle(&mut self, offset: ByteOffset) -> Result<(), LowerError>;
    fn lower_v128_shuffle(&mut self, offset: ByteOffset, lanes: [u8; 16]) -> Result<(), LowerError>;
```

Push types: float unary and swizzle/shuffle push v128. Conversions push
v128 of the destination width (all still ValType v128 — the shape lives
in the IR): TruncSatF32x4* consume F32x4 push I32x4-width; ConvertI32x4*
consume I32x4 push F32x4; TruncSatF64x2*Zero consume F64x2 push I32x4;
Demote consumes F64x2 pushes F32x4; Promote consumes F32x4 pushes F64x2;
ConvertLowI32x4* consume I32x4 push F64x2 — all v128 on the stack, so
every helper pops v128 and pushes v128.

min/max/pmin/pmax lower through the EXISTING `lower_v128_binary` with
the new kinds, e.g. `Instr::F32x4Min =>
self.lower_v128_binary(offset, LaneShape::F32x4, V128BinaryKind::FMin)?`.

**(e)** 34 lowering arms, one line each (naming rule mechanical;
`I8x16Shuffle(lanes)` passes `*lanes` through).

### C4 — Validation (`validate/mod.rs`)

- 14 float unary: unary v128→v128 — mirror the 1b group.
- 8 min/max/pmin/pmax: join the `validate_numeric_binary` group.
- 10 conversions: unary v128→v128 — mirror the 1b extend group.
- swizzle: binary (v128, v128)→v128 — join the binary group.
- shuffle: binary (v128, v128)→v128 PLUS every immediate must be < 32 —
  dedicated arm that first checks `lanes.iter().all(|&l| l < 32)`,
  returning the repo's lane-index validation error if not (reuse an
  existing variant if one fits; else add `InvalidShuffleLane { index: u8 }`
  to `validate/error.rs` — the ONLY permitted change there).

### C5 — Runtime (`runtime/mod.rs`)

**(a)** Wasm float min/max helpers (verbatim — these are the
semantics-critical pieces; Rust's `f32::min`/`max` do NOT match wasm):

```rust
/// wasm f32x4.min: NaN (either input) → canonical NaN; equal operands →
/// prefer -0 for min / +0 for max; else the smaller/larger.
fn wasm_f32_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::from_bits(0x7FC0_0000)
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

fn wasm_f32_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::from_bits(0x7FC0_0000)
    } else if a == b {
        if a.is_sign_negative() { b } else { a }
    } else if a < b {
        b
    } else {
        a
    }
}

fn wasm_f64_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::from_bits(0x7FF8_0000_0000_0000)
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

fn wasm_f64_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::from_bits(0x7FF8_0000_0000_0000)
    } else if a == b {
        if a.is_sign_negative() { b } else { a }
    } else if a < b {
        b
    } else {
        a
    }
}
```

**(b)** In `apply_lane_binary`, the F32x4 arm's result computation
becomes (same shape for F64x2 with f64 helpers):

```rust
            let result = match kind {
                V128BinaryKind::FMin => wasm_f32_min(a, b),
                V128BinaryKind::FMax => wasm_f32_max(a, b),
                V128BinaryKind::PMin => {
                    if b < a { b } else { a }
                }
                V128BinaryKind::PMax => {
                    if a < b { b } else { a }
                }
                _ => lane_float_op!(kind, a, b, f32),
            };
```

(pmin/pmax are operand-selecting and NON-canonicalizing: on unordered
input they return `a`'s payload. Int shapes never see these kinds —
their lanes route through `lane_int_op!`/`lane_uint_op!` unreachable
arms.)

**(c)** Float unary helper (verbatim):

```rust
fn v128_float_unary(
    shape: LaneShape,
    kind: V128FloatUnaryKind,
    dst: &mut [u8],
    src: &[u8],
) {
    let width = shape.lane_width();
    match width {
        4 => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                let r = match kind {
                    V128FloatUnaryKind::Abs => f32::from_bits(v.to_bits() & 0x7FFF_FFFF),
                    V128FloatUnaryKind::Neg => f32::from_bits(v.to_bits() ^ 0x8000_0000),
                    V128FloatUnaryKind::Sqrt => v.sqrt(),
                    V128FloatUnaryKind::Ceil => v.ceil(),
                    V128FloatUnaryKind::Floor => v.floor(),
                    V128FloatUnaryKind::Trunc => v.trunc(),
                    V128FloatUnaryKind::Nearest => v.round_ties_even(),
                };
                d.copy_from_slice(&r.to_bits().to_le_bytes());
            }
        }
        8 => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                let r = match kind {
                    V128FloatUnaryKind::Abs => f64::from_bits(v.to_bits() & 0x7FFF_FFFF_FFFF_FFFF),
                    V128FloatUnaryKind::Neg => f64::from_bits(v.to_bits() ^ 0x8000_0000_0000_0000),
                    V128FloatUnaryKind::Sqrt => v.sqrt(),
                    V128FloatUnaryKind::Ceil => v.ceil(),
                    V128FloatUnaryKind::Floor => v.floor(),
                    V128FloatUnaryKind::Trunc => v.trunc(),
                    V128FloatUnaryKind::Nearest => v.round_ties_even(),
                };
                d.copy_from_slice(&r.to_bits().to_le_bytes());
            }
        }
        _ => unreachable!("float unary is f32x4/f64x2 only"),
    }
}
```

**(d)** Conversion helper (verbatim — Rust's float→int `as` cast has
EXACTLY wasm trunc_sat semantics: toward zero, saturating, NaN → 0;
int→float `as` rounds to nearest; f64→f32 `as` rounds to nearest even):

```rust
fn v128_convert(kind: V128ConvertKind, dst: &mut [u8], src: &[u8]) {
    match kind {
        V128ConvertKind::TruncSatF32x4S => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as i32).to_le_bytes());
            }
        }
        V128ConvertKind::TruncSatF32x4U => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as u32).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertI32x4S => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f32).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertI32x4U => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = u32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f32).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::TruncSatF64x2SZero => {
            for (d, s) in dst.chunks_exact_mut(4).take(2).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as i32).to_le_bytes());
            }
            for d in dst[8..].chunks_exact_mut(4) {
                d.copy_from_slice(&0i32.to_le_bytes());
            }
        }
        V128ConvertKind::TruncSatF64x2UZero => {
            for (d, s) in dst.chunks_exact_mut(4).take(2).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&(v as u32).to_le_bytes());
            }
            for d in dst[8..].chunks_exact_mut(4) {
                d.copy_from_slice(&0u32.to_le_bytes());
            }
        }
        V128ConvertKind::DemoteF64x2Zero => {
            for (d, s) in dst.chunks_exact_mut(4).take(2).zip(src.chunks_exact(8)) {
                let v = f64::from_bits(u64::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&((v as f32).to_bits()).to_le_bytes());
            }
            for d in dst[8..].chunks_exact_mut(4) {
                d.copy_from_slice(&0f32.to_bits().to_le_bytes());
            }
        }
        V128ConvertKind::PromoteLowF32x4 => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(4).take(2)) {
                let v = f32::from_bits(u32::from_le_bytes(s.try_into().expect("width")));
                d.copy_from_slice(&((v as f64).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertLowI32x4S => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(4).take(2)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f64).to_bits()).to_le_bytes());
            }
        }
        V128ConvertKind::ConvertLowI32x4U => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(4).take(2)) {
                let v = u32::from_le_bytes(s.try_into().expect("width"));
                d.copy_from_slice(&((v as f64).to_bits()).to_le_bytes());
            }
        }
    }
}
```

**(e)** Swizzle + shuffle (verbatim):

```rust
/// result[i] = b[i] < 16 ? a[b[i]] : 0.
fn v128_swizzle(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for i in 0..16 {
        dst[i] = if b[i] < 16 { a[b[i] as usize] } else { 0 };
    }
}

/// result[i] = lanes[i] < 16 ? a[lanes[i]] : b[lanes[i] - 16]. Lane
/// bounds are validated (< 32) before lowering.
fn v128_shuffle(dst: &mut [u8], a: &[u8], b: &[u8], lanes: &[u8; 16]) {
    for i in 0..16 {
        let idx = lanes[i];
        dst[i] = if idx < 16 { a[idx as usize] } else { b[(idx - 16) as usize] };
    }
}
```

**(f)** `execute_reg_op` arms following the established exemplars
(V128FloatUnary like V128Unary; V128Convert like V128Unary; V128Swizzle/
V128Shuffle like V128Dot — two v128 operands, lanes carried in the IR).

### C6 — New test file `crates/baedeker-core/tests/simd_float_conversions.rs`

Full file content:

```rust
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
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
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
        [1.0f32.to_bits(), (-0.0f32).to_bits(), 0x7FC0_0000, 0x7FC0_0000]
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
    let pmin = run_v128(
        "(f64x2.pmin (v128.const f64x2 1.0 nan) (v128.const f64x2 2.0 3.0))",
    );
    assert_eq!(
        f64x2_lanes(pmin),
        [1.0f64.to_bits(), 0x7FF8_0000_0000_0000]
    );
    let pmax = run_v128(
        "(f64x2.pmax (v128.const f64x2 1.0 nan) (v128.const f64x2 2.0 3.0))",
    );
    assert_eq!(
        f64x2_lanes(pmax),
        [2.0f64.to_bits(), 0x7FF8_0000_0000_0000]
    );
    let pmin_zero = run_v128(
        "(f32x4.pmin (v128.const f32x4 -0.0 0.0 0 0) (v128.const f32x4 0.0 -0.0 0 0))",
    );
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
    let abs = run_v128(
        "(f32x4.abs (v128.const f32x4 -1.5 2.0 -0.0 -nan))",
    );
    assert_eq!(
        f32x4_lanes(abs),
        [1.5f32.to_bits(), 2.0f32.to_bits(), 0.0f32.to_bits(), 0x7FC0_0000]
    );
    let sqrt = run_v128("(f64x2.sqrt (v128.const f64x2 4.0 2.25))");
    assert_eq!(f64x2_lanes(sqrt), [2.0f64.to_bits(), 1.5f64.to_bits()]);
    let nearest = run_v128(
        "(f32x4.nearest (v128.const f32x4 0.5 1.5 2.5 -0.5))",
    );
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
    let s = run_v128(
        "(i32x4.trunc_sat_f32x4_s (v128.const f32x4 1.5 -1.5 3.0e10 nan))",
    );
    assert_eq!(i32x4_lanes(s), [1, -1, i32::MAX, 0]);
    let u = run_v128(
        "(i32x4.trunc_sat_f32x4_u (v128.const f32x4 -1.0 1.5 4.0e9 nan))",
    );
    assert_eq!(
        u,
        [
            0, 0, 0, 0, 1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0,
        ]
    );
    let s_zero = run_v128(
        "(i32x4.trunc_sat_f64x2_s_zero (v128.const f64x2 1.9 -1.9))",
    );
    assert_eq!(i32x4_lanes(s_zero), [1, -1, 0, 0]);
}

#[test]
fn int_float_conversions_round() {
    let s = run_v128(
        "(f32x4.convert_i32x4_s (v128.const i32x4 -1 1 0x7fffffff 0x80000000))",
    );
    assert_eq!(
        f32x4_lanes(s),
        [
            (-1.0f32).to_bits(),
            1.0f32.to_bits(),
            2147483648.0f32.to_bits(),
            (-2147483648.0f32).to_bits()
        ]
    );
    let u = run_v128(
        "(f32x4.convert_i32x4_u (v128.const i32x4 0xffffffff 1 2 3))",
    );
    assert_eq!(
        f32x4_lanes(u),
        [
            4294967296.0f32.to_bits(),
            1.0f32.to_bits(),
            2.0f32.to_bits(),
            3.0f32.to_bits()
        ]
    );
    let low_s = run_v128(
        "(f64x2.convert_low_i32x4_s (v128.const i32x4 5 -6 0 0))",
    );
    assert_eq!(
        f64x2_lanes(low_s),
        [5.0f64.to_bits(), (-6.0f64).to_bits()]
    );
}

#[test]
fn demote_and_promote_with_zero_fill() {
    let demote = run_v128(
        "(f32x4.demote_f64x2_zero (v128.const f64x2 1.5 -0.25))",
    );
    assert_eq!(
        f32x4_lanes(demote),
        [
            1.5f32.to_bits(),
            (-0.25f32).to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits()
        ]
    );
    let promote = run_v128(
        "(f64x2.promote_low_f32x4 (v128.const f32x4 1.5 -0.25 99.0 99.0))",
    );
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
    assert_eq!(
        got,
        [15, 14, 1, 0, 0, 0, 0, 7, 2, 3, 4, 5, 6, 7, 8, 9]
    );
}

#[test]
fn shuffle_selects_from_both_operands() {
    let got = run_v128(
        "(i8x16.shuffle 0 15 16 31 3 4 17 18 5 6 7 8 19 20 21 22 \
         (v128.const i8x16 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15) \
         (v128.const i8x16 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31))",
    );
    assert_eq!(
        got,
        [0, 15, 16, 31, 3, 4, 17, 18, 5, 6, 7, 8, 19, 20, 21, 22]
    );
}
```

## Tests (the behavioral pin)

Every test in C6, plus the full existing suite stays green (official
suite remains 88 files / 9 deferred / 19,374 assertions / 0 failures).

## Constraints

- No new crates. No new files beyond C6 (+ conditional
  `validate/error.rs` variant). Everything else out of bounds.
- wasm min/max MUST canonicalize NaN to the exact bit patterns in the
  helpers and order ±0 as specified — Rust's `f32::min`/`max` are
  WRONG here (they propagate the non-NaN operand).
- `nearest` MUST be `round_ties_even()` — `.round()` is half-away-from-
  zero and WRONG.
- trunc_sat relies on Rust `as` cast semantics (toward zero, saturate,
  NaN → 0) — do NOT add manual clamps.
- Do NOT invent relaxed_* or GC-era conversions.
- The compiler is the arbiter for helper/cursor signatures; report
  deviations in the ambiguity footer.

## Completion (run in repo root; fix until all green)

```bash
cargo fmt --all
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|^error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
```

## Out of scope

Unit 4 (vendoring + un-defer + grind); relaxed SIMD; GC-era ops; perf.
