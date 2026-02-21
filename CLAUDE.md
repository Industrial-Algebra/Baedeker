# CLAUDE.md — Baedeker

## Project Identity

Baedeker is a WebAssembly runtime implemented in Rust, targeting iOS as a first-class platform.
Named after the Hindmost Baedeker from Larry Niven's Ringworld/Fleet of Worlds series — cautious,
methodical, but ultimately willing to venture into the unknown.

The project is authored by Justin, founder of Industrial Algebra LLC. Baedeker is part of a
broader ecosystem of Rust crates (Amari, Cliffy, Minuet, Orlando) focused on geometric algebra,
information geometry, and high-performance functional programming. A primary motivation is
running this ecosystem on iOS hardware without rewriting in Swift.

## Architecture

### Workspace layout

```
baedeker/
├── Cargo.toml                    # workspace root
├── crates/
│   ├── baedeker-core/            # no_std engine — decode, validate, execute
│   ├── baedeker-cli/             # command-line test harness
│   └── baedeker-wasm/            # baedeker compiled to WASM (future)
├── tests/                        # integration tests
└── fuzz/                         # fuzzing targets
```

### Key constraints

- `baedeker-core` is `no_std` + `alloc`. No filesystem, no threads, no OS.
- All public APIs use owned types or explicit lifetime parameters.
- Error types carry byte offsets into the original binary and structured context.
- The interpreter uses a register-based IR (future phase), not direct stack interpretation.

## Coding Conventions

### Style
- Idiomatic Rust. No `unsafe` in `baedeker-core` except where required for performance-critical
  interpreter dispatch (with `// SAFETY:` comment).
- Prefer `enum` with variants over boolean flags. Prefer newtypes over bare primitives
  for indices (`TypeIdx(u32)`, `FuncIdx(u32)`, `LabelIdx(u32)`, etc.).
- Use `thiserror` for error types in CLI. Manual `Display` + `core::error::Error` in core (no_std).
- Modules should be small and focused. If a file exceeds ~400 lines, split it.

### Naming
- Follow the WASM spec's naming: `FuncType`, `ValType`, `BlockType`, `MemArg`, etc.
- Internal IR types prefixed: `RegInstr`, `RegBlock`, `RegFunc`.

### Testing
- Every module has inline unit tests for the happy path.
- Edge cases and spec compliance go in `tests/` as integration tests.
- Spec test suite is the ultimate arbiter.

### Documentation
- Public items have doc comments referencing the relevant spec section.
- Internal comments explain *why*, not *what*.

## Git Workflow (Gitflow)
- **Branches**: `main` (releases), `develop` (integration), `feature/*`, `chore/*`, `fix/*`, `release/*`
- Feature work: branch from `develop` → PR to `develop`
- Releases: `develop` → release PR → `main`
- Never push directly to `main` or `develop`

## Pre-commit Checks
1. `cargo fmt -- --check`
2. `cargo clippy -- -D warnings`
3. `cargo test`

## CI/CD
GitHub Actions on every PR and pushes to `develop`/`main`.

## Commit Messages
- Conventional prefixes: `feat:`, `fix:`, `chore:`, `refactor:`, `test:`, `docs:`
- Subject line under 72 characters

## References
- [WebAssembly Spec](https://webassembly.github.io/spec/core/)
- [WASM Binary Format](https://webassembly.github.io/spec/core/binary/index.html)

## Related IA Projects
- **Amari**: First workload — geometric algebra on WASM/iOS
- **Cliffy**: Geometric FRP, informs IR pipeline architecture
- **Orlando**: "Transform transformations" philosophy for lowering passes
- **Minuet**: Information geometry primitives
- **Flynn**: Creusot-style contracts for Baedeker invariants
- **ShaperOS**: Potential userspace execution engine
