# Verification & Hardening

Baedeker is verified against three independent oracles, each catching a
different class of bug.

## The official spec suite

The WebAssembly specification ships a conformance test suite. Baedeker vendors
it (`baedeker-testdata`) and runs it through a harness:

**85 files, 12 deferred, 19,204 assertions, 993 modules, 0 failures.**

The 12 deferred files are the WebAssembly 3.0 surface (tail calls, GC types,
exceptions, memory64) — features on the post-release roadmap, not a 2.0 gap.
Every conformance bug the suite exposed during development was fixed; the
remaining green is real.

## Differential testing vs Wasmtime

A differential harness generates modules with `wasm-smith`, executes them on
both Baedeker and [Wasmtime](https://wasmtime.dev), and compares results. A
sweep of 9,024 executions produced **zero divergences**. This catches
correctness bugs the spec suite does not exercise (spec-conformant but unusual
module shapes) and guards against regressions.

## Fuzzing

cargo-fuzz targets cover the untrusted-input surface:

- `decode` — arbitrary bytes through the decoder (OOM-hardened).
- `validate_lower` — decode + validate + lower, asserting no panics.
- `trap_edges` — modules that should trap, asserting the right trap.
- `smith_module` — wasm-smith modules end-to-end.

Trap-edge fuzzing ran 3M iterations clean.

## GPU verification (both, layered)

GPU dispatch correctness matters because GPU floating-point is non-associative —
different thread orderings produce different accumulation sequences, so
tolerance-based checks are unreliable. Baedeker verifies in two layers:

1. **`dispatch_verified`** (structural, every dispatch) — the explicit
   `threads_per_group` is honoured via Borsalino's `dispatch_ex`, and a
   workgroup-divisibility proof confirms the config is sound. This catches
   silent mis-dispatch of non-default `@workgroup_size` kernels.
2. **Exact-match numerical** (opt-in, on demand) — for known-linear kernels,
   binary `{0,1}` inputs guarantee every partial sum is a small non-negative
   integer, exact within the FP16 ceiling (2048). The GPU output is compared
   against an FP32 CPU reference with bit-exact equality below the threshold.

The pure comparison core (`compare_outputs`) is unit-tested without a GPU; the
dispatch driver is generic over `GpuBackend` so the pass/fail logic is tested
with CPU fakes, and an `#[ignore]` lavapipe test proves it on real hardware.
