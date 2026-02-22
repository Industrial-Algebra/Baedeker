# Baedeker

A WebAssembly runtime implemented in Rust, targeting iOS as a first-class platform.

Named after the Hindmost Baedeker from Larry Niven's *Ringworld* / *Fleet of Worlds* series —
cautious, methodical, but ultimately willing to venture into the unknown.

## Why

The [Industrial Algebra](https://github.com/Industrial-Algebra) ecosystem includes several Rust
crates for geometric algebra, information geometry, and high-performance functional programming
(Amari, Cliffy, Minuet, Orlando). Running these on iOS today means rewriting in Swift or
accepting the limitations of existing WASM runtimes. Baedeker exists to close that gap: a
purpose-built WASM 2.0 engine that embeds natively into iOS via Rust FFI, with a clear path to
GPU acceleration through Metal compute.

## Goals

- **Spec compliance** — full WebAssembly 2.0 core, validated against the official spec test suite.
- **iOS-first embedding** — `no_std` + `alloc` core with no OS dependencies, designed for
  static linking into Swift apps via C FFI.
- **Interpreter-first, AOT later** — a register-based interpreter with an explicit
  stack-to-register lowering pass. AOT compilation is a future layer, not a prerequisite.
- **Diagnostic quality** — errors carry byte offsets, structured context, and enough information
  to pinpoint exactly what went wrong and where.
- **Prove the thesis** — run Amari's geometric algebra computations on iPad hardware through
  Baedeker, demonstrating that the entire IA ecosystem can target iOS without leaving Rust.

## Status

**Phase 0 — Foundation.** The binary decoder can ingest any `.wasm` file and report its section
layout. LEB128 encoding, section parsing, and module structure are implemented with full edge-case
coverage. See [docs/ROADMAP.md](docs/ROADMAP.md) for the full phase plan.

## Building

```
cargo build
cargo test
cargo run -p baedeker-cli -- path/to/module.wasm
```

## License

MIT
