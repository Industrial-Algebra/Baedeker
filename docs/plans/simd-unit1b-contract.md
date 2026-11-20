# SIMD Unit 1b — Unary / Extend / ExtMul / ExtAddPairwise / Narrow / Dot / Bitselect / Any-AllTrue / Bitmask (Plan Contract)

**Unit:** simd-1b · **Depends on:** 1a (merged into this branch) · **Followed by:** 2 (shifts + compares)
**Ladder plan:** `docs/plans/simd-completeness.md` (read its Semantics risk register first).

## Scope

52 instructions in 10 families. Unlike 1a these need NEW IR shapes
(shape-changing and ternary ops):

| Family | Ops | Opcodes (all 0xFD-prefixed) |
|---|---|---|
| integer unary | i8x16.abs/neg/popcnt; i16x8/i32x4/i64x2.abs/neg | 96–98, 128–129, 160–161, 192–193 |
| v128.any_true | 1 | 83 |
| all_true | ×4 shapes | 99, 131, 163, 195 |
| bitmask | ×4 shapes | 100, 132, 164, 196 |
| narrow | i8x16.narrow_i16x8_s/u; i16x8.narrow_i32x4_s/u | 101–102, 133–134 |
| extend | low/high × s/u × {i8x16→i16x8, i16x8→i32x4, i32x4→i64x2} | 135–138, 167–170, 199–202 |
| extmul | low/high × s/u × same three widenings | 156–159, 188–191, 220–223 |
| extadd_pairwise | i16x8←i8x16_s/u; i32x4←i16x8_s/u | 124–127 |
| dot | i32x4.dot_i16x8_s | 186 |
| bitselect | v128.bitselect | 82 |

Opcode numbers verified against the wast-240 encoder table. There is NO
i16x8/i32x4/i64x2 popcnt, NO i64x2 narrow/extend-from/extmul-to, NO
i64x2.dot, NO unsigned dot — do not invent them.

## Files

1. **Modify** `crates/baedeker-core/src/binary/instr.rs` — 52 `Instr` variants + decode arms.
2. **Modify** `crates/baedeker-core/src/lower/mod.rs` — `V128UnaryKind`, 10 new `RegOp` variants, 8 lowering helpers, 52 arms.
3. **Modify** `crates/baedeker-core/src/validate/mod.rs` — grouped arms.
4. **Modify** `crates/baedeker-core/src/runtime/mod.rs` — 6 helper fns + `execute_reg_op` arms.
5. **Create** `crates/baedeker-core/tests/simd_widening_lanes.rs` — verbatim in C7.

No other file may be modified (`Cargo.lock` excepted).

## Contracts

### C1 — `Instr` variants (`binary/instr.rs`, after the 1a SIMD group)

```rust
    V128Bitselect,
    V128AnyTrue,
    I8x16Abs,
    I8x16Neg,
    I8x16Popcnt,
    I8x16AllTrue,
    I8x16Bitmask,
    I8x16NarrowI16x8S,
    I8x16NarrowI16x8U,
    I16x8ExtAddPairwiseI8x16S,
    I16x8ExtAddPairwiseI8x16U,
    I16x8Abs,
    I16x8Neg,
    I16x8AllTrue,
    I16x8Bitmask,
    I16x8NarrowI32x4S,
    I16x8NarrowI32x4U,
    I16x8ExtendLowI8x16S,
    I16x8ExtendHighI8x16S,
    I16x8ExtendLowI8x16U,
    I16x8ExtendHighI8x16U,
    I16x8ExtMulLowI8x16S,
    I16x8ExtMulHighI8x16S,
    I16x8ExtMulLowI8x16U,
    I16x8ExtMulHighI8x16U,
    I32x4ExtAddPairwiseI16x8S,
    I32x4ExtAddPairwiseI16x8U,
    I32x4Abs,
    I32x4Neg,
    I32x4AllTrue,
    I32x4Bitmask,
    I32x4ExtendLowI16x8S,
    I32x4ExtendHighI16x8S,
    I32x4ExtendLowI16x8U,
    I32x4ExtendHighI16x8U,
    I32x4DotI16x8S,
    I32x4ExtMulLowI16x8S,
    I32x4ExtMulHighI16x8S,
    I32x4ExtMulLowI16x8U,
    I32x4ExtMulHighI16x8U,
    I64x2Abs,
    I64x2Neg,
    I64x2AllTrue,
    I64x2Bitmask,
    I64x2ExtendLowI32x4S,
    I64x2ExtendHighI32x4S,
    I64x2ExtendLowI32x4U,
    I64x2ExtendHighI32x4U,
    I64x2ExtMulLowI32x4S,
    I64x2ExtMulHighI32x4S,
    I64x2ExtMulLowI32x4U,
    I64x2ExtMulHighI32x4U,
```

