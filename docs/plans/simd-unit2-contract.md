# SIMD Unit 2 — Shifts + Compares (Plan Contract)

**Unit:** simd-2 · **Depends on:** 1a+1b (merged into this branch) · **Followed by:** 3 (float unary/min-max/conversions/swizzle/shuffle), 4 (vendor + un-defer)
**Ladder plan:** `docs/plans/simd-completeness.md` (read its Semantics risk register first).

## Scope

60 instructions (opcodes verified against the wast-240 encoder table):

| Family | Ops | Opcodes (0xFD-prefixed) |
|---|---|---|
| integer compares | i8x16/i16x8/i32x4 eq, ne, lt_s/u, gt_s/u, le_s/u, ge_s/u (10 each) | 35–64 |
| float compares | f32x4/f64x2 eq, ne, lt, gt, le, ge (6 each) | 65–70, 71–76 |
| i64x2 compares | eq, ne, lt_s, gt_s, le_s, ge_s | 214–219 |
| shifts | i8x16/i16x8/i32x4/i64x2 shl, shr_s, shr_u (3 each) | 107–109, 139–141, 171–173, 203–205 |

Two new IR shapes. NO i64x2 unsigned compares, NO float s/u compares —
do not invent them.

## Files

1. **Modify** `crates/baedeker-core/src/binary/instr.rs` — 60 `Instr` variants + decode arms.
2. **Modify** `crates/baedeker-core/src/lower/mod.rs` — 2 enums, 2 `RegOp` variants, 2 lowering helpers, 60 arms.
3. **Modify** `crates/baedeker-core/src/validate/mod.rs` — compare group + shift arm.
4. **Modify** `crates/baedeker-core/src/runtime/mod.rs` — 2 helper fns + `execute_reg_op` arms.
5. **Create** `crates/baedeker-core/tests/simd_shifts_compares.rs` — verbatim in C6.

No other file may be modified (`Cargo.lock` excepted).

## Contracts

### C1 — `Instr` variants (after the 1b group)

```rust
    I8x16Eq,
    I8x16Ne,
    I8x16LtS,
    I8x16LtU,
    I8x16GtS,
    I8x16GtU,
    I8x16LeS,
    I8x16LeU,
    I8x16GeS,
    I8x16GeU,
    I16x8Eq,
    I16x8Ne,
    I16x8LtS,
    I16x8LtU,
    I16x8GtS,
    I16x8GtU,
    I16x8LeS,
    I16x8LeU,
    I16x8GeS,
    I16x8GeU,
    I32x4Eq,
    I32x4Ne,
    I32x4LtS,
    I32x4LtU,
    I32x4GtS,
    I32x4GtU,
    I32x4LeS,
    I32x4LeU,
    I32x4GeS,
    I32x4GeU,
    F32x4Eq,
    F32x4Ne,
    F32x4Lt,
    F32x4Gt,
    F32x4Le,
    F32x4Ge,
    F64x2Eq,
    F64x2Ne,
    F64x2Lt,
    F64x2Gt,
    F64x2Le,
    F64x2Ge,
    I8x16Shl,
    I8x16ShrS,
    I8x16ShrU,
    I16x8Shl,
    I16x8ShrS,
    I16x8ShrU,
    I32x4Shl,
    I32x4ShrS,
    I32x4ShrU,
    I64x2Shl,
    I64x2ShrS,
    I64x2ShrU,
    I64x2Eq,
    I64x2Ne,
    I64x2LtS,
    I64x2GtS,
    I64x2LeS,
    I64x2GeS,
```

### C2 — Decode arms (`decode_simd_instr`)

