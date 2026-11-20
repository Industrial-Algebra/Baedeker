# Tail Calls — WASM 3.0 Slice 1 (Plan Contract)

**Unit:** `tail-calls-slice-1` · **Depends on:** none (decoder + validator already merged)
**Followed by:** SIMD completeness (slice 2)
**Status:** Contract for generation dispatch (ia-moment). Branch `feature/tail-calls`.

## Design (why — read once before implementing)

`return_call` / `return_call_indirect` / `return_call_ref` are decoded
(`0x12/0x13/0x15`) and validated (tail-position type checks at
`validate/mod.rs:1035-1103`) but not lowered. WASM guarantees tail calls are
**stack-safe**: they must not grow the call stack. The runtime implements
calls as Rust recursion (`execute_func_in` → `depth + 1`, `MAX_CALL_DEPTH =
512`), so tail calls need a **trampoline**: the block-dispatch loop in
`execute_func_in` becomes restartable — a tail-call *terminator* replaces the
current function at the same depth instead of recursing.

Resolution classes (this is the contract's core semantics):

| Target | Behavior |
|---|---|
| Same-instance defined function | Trampoline hop: replace `func`/`args`, same depth |
| Host (imported) function | Call host, its results ARE the tail-call results |
| Cross-instance funcref | Delegate to existing `execute_funcref` at `depth + 1` (correct, bounded) |

Direct `return_call` targets are always same-instance (module-local indices):
host or hop, never cross-instance. The 10M-iteration backstop and fuel charge
must span tail hops (a no-fuel infinite tail loop must still be bounded).

## Files

1. **Modify** `crates/baedeker-core/src/lower/mod.rs` — RegTerm variants +
   3 lowering arms + name strings + flow-analysis match arms.
2. **Modify** `crates/baedeker-core/src/runtime/mod.rs` — trampoline +
   helpers + terminator arms + `execute_call_indirect` refactor.
3. **Modify** `crates/baedeker-core/tests/runtime_official.rs` — delete 3
   DEFERRED_FILES entries.
4. **Create** `crates/baedeker-core/tests/tail_calls.rs` — verbatim in C5.
5. **Create** `fuzz/fuzz_targets/return_call_depth.rs` — verbatim in C6.
6. **Modify** `fuzz/Cargo.toml` — verbatim in C6.

No other file may be created, modified, or deleted (`Cargo.lock` excepted).

## Contracts

### C1 — `RegTerm` variants (`lower/mod.rs`, inside `pub enum RegTerm`)

Insert after the `Return { values: Vec<Reg> },` variant, before `Trap`:

```rust
    /// Tail call (`return_call`): replace the current frame with the callee
    /// at the same stack depth. Terminates the block like `Return`.
    TailCall { func: FuncIdx, args: Vec<Reg> },
    /// Tail call through a table (`return_call_indirect`).
    TailCallIndirect {
        type_idx: TypeIdx,
        table: TableIdx,
        index: Reg,
        args: Vec<Reg>,
    },
    /// Tail call through a function reference (`return_call_ref`).
    TailCallRef { type_idx: TypeIdx, func: Reg, args: Vec<Reg> },
```

The enum's existing `#[derive(...)]` / `#[cfg_attr(feature = "serde", ...)]`
apply unchanged (no new attributes).

### C2 — Lowering arms (`lower/mod.rs`)

**(a)** Insert these three arms in the instruction lowering `match`, adjacent
to the existing `Instr::Return` arm (after it, before `Instr::CallRef`):

```rust
            Instr::ReturnCall(func) => {
                let Some(callee_ty) = self.tables.func_types.get(func.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidFunction { func: func.0 },
                    });
                };
                let mut args = Vec::with_capacity(callee_ty.params.len());
                for &expected in callee_ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "return_call", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                self.finish_block(RegTerm::TailCall { func, args });
                self.set_unreachable();
            }
            Instr::ReturnCallIndirect {
                type_idx,
                table_idx,
            } => {
                let Some(ty) = self.tables.types.get(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                let index = self.pop_expect(offset, "return_call_indirect", ValType::Num(NumType::I32))?;
                let mut args = Vec::with_capacity(ty.params.len());
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "return_call_indirect", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                self.finish_block(RegTerm::TailCallIndirect {
                    type_idx,
                    table: table_idx,
                    index: index.reg,
                    args,
                });
                self.set_unreachable();
            }
            Instr::ReturnCallRef(type_idx) => {
                let Some(ty) = self.tables.types.get(type_idx.0 as usize) else {
                    return Err(LowerError {
                        offset,
                        function: Some(self.func_idx),
                        kind: LowerErrorKind::InvalidType {
                            type_idx: type_idx.0,
                        },
                    });
                };
                // Stack order: [args..., funcref] — the reference is on top.
                let func = self.pop_any(offset, "return_call_ref")?;
                let mut args = Vec::with_capacity(ty.params.len());
                for &expected in ty.params.iter().rev() {
                    let found = self.pop_expect(offset, "return_call_ref", expected)?;
                    args.push(found.reg);
                }
                args.reverse();
                self.finish_block(RegTerm::TailCallRef {
                    type_idx,
                    func: func.reg,
                    args,
                });
                self.set_unreachable();
            }
```

Note the shape mirrors the existing `Call`/`CallIndirect`/`CallRef` arms for
arg popping, but ends the block (`finish_block`) and goes polymorphic
(`set_unreachable()`), exactly like `Instr::Return`. No result registers are
allocated — the callee's results are the function's results (validation
already enforced the match).

**(b)** Every other exhaustive `match` over `Instr` or `RegTerm` in this file
must handle the new variants; the compiler will point them out. Rules:

- The instruction-name helper (the match containing `Instr::Return =>
  "return"`) gains: `Instr::ReturnCall(_) => "return_call"`,
  `Instr::ReturnCallIndirect { .. } => "return_call_indirect"`,
  `Instr::ReturnCallRef(_) => "return_call_ref"`.
- Any control-flow/reachability match over `RegTerm` treats the three
  `TailCall*` terminators exactly like `RegTerm::Return` (the function exits;
  e.g. a `RegTerm::Return { .. } => break` arm becomes
  `RegTerm::Return { .. } | RegTerm::TailCall { .. } | RegTerm::TailCallIndirect { .. } | RegTerm::TailCallRef { .. } => break`).

### C3 — Runtime trampoline (`runtime/mod.rs`)

**(a)** Insert these helpers after the `MAX_CALL_DEPTH` const, before
`execute_func_in`:

```rust
/// A resolved tail-call target: `Defined` transfers control to a
/// same-instance function at the current depth (trampoline hop); `Ready`
/// carries the final results (host target, or cross-instance delegation
/// which recurses depth + 1 as before).
enum TailTarget<'a> {
    Defined(&'a RegFunc),
    Ready(Vec<Value>),
}

/// Resolve a direct `return_call` target. Direct indices are module-local,
/// so the target is either a host import (dispatched immediately) or a
/// same-instance defined function (trampoline hop).
fn resolve_tail_direct<'a>(
    module: Option<&'a RegModule>,
    store: Option<&Store>,
    callee_idx: &FuncIdx,
    call_args: &[Value],
) -> Result<TailTarget<'a>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
    })?;
    if callee_idx.0 < module.imported_func_count {
        // Tail call to an imported function: dispatch to the host; its
        // results are the tail-call results.
        let store = store.ok_or(RuntimeError {
            kind: RuntimeErrorKind::MissingStore,
        })?;
        return Ok(TailTarget::Ready(store.call_host(callee_idx.0, call_args)?));
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx == *callee_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: callee_idx.0 },
        })?;
    Ok(TailTarget::Defined(callee))
}

/// Resolve an indirect/ref tail-call target. Host targets and cross-instance
/// targets are executed immediately (results in `Ready`); same-instance
/// defined targets transfer via the trampoline (`Defined`).
#[allow(clippy::too_many_arguments)]
fn resolve_tail_funcref<'a>(
    module: Option<&'a RegModule>,
    store: Option<&Store>,
    type_idx: &crate::types::TypeIdx,
    instance_id: u32,
    func_idx: u32,
    call_args: &[Value],
    depth: usize,
) -> Result<TailTarget<'a>, RuntimeError> {
    let module = module.ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownFunction { func: 0 },
    })?;
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    if instance_id != store.instance_id() {
        // Cross-instance tail target: delegate to the existing funcref path
        // (structural type check against the remote module, depth + 1).
        let results = execute_funcref(
            module,
            store,
            type_idx,
            instance_id,
            func_idx,
            call_args,
            depth,
        )?;
        return Ok(TailTarget::Ready(results));
    }
    // Same instance: structural type check first (mirrors `execute_funcref`).
    let expected = module.types.get(type_idx.0 as usize).ok_or(RuntimeError {
        kind: RuntimeErrorKind::UnknownType { type_idx: type_idx.0 },
    })?;
    if func_idx < module.imported_func_count {
        let import_ty = &module
            .imported_funcs
            .get(func_idx as usize)
            .ok_or(RuntimeError {
                kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
            })?
            .ty;
        if expected != import_ty {
            return Err(trap(RuntimeTrap::IndirectCallTypeMismatch));
        }
        return Ok(TailTarget::Ready(store.call_host(func_idx, call_args)?));
    }
    let callee = module
        .funcs
        .iter()
        .find(|func| func.idx.0 == func_idx)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownFunction { func: func_idx },
        })?;
    let actual = module
        .types
        .get(callee.type_idx.0 as usize)
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownType {
                type_idx: callee.type_idx.0,
            },
        })?;
    if expected != actual {
        return Err(trap(RuntimeTrap::IndirectCallTypeMismatch));
    }
    Ok(TailTarget::Defined(callee))
}

/// Look up an indirect-call target in a table: `(instance_id, func_idx)`.
fn table_lookup_target(
    store: Option<&Store>,
    table: &TableIdx,
    idx: u32,
) -> Result<(u32, u32), RuntimeError> {
    let store = store.ok_or(RuntimeError {
        kind: RuntimeErrorKind::MissingStore,
    })?;
    let target = store
        .with_table(table.0, |table| table.get(idx))
        .ok_or(RuntimeError {
            kind: RuntimeErrorKind::UnknownTable { table: table.0 },
        })?
        .ok_or(trap(RuntimeTrap::UndefinedElement))?;
    let Value::FuncRef(func_idx) = target else {
        return Err(RuntimeError {
            kind: RuntimeErrorKind::TypeMismatch {
                expected: ValType::Ref(RefType::FuncRef),
                found: target.val_type(),
            },
        });
    };
    let Some((instance_id, func_idx)) = func_idx else {
        return Err(trap(RuntimeTrap::UninitializedElement));
    };
    Ok((instance_id, func_idx))
}
```

**(b)** Restructure `execute_func_in`. Replace its head — everything from
`pub(crate) fn execute_func_in(` down to and including the
`if args.len() != func.params.len() {` line — with:

```rust
pub(crate) fn execute_func_in(
    module: Option<&RegModule>,
    store: Option<&Store>,
    func: &RegFunc,
    args: &[Value],
    depth: usize,
) -> Result<Vec<Value>, RuntimeError> {
    if depth >= MAX_CALL_DEPTH {
        return Err(trap(RuntimeTrap::CallStackExhausted));
    }

    // Tail-call trampoline: `return_call*` terminators replace the current
    // frame at the same depth instead of recursing, keeping tail recursion
    // stack-safe. `func`/`args` are reassigned per hop; `module`/`store`
    // stay fixed — cross-instance tail targets delegate through
    // `resolve_tail_funcref` with depth + 1 (correct, depth-bounded).
    let mut func = func;
    let mut owned_args: Vec<Value> = Vec::new();
    let mut use_owned_args = false;
    // Backstop against true infinite loops when no fuel budget is set (the
    // coarse guard predates configurable fuel; with fuel, exhaustion is the
    // honest `FuelExhausted` error instead of `MissingReturn`). The cap
    // spans tail hops: an unbounded no-fuel tail loop is still bounded.
    let max_iterations = 10_000_000;
    let mut iteration: usize = 0;

    'tail: loop {
        let args: &[Value] = if use_owned_args { &owned_args } else { args };
        if args.len() != func.params.len() {
```

The arity-error body, arg type-check loop, locals setup, zero-init loop,
`registers` allocation, and `func.blocks.is_empty()` check that follow are
**unchanged** (they now sit inside the `'tail:` loop; re-indent under
`cargo fmt`).

Then **delete** the now-duplicated declarations in what was the per-function
setup (they were hoisted above):

```rust
    let mut block_idx: u32 = 0;
    let max_iterations = 10_000_000;
    let mut iteration: usize = 0;
```

keeping only `let mut block_idx: u32 = 0;`, followed by the existing inner
`loop {` (with `iteration += 1;`, fuel charge, block fetch, instruction loop,
terminator match — all unchanged except the new arms below).

**(c)** Insert these arms into the existing terminator `match` (after the
`RegTerm::Return` arm):

```rust
            RegTerm::TailCall {
                func: callee_idx,
                args: arg_regs,
            } => {
                let call_args = arg_regs
                    .iter()
                    .map(|&reg| get_reg(&registers, reg))
                    .collect::<Result<Vec<_>, _>>()?;
                match resolve_tail_direct(module, store, callee_idx, &call_args)? {
                    TailTarget::Defined(callee) => {
                        func = callee;
                        owned_args = call_args;
                        use_owned_args = true;
                        continue 'tail;
                    }
                    TailTarget::Ready(results) => return Ok(results),
                }
            }
            RegTerm::TailCallIndirect {
                type_idx,
                table,
                index,
                args: arg_regs,
            } => {
                let idx = expect_addr(get_reg(&registers, *index)?)?;
                let call_args = arg_regs
                    .iter()
                    .map(|&reg| get_reg(&registers, reg))
                    .collect::<Result<Vec<_>, _>>()?;
                let (instance_id, target_func_idx) =
                    table_lookup_target(store, table, idx)?;
                match resolve_tail_funcref(
                    module,
                    store,
                    type_idx,
                    instance_id,
                    target_func_idx,
                    &call_args,
                    depth,
                )? {
                    TailTarget::Defined(callee) => {
                        func = callee;
                        owned_args = call_args;
                        use_owned_args = true;
                        continue 'tail;
                    }
                    TailTarget::Ready(results) => return Ok(results),
                }
            }
            RegTerm::TailCallRef {
                type_idx,
                func: func_reg,
                args: arg_regs,
            } => {
                let target = get_reg(&registers, *func_reg)?;
                let call_args = arg_regs
                    .iter()
                    .map(|&reg| get_reg(&registers, reg))
                    .collect::<Result<Vec<_>, _>>()?;
                let Value::FuncRef(func_idx) = target else {
                    return Err(RuntimeError {
                        kind: RuntimeErrorKind::TypeMismatch {
                            expected: ValType::Ref(RefType::FuncRef),
                            found: target.val_type(),
                        },
                    });
                };
                let Some((instance_id, target_func_idx)) = func_idx else {
                    return Err(trap(RuntimeTrap::NullFunctionReference));
                };
                match resolve_tail_funcref(
                    module,
                    store,
                    type_idx,
                    instance_id,
                    target_func_idx,
                    &call_args,
                    depth,
                )? {
                    TailTarget::Defined(callee) => {
                        func = callee;
                        owned_args = call_args;
                        use_owned_args = true;
                        continue 'tail;
                    }
                    TailTarget::Ready(results) => return Ok(results),
                }
            }
```

**(d)** In `execute_call_indirect`, replace the table-lookup portion (from
`let target = store` through the `let Some((instance_id, func_idx)) =
func_idx else { ... };` block, inclusive) with:

```rust
    let (instance_id, func_idx) = table_lookup_target(store, table, idx)?;
```

so the function tail-calls `execute_funcref(...)` exactly as before. Behavior
is identical (the helper is the extracted verbatim of the replaced code).

### C4 — Un-defer the spec files (`tests/runtime_official.rs`)

Delete exactly these four lines from `DEFERRED_FILES`:

```rust
    // Tail calls: return_call/return_call_indirect not lowered.
    ("return_call.wast", "tail calls not implemented"),
    ("return_call_indirect.wast", "tail calls not implemented"),
    ("return_call_ref.wast", "tail calls not implemented"),
```

### C5 — New integration test `crates/baedeker-core/tests/tail_calls.rs`

Full file content:

```rust
// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Tail-call execution: spec-mandated stack safety.
//!
//! WASM guarantees `return_call*` does not grow the call stack. These tests
//! drive tail recursion far beyond `MAX_CALL_DEPTH` (512) and assert normal
//! completion. Where a call below mirrors an existing test's API usage
//! (wat encoding, lowering, instantiation, host registration), copy the
//! exact form used in `crates/baedeker-core/tests/differential.rs` /
//! `runtime_wast.rs`.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeError, Store, Value, execute_export};

const DEPTH: i32 = 1_000_000;

fn run_export(wat: &str, export: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    let mut buffer = wast::parser::ParseBuffer::new(wat).expect("wat parses");
    let parsed = wast::parser::parse::<wast::Wat>(&mut buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    let reg = module.lower().expect("module lowers");
    let store = Store::instantiate(&reg).expect("module instantiates");
    execute_export(&reg, &store, export, args)
}

#[test]
fn direct_tail_recursion_is_stack_safe() {
    let wat = r#"
        (module
          (func $run (export "run") (param $n i32) (result i32)
            (if (result i32) (i32.eqz (local.get $n))
              (then (i32.const 42))
              (else (return_call $run
                (i32.sub (local.get $n) (i32.const 1)))))))
    "#;
    let result = run_export(wat, "run", &[Value::I32(DEPTH)]).expect("no stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
}

#[test]
fn indirect_tail_recursion_is_stack_safe() {
    let wat = r#"
        (module
          (type $t (func (param i32) (result i32)))
          (table $tbl 1 funcref)
          (elem (i32.const 0) $run)
          (func $run (export "run") (param $n i32) (result i32)
            (if (result i32) (i32.eqz (local.get $n))
              (then (i32.const 42))
              (else (return_call_indirect $t (table $tbl)
                (i32.sub (local.get $n) (i32.const 1)) (i32.const 0))))))
    "#;
    let result = run_export(wat, "run", &[Value::I32(DEPTH)]).expect("no stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
}

#[test]
fn ref_tail_recursion_is_stack_safe() {
    let wat = r#"
        (module
          (type $t (func (param i32) (result i32)))
          (elem declare func $run)
          (func $run (export "run") (param $n i32) (result i32)
            (if (result i32) (i32.eqz (local.get $n))
              (then (i32.const 42))
              (else (return_call_ref $t (ref.func $run)
                (i32.sub (local.get $n) (i32.const 1)))))))
    "#;
    let result = run_export(wat, "run", &[Value::I32(DEPTH)]).expect("no stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
}
```

Additionally add one host-path test, `tail_call_to_host_returns_host_results`,
in the same file: a module importing `(func $inc (import "env" "inc") (param
i32) (result i32))` and an exported wrapper that `return_call`s `$inc`;
register a host function (mirroring the registration API used in the repo's
existing host-function tests) that adds 1 and records its argument; assert the
export returns `[I32(8)]` for input `7` and the recorded argument is `7`.

### C6 — Fuzz target + registration

`fuzz/fuzz_targets/return_call_depth.rs`, full file content:

```rust
#![no_main]

//! Fuzz target: tail-call stack safety on the trampoline.
//!
//! Builds a self-recursive `return_call` module at a fuzzed depth (1 to
//! 100_000 — far beyond MAX_CALL_DEPTH) and asserts it completes with the
//! base-case value instead of trapping `CallStackExhausted`. Invariant:
//! tail calls must not grow the call stack. Where a call mirrors existing
//! API usage, copy the exact form from `validate_lower.rs` / the core
//! integration tests.

use baedeker_core::binary::module::Module;
use baedeker_core::runtime::{Store, Value, execute_export};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|depth: u32| {
    let depth = (depth % 100_000) + 1;
    let wat = format!(
        "(module (func $run (export \"run\") (param $n i32) (result i32) \
           (if (result i32) (i32.eqz (local.get $n)) \
             (then (i32.const 42)) \
             (else (return_call $run (i32.sub (local.get $n) (i32.const 1)))))))"
    );
    let mut buffer = wast::parser::ParseBuffer::new(&wat).expect("wat parses");
    let parsed = wast::parser::parse::<wast::Wat>(&mut buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    let reg = module.lower().expect("module lowers");
    let store = Store::instantiate(&reg).expect("module instantiates");
    let result = execute_export(&reg, &store, "run", &[Value::I32(depth)])
        .expect("tail recursion completes without stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
});
```

`fuzz/Cargo.toml`: add `wast = "240"` to `[dependencies]` (same version as
the workspace dev-dependency), and append:

```toml
[[bin]]
name = "return_call_depth"
path = "fuzz_targets/return_call_depth.rs"
test = false
doc = false
```

## Tests (the complete behavioral pin)

1. **Spec suite (RED → GREEN):** after C4, `cargo test -p baedeker-core
   --test runtime_official` must pass with `return_call.wast`,
   `return_call_indirect.wast`, `return_call_ref.wast` executing (deferred
   count 12 → 9; total failures 0).
2. `direct_tail_recursion_is_stack_safe` — DEPTH 1_000_000, result exactly
   `[I32(42)]`, no `CallStackExhausted`.
3. `indirect_tail_recursion_is_stack_safe` — same via table +
   `return_call_indirect`.
4. `ref_tail_recursion_is_stack_safe` — same via `ref.func` +
   `return_call_ref`.
5. `tail_call_to_host_returns_host_results` — result `[I32(8)]` for input
   `7`; host observed argument `7`.
6. **No regressions:** every existing test in the workspace stays green.

## Constraints

- No new crates in the root workspace. The ONLY dependency addition is
  `wast = "240"` in `fuzz/Cargo.toml` (C6).
- License headers (as in C5/C6) on new files; Apache-2.0 SPDX line.
- Files out of bounds: everything not in the Files list — in particular
  `validate/`, `binary/`, `types.rs`, `baedeker-ffi`, `baedeker-cli`,
  `baedeker-gpu`, `baedeker-borsalino`, `swift/`, `book/`.
- Do not rename or alter `MAX_CALL_DEPTH`, fuel semantics, or any existing
  error kind. Tail calls reuse existing traps only.
- Where a Contract references an API whose exact signature it cannot know
  (`Store::instantiate`, `module.lower`, `execute_export`, host-function
  registration, `wast::Wat::encode`), copy the repo's existing call form —
  the compiler is the arbiter; report deviations in the ambiguity footer.

## Completion (run in repo root; fix until all green)

```bash
cargo fmt --all
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|^error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
(cd fuzz && cargo build --bin return_call_depth 2>&1 | tail -5)
```

## Out of scope

- Exception handling, SIMD, memory64, GC, threads (later slices).
- Any validator or decoder change (already correct).
- Cross-instance trampoline transfer (documented delegation at depth + 1).
- Performance tuning beyond preserving existing call throughput.