### C2 — Decode arms (`decode_simd_instr`)

```rust
        82 => Ok(Instr::V128Bitselect),
        83 => Ok(Instr::V128AnyTrue),
        96 => Ok(Instr::I8x16Abs),
        97 => Ok(Instr::I8x16Neg),
        98 => Ok(Instr::I8x16Popcnt),
        99 => Ok(Instr::I8x16AllTrue),
        100 => Ok(Instr::I8x16Bitmask),
        101 => Ok(Instr::I8x16NarrowI16x8S),
        102 => Ok(Instr::I8x16NarrowI16x8U),
        124 => Ok(Instr::I16x8ExtAddPairwiseI8x16S),
        125 => Ok(Instr::I16x8ExtAddPairwiseI8x16U),
        128 => Ok(Instr::I16x8Abs),
        129 => Ok(Instr::I16x8Neg),
        131 => Ok(Instr::I16x8AllTrue),
        132 => Ok(Instr::I16x8Bitmask),
        133 => Ok(Instr::I16x8NarrowI32x4S),
        134 => Ok(Instr::I16x8NarrowI32x4U),
        135 => Ok(Instr::I16x8ExtendLowI8x16S),
        136 => Ok(Instr::I16x8ExtendHighI8x16S),
        137 => Ok(Instr::I16x8ExtendLowI8x16U),
        138 => Ok(Instr::I16x8ExtendHighI8x16U),
        156 => Ok(Instr::I16x8ExtMulLowI8x16S),
        157 => Ok(Instr::I16x8ExtMulHighI8x16S),
        158 => Ok(Instr::I16x8ExtMulLowI8x16U),
        159 => Ok(Instr::I16x8ExtMulHighI8x16U),
        126 => Ok(Instr::I32x4ExtAddPairwiseI16x8S),
        127 => Ok(Instr::I32x4ExtAddPairwiseI16x8U),
        160 => Ok(Instr::I32x4Abs),
        161 => Ok(Instr::I32x4Neg),
        163 => Ok(Instr::I32x4AllTrue),
        164 => Ok(Instr::I32x4Bitmask),
        167 => Ok(Instr::I32x4ExtendLowI16x8S),
        168 => Ok(Instr::I32x4ExtendHighI16x8S),
        169 => Ok(Instr::I32x4ExtendLowI16x8U),
        170 => Ok(Instr::I32x4ExtendHighI16x8U),
        186 => Ok(Instr::I32x4DotI16x8S),
        188 => Ok(Instr::I32x4ExtMulLowI16x8S),
        189 => Ok(Instr::I32x4ExtMulHighI16x8S),
        190 => Ok(Instr::I32x4ExtMulLowI16x8U),
        191 => Ok(Instr::I32x4ExtMulHighI16x8U),
        192 => Ok(Instr::I64x2Abs),
        193 => Ok(Instr::I64x2Neg),
        195 => Ok(Instr::I64x2AllTrue),
        196 => Ok(Instr::I64x2Bitmask),
        199 => Ok(Instr::I64x2ExtendLowI32x4S),
        200 => Ok(Instr::I64x2ExtendHighI32x4S),
        201 => Ok(Instr::I64x2ExtendLowI32x4U),
        202 => Ok(Instr::I64x2ExtendHighI32x4U),
        220 => Ok(Instr::I64x2ExtMulLowI32x4S),
        221 => Ok(Instr::I64x2ExtMulHighI32x4S),
        222 => Ok(Instr::I64x2ExtMulLowI32x4U),
        223 => Ok(Instr::I64x2ExtMulHighI32x4U),
```

### C3 — IR (`lower/mod.rs`)

**(a)** New enum next to `V128BinaryKind`:

```rust
/// Integer lane-wise unary kinds (`f32x4`/`f64x2` abs/neg are separate
/// float ops and are NOT part of this unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V128UnaryKind {
    Abs,
    Neg,
    Popcnt,
}
```

**(b)** New `RegOp` variants (after `V128Not`):

```rust
    /// Lane-wise integer unary (abs/neg/popcnt).
    V128Unary {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
        kind: V128UnaryKind,
    },
    /// Widen the low (`high == false`) or high half of `src`'s lanes to
    /// double width; dst shape is implied by `src_shape`.
    V128Extend {
        dst: Reg,
        src: Reg,
        src_shape: LaneShape,
        high: bool,
        signed: bool,
    },
    /// Extended multiply: widen both operands' selected half, then
    /// multiply lane-wise in the wider type.
    V128ExtMul {
        dst: Reg,
        a: Reg,
        b: Reg,
        src_shape: LaneShape,
        high: bool,
        signed: bool,
    },
    /// Pairwise saturating-free sum of adjacent lanes, doubling width.
    V128ExtAddPairwise {
        dst: Reg,
        src: Reg,
        src_shape: LaneShape,
        signed: bool,
    },
    /// Saturating-narrow `a` into the dst low half and `b` into the dst
    /// high half, halving width. `_u` clamps negative lanes to 0.
    V128Narrow {
        dst: Reg,
        a: Reg,
        b: Reg,
        dst_shape: LaneShape,
        signed: bool,
    },
    /// i32x4.dot_i16x8_s: signed dot of adjacent i16 pairs, wrapping.
    V128Dot {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// Per-bit select: mask bit 1 → a, 0 → b.
    V128Bitselect {
        dst: Reg,
        a: Reg,
        b: Reg,
        mask: Reg,
    },
    /// i32 1 if any bit in the vector is nonzero, else 0.
    V128AnyTrue {
        dst: Reg,
        src: Reg,
    },
    /// i32 1 if every lane (per `shape`) is nonzero, else 0.
    V128AllTrue {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
    },
    /// i32 with bit i = MSB of lane i (per `shape`).
    V128Bitmask {
        dst: Reg,
        src: Reg,
        shape: LaneShape,
    },
```

**(c)** Lowering helpers — mirror `lower_v128_binary`'s structure
(pop_expect/alloc_reg/emit) and the `V128Not` inline pattern. Signatures:

```rust
    fn lower_v128_unary(&mut self, offset: ByteOffset, shape: LaneShape, kind: V128UnaryKind) -> Result<(), LowerError>;
    fn lower_v128_extend(&mut self, offset: ByteOffset, src_shape: LaneShape, high: bool, signed: bool) -> Result<(), LowerError>;
    fn lower_v128_extmul(&mut self, offset: ByteOffset, src_shape: LaneShape, high: bool, signed: bool) -> Result<(), LowerError>;
    fn lower_v128_extadd_pairwise(&mut self, offset: ByteOffset, src_shape: LaneShape, signed: bool) -> Result<(), LowerError>;
    fn lower_v128_narrow(&mut self, offset: ByteOffset, dst_shape: LaneShape, signed: bool) -> Result<(), LowerError>;
    fn lower_v128_dot(&mut self, offset: ByteOffset) -> Result<(), LowerError>;
    fn lower_v128_bitselect(&mut self, offset: ByteOffset) -> Result<(), LowerError>;
    fn lower_v128_any_true(&mut self, offset: ByteOffset) -> Result<(), LowerError>;
    fn lower_v128_all_true(&mut self, offset: ByteOffset, shape: LaneShape) -> Result<(), LowerError>;
    fn lower_v128_bitmask(&mut self, offset: ByteOffset, shape: LaneShape) -> Result<(), LowerError>;
```

Shape rules (push the CORRECT result type):
- unary/extend/extmul/extadd/narrow/dot/bitselect push v128.
- any_true/all_true/bitmask push i32.
- unary result shape == operand shape. extend result width = double:
  I8x16→I16x8, I16x8→I32x4, I32x4→I64x2. extmul same. extadd same
  (I8x16→I16x8, I16x8→I32x4 only). narrow halves: I16x8→I8x16,
  I32x4→I16x8. dot: operands i16x8-width lanes (LaneShape::I16x8),
  result I32x4.