```rust
        35 => Ok(Instr::I8x16Eq),
        36 => Ok(Instr::I8x16Ne),
        37 => Ok(Instr::I8x16LtS),
        38 => Ok(Instr::I8x16LtU),
        39 => Ok(Instr::I8x16GtS),
        40 => Ok(Instr::I8x16GtU),
        41 => Ok(Instr::I8x16LeS),
        42 => Ok(Instr::I8x16LeU),
        43 => Ok(Instr::I8x16GeS),
        44 => Ok(Instr::I8x16GeU),
        45 => Ok(Instr::I16x8Eq),
        46 => Ok(Instr::I16x8Ne),
        47 => Ok(Instr::I16x8LtS),
        48 => Ok(Instr::I16x8LtU),
        49 => Ok(Instr::I16x8GtS),
        50 => Ok(Instr::I16x8GtU),
        51 => Ok(Instr::I16x8LeS),
        52 => Ok(Instr::I16x8LeU),
        53 => Ok(Instr::I16x8GeS),
        54 => Ok(Instr::I16x8GeU),
        55 => Ok(Instr::I32x4Eq),
        56 => Ok(Instr::I32x4Ne),
        57 => Ok(Instr::I32x4LtS),
        58 => Ok(Instr::I32x4LtU),
        59 => Ok(Instr::I32x4GtS),
        60 => Ok(Instr::I32x4GtU),
        61 => Ok(Instr::I32x4LeS),
        62 => Ok(Instr::I32x4LeU),
        63 => Ok(Instr::I32x4GeS),
        64 => Ok(Instr::I32x4GeU),
        65 => Ok(Instr::F32x4Eq),
        66 => Ok(Instr::F32x4Ne),
        67 => Ok(Instr::F32x4Lt),
        68 => Ok(Instr::F32x4Gt),
        69 => Ok(Instr::F32x4Le),
        70 => Ok(Instr::F32x4Ge),
        71 => Ok(Instr::F64x2Eq),
        72 => Ok(Instr::F64x2Ne),
        73 => Ok(Instr::F64x2Lt),
        74 => Ok(Instr::F64x2Gt),
        75 => Ok(Instr::F64x2Le),
        76 => Ok(Instr::F64x2Ge),
        107 => Ok(Instr::I8x16Shl),
        108 => Ok(Instr::I8x16ShrS),
        109 => Ok(Instr::I8x16ShrU),
        139 => Ok(Instr::I16x8Shl),
        140 => Ok(Instr::I16x8ShrS),
        141 => Ok(Instr::I16x8ShrU),
        171 => Ok(Instr::I32x4Shl),
        172 => Ok(Instr::I32x4ShrS),
        173 => Ok(Instr::I32x4ShrU),
        203 => Ok(Instr::I64x2Shl),
        204 => Ok(Instr::I64x2ShrS),
        205 => Ok(Instr::I64x2ShrU),
        214 => Ok(Instr::I64x2Eq),
        215 => Ok(Instr::I64x2Ne),
        216 => Ok(Instr::I64x2LtS),
        217 => Ok(Instr::I64x2GtS),
        218 => Ok(Instr::I64x2LeS),
        219 => Ok(Instr::I64x2GeS),
```

### C3 — IR (`lower/mod.rs`)

**(a)** New enums next to `V128BinaryKind`:

```rust
/// Lane-wise compare kinds. Integer shapes use the s/u variants; float
/// shapes use the plain variants. Result lanes are all-ones/all-zeros at
/// the OPERAND lane width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V128CompareKind {
    Eq,
    Ne,
    LtS,
    LtU,
    GtS,
    GtU,
    LeS,
    LeU,
    GeS,
    GeU,
    Lt,
    Gt,
    Le,
    Ge,
}

/// Lane-wise shift kinds (`shr_s` is arithmetic on the signed lane
/// view; `shr_u` is logical on the unsigned view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V128ShiftKind {
    Shl,
    ShrS,
    ShrU,
}
```

**(b)** New `RegOp` variants (after the 1b variants):

```rust
    /// Lane-wise compare; dst lanes are all-ones/all-zeros at the
    /// operand lane width.
    V128Compare {
        dst: Reg,
        a: Reg,
        b: Reg,
        shape: LaneShape,
        kind: V128CompareKind,
    },
    /// Lane-wise shift by an i32 scalar `amount`, taken modulo the lane
    /// bit width (spec semantics; e.g. i8x16.shl by 9 == by 1).
    V128Shift {
        dst: Reg,
        src: Reg,
        amount: Reg,
        shape: LaneShape,
        kind: V128ShiftKind,
    },
```

