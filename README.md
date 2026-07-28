# Baedeker

A fully-general WebAssembly runtime implemented in Rust, targeting every platform Rust
compiles to: Linux, macOS, iOS, Android, and beyond — with full WebAssembly 3.0
compliance as the goal.

Baedeker decodes, validates, lowers, and executes WebAssembly modules. Validated programs are
lowered into a register-based internal IR and executed by an interpreter designed for
embedding: `no_std` + `alloc` core, no OS dependencies, explicit resource limits, and
diagnostics that carry byte offsets into the original binary.

Named after the Hindmost Baedeker from Larry Niven's *Ringworld* / *Fleet of Worlds* series —
cautious, methodical, but ultimately willing to venture into the unknown.

[![CI](https://github.com/Industrial-Algebra/Baedeker/actions/workflows/ci.yml/badge.svg)](https://github.com/Industrial-Algebra/Baedeker/actions/workflows/ci.yml)

## Status

**WebAssembly 2.0 core: complete and green — the first milestone on the road to 3.0.**
The interpreter passes the official WebAssembly spec test suite: **85 files, 19,204
assertions, 993 modules, 0 failures**. Every remaining deferred file exercises a post-2.0
proposal — tail calls, GC, exception handling, memory64, and friends — which is precisely
the 3.0 surface. Baedeker is meant to be a fully-general runtime: 3.0 spec support is the
destination, both to widen what the runtime can execute and to help push WebAssembly
adoption in general.

- **Full 2.0 feature set** — structured control flow with multi-value, direct/indirect calls,
  bulk memory, tables, globals, SIMD v128 core, and the reference-types + function-references
  proposals (`call_ref`, `br_on_null`, `br_on_non_null`, `ref.as_non_null`, typed references).
- **Multi-module linking** — shared memories, globals, tables, and functions across
  instantiations, with host functions registered against the store.
- **Embeddable by design** — per-instance fuel budgets for untrusted code, sparse table
  storage (a `u32::MAX`-entry table instantiates), fallible allocation on memory growth,
  and reentrant `&Store` execution.
- **Differentially tested** — wasm-smith generated modules executed against Wasmtime
  (zero divergences across thousands of executions), plus cargo-fuzz targets for decode,
  validation/lowering, and trap-edge semantics.

**Next: Phase 5 — platform integration.** C FFI, Swift package, AOT pipeline, and the GPU
host module, ahead of a 0.1.0 release. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Why

The [Industrial Algebra](https://github.com/Industrial-Algebra) ecosystem includes several Rust
crates for geometric algebra, information geometry, and high-performance functional programming
(Amari, Cliffy, Minuet, Orlando). Running these across platforms today means per-platform
rewrites (Swift on Apple, Kotlin on Android) or accepting the limitations of existing WASM
runtimes. Baedeker exists to close that gap: a purpose-built WASM engine written in Rust
that runs anywhere Rust does, embeds into host applications via a C-compatible FFI (including
idiomatic Swift on Apple platforms), and offers GPU acceleration through Vulkan-based compute
that runs across Metal, Nvidia, and AMD hardware.

## Goals

- **Spec compliance** — full WebAssembly 3.0 core, validated against the official spec test
  suite. *(2.0 done — see Status; 3.0 proposals tracked as
  [deferred work](https://github.com/Industrial-Algebra/Baedeker/issues/15).)*
- **Cross-platform embedding** — `no_std` + `alloc` core with no OS dependencies, designed for
  static linking into host apps via C FFI on any platform (Swift interop on Apple platforms).
- **Interpreter-first, AOT later** — a register-based interpreter with an explicit
  stack-to-register lowering pass. AOT compilation is a future layer, not a prerequisite.
- **Diagnostic quality** — errors carry byte offsets, structured context, and enough information
  to pinpoint exactly what went wrong and where.
- **Prove the thesis** — run Amari's geometric algebra computations through Baedeker on
  commodity hardware (iPad-class mobile GPUs included), demonstrating that the entire IA
  ecosystem can target any platform without leaving Rust.

## Non-goals

- Baedeker is not a security exploitation framework.
- It is not intended for offensive security workflows.
- Its binary parsing, validation, malformed-input handling, and robustness testing exist
  to improve correctness, spec compliance, and runtime reliability.

## Workspace

```
crates/
├── baedeker-core/       # no_std engine — decode, validate, lower, execute
├── baedeker-cli/        # command-line harness
├── baedeker-borsalino/  # optional Vulkan GPU offload (via Borsalino)
└── baedeker-testdata/   # spec fixtures, incl. the vendored official suite
fuzz/                    # cargo-fuzz targets (standalone nightly workspace)
```

## Building

Baedeker's test fixtures are compiled to WebAssembly during the build, so the
`wasm32-unknown-unknown` target must be installed locally.

```
rustup target add wasm32-unknown-unknown
cargo build
cargo test
cargo run -p baedeker-cli -- path/to/module.wasm
```

The official spec suite runs as part of `cargo test`
(`crates/baedeker-core/tests/runtime_official.rs`). Fuzzing requires nightly; see
[fuzz/README.md](fuzz/README.md).

## License

MIT
