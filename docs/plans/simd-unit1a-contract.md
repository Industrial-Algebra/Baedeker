# SIMD Unit 1a — Saturating / Min-Max / Avgr / Q15 / Andnot + Lane Gaps (Plan Contract)

**Unit:** simd-1a · **Depends on:** none · **Followed by:** 1b (extend/extmul/narrow/dot/bitselect/any/all/bitmask), 2 (shifts+compares), 3 (float+conversions+swizzle/shuffle), 4 (vendor+un-defer)
**Ladder plan:** `docs/plans/simd-completeness.md` (read its Semantics risk register first).

## Scope

34 instructions, all table-extension over the existing generic IR (no
`RegOp` changes — `V128Binary`/`V128ExtractLane`/`V128ReplaceLane` are
already shape-generic):

1. `v128.andnot` (0xFD 79) and the integer lane families
   `add_sat_s/u`, `sub_sat_s/u` (i8x16: 111,112,114,115; i16x8:
   143,144,146,147), `min_s/u`, `max_s/u` (i8x16: 118–121; i16x8:
   150–153; i32x4: 182–185), `avgr_u` (i8x16: 123; i16x8: 155),
   `i16x8.q15mulr_sat_s` (130).
2. Lane extract/replace gaps: `i8x16.extract_lane_s/u` (21,22),
   `i8x16.replace_lane` (23), `i16x8.extract_lane_s/u` (24,25),
   `i16x8.replace_lane` (26), `i64x2.extract_lane/replace_lane` (29,30),
   `f64x2.extract_lane/replace_lane` (33,34).

NOTE: there is **no** i32x4/i64x2 saturating add/sub and **no** i64x2
min/max/avgr in the instruction set — do not invent them. Opcode table
cross-checked against the `wast` 240 encoder table
(`~/.cargo/registry/src/index.crates.io-*/wast-240.0.0/src/core/expr.rs`).

## Files

1. **Modify** `crates/baedeker-core/src/binary/instr.rs` — Instr variants + decode arms.
2. **Modify** `crates/baedeker-core/src/lower/mod.rs` — `V128BinaryKind` variants + lowering arms.
3. **Modify** `crates/baedeker-core/src/validate/mod.rs` — grouped or-patterns.
4. **Modify** `crates/baedeker-core/src/runtime/mod.rs` — lane macros + `v128_binary` + `apply_lane_binary`.
5. **Create** `crates/baedeker-core/tests/simd_integer_lanes.rs` — verbatim in C6.

No other file may be modified (`Cargo.lock` excepted).

## Contracts

### C1 — `Instr` variants (`binary/instr.rs`, next to the existing SIMD variants)

```rust
    V128Andnot,
    I8x16AddSatS,
    I8x16AddSatU,
    I8x16SubSatS,
    I8x16SubSatU,
    I8x16MinS,
    I8x16MinU,
    I8x16MaxS,
    I8x16MaxU,
    I8x16AvgrU,
    I16x8AddSatS,
    I16x8AddSatU,
    I16x8SubSatS,
    I16x8SubSatU,
    I16x8MinS,
    I16x8MinU,
    I16x8MaxS,
    I16x8MaxU,
    I16x8AvgrU,
    I16x8Q15MulrSatS,
    I32x4MinS,
    I32x4MinU,
    I32x4MaxS,
    I32x4MaxU,
    I8x16ExtractLaneS(u8),
    I8x16ExtractLaneU(u8),
    I8x16ReplaceLane(u8),
    I16x8ExtractLaneS(u8),
    I16x8ExtractLaneU(u8),
    I16x8ReplaceLane(u8),
    I64x2ExtractLane(u8),
    I64x2ReplaceLane(u8),
    F64x2ExtractLane(u8),
    F64x2ReplaceLane(u8),
```

Match the enum's existing derive/serde attributes (copy the neighbors').

### C2 — Decode arms (`decode_simd_instr`, next to the existing lane/arith arms)