**(c)** Lowering helpers (mirror 1a's `lower_v128_binary` structure):

```rust
    fn lower_v128_compare(&mut self, offset: ByteOffset, shape: LaneShape, kind: V128CompareKind) -> Result<(), LowerError>;
    fn lower_v128_shift(&mut self, offset: ByteOffset, shape: LaneShape, kind: V128ShiftKind) -> Result<(), LowerError>;
```

- compare: pop b then a (both v128), push v128 — same order as
  `lower_v128_binary`.
- shift: pop `amount` FIRST (i32 — use the same pop_expect pattern the
  scalar `Instr::I32Shl` lowering uses for its i32 operand; the compiler
  is the arbiter for the exact ValType spelling), then pop src (v128),
  push v128. WAT operand order is `(i8x16.shl src amount)`.

**(d)** 60 lowering arms, one line each:

```rust
            Instr::I8x16Eq => self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::Eq)?,
            Instr::I8x16LtU => self.lower_v128_compare(offset, LaneShape::I8x16, V128CompareKind::LtU)?,
            Instr::F32x4Ge => self.lower_v128_compare(offset, LaneShape::F32x4, V128CompareKind::Ge)?,
            Instr::I64x2LeS => self.lower_v128_compare(offset, LaneShape::I64x2, V128CompareKind::LeS)?,
            Instr::I8x16Shl => self.lower_v128_shift(offset, LaneShape::I8x16, V128ShiftKind::Shl)?,
            Instr::I32x4ShrU => self.lower_v128_shift(offset, LaneShape::I32x4, V128ShiftKind::ShrU)?,
```

…extended mechanically to all 60 (naming rule: `LtS`→`LtS`, float
`Lt`→`Lt`; `ShrS`→`ShrS`, etc.).

### C4 — Validation (`validate/mod.rs`)

- All 48 compares: join the 1a `validate_numeric_binary` group
  ((v128, v128) → v128).
- All 12 shifts: one dedicated grouped arm — pop i32 (amount), pop v128
  (src), push v128. Mirror the 1b bitselect arm's bespoke structure; the
  scalar i32 shift validation arm shows the i32 pop idiom.

### C5 — Runtime (`runtime/mod.rs`)

**(a)** Helper functions (verbatim):

```rust
/// Lane-wise compare. Result lanes are all-ones (true) or all-zeros
/// (false) at the operand lane width. Integer shapes route s/u kinds
/// through the signed/unsigned lane view; float shapes use Rust's
/// ordered/unordered semantics (NaN compares false except `ne`;
/// -0.0 == +0.0).
fn v128_compare(
    shape: LaneShape,
    kind: V128CompareKind,
    dst: &mut [u8],
    a: &[u8],
    b: &[u8],
) {
    let int_kind = |x, y, signed: bool| -> bool {
        match kind {
            V128CompareKind::Eq => x == y,
            V128CompareKind::Ne => x != y,
            V128CompareKind::LtS | V128CompareKind::Lt if signed => x < y,
            V128CompareKind::GtS | V128CompareKind::Gt if signed => x > y,
            V128CompareKind::LeS | V128CompareKind::Le if signed => x <= y,
            V128CompareKind::GeS | V128CompareKind::Ge if signed => x >= y,
            _ => unreachable!("unsigned kind on signed view"),
        }
    };
    let uint_kind = |x: u64, y: u64| -> bool {
        match kind {
            V128CompareKind::Eq => x == y,
            V128CompareKind::Ne => x != y,
            V128CompareKind::LtU => x < y,
            V128CompareKind::GtU => x > y,
            V128CompareKind::LeU => x <= y,
            V128CompareKind::GeU => x >= y,
            _ => unreachable!("signed/float kind on unsigned view"),
        }
    };
    let float_kind = |x: f64, y: f64| -> bool {
        match kind {
            V128CompareKind::Eq => x == y,
            V128CompareKind::Ne => x != y,
            V128CompareKind::Lt => x < y,
            V128CompareKind::Gt => x > y,
            V128CompareKind::Le => x <= y,
            V128CompareKind::Ge => x >= y,
            _ => unreachable!("integer kind on float shape"),
        }
    };
    let write = |d: &mut [u8], truth: bool| {
        for byte in d.iter_mut() {
            *byte = if truth { 0xFF } else { 0x00 };
        }
    };
    match shape {
        LaneShape::I8x16 => {
            for i in 0..16 {
                let (x, y) = (a[i] as i8, b[i] as i8);
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(a[i] as u64, b[i] as u64)
                } else {
                    int_kind(x, y, true)
                };
                write(&mut dst[i..i + 1], truth);
            }
        }
        LaneShape::I16x8 => {
            for i in 0..8 {
                let (x, y) = (
                    i16::from_le_bytes(a[i * 2..i * 2 + 2].try_into().expect("width")),
                    i16::from_le_bytes(b[i * 2..i * 2 + 2].try_into().expect("width")),
                );
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(x as u16 as u64, y as u16 as u64)
                } else {
                    int_kind(x, y, true)
                };
                write(&mut dst[i * 2..i * 2 + 2], truth);
            }
        }
        LaneShape::I32x4 => {
            for i in 0..4 {
                let (x, y) = (
                    i32::from_le_bytes(a[i * 4..i * 4 + 4].try_into().expect("width")),
                    i32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().expect("width")),
                );
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(x as u32 as u64, y as u32 as u64)
                } else {
                    int_kind(x, y, true)
                };
                write(&mut dst[i * 4..i * 4 + 4], truth);
            }
        }
        LaneShape::I64x2 => {
            for i in 0..2 {
                let (x, y) = (
                    i64::from_le_bytes(a[i * 8..i * 8 + 8].try_into().expect("width")),
                    i64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("width")),
                );
                let truth = if uses_unsigned_compare(kind) {
                    uint_kind(x as u64, y as u64)
                } else {
                    int_kind(x, y, true)
                };
                write(&mut dst[i * 8..i * 8 + 8], truth);
            }
        }
        LaneShape::F32x4 => {
            for i in 0..4 {
                let (x, y) = (
                    f32::from_bits(u32::from_le_bytes(a[i * 4..i * 4 + 4].try_into().expect("width"))),
                    f32::from_bits(u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().expect("width"))),
                );
                let truth = float_kind(x as f64, y as f64);
                write(&mut dst[i * 4..i * 4 + 4], truth);
            }
        }
        LaneShape::F64x2 => {
            for i in 0..2 {
                let (x, y) = (
                    f64::from_bits(u64::from_le_bytes(a[i * 8..i * 8 + 8].try_into().expect("width"))),
                    f64::from_bits(u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("width"))),
                );
                let truth = float_kind(x, y);
                write(&mut dst[i * 8..i * 8 + 8], truth);
            }
        }
    }
}

/// Whether the compare kind interprets integer lanes as unsigned.
fn uses_unsigned_compare(kind: V128CompareKind) -> bool {
    matches!(
        kind,
        V128CompareKind::LtU | V128CompareKind::GtU | V128CompareKind::LeU | V128CompareKind::GeU
    )
}

/// Lane-wise shift by a scalar amount taken modulo the lane bit width.
fn v128_shift(
    shape: LaneShape,
    kind: V128ShiftKind,
    dst: &mut [u8],
    src: &[u8],
    amount: i32,
) {
    let bits = shape.lane_width() as u32 * 8;
    let s = (amount as u32) % bits;
    macro_rules! shift_lane {
        ($x:expr, $t:ty, $u:expr) => {{
            let x = $x;
            match kind {
                V128ShiftKind::Shl => ((x as $t) << s) as $u,
                V128ShiftKind::ShrS => ((x as $t) >> s) as $u,
                V128ShiftKind::ShrU => ((x as <$u as SignedPair>::Unsigned) >> s) as $u,
            }
        }};
    }
    match shape {
        LaneShape::I8x16 => {
            for i in 0..16 {
                let x = src[i] as i8;
                let out: i8 = match kind {
                    V128ShiftKind::Shl => ((x as u8) << s) as i8,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u8) >> s) as i8,
                };
                dst[i] = out as u8;
            }
        }
        LaneShape::I16x8 => {
            for (d, chunk) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                let x = i16::from_le_bytes(chunk.try_into().expect("width"));
                let out: i16 = match kind {
                    V128ShiftKind::Shl => ((x as u16) << s) as i16,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u16) >> s) as i16,
                };
                d.copy_from_slice(&out.to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            for (d, chunk) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let x = i32::from_le_bytes(chunk.try_into().expect("width"));
                let out: i32 = match kind {
                    V128ShiftKind::Shl => ((x as u32) << s) as i32,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u32) >> s) as i32,
                };
                d.copy_from_slice(&out.to_le_bytes());
            }
        }
        LaneShape::I64x2 => {
            for (d, chunk) in dst.chunks_exact_mut(8).zip(src.chunks_exact(8)) {
                let x = i64::from_le_bytes(chunk.try_into().expect("width"));
                let out: i64 = match kind {
                    V128ShiftKind::Shl => ((x as u64) << s) as i64,
                    V128ShiftKind::ShrS => x >> s,
                    V128ShiftKind::ShrU => ((x as u64) >> s) as i64,
                };
                d.copy_from_slice(&out.to_le_bytes());
            }
        }
        LaneShape::F32x4 | LaneShape::F64x2 => unreachable!("no float shifts"),
    }
}
```

(The `shift_lane!` macro sketch above is NON-NORMATIVE — the four
explicit match blocks under it are the verbatim requirement; delete the
macro sketch when implementing.)

**(b)** `execute_reg_op` arms (following the 1b `V128Unary` exemplar):

```rust
        RegOp::V128Compare {
            dst,
            a,
            b,
            shape,
            kind,
        } => {
            let (Value::V128(a), Value::V128(b)) = (get_reg(registers, *a)?, get_reg(registers, *b)?)
            else {
                return Err(type_mismatch_v128(registers, *b));
            };
            let mut out = [0u8; 16];
            v128_compare(*shape, *kind, &mut out, &a, &b);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Shift {
            dst,
            src,
            amount,
            shape,
            kind,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let Value::I32(amount) = get_reg(registers, *amount)? else {
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::TypeMismatch {
                        expected: ValType::I32,
                        found: get_reg(registers, *amount)?.val_type(),
                    },
                });
            };
            let mut out = [0u8; 16];
            v128_shift(*shape, *kind, &mut out, &bytes, amount);
            set_reg(registers, *dst, Value::V128(out))?;
        }
```

(`type_mismatch_v128` exists since 1b; if its signature differs, follow
its existing form — the compiler is the arbiter.)

NOTE on the f32x4 compare helper: casting `f32 as f64` before comparing
preserves ordering/equality semantics exactly (both are binary32 values
embedded in binary64; NaN stays NaN, ±0 stays ±0). This is required to
keep the closure generic; it is semantically identical to comparing the
f32s directly.

### C6 — New test file `crates/baedeker-core/tests/simd_shifts_compares.rs`

Full file content:

```rust
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
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
        [Value::V128(bytes)] => *bytes,
        other => panic!("expected one v128 result, got {other:?}"),
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
            0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff,
            0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff,
        ]
    );
    let gt_u = run_v128(
        "(i32x4.gt_u (v128.const i32x4 0xffffffff 0x00000001 5 7) \
         (v128.const i32x4 0x00000001 0xffffffff 6 7))",
    );
    assert_eq!(
        gt_u,
        [
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]
    );
}

#[test]
fn i64x2_lt_s_eq_ne() {
    let lt_s = run_v128(
        "(i64x2.lt_s (v128.const i64x2 -2 5) (v128.const i64x2 -1 5))",
    );
    assert_eq!(
        lt_s,
        [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]
    );
    let ne = run_v128(
        "(i64x2.ne (v128.const i64x2 -2 5) (v128.const i64x2 -2 6))",
    );
    assert_eq!(
        ne,
        [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
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
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
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
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ]
    );
}

#[test]
fn f64x2_lt_ge_zero_signs() {
    let lt = run_v128(
        "(f64x2.lt (v128.const f64x2 1.0 -0.0) (v128.const f64x2 2.0 0.0))",
    );
    assert_eq!(
        lt,
        [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]
    );
    let ge = run_v128(
        "(f64x2.ge (v128.const f64x2 1.0 -0.0) (v128.const f64x2 2.0 0.0))",
    );
    assert_eq!(
        ge,
        [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
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
    let shl16 = run_v128(
        "(i16x8.shl (v128.const i16x8 0x8000 0x0001 0 0 0 0 0 0) (i32.const 1))",
    );
    assert_eq!(
        shl16,
        [
            0x00, 0x00, 0x02, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    );
    let shr64 = run_v128(
        "(i64x2.shr_s (v128.const i64x2 0x8000000000000000 -8) (i32.const 1))",
    );
    assert_eq!(
        shr64,
        [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0,
            0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ]
    );
}
```

## Tests (the behavioral pin)

Every test in C6, plus the full existing suite stays green (official
suite remains 88 files / 9 deferred / 19,374 assertions / 0 failures).

## Constraints

- No new crates. No new files beyond C6. Everything else out of bounds.
- Do NOT invent i64x2 unsigned compares or float s/u compares.
- Shift amount MUST be taken as `(amount as u32) % lane_bits` (so
  negative i32 amounts like -1 become 0xFFFFFFFF → 7 mod 8); shl drops
  shifted-out bits (no panic: shift after the mod is < lane width, use
  the unsigned lane view for shl).
- Float compares MUST use Rust's `==`/`<`/etc. (NaN false except ne,
  ±0 equal) — never bit equality.
- The compiler is the arbiter for helper signatures; report deviations
  in the ambiguity footer.

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

Units 3/4; float unary/min/max/pmin/pmax; conversions; swizzle/shuffle;
perf.
