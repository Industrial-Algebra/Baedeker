# Introduction

**Baedeker** is a WebAssembly runtime implemented in Rust, targeting every
platform Rust compiles to — Linux, macOS, iOS, Android, and beyond — as a
first-class, embeddable engine.

Named after the Hindmost Baedeker from Larry Niven's *Ringworld* / *Fleet of
Worlds* series: cautious, methodical, but ultimately willing to venture into the
unknown.

## What it does

Baedeker runs WebAssembly modules through a four-stage pipeline:

1. **Decode** the binary format into a typed module structure.
2. **Validate** it against the WebAssembly specification.
3. **Lower** the stack-machine bytecode into a register IR.
4. **Execute** the register IR on a self-contained interpreter.

The core is `no_std` + `alloc` — no filesystem, no threads, no OS — so it embeds
cleanly into bare metal, mobile apps, or even another runtime. A complete
platform-integration surface (C ABI, Swift package, GPU offload, ahead-of-time
compilation) wraps that core for production embedding.

## Why it exists

Industrial Algebra builds a broader ecosystem of Rust crates focused on
geometric algebra, information geometry, and high-performance functional
programming (Amari, Cliffy, Orlando, Minuet). A primary motivation is running
that ecosystem on any platform — mobile, desktop, and embedded — without
per-platform rewrites. WebAssembly is the portable substrate; Baedeker is the
engine that runs it.

Baedeker is a language-runtime and systems project. Its binary decoding,
malformed-input handling, validation, and robustness testing exist strictly to
improve standards-compliant WebAssembly execution, portability, embedding, and
implementation quality.

## Key features

- **WASM 2.0 core complete** — full reference types, fixed-width SIMD,
  multi-value, bulk memory, multi-module linking, and host functions.
- **Spec-verified** — the official WebAssembly test suite runs clean: 85 files,
  19,204 assertions, 993 modules, 0 failures.
- **Differentially tested** — generated modules executed against Wasmtime with
  zero divergences.
- **Embeddable** — `no_std` core; C ABI and Swift package for native embedding.
- **GPU offload** — bulk SIMD work dispatches to GPU compute via Borsalino
  (Vulkan/Metal), with a layered verification strategy.
- **Fuel-bound** — interpreter fuel caps execution; categorised traps give
  precise diagnostics.

## Status

Baedeker's first public release targets WebAssembly 2.0 as a complete,
verified milestone. WebAssembly 3.0 proposals (tail calls, exception handling,
memory64, GC, threads) are the post-release roadmap. See
[Roadmap](./design/roadmap.md).