**(d)** Operand pop order** — bitselect pops mask FIRST (top of stack),
then b, then a (wat operand order is `(v128.bitselect a b mask)`).
extmul/narrow/dot pop b then a (standard binary order, like
lower_v128_binary).

**(e)** The 52 lowering arms, one line each calling the helpers, e.g.:

```rust
            Instr::I8x16Abs => self.lower_v128_unary(offset, LaneShape::I8x16, V128UnaryKind::Abs)?,
            Instr::I8x16Neg => self.lower_v128_unary(offset, LaneShape::I8x16, V128UnaryKind::Neg)?,
            Instr::I8x16Popcnt => self.lower_v128_unary(offset, LaneShape::I8x16, V128UnaryKind::Popcnt)?,
            Instr::V128Bitselect => self.lower_v128_bitselect(offset)?,
            Instr::V128AnyTrue => self.lower_v128_any_true(offset)?,
            Instr::I8x16AllTrue => self.lower_v128_all_true(offset, LaneShape::I8x16)?,
            Instr::I8x16Bitmask => self.lower_v128_bitmask(offset, LaneShape::I8x16)?,
            Instr::I8x16NarrowI16x8S => self.lower_v128_narrow(offset, LaneShape::I8x16, true)?,
            Instr::I8x16NarrowI16x8U => self.lower_v128_narrow(offset, LaneShape::I8x16, false)?,
            Instr::I16x8ExtAddPairwiseI8x16S => self.lower_v128_extadd_pairwise(offset, LaneShape::I8x16, true)?,
            Instr::I16x8ExtendLowI8x16S => self.lower_v128_extend(offset, LaneShape::I8x16, false, true)?,
            Instr::I16x8ExtendHighI8x16S => self.lower_v128_extend(offset, LaneShape::I8x16, true, true)?,
            Instr::I16x8ExtMulLowI8x16U => self.lower_v128_extmul(offset, LaneShape::I8x16, false, false)?,
            Instr::I32x4DotI16x8S => self.lower_v128_dot(offset)?,
            Instr::I64x2ExtMulHighI32x4S => self.lower_v128_extmul(offset, LaneShape::I32x4, true, true)?,
```

…extended mechanically to all 52 (naming rule: Low→`high: false`,
High→`true`; `S`→`signed: true`, `U`→`false`).

### C4 — Validation (`validate/mod.rs`)

- abs/neg/popcnt/extend/extadd_pairwise: unary v128→v128 — mirror the
  `Instr::V128Not` arm's call shape (it uses `validate_numeric_unary`).
- extmul/narrow/dot: binary (v128, v128)→v128 — join the
  `validate_numeric_binary` group used by 1a's binary instrs.
- bitselect: three v128 operands → v128. Pop mask, b, a (same order as
  lowering); check each against v128; push v128. Write a dedicated small
  arm (no existing ternary numeric helper).
- any_true/all_true/bitmask: v128 → i32 — mirror the existing
  `I32x4ExtractLane` arm shape (v128 operand, i32 result), grouped.

### C5 — Runtime (`runtime/mod.rs`)

**(a)** Helper functions (verbatim):

