# SIMD Completeness — WASM 3.0 Slice 2 (Ladder Plan)

**Status:** Plan. Branch `feature/simd-completeness`.
**Goal:** implement the full fixed-width v128 instruction surface (~200 ops
beyond the current ~55), vendor the official `simd_*.wast` spec files, and
un-defer them. Official suite deferred count 9 → 0 for the `simd` prefix.

## Current state

Decoded/executed today (see `binary/instr.rs::decode_simd_instr`): all
load/store lane variants, `v128.const`, all 6 splats, i32x4/f32x4
extract/replace lanes, not/and/or/xor, per-shape add/sub (+i32x4/i16x8/i64x2
mul), f32x4/f64x2 add/sub/mul/div. The IR is already generic:
`RegOp::V128Binary { shape: LaneShape, kind: V128BinaryKind }` with
lane-wise runtime dispatch (`v128_binary` / `apply_lane_binary` /
`lane_int_op!` / `lane_float_op!`) and one-line lowering
(`lower_v128_binary(offset, shape, kind)`) and validation
(`validate_numeric_binary(..., v128, v128)`) helpers.

**Therefore this slice is overwhelmingly table extension**: new
`V128BinaryKind`/`V128UnaryKind`/etc. entries + decode arms + grouped
validate or-patterns + one-line lowering arms + lane-macro arms. New IR
shapes are needed only for extmul/extend/narrow/dot/bitselect/
any_true/all_true/bitmask/shifts/compares/swizzle/shuffle/conversions.

## Opcode source of truth

The authoritative opcode↔mnemonic table is the `wast` crate (240)
encoder table at
`~/.cargo/registry/src/index.crates.io-*/wast-240.0.0/src/core/expr.rs`
(grep `0xfd`). Every contract's opcode mapping MUST be cross-checked
against it; unit tests encode wat via `wast` and round-trip through our
decoder, which catches mapping errors mechanically.

## Unit ladder (each independently gate-green; one dispatch each)

| Unit | Scope | Ops |
|---|---|---|
| **1a** | Saturating/min/max/avgr/q15/andnot + extract/replace gaps (i8x16 s/u, i16x8 s/u, i64x2, f64x2) | ~34 |
| **1b** | New IR: `V128Unary` (int abs/neg, popcnt), `V128Extend`, `V128ExtMul`, `V128ExtAddPairwise`, `V128Narrow`, `V128Dot`, `V128BitSelect`, `V128AnyTrue`, `V128AllTrue`, `V128Bitmask` | ~35 |
| **2** | Shifts (`V128Shift`: shl/shr_s/shr_u, amount mod lane width) + compares (`V128Cmp`: eq/ne + s/u + float; result lanes all-ones/all-zero) | ~76 |
| **3** | Float unary (abs/neg/sqrt/ceil/floor/trunc/nearest), min/max (NaN/±0 canonicalization), pmin/pmax, conversions (trunc_sat, convert, *_zero forms), swizzle, shuffle | ~45 |
| **4** | Vendor official `simd_*.wast` (~45 files) into `baedeker-testdata/spec/wast-official/`, un-defer the `("simd", …)` entry, grind conformance bugs | — |

## Semantics risk register (author-side care items)

- **Signed vs unsigned lane views**: `saturating_add` on `i8` is *not* the
  `_u` variant — unsigned kinds must reinterpret lanes (`u8`/`u16`/`u32`).
- **avgr_u**: widening needed — `(a as u16 + b as u16 + 1) >> 1` for u8
  lanes (no overflow), then truncate.
- **q15mulr_sat_s**: `saturate_i16((a as i32 * b as i32 + 0x4000) >> 15)`;
  `-32768 * -32768` must saturate to `32767`.
- **Float min/max**: spec canonicalization — if either operand NaN →
  canonical NaN (positive) output; `-0 < +0` handling. pmin/pmax are the
  non-canonicalizing variants (operand-selection semantics).
- **Shifts**: shift amount taken mod lane width (i8x16: mod 8).
- **Compares**: full-lane masks (`0xFF…`/`0x00`), not `1`/`0`.
- **extend/narrow**: narrow saturates (s) or clamps-to-zero (u: input range
  check, values < 0 become 0); extmul zero/sign-extends halves.
- **Conversions**: trunc_sat NaN → 0; out-of-range saturates.
- **Extract i8x16/i16x8**: `_s` sign-extends the lane to i32, `_u`
  zero-extends. Replace truncates the i32 to the lane width.

## Vendoring (Unit 4)

No vendoring script exists; Phases 3–4 vendored manually. U4 fetches
`test/core/simd/*.wast` from the WebAssembly/spec GitHub repo. **Open
question:** pin the rev by content-matching an existing vendored file
(e.g. `align.wast`) against the spec repo history before adding new files,
so the whole suite stays at one consistent revision.

## Out of scope

- relaxed-simd (separate proposal), GC-era types, memory64.
- Any perf work (lane loops are correct-first; autovectorization later).

## Verification (every unit)

`cargo fmt --all -- --check`; clippy core + workspace (both feature
configs); `cargo test -p baedeker-core` (+`--features aot`); official
suite stays 0 failures. New per-unit test files pin exact lane semantics
(wat → execute → exact `Value::V128` byte compare).