```rust
        21 => Ok(Instr::I8x16ExtractLaneS(parse_lane_idx(cursor, base_offset)?)),
        22 => Ok(Instr::I8x16ExtractLaneU(parse_lane_idx(cursor, base_offset)?)),
        23 => Ok(Instr::I8x16ReplaceLane(parse_lane_idx(cursor, base_offset)?)),
        24 => Ok(Instr::I16x8ExtractLaneS(parse_lane_idx(cursor, base_offset)?)),
        25 => Ok(Instr::I16x8ExtractLaneU(parse_lane_idx(cursor, base_offset)?)),
        26 => Ok(Instr::I16x8ReplaceLane(parse_lane_idx(cursor, base_offset)?)),
        29 => Ok(Instr::I64x2ExtractLane(parse_lane_idx(cursor, base_offset)?)),
        30 => Ok(Instr::I64x2ReplaceLane(parse_lane_idx(cursor, base_offset)?)),
        33 => Ok(Instr::F64x2ExtractLane(parse_lane_idx(cursor, base_offset)?)),
        34 => Ok(Instr::F64x2ReplaceLane(parse_lane_idx(cursor, base_offset)?)),
        79 => Ok(Instr::V128Andnot),
        111 => Ok(Instr::I8x16AddSatS),
        112 => Ok(Instr::I8x16AddSatU),
        114 => Ok(Instr::I8x16SubSatS),
        115 => Ok(Instr::I8x16SubSatU),
        118 => Ok(Instr::I8x16MinS),
        119 => Ok(Instr::I8x16MinU),
        120 => Ok(Instr::I8x16MaxS),
        121 => Ok(Instr::I8x16MaxU),
        123 => Ok(Instr::I8x16AvgrU),
        130 => Ok(Instr::I16x8Q15MulrSatS),
        143 => Ok(Instr::I16x8AddSatS),
        144 => Ok(Instr::I16x8AddSatU),
        146 => Ok(Instr::I16x8SubSatS),
        147 => Ok(Instr::I16x8SubSatU),
        150 => Ok(Instr::I16x8MinS),
        151 => Ok(Instr::I16x8MinU),
        152 => Ok(Instr::I16x8MaxS),
        153 => Ok(Instr::I16x8MaxU),
        155 => Ok(Instr::I16x8AvgrU),
        182 => Ok(Instr::I32x4MinS),
        183 => Ok(Instr::I32x4MinU),
        184 => Ok(Instr::I32x4MaxS),
        185 => Ok(Instr::I32x4MaxU),
```

### C3 — IR kind + runtime (`lower/mod.rs`, `runtime/mod.rs`)

**(a)** Extend `pub enum V128BinaryKind` (lower/mod.rs:620) to:

```rust
pub enum V128BinaryKind {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Xor,
    Andnot,
    AddSatS,
    AddSatU,
    SubSatS,
    SubSatU,
    MinS,
    MinU,
    MaxS,
    MaxU,
    AvgrU,
    Q15MulrSatS,
}
```

**(b)** `runtime/mod.rs` — replace `lane_int_op!` with:

```rust
/// Lane-wise integer arithmetic (wrapping, per spec).
macro_rules! lane_int_op {
    ($kind:expr, $a:expr, $b:expr, $ty:ty) => {{
        let (a, b) = ($a as $ty, $b as $ty);
        match $kind {
            V128BinaryKind::Add => a.wrapping_add(b),
            V128BinaryKind::Sub => a.wrapping_sub(b),
            V128BinaryKind::Mul => a.wrapping_mul(b),
            V128BinaryKind::Div => a.wrapping_div(b),
            V128BinaryKind::AddSatS => a.saturating_add(b),
            V128BinaryKind::SubSatS => a.saturating_sub(b),
            V128BinaryKind::MinS => a.min(b),
            V128BinaryKind::MaxS => a.max(b),
            _ => unreachable!("unsigned kinds handled by lane_uint_op!"),
        }
    }};
}

/// Lane-wise unsigned-interpretation integer arithmetic. The `_u` variants
/// reinterpret the lane bytes as unsigned before operating; `avgr_u`
/// widens so `(a + b + 1)` cannot overflow.
macro_rules! lane_uint_op {
    ($kind:expr, $a:expr, $b:expr, $ty:ty) => {{
        let (a, b) = ($a as $ty, $b as $ty);
        match $kind {
            V128BinaryKind::AddSatU => a.saturating_add(b),
            V128BinaryKind::SubSatU => a.saturating_sub(b),
            V128BinaryKind::MinU => a.min(b),
            V128BinaryKind::MaxU => a.max(b),
            V128BinaryKind::AvgrU => ((a as u32 + b as u32 + 1) / 2) as $ty,
            _ => unreachable!("signed kinds handled by lane_int_op!"),
        }
    }};
}
```

**(c)** `runtime/mod.rs` — `v128_binary`: add `Andnot` to the byte-wise match (after `Xor`):

```rust
        V128BinaryKind::Andnot => {
            for i in 0..16 {
                out[i] = lhs[i] & !rhs[i];
            }
        }
```

