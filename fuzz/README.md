# Baedeker fuzzing

Fuzz targets for the `baedeker-core` decode → validate → lower pipeline,
using [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer).

Fuzzing here serves standards compliance and implementation quality:
the invariants under test are *no panics, no aborts, no OOM-without-error*
on arbitrary or structurally-generated input.

## Targets

| Target | Input | Invariant |
|---|---|---|
| `decode` | arbitrary bytes | `Module::decode` never panics; pipeline continues through validate+lower on success |
| `validate_lower` | arbitrary bytes (seeded with valid fixtures) | validator and lowering never panic |
| `smith_module` | arbitrary bytes → wasm-smith structured modules | no panics on deep valid-module shapes |

## Running

Requires nightly Rust and cargo-fuzz:

```sh
cargo install cargo-fuzz
fuzz/seed.sh
cargo +nightly fuzz run decode        # or validate_lower / smith_module
```

Crash reproducers land in `fuzz/artifacts/<target>/`. Minimize them and
convert to a regression test in `crates/baedeker-core/tests/` (or a unit
test next to the fix) — do not commit raw corpora or artifacts.

## Scope notes

- The `smith_module` target restricts wasm-smith to proposals Baedeker
  supports (bulk memory, multi-value, SIMD); other proposals are disabled
  so generated modules stay within the tested surface.
- Execution fuzzing (running generated modules) is deliberately out of
  scope for now: arbitrary modules can loop forever, and the interpreter
  has no fuel limit yet. Differential execution against Wasmtime is
  tracked in #21 as follow-up work.
