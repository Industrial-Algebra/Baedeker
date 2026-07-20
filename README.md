# Baedeker

A WebAssembly runtime implemented in Rust, targeting every platform Rust compiles to:
Linux, macOS, iOS, Android, and beyond.

Baedeker focuses on correct decoding, validation, lowering, and execution of WebAssembly modules,
with an architecture that begins from spec-aligned validation and later lowers validated programs
into a register-based internal IR for execution. The project is intended for language runtime
research, portability, and safe systems engineering.

Named after the Hindmost Baedeker from Larry Niven's *Ringworld* / *Fleet of Worlds* series —
cautious, methodical, but ultimately willing to venture into the unknown.

## Why

The [Industrial Algebra](https://github.com/Industrial-Algebra) ecosystem includes several Rust
crates for geometric algebra, information geometry, and high-performance functional programming
(Amari, Cliffy, Minuet, Orlando). Running these across platforms today means per-platform
rewrites (Swift on Apple, Kotlin on Android) or accepting the limitations of existing WASM
runtimes. Baedeker exists to close that gap: a purpose-built WASM 2.0 engine written in Rust
that runs anywhere Rust does, embeds into host applications via a C-compatible FFI (including
idiomatic Swift on Apple platforms), and offers GPU acceleration through Vulkan-based compute
that runs across Metal, Nvidia, and AMD hardware.

## Non-goals

- Baedeker is not a security exploitation framework.
- It is not intended for offensive security workflows.
- Its binary parsing, validation, malformed-input handling, and future robustness testing exist
  to improve correctness, spec compliance, and runtime reliability.

## Goals

- **Spec compliance** — full WebAssembly 2.0 core, validated against the official spec test suite.
- **Cross-platform embedding** — `no_std` + `alloc` core with no OS dependencies, designed for
  static linking into host apps via C FFI on any platform (Swift interop on Apple platforms).
- **Interpreter-first, AOT later** — a register-based interpreter with an explicit
  stack-to-register lowering pass. AOT compilation is a future layer, not a prerequisite.
- **Diagnostic quality** — errors carry byte offsets, structured context, and enough information
  to pinpoint exactly what went wrong and where.
- **Prove the thesis** — run Amari's geometric algebra computations through Baedeker on
  commodity hardware (iPad-class mobile GPUs included), demonstrating that the entire IA
  ecosystem can target any platform without leaving Rust.

## Status

**Phase 2 — Register-based lowering and runtime bring-up.** The spec-aligned decoder and
validator are complete (Phase 1); the register-IR interpreter executes straight-line numeric
code, full structured control flow (including multi-value), direct and indirect calls,
linear memory, globals, tables, and a SIMD v128 core. An optional GPU backend slot exists
for Vulkan-based bulk SIMD offload. See [docs/ROADMAP.md](docs/ROADMAP.md) for the full
phase plan.

## Building

Baedeker's test fixtures are compiled to WebAssembly during the build, so the
`wasm32-unknown-unknown` target must be installed locally.

```
rustup target add wasm32-unknown-unknown
cargo build
cargo test
cargo run -p baedeker-cli -- path/to/module.wasm
```

## License

MIT