**(d)** `runtime/mod.rs` — replace `apply_lane_binary` entirely with:

```rust
fn apply_lane_binary(
    shape: LaneShape,
    kind: V128BinaryKind,
    dst: &mut [u8],
    lhs: &[u8],
    rhs: &[u8],
) {
    // Unsigned-interpretation kinds route through `lane_uint_op!` on the
    // unsigned view of the lane width; everything else keeps the signed
    // path. q15mulr is i16-only and needs i32 intermediates.
    match shape {
        LaneShape::I8x16 => {
            if kind == V128BinaryKind::Q15MulrSatS {
                unreachable!("q15mulr_sat_s is i16x8-only");
            }
            if uses_unsigned_view(kind) {
                dst[0] = lane_uint_op!(kind, lhs[0], rhs[0], u8);
            } else {
                dst[0] = lane_int_op!(kind, lhs[0], rhs[0], i8) as u8;
            }
        }
        LaneShape::I16x8 => {
            let (a, b) = (
                i16::from_le_bytes(lhs.try_into().expect("width")),
                i16::from_le_bytes(rhs.try_into().expect("width")),
            );
            let result = if kind == V128BinaryKind::Q15MulrSatS {
                // q15: saturate_i16((a*b + 0x4000) >> 15) on i32 lanes.
                let product = a as i32 * b as i32;
                let shifted = (product + 0x4000) >> 15;
                shifted.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            } else if uses_unsigned_view(kind) {
                lane_uint_op!(kind, a, b, u16) as i16
            } else {
                lane_int_op!(kind, a, b, i16)
            };
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::I32x4 => {
            let (a, b) = (
                i32::from_le_bytes(lhs.try_into().expect("width")),
                i32::from_le_bytes(rhs.try_into().expect("width")),
            );
            let result = if uses_unsigned_view(kind) {
                lane_uint_op!(kind, a, b, u32) as i32
            } else {
                lane_int_op!(kind, a, b, i32)
            };
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::I64x2 => {
            let (a, b) = (
                i64::from_le_bytes(lhs.try_into().expect("width")),
                i64::from_le_bytes(rhs.try_into().expect("width")),
            );
            let result = lane_int_op!(kind, a, b, i64);
            dst.copy_from_slice(&result.to_le_bytes());
        }
        LaneShape::F32x4 => {
            let a = f32::from_bits(u32::from_le_bytes(lhs.try_into().expect("width")));
            let b = f32::from_bits(u32::from_le_bytes(rhs.try_into().expect("width")));
            let result = lane_float_op!(kind, a, b, f32);
            dst.copy_from_slice(&result.to_bits().to_le_bytes());
        }
        LaneShape::F64x2 => {
            let a = f64::from_bits(u64::from_le_bytes(lhs.try_into().expect("width")));
            let b = f64::from_bits(u64::from_le_bytes(rhs.try_into().expect("width")));
            let result = lane_float_op!(kind, a, b, f64);
            dst.copy_from_slice(&result.to_bits().to_le_bytes());
        }
    }
}

/// Whether the kind interprets lanes as unsigned (the `_u` families).
fn uses_unsigned_view(kind: V128BinaryKind) -> bool {
    matches!(
        kind,
        V128BinaryKind::AddSatU
            | V128BinaryKind::SubSatU
            | V128BinaryKind::MinU
            | V128BinaryKind::MaxU
            | V128BinaryKind::AvgrU
    )
}
```

(V128BinaryKind must derive PartialEq for `==`; if it currently only
derives Debug/Clone/Copy/Eq without PartialEq, add `PartialEq,` to the
derive list.)

### C4 — Lowering arms (`lower/mod.rs`, next to `Instr::I8x16Add`'s arm)

