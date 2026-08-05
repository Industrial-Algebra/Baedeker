# Contributing to Baedeker

Thanks for your interest in contributing to Baedeker!

## Contributor License Agreement

All contributors must sign the
[Industrial Algebra CLA](https://github.com/Industrial-Algebra/.github/blob/main/CLA.md).
The CLA grants Industrial Algebra the right to relicense contributions, and
covers every IA project once signed — there is no per-project signing.

Baedeker is licensed under [Apache-2.0](./LICENSE).

## Development workflow

Baedeker follows the IA gitflow discipline documented in
[`AGENTS.md`](./AGENTS.md): branch from `develop`, open a PR, never push
directly to `main` or `develop`. Pre-commit, all three must pass:

```
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Code standards

See the IA coding standards for the conventions this crate follows (TDD,
newtype indices, exhaustive enums, no `unsafe` outside performance-critical
interpreter dispatch). Prefer a focused PR per work unit.
