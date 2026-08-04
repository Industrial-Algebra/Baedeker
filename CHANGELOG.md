# Changelog

All notable changes to Baedeker are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/Industrial-Algebra/Baedeker/compare/HEAD