```rust
            Instr::V128Andnot => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::Andnot)?
            }
            Instr::I8x16AddSatS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::AddSatS)?
            }
            Instr::I8x16AddSatU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::AddSatU)?
            }
            Instr::I8x16SubSatS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::SubSatS)?
            }
            Instr::I8x16SubSatU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::SubSatU)?
            }
            Instr::I8x16MinS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MinS)?
            }
            Instr::I8x16MinU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MinU)?
            }
            Instr::I8x16MaxS => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MaxS)?
            }
            Instr::I8x16MaxU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::MaxU)?
            }
            Instr::I8x16AvgrU => {
                self.lower_v128_binary(offset, LaneShape::I8x16, V128BinaryKind::AvgrU)?
            }
            Instr::I16x8AddSatS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::AddSatS)?
            }
            Instr::I16x8AddSatU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::AddSatU)?
            }
            Instr::I16x8SubSatS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::SubSatS)?
            }
            Instr::I16x8SubSatU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::SubSatU)?
            }
            Instr::I16x8MinS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MinS)?
            }
            Instr::I16x8MinU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MinU)?
            }
            Instr::I16x8MaxS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MaxS)?
            }
            Instr::I16x8MaxU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::MaxU)?
            }
            Instr::I16x8AvgrU => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::AvgrU)?
            }
            Instr::I16x8Q15MulrSatS => {
                self.lower_v128_binary(offset, LaneShape::I16x8, V128BinaryKind::Q15MulrSatS)?
            }
            Instr::I32x4MinS => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MinS)?
            }
            Instr::I32x4MinU => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MinU)?
            }
            Instr::I32x4MaxS => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MaxS)?
            }
            Instr::I32x4MaxU => {
                self.lower_v128_binary(offset, LaneShape::I32x4, V128BinaryKind::MaxU)?
            }
```

Lane ops: mirror the existing `I32x4ExtractLane`/`I32x4ReplaceLane`/
`F32x4ExtractLane`/`F32x4ReplaceLane` lowering arms exactly (they call
`lower_extract_lane` / the replace helper with the right `LaneShape`),
adding arms for the ten new lane instrs:

```rust
            Instr::I8x16ExtractLaneS(lane) => self.lower_extract_lane(offset, LaneShape::I8x16, lane)?,
            Instr::I8x16ExtractLaneU(lane) => self.lower_extract_lane(offset, LaneShape::I8x16, lane)?,
            Instr::I8x16ReplaceLane(lane) => self.lower_replace_lane(offset, LaneShape::I8x16, lane)?,
            Instr::I16x8ExtractLaneS(lane) => self.lower_extract_lane(offset, LaneShape::I16x8, lane)?,
            Instr::I16x8ExtractLaneU(lane) => self.lower_extract_lane(offset, LaneShape::I16x8, lane)?,
            Instr::I16x8ReplaceLane(lane) => self.lower_replace_lane(offset, LaneShape::I16x8, lane)?,
            Instr::I64x2ExtractLane(lane) => self.lower_extract_lane(offset, LaneShape::I64x2, lane)?,
            Instr::I64x2ReplaceLane(lane) => self.lower_replace_lane(offset, LaneShape::I64x2, lane)?,
            Instr::F64x2ExtractLane(lane) => self.lower_extract_lane(offset, LaneShape::F64x2, lane)?,
            Instr::F64x2ReplaceLane(lane) => self.lower_replace_lane(offset, LaneShape::F64x2, lane)?,
```

(Check the replace helper's real name via the `I32x4ReplaceLane` arm; use
its exact form. `lower_extract_lane` handles sign/zero extension? NO — the
runtime does; verify `RegOp::V128ExtractLane` runtime handling extends
i8/i16 lanes correctly. If the runtime's extract for I8x16/I16x8 shapes
does not yet distinguish `_s`/`_u`, extend the `RegOp::V128ExtractLane`
runtime arm so `I8x16`/`I16x8` extraction sign-extends, and add a
sign-extended flag or a dedicated path for the `_u` forms. RECOMMENDED
minimal design: add `signed: bool` field to `RegOp::V128ExtractLane`
(defaulting semantics: signed extract for `_s`, unsigned for `_u`,
irrelevant for shapes ≥32-bit) and thread it from lowering; the runtime
uses it only for 8/16-bit shapes.)

### C5 — Validation (`validate/mod.rs`)

Join the new binary instrs to the existing `validate_numeric_binary(..., v128, v128)` group (the or-pattern containing `Instr::I8x16Add` … `Instr::F64x2Div`), adding:

```rust
        | Instr::V128Andnot
        | Instr::I8x16AddSatS
        | Instr::I8x16AddSatU
        | Instr::I8x16SubSatS
        | Instr::I8x16SubSatU
        | Instr::I8x16MinS
        | Instr::I8x16MinU
        | Instr::I8x16MaxS
        | Instr::I8x16MaxU
        | Instr::I8x16AvgrU
        | Instr::I16x8AddSatS
        | Instr::I16x8AddSatU
        | Instr::I16x8SubSatS
        | Instr::I16x8SubSatU
        | Instr::I16x8MinS
        | Instr::I16x8MinU
        | Instr::I16x8MaxS
        | Instr::I16x8MaxU
        | Instr::I16x8AvgrU
        | Instr::I16x8Q15MulrSatS
        | Instr::I32x4MinS
        | Instr::I32x4MinU
        | Instr::I32x4MaxS
        | Instr::I32x4MaxU
```

