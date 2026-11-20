# SIMD Unit 4b — v128 Memory Ops Completion (Plan Contract)

**Unit:** simd-4b (part of Unit 4: vendored suite grind) · **Depends on:** 1a–3 + vendored simd files (all on this branch)
**Context:** the vendored official simd files fail 379 assertions. Root
causes found by probe: (A) the load_extend/load_splat/load_zero families
and the load_lane/store_lane families decode and validate but have NO
lowering arms and NO runtime ops (`load_op()` maps only plain
`V128Load`); (B) `validate_simd_load_lane` (and likely
`validate_simd_store_lane`) pop in the wrong order (pops v128 before
the i32 address — the address is on TOP).

## Scope

20 memory instructions, full stack (lowering IR + runtime + validation
fix):

| Family | Instr (already decoded) | Semantics |
|---|---|---|
| load_extend | V128Load8x8S/U, V128Load16x4S/U, V128Load32x2S/U | read 8/4/2 lanes of 1/2/4 bytes, sign/zero-extend each to double width |
| load_splat | V128Load8Splat, 16, 32, 64 | read 1/2/4/8 bytes, replicate across all lanes |
| load_zero | V128Load32Zero, V128Load64Zero | read 4/8 bytes into lane 0, zero the rest |
| load_lane | V128Load8Lane, 16, 32, 64 | read N bytes into lane L of the v128 operand (others preserved) |
| store_lane | V128Store8Lane, 16, 32, 64 | store lane L's low N bytes to memory |

## Files

1. **Modify** `crates/baedeker-core/src/lower/mod.rs` — LoadOp variants, lane RegOps, lowering arms.
2. **Modify** `crates/baedeker-core/src/runtime/mod.rs` — load arm extensions, lane RegOp arms.
3. **Modify** `crates/baedeker-core/src/validate/mod.rs` — pop-order fixes ONLY.
4. **Modify** `crates/baedeker-core/src/binary/instr.rs` — ONLY if the lowering match needs the lane ops routed (no decode changes).
5. **Create** `crates/baedeker-core/tests/simd_memory_lanes.rs` — verbatim in C5.

No other file may be modified (`Cargo.lock` excepted).

## Contracts

### C1 — `LoadOp` extension (`lower/mod.rs`)

```rust
    V128Load8x8S,
    V128Load8x8U,
    V128Load16x4S,
    V128Load16x4U,
    V128Load32x2S,
    V128Load32x2U,
    V128Load8Splat,
    V128Load16Splat,
    V128Load32Splat,
    V128Load64Splat,
    V128Load32Zero,
    V128Load64Zero,
```

`byte_width()`: 8x8 → 8; 16x4 → 8; 32x2 → 8; 8Splat → 1; 16Splat → 2;
32Splat/32Zero → 4; 64Splat/64Zero → 8. `value_type()`/result type: all
`ValType::Vec(VecType::V128)`.

### C2 — Lane-indexed RegOps (`lower/mod.rs`)

The lane loads/stores take an EXTRA v128 operand, so they cannot ride
`RegOp::Load`/`RegOp::Store`:

```rust
    /// v128.loadN_lane: read N bytes into `lane` of the operand vector.
    V128LoadLane {
        width: u8,
        dst: Reg,
        vec: Reg,
        addr: Reg,
        memarg: MemArg,
        lane: u8,
    },
    /// v128.storeN_lane: write `lane`'s low N bytes to memory.
    V128StoreLane {
        width: u8,
        addr: Reg,
        vec: Reg,
        memarg: MemArg,
        lane: u8,
    },
```

(`width` ∈ {1, 2, 4, 8}; if the codebase prefers an enum, a
`V128LaneWidth` enum is acceptable — report the choice.)

### C3 — Lowering arms

**(a)** `load_op()` mapping additions (mechanical):