```rust
/// Lane-wise integer unary. abs/neg WRAP on the type's MIN value
/// (spec: i8x16.abs(-128) = -128).
fn v128_unary(shape: LaneShape, kind: V128UnaryKind, dst: &mut [u8], src: &[u8]) {
    match shape {
        LaneShape::I8x16 => {
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d = match kind {
                    V128UnaryKind::Abs => (*s as i8).wrapping_abs() as u8,
                    V128UnaryKind::Neg => (*s as i8).wrapping_neg() as u8,
                    V128UnaryKind::Popcnt => s.count_ones() as u8,
                };
            }
        }
        LaneShape::I16x8 => {
            for (d, s) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                let v = i16::from_le_bytes(s.try_into().expect("width"));
                let r = match kind {
                    V128UnaryKind::Abs => v.wrapping_abs(),
                    V128UnaryKind::Neg => v.wrapping_neg(),
                    V128UnaryKind::Popcnt => unreachable!("no i16x8 popcnt"),
                };
                d.copy_from_slice(&r.to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                let r = match kind {
                    V128UnaryKind::Abs => v.wrapping_abs(),
                    V128UnaryKind::Neg => v.wrapping_neg(),
                    V128UnaryKind::Popcnt => unreachable!("no i32x4 popcnt"),
                };
                d.copy_from_slice(&r.to_le_bytes());
            }
        }
        LaneShape::I64x2 => {
            for (d, s) in dst.chunks_exact_mut(8).zip(src.chunks_exact(8)) {
                let v = i64::from_le_bytes(s.try_into().expect("width"));
                let r = match kind {
                    V128UnaryKind::Abs => v.wrapping_abs(),
                    V128UnaryKind::Neg => v.wrapping_neg(),
                    V128UnaryKind::Popcnt => unreachable!("no i64x2 popcnt"),
                };
                d.copy_from_slice(&r.to_le_bytes());
            }
        }
        LaneShape::F32x4 | LaneShape::F64x2 => unreachable!("float unary is a later unit"),
    }
}

/// Widen `src`'s selected half to double lane width. Dst shape is
/// implied: I8x16→I16x8, I16x8→I32x4, I32x4→I64x2.
fn v128_extend(
    src_shape: LaneShape,
    high: bool,
    signed: bool,
    dst: &mut [u8],
    src: &[u8],
) {
    match src_shape {
        LaneShape::I8x16 => {
            let half: &[u8] = if high { &src[8..] } else { &src[..8] };
            for (d, s) in dst.chunks_exact_mut(2).zip(half.iter()) {
                let lane: u16 = if signed { (*s as i8) as i16 as u16 } else { *s as u16 };
                d.copy_from_slice(&lane.to_le_bytes());
            }
        }
        LaneShape::I16x8 => {
            let half: &[u8] = if high { &src[8..] } else { &src[..8] };
            for (d, s) in dst.chunks_exact_mut(4).zip(half.chunks_exact(2)) {
                let v = i16::from_le_bytes(s.try_into().expect("width"));
                let lane: u32 = if signed { v as i32 as u32 } else { v as u16 as u32 };
                d.copy_from_slice(&lane.to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            let half: &[u8] = if high { &src[8..] } else { &src[..8] };
            for (d, s) in dst.chunks_exact_mut(8).zip(half.chunks_exact(4)) {
                let v = i32::from_le_bytes(s.try_into().expect("width"));
                let lane: u64 = if signed { v as i64 as u64 } else { v as u32 as u64 };
                d.copy_from_slice(&lane.to_le_bytes());
            }
        }
        _ => unreachable!("extend widens i8x16/i16x8/i32x4 only"),
    }
}

/// Extended multiply: widen the selected half of both operands, then
/// multiply lane-wise in double width.
fn v128_extmul(
    src_shape: LaneShape,
    high: bool,
    signed: bool,
    dst: &mut [u8],
    a: &[u8],
    b: &[u8],
) {
    match src_shape {
        LaneShape::I8x16 => {
            let (a_half, b_half): (&[u8], &[u8]) = if high {
                (&a[8..], &b[8..])
            } else {
                (&a[..8], &b[..8])
            };
            for ((d, x), y) in dst
                .chunks_exact_mut(2)
                .zip(a_half.iter())
                .zip(b_half.iter())
            {
                let product: i32 = if signed {
                    (*x as i8 as i16 as i32) * (*y as i8 as i16 as i32)
                } else {
                    (*x as u8 as u16 as i32) * (*y as u8 as u16 as i32)
                };
                d.copy_from_slice(&(product as i16).to_le_bytes());
            }
        }
        LaneShape::I16x8 => {
            let (a_half, b_half): (&[u8], &[u8]) = if high {
                (&a[8..], &b[8..])
            } else {
                (&a[..8], &b[..8])
            };
            for ((d, x), y) in dst
                .chunks_exact_mut(4)
                .zip(a_half.chunks_exact(2))
                .zip(b_half.chunks_exact(2))
            {
                let x = i16::from_le_bytes(x.try_into().expect("width"));
                let y = i16::from_le_bytes(y.try_into().expect("width"));
                let product: i64 = if signed {
                    (x as i32 as i64) * (y as i32 as i64)
                } else {
                    (x as u16 as u32 as i64) * (y as u16 as u32 as i64)
                };
                d.copy_from_slice(&(product as i32).to_le_bytes());
            }
        }
        LaneShape::I32x4 => {
            let (a_half, b_half): (&[u8], &[u8]) = if high {
                (&a[8..], &b[8..])
            } else {
                (&a[..8], &b[..8])
            };
            for ((d, x), y) in dst
                .chunks_exact_mut(8)
                .zip(a_half.chunks_exact(4))
                .zip(b_half.chunks_exact(4))
            {
                let x = i32::from_le_bytes(x.try_into().expect("width"));
                let y = i32::from_le_bytes(y.try_into().expect("width"));
                let product: i128 = if signed {
                    (x as i64 as i128) * (y as i64 as i128)
                } else {
                    (x as u32 as u64 as i128) * (y as u32 as u64 as i128)
                };
                d.copy_from_slice(&(product as i64).to_le_bytes());
            }
        }
        _ => unreachable!("extmul widens i8x16/i16x8/i32x4 only"),
    }
}

/// Pairwise sum of adjacent lanes, doubling width. Never overflows the
/// wider lane.
fn v128_extadd_pairwise(
    src_shape: LaneShape,
    signed: bool,
    dst: &mut [u8],
    src: &[u8],
) {
    match src_shape {
        LaneShape::I8x16 => {
            for (d, pair) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                let sum: i32 = if signed {
                    (pair[0] as i8 as i16 as i32) + (pair[1] as i8 as i16 as i32)
                } else {
                    (pair[0] as u8 as u16 as i32) + (pair[1] as u8 as u16 as i32)
                };
                d.copy_from_slice(&(sum as i16).to_le_bytes());
            }
        }
        LaneShape::I16x8 => {
            for (d, pair) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let (x, y) = (
                    i16::from_le_bytes(pair[..2].try_into().expect("width")),
                    i16::from_le_bytes(pair[2..].try_into().expect("width")),
                );
                let sum: i64 = if signed {
                    (x as i32 as i64) + (y as i32 as i64)
                } else {
                    (x as u16 as u32 as i64) + (y as u16 as u32 as i64)
                };
                d.copy_from_slice(&(sum as i32).to_le_bytes());
            }
        }
        _ => unreachable!("extadd_pairwise widens i8x16/i16x8 only"),
    }
}

/// Saturating narrow: a's lanes → dst low half, b's lanes → dst high
/// half. `_s` saturates to the signed range; `_u` clamps negatives to 0
/// and saturates at the unsigned max (operating on the SIGNED source
/// value).
fn v128_narrow(dst_shape: LaneShape, signed: bool, dst: &mut [u8], a: &[u8], b: &[u8]) {
    let (dst_w, src_w) = match dst_shape {
        LaneShape::I8x16 => (1usize, 2usize),
        LaneShape::I16x8 => (2usize, 4usize),
        _ => unreachable!("narrow halves width only"),
    };
    let mid = 16 / 2;
    for (d, s) in dst[..mid]
        .chunks_exact_mut(dst_w)
        .zip(a.chunks_exact(src_w))
    {
        let v = match src_w {
            2 => i16::from_le_bytes(s.try_into().expect("width")) as i32,
            _ => i32::from_le_bytes(s.try_into().expect("width")),
        };
        let out = if signed {
            v.clamp(
                (-(1i32 << (dst_w * 8 - 1))) as i32,
                (1i32 << (dst_w * 8 - 1)) - 1,
            )
        } else {
            v.clamp(0, (1i32 << (dst_w * 8)) - 1)
        };
        d.copy_from_slice(&(out as u16).to_le_bytes()[..dst_w]);
    }
    for (d, s) in dst[mid..]
        .chunks_exact_mut(dst_w)
        .zip(b.chunks_exact(src_w))
    {
        let v = match src_w {
            2 => i16::from_le_bytes(s.try_into().expect("width")) as i32,
            _ => i32::from_le_bytes(s.try_into().expect("width")),
        };
        let out = if signed {
            v.clamp(
                (-(1i32 << (dst_w * 8 - 1))) as i32,
                (1i32 << (dst_w * 8 - 1)) - 1,
            )
        } else {
            v.clamp(0, (1i32 << (dst_w * 8)) - 1)
        };
        d.copy_from_slice(&(out as u16).to_le_bytes()[..dst_w]);
    }
}

/// i32x4.dot_i16x8_s: dst lane i = a[2i]·b[2i] + a[2i+1]·b[2i+1] on
/// SIGNED i16 lanes, products and sum WRAPPING at i32 (2·(−32768)²
/// wraps to i32::MIN per spec).
fn v128_dot(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for (d, i) in dst.chunks_exact_mut(4).zip(0..4usize) {
        let lane = |bytes: &[u8], idx: usize| {
            i16::from_le_bytes(bytes[idx * 2..idx * 2 + 2].try_into().expect("width")) as i32
        };
        let acc = lane(a, 2 * i)
            .wrapping_mul(lane(b, 2 * i))
            .wrapping_add(lane(a, 2 * i + 1).wrapping_mul(lane(b, 2 * i + 1)));
        d.copy_from_slice(&acc.to_le_bytes());
    }
}
```

