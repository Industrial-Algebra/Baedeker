# memory64 — Unit 1 contract: binary surface

Branch `feature/memory64` (checked out at repo root). No git commands.
Scope: decode-side surface for 64-bit limits and memarg offsets. No
validation-semantics or runtime-addressing changes (U2/U3).

## Changes

1. **`Limits` (crates/baedeker-core/src/types.rs:242)** becomes
   `{ pub min: u64, pub max: Option<u64>, pub is64: bool }`.
   Fix all compile fallout mechanically (values stay small; casts must
   not silently truncate — prefer widening call sites to u64).

2. **Four `parse_limits` helpers** — `binary/memorysec.rs:41`,
   `binary/tablesec.rs:112`, `binary/importsec.rs:118` — decode flags:
   - `0x00`/`0x01`: as today, u32 LEB (cast to u64), `is64: false`
   - `0x04`: u64 LEB min, no max, `is64: true`
   - `0x05`: u64 LEB min + u64 LEB max, `is64: true`
   - anything else (incl. `0x02` shared, `0x06`, `0x08`): current
     `UnexpectedByte { expected: 0x00, found }` error, unchanged shape.
   u64 LEBs via existing `leb128::decode_u64` (crates/baedeker-core/src/binary/leb128.rs:146).

3. **`MemArg.offset` (crates/baedeker-core/src/types.rs:259-264) → u64**;
   memarg decode in `binary/instr.rs` switches the offset field from
   `decode_u32` to `decode_u64` (align stays u32). Existing i32-memory
   behavior unchanged (offsets < 2^32 pass through).

4. **AOT v6**: `aot.rs` serializes limits and memargs — widen both
   encodings to u64 + `is64` bit; bump `AOT_FORMAT_VERSION` to 6; the
   version-rejection test range (`1..AOT_FORMAT_VERSION`) updates
   automatically. Update any hardcoded v5 roundtrip fixtures.

5. **Data/elem i64 offsets**: `lower_const_expr` (lower/mod.rs:1970)
   already decodes `I64Const` — no change expected; if instantiate-time
   offset evaluation rejects i64 today, leave it (U3's job) — U1 only
   guarantees decode+lower succeed.

## Fixtures (new test file `crates/baedeker-core/tests/memory64_binary.rs`)

All byte strings computed twice (encoder table + hand LEB); use raw
`Module::decode`:

1. memory64 min-only — section body `04 01` under id 5:
   full `05 01 04 01` → one memory, `limits == { min: 1, max: None, is64: true }`
2. memory64 min+max: `05 01 05 01 0a` → `{ min: 1, max: Some(10), is64: true }`
3. table64: `04 01 04 02 70` (funcref 0x70, min 2) → `{ min: 2, max: None, is64: true }`
4. imported memory64: `02 01 01 61 01 6d 02 04 01` ("a"."m", kind 0x02) →
   imported memory limits is64, min 1
5. i64 data offset: `00 61 73 6d 01 00 00 00 05 01 04 01 0b 01 00 42 00 0b 01 78`
   → decodes; data segment active, offset expr contains i64.const 0
6. malformed flags 0x06: `05 01 06 01` → `UnexpectedByte` decode error
7. malformed 11-byte u64 LEB min: `05 01 04 80 80 80 80 80 80 80 80 80 80 00`
   → decode error
8. memarg u64 offset: valid module, memory64 + func with
   `i64.load offset=4294967296` (encode via `wast` crate from the test,
   mirroring `eh_binary.rs` helpers if present) → decoded `MemArg.offset == 0x1_0000_0000`

Existing tests that assert `limits.min` (memorysec.rs:106,115;
tablesec.rs:178,188,215) keep passing (u64 literal compare).

## Gates

`cargo fmt --all`; `cargo clippy -p baedeker-core --all-targets -- -D warnings`;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`;
`cargo test --workspace`; `cargo test -p baedeker-core --features aot`;
official suite unchanged: `153 files / 45,135 assertions / 0 failures`.

## Rules

Files only listed here; no scope creep. If an anchor is missing or the
file differs from the description, STOP and report (rule 7) — do not
improvise. Compute expected bytes yourself before writing them into
tests (compute-twice); report any disagreement with this contract's
bytes instead of silently picking one.