```rust
        Instr::V128Load8x8S(memarg) => (LoadOp::V128Load8x8S, memarg),
        Instr::V128Load8x8U(memarg) => (LoadOp::V128Load8x8U, memarg),
        Instr::V128Load16x4S(memarg) => (LoadOp::V128Load16x4S, memarg),
        Instr::V128Load16x4U(memarg) => (LoadOp::V128Load16x4U, memarg),
        Instr::V128Load32x2S(memarg) => (LoadOp::V128Load32x2S, memarg),
        Instr::V128Load32x2U(memarg) => (LoadOp::V128Load32x2U, memarg),
        Instr::V128Load8Splat(memarg) => (LoadOp::V128Load8Splat, memarg),
        Instr::V128Load16Splat(memarg) => (LoadOp::V128Load16Splat, memarg),
        Instr::V128Load32Splat(memarg) => (LoadOp::V128Load32Splat, memarg),
        Instr::V128Load64Splat(memarg) => (LoadOp::V128Load64Splat, memarg),
        Instr::V128Load32Zero(memarg) => (LoadOp::V128Load32Zero, memarg),
        Instr::V128Load64Zero(memarg) => (LoadOp::V128Load64Zero, memarg),
```

**(b)** Lane arms in the main lowering match (mirror the existing
scalar load/store lowering arms' pop/emit idiom). Stack order per spec
is `[i32 v128]`: the v128 operand is ON TOP — pop v128 FIRST, then the
i32 addr. Emit `RegOp::V128LoadLane`/`V128StoreLane` with `width`
1/2/4/8 and the decoded `lane`. (Validation already handles these
correctly — no validation changes in this unit.)

### C4 — Runtime

**(a)** In the `RegOp::Load` arm's value construction match, add
(verbatim):

```rust
                crate::lower::LoadOp::V128Load8x8S => {
                    let mut out = [0u8; 16];
                    for (lane, byte) in out.chunks_exact_mut(2).zip(bytes.iter()) {
                        let extended = (*byte as i8) as i16;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load8x8U => {
                    let mut out = [0u8; 16];
                    for (lane, byte) in out.chunks_exact_mut(2).zip(bytes.iter()) {
                        lane.copy_from_slice(&(*byte as u16).to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load16x4S => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(4).zip(bytes.chunks_exact(2)) {
                        let extended = i16::from_le_bytes(chunk.try_into().expect("width")) as i32;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load16x4U => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(4).zip(bytes.chunks_exact(2)) {
                        let extended = u16::from_le_bytes(chunk.try_into().expect("width")) as u32;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32x2S => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(8).zip(bytes.chunks_exact(4)) {
                        let extended = i32::from_le_bytes(chunk.try_into().expect("width")) as i64;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32x2U => {
                    let mut out = [0u8; 16];
                    for (lane, chunk) in out.chunks_exact_mut(8).zip(bytes.chunks_exact(4)) {
                        let extended = u32::from_le_bytes(chunk.try_into().expect("width")) as u64;
                        lane.copy_from_slice(&extended.to_le_bytes());
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load8Splat => {
                    Value::V128([bytes[0]; 16])
                }
                crate::lower::LoadOp::V128Load16Splat => {
                    let mut out = [0u8; 16];
                    for lane in out.chunks_exact_mut(2) {
                        lane.copy_from_slice(&bytes[..2]);
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32Splat => {
                    let mut out = [0u8; 16];
                    for lane in out.chunks_exact_mut(4) {
                        lane.copy_from_slice(&bytes[..4]);
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load64Splat => {
                    let mut out = [0u8; 16];
                    for lane in out.chunks_exact_mut(8) {
                        lane.copy_from_slice(&bytes[..8]);
                    }
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load32Zero => {
                    let mut out = [0u8; 16];
                    out[..4].copy_from_slice(&bytes[..4]);
                    Value::V128(out)
                }
                crate::lower::LoadOp::V128Load64Zero => {
                    let mut out = [0u8; 16];
                    out[..8].copy_from_slice(&bytes[..8]);
                    Value::V128(out)
                }
```

**(b)** New `execute_reg_op` arms (verbatim bodies; reuse the Load
arm's addr/mem/bounds idiom — the bounds check uses `width as usize`):

```rust
        RegOp::V128LoadLane {
            width,
            dst,
            vec,
            addr,
            memarg,
            lane,
        } => {
            let addr = expect_addr(get_reg(registers, *addr)?)?;
            let Value::V128(mut bytes) = get_reg(registers, *vec)? else {
                return Err(type_mismatch_v128(registers, *vec));
            };
            let store = require_store(store)?;
            let mem = store.shared_memory(memarg.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: memarg.memory.0,
                },
            })?;
            let mem = mem.borrow();
            let range = memory_bounds(&mem, memarg, addr, *width as usize)?;
            let start = *lane as usize * *width as usize;
            bytes[start..start + *width as usize].copy_from_slice(&mem[range]);
            drop(mem);
            set_reg(registers, *dst, Value::V128(bytes))?;
        }
        RegOp::V128StoreLane {
            width,
            addr,
            vec,
            memarg,
            lane,
        } => {
            let addr = expect_addr(get_reg(registers, *addr)?)?;
            let Value::V128(bytes) = get_reg(registers, *vec)? else {
                return Err(type_mismatch_v128(registers, *vec));
            };
            let store = require_store(store)?;
            let mem = store.shared_memory(memarg.memory.0).ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownMemory {
                    memory: memarg.memory.0,
                },
            })?;
            let mut mem = mem.borrow_mut();
            let range = memory_bounds(&mem, memarg, addr, *width as usize)?;
            let start = *lane as usize * *width as usize;
            mem[range].copy_from_slice(&bytes[start..start + *width as usize]);
        }
```

(If `memory_bounds`'s signature/borrow pattern differs from this sketch,
follow the Load/Store arms' exact idiom — the compiler is the arbiter.
The `drop(mem)` before `set_reg` may be unnecessary depending on that
idiom.)

### C5 — New test file `crates/baedeker-core/tests/simd_memory_lanes.rs`

Full file content:

```rust
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
    match execute_export(&reg, &store, "run", &[]).expect("executes").as_slice() {
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
            0x80, 0xFF, 0x01, 0x00, 0xFE, 0xFF, 0xFF, 0xFF,
            0x00, 0x00, 0x11, 0x00, 0x22, 0x00, 0x33, 0x00,
        ]
    );
    let u8_ = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load8x8_u (i32.const 0))))"
    ));
    assert_eq!(
        u8_,
        [
            0x80, 0x00, 0x01, 0x00, 0xFE, 0x00, 0xFF, 0x00,
            0x00, 0x00, 0x11, 0x00, 0x22, 0x00, 0x33, 0x00,
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
            0x80, 0x01, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF,
            0x00, 0x11, 0x00, 0x00, 0x22, 0x33, 0x00, 0x00,
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
            0x80, 0x01, 0xFE, 0xFF, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x11, 0x22, 0x33, 0x00, 0x00, 0x00, 0x00,
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
            0x00, 0x11, 0x00, 0x11, 0x00, 0x11, 0x00, 0x11,
            0x00, 0x11, 0x00, 0x11, 0x00, 0x11, 0x00, 0x11,
        ]
    );
    let d = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load64_splat (i32.const 8))))"
    ));
    assert_eq!(
        d,
        [
            0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
            0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
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
        [
            0x80, 0x01, 0xFE, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    );
    let z64 = run_v128(&format!(
        "(module {DATA} (func (export \"run\") (result v128) \
          (v128.load64_zero (i32.const 8))))"
    ));
    assert_eq!(
        z64,
        [
            0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
            0, 0, 0, 0, 0, 0, 0, 0,
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
    assert_eq!(got[5], 0xEE);
}
```

## Tests (the behavioral pin)

Every test in C5, plus the full existing suite stays green, and the
official simd suite failure count DROPS (rerun
`cargo test -p baedeker-core --test runtime_official -- --nocapture 2>&1 | grep 'official summary'`
and report the new numbers; remaining non-memory failures are Unit 4c
scope, NOT yours to fix).

## Constraints

- No new crates. No new files beyond C5. Everything else out of bounds.
- Do NOT modify validate/mod.rs — the lane validators are CORRECT (spec
  stack order is [i32 v128]; they pop v128 first, then i32). An earlier
  draft of this contract misread that; the file list entry for
  validate/mod.rs is void.
- load8_lane/store8_lane lane idx < 16; 16 → < 8; 32 → < 4; 64 → < 2
  (validation already enforces this — do not duplicate at runtime).
- The compiler is the arbiter for signatures/idioms; report deviations
  in the ambiguity footer.

## Completion (run in repo root; fix until all green)

```bash
cargo fmt --all
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|^error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
cargo test -p baedeker-core --test runtime_official -- --nocapture 2>&1 | grep 'official summary'
```

## Out of scope

Unit 4c (remaining conformance failures after this lands); perf.
