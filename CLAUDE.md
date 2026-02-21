# Baedeker — Development Guidelines

## Project Overview
Baedeker is an Industrial Algebra (IA) Rust project (edition 2024).

## Rust Style & Patterns
- Write idiomatic Rust — prefer ownership over borrowing when semantics are clearer
- Use phantom types and algebraic type patterns to encode invariants at compile time
- Prefer `Result` / `Option` chaining over early returns where readable
- No `unwrap()` in production code; use `expect()` with context or propagate errors

## Testing
- **TDD**: write tests before or alongside implementation
- Unit tests live in the same file (`#[cfg(test)]` module)
- Integration tests go in `tests/`
- All PRs must pass `cargo test` before merge

## Concurrency & Acceleration
- **CPU parallelism**: `rayon` for data-parallel workloads
- **GPU acceleration**: `wgpu` when compute shaders are beneficial
- Avoid `unsafe` unless absolutely necessary; document why when used

## TUI
- Primary: `ratatui`
- Alternative: NotCurses (if ratatui proves insufficient)

## Git Workflow (Gitflow)
- **Branches**: `main` (releases), `develop` (integration), `feature/*`, `chore/*`, `fix/*`, `release/*`
- Feature work: branch from `develop` → PR to `develop`
- Releases: `develop` → release PR → `main`
- Never push directly to `main` or `develop`

## Pre-commit Checks
The `.git/hooks/pre-commit` hook runs automatically:
1. `cargo fmt -- --check`
2. `cargo clippy -- -D warnings`
3. `cargo test`

All three must pass for a commit to succeed.

## CI/CD
GitHub Actions runs on every PR and on pushes to `develop`/`main`:
- `cargo fmt -- --check`
- `cargo clippy -- -D warnings`
- `cargo test`

## Commit Messages
- Use conventional-style prefixes: `feat:`, `fix:`, `chore:`, `refactor:`, `test:`, `docs:`
- Keep the subject line under 72 characters
- Reference issue numbers when applicable
