# Changelog

All notable changes to Baedeker are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — Threads (classic proposal: shared memory + atomics)

Slice 6, pinned to the frozen browser-shipped state of WebAssembly/threads
(`main-legacy`, commit `33b2dcd6`). Single-agent semantics; shared-everything
threads (`thread.spawn`, shared GC, per-op ordering params) stays out of scope.

- Shared linear memory: `Limits::shared` end to end (binary limits tags
  `0x02`/`0x03`, import/link matching requires shared≡shared), with
  shared-requires-maximum enforced at decode (`shared memory must have
  maximum`) and validation.
- The `0xFE` atomic instruction family: atomic loads/stores at i8/i16/i32/i64,
  read-modify-write (add/sub/and/or/xor/xchg) and cmpxchg at i32/i64,
  `memory.atomic.wait32`/`wait64`, `memory.atomic.notify`, and
  `memory.atomic.fence`. Natural alignment is a validation requirement.
- Execution: atomics behave as their plain access with the memory family's
  OOB/misalignment traps; `fence` is a no-op; `wait` returns 1 on a value
  mismatch and 2 on an epsilon timeout, `notify` returns 0 — the deterministic
  single-agent answers the proposal specifies.
- Official corpus: vendored `atomic.wast` and `atomic_wait_notify.wast` (the
  file's four multi-agent directives — two `thread` blocks, two `wait`
  directives — are balanced-form stripped before parsing), plus the pinned
  fork deltas `binary-threads-delta.wast` (the shared-table rejection) and
  `memory-threads-delta.wast` (two shared memory types + the shared-no-max
  `assert_invalid`). The `linking.wast` threads delta is empty. Suite:
  **204 files, 3 deferred, 0 failures**. `thread.wast` joins the deferral
  ledger (`skip-stack-guard-page.wast`, `names.wast`) with the recorded reason
  "multi-agent behavioral coverage (thread/wait directives) — single-agent
  engine".
- wasm-smith campaigns (the `gc_u4_smith` test and the `gc-smith` fuzz target)
  enable threads and admit `InstructionKind::MemoryInt`, so generated modules
  carry decoded `0xFE` atomics. On the pinned seed set the campaign stays total
  (403 generated = decoded = validated = lowered) and decodes 148 atomic
  instructions across 45 modules; execution stays out of the gate (a generated
  `atomic.wait` could block).

## [0.1.0] — 2026-08-04

Baedeker's first public release. A `no_std` + `alloc` WebAssembly 2.0 engine
(decode → validate → lower → execute) targeting every platform Rust compiles
to, with a complete platform-integration surface for embedding. WebAssembly 3.0
proposals (tail calls, exception handling, memory64, GC, threads) are tracked
as the post-0.1.0 roadmap.

### Added — Engine (WASM 2.0 core)

- Register-IR interpreter: binary decoding, structural validation, lowering to
  a register IR, and execution with full control flow, calls, memory, tables,
  globals, bulk memory, and multi-value.
- Reference types: `call_ref`, `br_on_null`/`br_on_non_null`, `ref.as_non_null`,
  `ref.is_null`, typed funcref tables, cross-instance funcref identity.
- Fixed-width SIMD (`v128`): arithmetic, memory, and lane operations.
- Multi-module linking: shared memories, globals, and tables across instances;
  host functions with type-checked registration.
- Interpreter fuel (`Store::set_fuel`) and a categorised `RuntimeError`/trap model.

### Added — Verification & hardening

- Vendored official WebAssembly spec test suite: **85 files, 19,204 assertions,
  993 modules, 0 failures** (12 files deferred — the 3.0 surface).
- Differential testing vs Wasmtime (wasm-smith sweep, 0 divergences) and
  trap-edge fuzzing.
- cargo-fuzz targets for decode / validate / lower / trap-edges / smith modules.

### Added — Platform integration (Phase 5)

- `baedeker-core`: the engine, `no_std` + `alloc`.
- `baedeker-ffi`: C ABI (staticlib/cdylib) with opaque handles, host functions,
  memory APIs, fuel, and a cbindgen-generated header (`baedeker.h`).
- `baedeker-cli`: decode/inspect, validate+lower, and `compile` (wasm → AOT).
- `baedeker-borsalino`: Borsalino GPU backend adapter with verified dispatch
  (Layer 1 structural + Layer 2 exact-match numerical verification).
- `baedeker-gpu`: GPU compute host module exposing the `baedeker:gpu` import ABI.
- AOT pipeline: postcard-serialized register IR (`BDKAOT1` envelope) for
  version-locked ahead-of-time loading.
- Swift package (`swift/`): `BaedekerModule`/`BaedekerInstance`, host-function
  closures, XCFramework build (macOS universal / iOS device / iOS sim).

### Changed

- License: **Apache-2.0** (with Industrial Algebra CLA). (Earlier development
  used MIT.)

[Unreleased]: https://github.com/Industrial-Algebra/Baedeker/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Industrial-Algebra/Baedeker/releases/tag/v0.1.0
