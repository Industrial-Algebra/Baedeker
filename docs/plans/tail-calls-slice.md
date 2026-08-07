# Tail Calls — WASM 3.0 Slice 1

**Status:** Plan (implementation pending). Branch `feature/tail-calls`.
**Goal:** Execute `return_call` / `return_call_indirect` / `return_call_ref`
with proper tail-call semantics — stack-safe, no call-stack growth — and
un-defer the 3 spec files that depend on them.

## Why this is slice 1

The validator is far ahead of the executor. Tail calls are already **decoded**
(`0x12/0x13/0x15` in `binary/instr.rs`) and **validated** (tail-position type
checks enforce that the callee's result type matches the caller's return type).
The only missing piece is lowering + execution. It is the cheapest, lowest-risk
3.0 proposal and unblocks the deferred-count drop (12 → 9).

## The core design decision: a trampoline

The current call model implements WASM calls as **Rust recursion**:
`execute_func_in` (`runtime/mod.rs:492`) calls itself with `depth + 1` for every
`Call` / `CallIndirect` / `CallRef`, bounded by `MAX_CALL_DEPTH = 512`.

A naive "return_call = a Call that doesn't increment depth" is **not enough**:
it still recurses in Rust, so the Rust stack still grows, and the spec's
deep-tail-recursion tests (`return_call.wast`) overflow. Correct tail calls
require the dispatch loop to become **iterative**: a tail call *replaces* the
current function at the same depth, reusing the Rust frame, instead of recursing.
That iteration is a trampoline.

## Design

### 1. Lowering (`crates/baedeker-core/src/lower/mod.rs`)

Tail calls are always in tail position, so they lower to **terminators**, not
regular ops. Add three variants to `enum RegTerm` (`lower/mod.rs:247`):

```rust
TailCall        { func: FuncIdx, args: Box<[RegIdx]>, }
TailCallIndirect{ type_idx: TypeIdx, table: TableIdx, index: RegIdx, args: Box<[RegIdx]>, }
TailCallRef     { type_idx: TypeIdx, func: RegIdx,   args: Box<[RegIdx]>, }
```

Lower `Instr::ReturnCall(f)` / `ReturnCallIndirect{..}` / `ReturnCallRef(t)`
to these terminators, reusing the existing `Call` / `CallIndirect` / `CallRef`
arg-resolution logic (templates at `lower/mod.rs:2701`, `:2814`, `:2667`).
A tail-call terminator *ends* its block (like `Return`), so it must be emitted
via `finish_block(...)`, not appended as a regular instruction.

### 2. Trampoline (`crates/baedeker-core/src/runtime/mod.rs`)

Wrap `execute_func_in`'s body in a `loop { … }`. Move the locals/registers
setup (built from `func` + `args`) **inside** the loop so it re-initializes for
each tail callee. Add terminator arms:

- `RegTerm::TailCall{func,args}` → resolve callee + build `call_args`, then set
  `func = callee; args = call_args;` and `continue` (restart at the same depth).
- `TailCallIndirect` / `TailCallRef` → resolve the indirect/ref target first,
  then `continue`.
- Existing `RegTerm::Return` → `return Ok(results)` (exits the trampoline).

### 3. Split resolve vs invoke

`execute_call` / `execute_call_indirect` / `execute_call_ref` (`:236` / `:266` /
`:310`) currently both *resolve* the target and *recurse*. Split each into:

- `resolve_*` — pure: returns the callee `RegFunc` + prepared `call_args`.
- the recursion stays as today (regular `Call` ops call `resolve_*` then recurse).

Tail-call terminators call `resolve_*` then `continue` the trampoline. Regular
calls are unchanged.

## Tests (TDD)

1. **RED** — remove the three entries from `DEFERRED_FILES`
   (`tests/runtime_official.rs:35-37`): `return_call.wast`,
   `return_call_indirect.wast`, `return_call_ref.wast`. Run
   `cargo test -p baedeker-core --test runtime_official` → the three files now
   execute and fail (return-call opcodes not lowered → error).
2. **GREEN** — implement lowering + trampoline → the three files pass. The full
   official suite stays at 0 failures; deferred count drops 12 → 9.

## Fuzz

New target `fuzz/fuzz_targets/return_call_depth.rs`: synthesize a module with
deep tail recursion (a function that `return_call`s itself `N` times), assert it
completes without a stack-exhaustion trap for `N = 1_000_000` (far beyond
`MAX_CALL_DEPTH = 512`) — proving stack-safety. Mirror it in the differential
harness vs Wasmtime.

## Scope & risk

- ~300–400 lines: `lower/mod.rs` (3 `RegTerm` variants + 3 lowering arms),
  `runtime/mod.rs` (trampoline + 3 terminator arms + split resolve/invoke).
- **No ABI change.** Validation is already done.
- Risk: the trampoline touches the hot call path — must not regress the
  19,204-assertion official suite, the differential suite, or call throughput.
  Run the full matrix before merge.

## Verification before claiming done

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test                      # full suite, 0 failures, deferred 12 → 9
cargo +nightly fuzz run return_call_depth -- -runs=50000
```

## Files

- `crates/baedeker-core/src/lower/mod.rs` — `RegTerm` enum (`:247`); Call
  lowering templates (`:2667`, `:2701`, `:2814`).
- `crates/baedeker-core/src/runtime/mod.rs` — `execute_func_in` (`:492`),
  `execute_call` (`:236`), `execute_call_indirect` (`:266`), `execute_call_ref`
  (`:310`), `MAX_CALL_DEPTH` (`:478`).
- `crates/baedeker-core/tests/runtime_official.rs` — `DEFERRED_FILES` (`:32`).
- `fuzz/fuzz_targets/return_call_depth.rs` — new.
