# Roadmap

> **Status snapshot: 0.1.0 (unreleased).** WebAssembly 2.0 is complete and
> verified; 3.0 proposals are the forward plan.

## Released

- **WebAssembly 2.0 core** — full reference types, fixed-width SIMD,
  multi-value, bulk memory, multi-module linking, host functions.
- **Platform integration (Phase 5)** — C ABI, Swift package, GPU offload
  (Borsalino), GPU host module, AOT pipeline, interpreter fuel.
- **Verification** — official spec suite green, differential testing vs
  Wasmtime, cargo-fuzz targets, layered GPU verification.

## In progress / next

- **0.1.0 release** — crates.io publish of core/cli/ffi/borsalino/gpu; tag
  `v0.1.0`. (This book is part of that release polish.)

## WebAssembly 3.0 proposals (the post-release track)

Ordered roughly by dependency and value:

1. **Tail calls** (`return_call` / `return_call_indirect` / `return_call_ref`) —
   the deferred spec files exist; validation lowering is the work.
2. **Exception handling** — the `try`/`catch`/`throw` proposal.
3. **memory64** — 64-bit memories. The FFI already uses `uint64_t` sizes in
   anticipation.
4. **Garbage collection** — struct/array/reference types, GC compaction. This is
   where Borsalino 0.6.0's GC-safety epoch tracking
   (`prove_quiescent` / `dispatch_verified_gc`) becomes relevant: compaction
   must defer while GPU dispatches are in flight.
5. **Threads / shared-everything** — shared memories and atomic operations;
   single-threaded today.

Each proposal is engine-internal and does not affect the ABI shape — except
memory64, which is why the FFI sizes are `u64` from day one.

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