**(b)** `execute_reg_op` arms — one per RegOp variant, following the
`V128Not` arm's get_reg/set_reg pattern. Exemplars:

```rust
        RegOp::V128Unary {
            dst,
            src,
            shape,
            kind,
        } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let mut out = [0u8; 16];
            v128_unary(*shape, *kind, &mut out, &bytes);
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128Bitselect {
            dst,
            a,
            b,
            mask,
        } => {
            let (Value::V128(a), Value::V128(b), Value::V128(m)) = (
                get_reg(registers, *a)?,
                get_reg(registers, *b)?,
                get_reg(registers, *mask)?,
            ) else {
                return Err(type_mismatch_v128(registers, *mask));
            };
            let mut out = [0u8; 16];
            for i in 0..16 {
                out[i] = (a[i] & m[i]) | (b[i] & !m[i]);
            }
            set_reg(registers, *dst, Value::V128(out))?;
        }
        RegOp::V128AnyTrue { dst, src } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            set_reg(
                registers,
                *dst,
                Value::I32(if bytes.iter().any(|&b| b != 0) { 1 } else { 0 }),
            )?;
        }
        RegOp::V128AllTrue { dst, src, shape } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let width = shape.lane_width();
            let all = bytes.chunks_exact(width).all(|lane| lane.iter().any(|&b| b != 0));
            set_reg(registers, *dst, Value::I32(if all { 1 } else { 0 }))?;
        }
        RegOp::V128Bitmask { dst, src, shape } => {
            let Value::V128(bytes) = get_reg(registers, *src)? else {
                return Err(type_mismatch_v128(registers, *src));
            };
            let width = shape.lane_width();
            let mut bits = 0u32;
            for (i, lane) in bytes.chunks_exact(width).enumerate() {
                if lane[width - 1] & 0x80 != 0 {
                    bits |= 1 << i;
                }
            }
            set_reg(registers, *dst, Value::I32(bits as i32))?;
        }
```