with the op-name string adjusted to `"simd.lane"` or left as the group's
existing string (whichever the or-pattern requires — match the group).

Lane instrs: mirror the existing `I32x4ExtractLane`/`I32x4ReplaceLane`
validation arms (extract: pop v128 push shape-scalar; replace: pop scalar
pop v128 push v128) for the ten new instrs, grouped in or-patterns.

### C6 — New test file `crates/baedeker-core/tests/simd_integer_lanes.rs`

Full file content:

```rust
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
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
        [Value::V128(bytes)] => *bytes,
        other => panic!("expected one v128 result, got {other:?}"),
    }
}

fn run_i32(body: &str) -> i32 {
    let wat = format!("(module (func (export \"run\") (result i32) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
        [Value::I32(v)] => *v,
        other => panic!("expected one i32 result, got {other:?}"),
    }
}

fn run_i64(body: &str) -> i64 {
    let wat = format!("(module (func (export \"run\") (result i64) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
        [Value::I64(v)] => *v,
        other => panic!("expected one i64 result, got {other:?}"),
    }
}

fn run_f64(body: &str) -> f64 {
    let wat = format!("(module (func (export \"run\") (result f64) {body}))");
    let reg = lower(&wat);
    let store = Store::instantiate(&reg).expect("instantiates");
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
        [Value::F64(v)] => *v,
        other => panic!("expected one f64 result, got {other:?}"),
    }
}

fn rep4(pattern: [u8; 4]) -> [u8; 16] {
    [
        pattern[0], pattern[1], pattern[2], pattern[3],
        pattern[0], pattern[1], pattern[2], pattern[3],
        pattern[0], pattern[1], pattern[2], pattern[3],
        pattern[0], pattern[1], pattern[2], pattern[3],
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
    assert_eq!(max_s, rep2([0x0002, 0x7fff, 0xffff, 0xffff]));
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
    for (chunk, lane) in expected
        .chunks_exact_mut(4)
        .zip([1u32, 0, 5, 7].iter())
    {
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
    let s = run_i32("(i8x16.extract_lane_s 0 (v128.const i8x16 0x80 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))");
    assert_eq!(s, -128);
    let u = run_i32("(i8x16.extract_lane_u 0 (v128.const i8x16 0x80 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))");
    assert_eq!(u, 128);
    let i16s = run_i32(
        "(i16x8.extract_lane_s 0 (v128.const i16x8 0x8000 0 0 0 0 0 0 0))",
    );
    assert_eq!(i16s, -32768);
    let i16u = run_i32(
        "(i16x8.extract_lane_u 0 (v128.const i16x8 0x8000 0 0 0 0 0 0 0))",
    );
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
    let v = run_i64(
        "(i64x2.extract_lane 0 (v128.const i64x2 0x0123456789abcdef 0xfffffffffffffffe))",
    );
    assert_eq!(v, 0x0123_4567_89ab_cdef);
    let got = run_v128(
        "(i64x2.replace_lane 1 (v128.const i64x2 0 0) (i64.const -1))",
    );
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
```

(If `module.lower()`'s return type differs from `RegModule`, adjust the
`lower` helper's signature to the repo's real form — the compiler is the
arbiter.)

## Tests (the behavioral pin)

Every test in C6, plus: full existing suite stays green (no regressions
in the 19,374-assertion official suite; `simd` files remain deferred).

## Constraints

- No new crates. No new files beyond C6. Files out of bounds: everything
  else, including `baedeker-ffi`, `swift/`, `book/`, `tests/runtime_official.rs`.
- Do NOT invent i32x4/i64x2 saturating ops or i64x2 min/max/avgr.
- Cross-check every opcode in C2 against the wast crate table if anything
  fails to encode — the contract's table is authoritative for NAMES; the
  wast crate is authoritative for ENCODINGS.
- Where a Contract references an existing helper's exact signature
  (`lower_replace_lane`, the replace validation arm), copy the repo's
  existing form — the compiler is the arbiter; report deviations in the
  ambiguity footer.

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

Units 1b/2/3/4 (see the ladder plan); relaxed SIMD; perf; the
`RegOp::V128Binary` serde representation (new enum variants ride the
existing derive).
