# Roadmap

> **Status snapshot: 0.2.0.** WebAssembly 3.0 is complete and verified; the
> classic threads proposal is implemented on top. Forward work is the real GC
> collector and shared-everything threads.

## Released

- **WebAssembly 2.0 core** — full reference types, fixed-width SIMD,
  multi-value, bulk memory, multi-module linking, host functions.
- **WebAssembly 3.0** (pinned spec `dfa3f32a`) — every proposal, each its own
  reviewed slice: **tail calls**, **exception handling** (`try`/`catch`/`throw`),
  **fixed-width SIMD**, **memory64 + table64**, **garbage collection**
  (struct/array types, casts, recursive types). Official suite: **204 files,
  54,601 assertions, 1,994 modules, 0 failures**.
- **Classic threads** — shared linear memory + the full `0xFE` atomic family,
  at the frozen browser-shipped pin (`WebAssembly/threads@main-legacy
  33b2dcd6`), with deterministic single-agent `wait`/`notify`/`fence`
  semantics.
- **Platform integration (Phase 5)** — C ABI, Swift package, GPU offload
  (Borsalino), GPU host module, AOT pipeline (format v12), interpreter fuel.
- **Verification** — official spec suite green, differential testing vs
  Wasmtime, cargo-fuzz targets, smith generation at total acceptance, layered
  GPU verification.

## In progress / next

- **0.2.0 release** — crates.io publish of core/cli/ffi/borsalino/gpu; tag
  `v0.2.0`. (This book is part of that release polish.)
- **GC 5b — the real collector** — the staged GC slice ships a never-free heap;
  the collecting pass is gated behind the R3/R4 payloads per the design doc.
  Compaction must defer while GPU dispatches are in flight (Borsalino 0.6.0's
  GC-safety epoch tracking: `prove_quiescent` / `dispatch_verified_gc`).

## Forward proposals

- **Shared-everything threads** (`thread.spawn`, shared GC, per-op ordering
  params — `main` @ `cc535ada`). NOTE: threads is NOT part of WebAssembly 3.0
  (the pinned 3.0 suite contains no threads content); the classic proposal is
  implemented. Shared-everything collides with the single-threaded GC-heap
  design and stays gated behind the GC collector work.
- **wasi-threads embedder surface** — now that shared memories and single-agent
  atomics exist, a WASI host API for multi-threaded modules is a plausible
  integration target.

Each proposal is engine-internal and does not affect the ABI shape — memory64
is why the FFI sizes are `u64` from day one.

## Ecosystem integration

- **Amari** is the first workload — geometric algebra on WASM across platforms.
- The GPU host module's geometric-product kernel and Borsalino's
  `GeometricProductReference` (a 5D GA exact-match reference) point at
  GPU-accelerated algebra as a near-term integration target once Amari targets
  Baedeker.

## Verification growth

- More offload kernels gain exact-match numerical references as they are added.
- A determinism-check layer (Borsalino's `determinism` module) is available for a
  future verification slice.