(`type_mismatch_v128` may not exist as a helper — construct the
`RuntimeErrorKind::TypeMismatch` inline exactly the way the `V128Not`
arm does, or add the tiny helper if cleaner; the compiler is the
arbiter.) Extend/ExtMul/ExtAddPairwise/Narrow/Dot arms follow the
V128Unary exemplar: get_reg operand(s) → call helper → set_reg
Value::V128(out).

### C6 — Validation details recap (fold into C4 implementation)

- Narrow: dst_shape I8x16 ← operands i16x8 (16 bytes each); I16x8 ←
  i32x4. Operands are v128 regardless (stack types are all v128; the
  shape lives in the IR only).

### C7 — New test file `crates/baedeker-core/tests/simd_widening_lanes.rs`

Full file content:

```rust
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
    let neg32 = run_v128(
        "(i32x4.neg (v128.const i32x4 0x80000000 0x00000000 0x00000001 0xffffffff))",
    );
    assert_eq!(neg32, rep1([0x80000000, 0x00000000, 0xffffffff, 0x00000001]));
    let abs64 = run_v128(
        "(i64x2.abs (v128.const i64x2 0x8000000000000000 0xffffffffffffffff))",
    );
    assert_eq!(abs64, rep64([0x8000000000000000, 0x0000000000000002]));
}

#[test]
fn any_true_and_all_true() {
    let any_set = run_i32(
        "(v128.any_true (v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1))",
    );
    assert_eq!(any_set, 1);
    let any_clear = run_i32(
        "(v128.any_true (v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))",
    );
    assert_eq!(any_clear, 0);
    let all_false = run_i32(
        "(i16x8.all_true (v128.const i16x8 1 2 3 4 5 6 7 0))",
    );
    assert_eq!(all_false, 0);
    let all_true = run_i32(
        "(i16x8.all_true (v128.const i16x8 1 2 3 4 5 6 7 8))",
    );
    assert_eq!(all_true, 1);
}

#[test]
fn bitmask_packs_lane_msbs() {
    let i8 = run_i32(
        "(i8x16.bitmask (v128.const i8x16 0x80 0x00 0xff 0x01 0x80 0x00 0xff 0x01 \
          0x80 0x00 0xff 0x01 0x80 0x00 0xff 0x01))",
    );
    assert_eq!(i8 as u32, 0x5555);
    let i32 = run_i32(
        "(i32x4.bitmask (v128.const i32x4 0x80000000 0x7fffffff 0xffffffff 0x00000001))",
    );
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
        rep2([0xff80, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007])
    );
    let high_u = run_v128(&format!("(i16x8.extend_high_i8x16_u {src})"));
    assert_eq!(
        high_u,
        rep2([0x0008, 0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x000e, 0x000f])
    );
    let high_s32 = run_v128(
        "(i32x4.extend_high_i16x8_s (v128.const i16x8 0x8000 0x0001 0x7fff 0xffff \
          0x8000 0x0001 0x7fff 0xffff))",
    );
    assert_eq!(
        high_s32,
        rep1([0xffff8000, 0x00000001, 0x7fffffff, 0xffffffff])
    );
    let low_s64 = run_v128(
        "(i64x2.extend_low_i32x4_s (v128.const i32x4 0x80000000 0x00000001 0 0))",
    );
    assert_eq!(
        low_s64,
        rep64([0xffffffff80000000, 0x0000000000000001])
    );
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
        rep2([0xff00, 0x0006, 0x000c, 0x0014, 0x001e, 0x002a, 0x0038, 0x0048])
    );
    let high_u = run_v128(
        "(i32x4.extmul_high_i16x8_u \
         (v128.const i16x8 0 0 0 0 0xffff 0x0002 0x00ff 0x8000) \
         (v128.const i16x8 0 0 0 0 0x0002 0x0003 0x0100 0x0002))",
    );
    assert_eq!(high_u, rep1([0x0001fffe, 0x00000006, 0x0000ff00, 0x00010000]));
    let high_s64 = run_v128(
        "(i64x2.extmul_high_i32x4_s \
         (v128.const i32x4 0 0 0xffffffff 0x00000002) \
         (v128.const i32x4 0 0 0x00000002 0x00000003))",
    );
    assert_eq!(
        high_s64,
        rep64([0xfffffffffffffffe, 0x0000000000000006])
    );
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
    assert_eq!(normal, rep1([0x40000002, 0x00000012, 0x00000032, 0x00000062]));
    let wrapping = run_v128(
        "(i32x4.dot_i16x8_s (v128.const i16x8 0x8000 0x8000 0x8000 0x8000 \
          0x8000 0x8000 0x8000 0x8000) \
         (v128.const i16x8 0x8000 0x8000 0x8000 0x8000 \
          0x8000 0x8000 0x8000 0x8000))",
    );
    assert_eq!(wrapping, rep1([0x80000000, 0x80000000, 0x80000000, 0x80000000]));
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
```

## Tests (the behavioral pin)

Every test in C7, plus the full existing suite stays green (official
suite remains 88 files / 9 deferred / 19,374 assertions / 0 failures).

## Constraints

- No new crates. No new files beyond C7. Everything else out of bounds.
- Do NOT invent ops outside the table (no i16x8 popcnt, no i64x2
  narrow/dot, no unsigned dot, no float abs/neg here).
- `abs`/`neg` MUST use wrapping semantics (`.abs()`/`-` panic in debug
  on MIN).
- `dot` MUST wrap at i32 (wrapping_mul + wrapping_add).
- narrow `_u` operates on the SIGNED source value and clamps to
  [0, unsigned_max].
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

Units 2/3/4; f32x4/f64x2 abs/neg/min/max/etc; shifts; compares;
conversions; swizzle/shuffle; perf.
