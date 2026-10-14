# EH Unit 3 Contract — IR, lowering, execution, AOT v3

Unit: U3 of the EH ladder (`docs/plans/eh-slice.md`). Depends on: U2
(`a659f60`). Followed by: U4 (linking/instance surface).

This is the architecture-carrying unit. Design deviations from the plan
doc (contract wins): `RegCatch` has NO `push_exnref` field — the exnref
slot rides as the LAST entry of `dst_regs` (the validator already types
`_ref` labels as tag-params + exnref, so the landing values are simply
`payload ++ [ExnRef(Some(exn))]`). `RuntimeErrorKind::Throw { exn: u32 }`
carries an arena index (keeps `Eq` derives intact — `Vec<Value>` cannot).

## Files

Modify:
- `crates/baedeker-core/src/runtime/mod.rs` — Value, errors, run loop
- `crates/baedeker-core/src/runtime/store.rs` — exception arena, tag types
- `crates/baedeker-core/src/lower/mod.rs` — IR types, lowering
- `crates/baedeker-core/src/aot.rs` — AOT_FORMAT_VERSION 3 + test
- `crates/baedeker-core/tests/eh_validation.rs` — the final valid module
  now lowers: move it from `validated_only` to the full pipeline
  (execution succeeds only if the function is not invoked; the runner
  does not invoke — keep it that way)

Create:
- `crates/baedeker-core/tests/eh_execution.rs`

## Contracts

### 1. `runtime/mod.rs` — values and errors

(a) `Value` gains (appended last):

```rust
    /// An exception reference: `None` is null; `Some(idx)` indexes the
    /// store's exception arena. Arena records live as long as the store.
    ExnRef(Option<u32>),
```

`val_type()` arm: `Value::ExnRef(_) => ValType::Ref(RefType::from_parts(true, HeapType::Exn))`.

(b) `RuntimeErrorKind` gains (appended last):

```rust
    /// A wasm exception is in flight (arena index into the store).
    /// Never crosses the host boundary: `execute_export` converts an
    /// escaping `Throw` into `UncaughtException`.
    Throw { exn: u32 },
    /// An exception escaped to the host. `exn` is the arena index when a
    /// store backs execution; `None` when a storeless execution threw.
    UncaughtException { exn: Option<u32> },
```

`Display`: `"uncaught exception"` for `UncaughtException`, `"exception in flight"` for `Throw`.

(c) Null plumbing: extend `is_null_ref` (and any sibling null-checking
match over `Value`) so `ExnRef(None)` is null, `ExnRef(Some(_))` is not.
Local zero-init (`ref_null_value` / the local-init match in `run`)
must produce `Value::ExnRef(None)` for nullable-exn local types.

### 2. `runtime/store.rs` — arena and tag registry

```rust
/// A thrown exception record. `tag` is `(instance_id, tag_idx)` — the
/// throwing frame's module-local tag index qualified by its instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ExceptionRecord {
    pub tag: (u32, u32),
    pub payload: Vec<Value>,
}
```

`Store` gains (match the existing `RefCell` field style):

```rust
    /// Exception arena: indexed by `Value::ExnRef(Some(idx))` and by the
    /// `Throw { exn }` error. Grows monotonically; reclamation is the GC
    /// slice's concern.
    exceptions: core::cell::RefCell<Vec<ExceptionRecord>>,
    /// Function types of registered tags, indexed by a module-agnostic
    /// tag id. Populated at instantiation (U4); used for host-side
    /// inspection and cross-instance matching.
    tag_types: Vec<FuncType>,
```

Methods (match existing accessor style):

```rust
    pub fn allocate_exception(&self, tag: (u32, u32), payload: Vec<Value>) -> u32;
    pub fn exception(&self, idx: u32) -> Option<ExceptionRecord>; // clone
    pub fn register_tag_type(&mut self, ty: FuncType) -> u32;      // returns tag id
```

Initialize both fields everywhere `Store` is constructed.

### 3. `lower/mod.rs` — IR types

(a) `RegTerm` gains (appended LAST — serde tag stability):

```rust
    /// `throw` — raise the module-local tag with the payload registers.
    Throw {
        tag: TagIdx,
        values: Vec<Reg>,
    },
    /// `throw_ref` — re-raise the exception held in the exnref register.
    ThrowRef {
        exn: Reg,
    },
```

(b) New IR types near `RegTerm`:

```rust
/// A static protected region produced by lowering one `try_table`.
/// `first_block..end_block` (exclusive) is the contiguous block range of
/// the try body; control outside that range is not protected by this
/// region even if it dynamically executed inside the try earlier.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProtectedRegion {
    pub first_block: u32,
    pub end_block: u32,
    pub catches: Vec<RegCatch>,
}

/// One catch clause, lowered. On a match the runtime writes the payload
/// values (plus the exnref, for `_ref` clauses, as the LAST value) into
/// `dst_regs` and enters `target_block`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RegCatch {
    /// `None` = catch_all. A module-local tag index (same module as the
    /// region's function).
    pub tag: Option<TagIdx>,
    pub target_block: u32,
    /// Registers the landing writes: tag params in order, then the
    /// exnref register for `catch_ref`/`catch_all_ref`.
    pub dst_regs: Vec<Reg>,
}
```

(c) `RegFunc` gains `pub regions: Vec<ProtectedRegion>,` (LAST field);
`RegModule` gains `pub tags: Vec<TypeIdx>,` (LAST field — the module's
defined-tag type indices, mirroring `Module.tags`; import tag types are
resolved at instantiation in U4). Initialize both in every construction
site (`lower_module`, any test builders — grep `RegFunc {` and
`RegModule {`).

(d) Lowering arms in the instruction walker:

`Instr::Throw(tag)`:
```rust
Instr::Throw(tag) => {
    let (params, _results) = <resolve the tag's func type the same way
        U2's resolve_tag_type does — at lower time, from module.types
        plus imported tag descs; factor or duplicate, report which>;
    let mut values = Vec::with_capacity(params.len());
    for &expected in params.iter().rev() {
        let found = self.pop_expect(offset, "throw", expected)?;
        values.push(found.reg);
    }
    values.reverse();
    self.finish_block(RegTerm::Throw { tag: *tag, values });
    self.enter_unreachable(); // same mechanism Br uses after finishing a block
}
```
(Adapt to how `Instr::Br` lower does it — pop values reverse, finish
block with terminator, mark frame unreachable so following code starts
a fresh block.)

`Instr::ThrowRef`:
```rust
Instr::ThrowRef => {
    let found = self.pop_expect(offset, "throw_ref",
        ValType::Ref(RefType::from_parts(true, HeapType::Exn)))?;
    self.finish_block(RegTerm::ThrowRef { exn: found.reg });
    self.enter_unreachable();
}
```

`Instr::TryTable(tt)`:
```rust
Instr::TryTable(tt) => {
    // 1. Type the blocktype like Block/Loop (existing helper).
    // 2. Pop params, push a LabelFrame with kind FrameKind::Block
    //    (the body is an ordinary block region).
    // 3. BEFORE pushing the frame: for each catch clause (in order):
    //    let frame_pos = self.label_stack.len() - 1 - catch.label;
    //    let value_types = tag params (+ exnref last for _ref; [] /
    //    [exnref] for _all / _all_ref) — same resolution as Throw's;
    //    record PendingCatch { region_idx, clause_idx, frame_pos,
    //    value_types } in a new `pending_catches: Vec<PendingCatch>`
    //    builder field;
    //    push a placeholder RegCatch { tag, target_block: 0,
    //    dst_regs: vec![] } into the region being built.
    // 4. Create the region: region_idx = self.regions.len();
    //    first_block = the block index the body will start at
    //    (self.blocks.len() as u32 — the next block to be created);
    //    end_block: 0 placeholder; catches: the placeholders.
    // 5. Push the frame (like Block: param regs materialized the same
    //    way — the body entry must start a fresh block).
}
```

`PendingCatch`:
```rust
struct PendingCatch {
    region_idx: usize,
    clause_idx: usize,
    /// Position in `label_stack` of the targeted frame at try_table
    /// time (the clause labels resolve in the OUTER context).
    frame_pos: usize,
    /// The value types the landing writes (tag params [+ exnref]).
    value_types: Vec<ValType>,
}
```

(e) Patch sites:

- At the **End of a frame** (the non-function path that patches
  `PendingBranch`es — and the function path that patches `br 0`):
  after computing the continuation's expected regs (`values`, the regs
  the continuation reads) and `continuation_idx`, ALSO patch every
  `PendingCatch` whose `frame_pos` matches: allocate fresh regs typed by
  `value_types` (`self.alloc_regs(&value_types)` or the existing reg
  allocation idiom — these are the dst_regs), record
  `RegCatch { target_block: continuation_idx, dst_regs }` into
  `self.regions[region_idx].catches[clause_idx]`. NOTE: do NOT emit
  copies — there is no branching block; the landing writes directly.
  Leave `PendingCatch`es targeting other frames in place (they patch at
  their own frame's End), exactly like `PendingBranch`.
  ⚠ For a **loop** target frame the branch convention differs
  (branches deliver into `param_regs` and target the loop header):
  catch clauses targeting a Loop frame patch at the point where
  br-to-loop resolves — dst_regs = the loop frame's `param_regs`,
  target_block = its `header_block`. Find where br-to-loop patches and
  add the matching PendingCatch patch there. (The validator already
  types such a label by the loop's params.)
- At the **End of the try_table's own frame** (FrameKind::Block frames
  can't distinguish — instead: when popping a frame, check whether any
  region has `end_block == 0` and `first_block == <the block index at
  frame push>`… simpler: tag the LabelFrame with the region: extend
  FrameKind::Block frames created for try_table with a distinct variant
  `FrameKind::TryTable { region_idx }` — treated EXACTLY like Block
  everywhere else (if-arms unaffected), and at its End set
  `self.regions[region_idx].end_block = continuation_idx` (the block
  AFTER the try body's last block — exclusive bound; the body's blocks
  are all blocks created between first_block and continuation_idx).

(f) `lower_module`: fill `RegModule.tags` from `module.tags` (defined
tags only). Every function gets `regions: Vec::new()` initially (the
builder fills it).

(g) Debug assertion (dev builds): after lowering a function with
regions, assert each region's `first_block < end_block` and that
regions are properly nested (for any two regions, either disjoint or
one contains the other).

### 4. `runtime/mod.rs` — execution

(a) Region consult helper:

```rust
/// Find the innermost protected region containing `block_idx` whose
/// catch clauses match the in-flight exception, and describe the
/// landing. `None` = not caught in this frame.
fn consult_catch(
    func: &RegFunc,
    block_idx: u32,
    exn_idx: u32,
    store: &Store,
) -> Option<(u32, &[Reg], Option<Value>)> {
    let record = store.exception(exn_idx)?;
    let region = func
        .regions
        .iter()
        .filter(|r| r.first_block <= block_idx && block_idx < r.end_block)
        .max_by_key(|r| r.first_block)?; // innermost nesting wins
    for catch in &region.catches {
        let matches = match catch.tag {
            None => true, // catch_all
            Some(tag) => record.tag == (store.instance_id(), tag.0),
        };
        if !matches {
            continue;
        }
        // landing values: payload, plus exnref for _ref clauses —
        // recognized by dst_regs being one longer than the payload.
        let is_ref = catch.dst_regs.len() == record.payload.len() + 1;
        return Some((catch.target_block, &catch.dst_regs[..], if is_ref {
            Some(Value::ExnRef(Some(exn_idx)))
        } else {
            None
        }));
    }
    None
}
```

(Expose `instance_id()` on Store if private. The `is_ref` derivation is
safe: the validator guarantees dst_regs length = payload arity (+1 iff
`_ref`); add a debug_assert.)

(b) Termination in the run loop's terminator match:

```rust
RegTerm::Throw { tag, values } => {
    let mut payload = Vec::with_capacity(values.len());
    for &reg in values {
        payload.push(get_reg(&registers, reg)?);
    }
    let Some(store) = store else {
        return Err(RuntimeError { kind: RuntimeErrorKind::UncaughtException { exn: None } });
    };
    let exn = store.allocate_exception((store.instance_id(), tag.0), payload);
    match consult_catch(func, block_idx, exn, store) {
        Some((target, dst_regs, exnref)) => {
            land_catch(&mut registers, target, dst_regs, exn, exnref, store, &mut block_idx)?;
            continue 'tail;
        }
        None => return Err(RuntimeError { kind: RuntimeErrorKind::Throw { exn } }),
    }
}
RegTerm::ThrowRef { exn } => {
    let value = get_reg(&registers, *exn)?;
    let idx = match value {
        Value::ExnRef(Some(idx)) => idx,
        Value::ExnRef(None) => {
            return Err(trap(RuntimeTrap::NullReference)); // null exnref rethrow traps
        }
        _ => return Err(trap(RuntimeTrap::NullReference)), // unreachable; validated
    };
    let Some(store) = store else {
        return Err(RuntimeError { kind: RuntimeErrorKind::UncaughtException { exn: None } });
    };
    match consult_catch(func, block_idx, idx, store) {
        Some((target, dst_regs, exnref)) => {
            land_catch(&mut registers, target, dst_regs, idx, exnref, store, &mut block_idx)?;
            continue 'tail;
        }
        None => return Err(RuntimeError { kind: RuntimeErrorKind::Throw { exn: idx } }),
    }
}
```

with

```rust
/// Write the landing: payload (+ exnref) into dst_regs, set block_idx.
fn land_catch(
    registers: &mut [Option<Value>],
    target: u32,
    dst_regs: &[Reg],
    exn_idx: u32,
    exnref: Option<Value>,
    store: &Store,
    block_idx: &mut u32,
) -> Result<(), RuntimeError> {
    let record = store
        .exception(exn_idx)
        .ok_or(RuntimeError { kind: RuntimeErrorKind::MissingReturn })?; // arena invariant
    for (&reg, value) in dst_regs.iter().zip(record.payload.iter().chain(exnref.iter())) {
        set_reg(registers, reg, *value)?;
    }
    *block_idx = target;
    Ok(())
}
```

(Adapt borrow shapes to the real `run` loop — `registers` is a local
`Vec<Option<Value>>` there; inline `land_catch` if borrowck prefers.)

(c) **Nested-call interception** — the three call ops (`RegOp::Call`,
`RegOp::CallIndirect`, `RegOp::CallRef` inside the straight-line loop)
each currently do `let returned = execute_call*(...)?;`. Change each to:

```rust
let returned = match execute_call(module, store, callee_idx, &call_args, depth) {
    Err(e) if matches!(e.kind, RuntimeErrorKind::Throw { .. }) => {
        let exn = match e.kind { RuntimeErrorKind::Throw { exn } => exn, _ => unreachable!() };
        match store.and_then(|s| consult_catch(func, block_idx, exn, s)) {
            Some((target, dst_regs, exnref)) => {
                // write landing then jump — adapt to land_catch/inline
                ...;
                block_idx = target;
                continue 'tail;   // exits the `for instr` loop into the next block
            }
            None => return Err(e),
        }
    }
    other => other?,
};
```

Do NOT intercept tail-call terminators (a tail hop replaces the frame;
the callee's own regions govern — correct semantics) or traps
(`RuntimeTrap` is not catchable).

(d) Host boundary: in `execute_export` (and `run_start`), map an
escaping `Throw { exn }` to `UncaughtException { exn: Some(exn) }` so
`Throw` never crosses to hosts.

### 5. `aot.rs`

`AOT_FORMAT_VERSION` 2 → 3. In `rejects_bad_envelope` add a `found: 2`
rejection case mirroring U-fix `0b48c81`'s `found: 1` one (comment:
v2 predates EH regions/tag fields on the serialized IR).

### 6. Tests — `tests/eh_execution.rs`

License header. Helpers (dev-dep `wast` is available to integration
tests — mirror `lower_wat` from `src/lower/mod.rs` tests):

```rust
fn wat_module(source: &str) -> RegModule { /* wast parse → encode → decode → lower */ }
fn run(source: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    /* full pipeline with a Store (link nothing; the EH tests import nothing) */
}
```

Use `assert_eq!` on `Value` results / `RuntimeErrorKind` (the ErrorKind
needs PartialEq — it derives it). Enumerate:

1. `throw_caught_with_payload` —
   `(module (tag (param i32)) (func (export "f") (result i32)
      (block $h (result i32)
        (try_table (catch 0 $h) (i32.const 42) (throw 0))
        (i32.const 7))))` → `run("f", [])` == `[I32(42)]`.
2. `catch_all_catches` — catch_all on a throw; label empty; body after
   try supplies value → 7.
3. `uncaught_exception_errors` — throw with no enclosing try →
   `Err(UncaughtException { exn: Some(_) })`.
4. `nested_innermost_region_wins` — inner try catch_all returns 1,
   outer catch would return 2 → expect 1.
5. `first_matching_clause_wins` — two clauses for the same tag
   (different labels/results) → first one's landing.
6. `cross_function_propagation` — callee throws, caller's try_table
   catches; caller returns the payload.
7. `catch_ref_and_rethrow` — inner catch_ref catches, rethrows via
   throw_ref, outer catch_all lands → verifies ExnRef plumbing.
8. `throw_ref_null_traps` — `(func (local exnref) (local.get 0)
   (throw_ref))` → `Err(Trap(NullReference))`.
9. `tag_mismatch_propagates` — throw tag 1; region catches only tag 0;
   outer catch_all catches → distinguishable result.
10. `multi_value_payload_order` — tag `(param i32 f64)`, block label
    `(result i32 f64)`, catch lands both; return them (via local.set or
    a second block) → `[I32(5), F64(6.5)]`.
11. `try_body_completes_normally` — no throw; region skipped; result
    from the body path.
12. `payload_not_copied_into_wrong_clause_type` — covered by 5/9
    (dedupe if redundant — keep at least 10 distinct tests).
13. `aot_roundtrip_preserves_eh` — `serialize`/`deserialize` a module
    with a try_table; execute before and after; identical results (this
    test lives where the other AOT roundtrip helpers are reachable —
    mirror `tests/aot_roundtrip.rs`'s fixture-lowering helper with the
    WAT path).
14. `storeless_throw_is_uncaught` — `execute_func(&funcs[0], &[])`
    (no store) on a throwing module → `Err(UncaughtException { exn: None })`.

Also extend the lower-shape unit tests in `src/lower/mod.rs`
(`#[cfg(test)]`): one test asserting the region table of a nested
try_table — innermost `first_block` greater than outer's, both
`end_block > first_block`, catch dst_regs lengths match tag arity (+1
for `_ref`).

## Constraints

- No new dependencies. `no_std` + alloc in the crate proper.
- Only the files listed (plus compile-required kind-name arms in
  `tests/spec.rs` — report if needed).
- serde-derived enums: append variants only; new struct fields LAST.
- Do not alter validator semantics (U2's).

## Completion

```
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
```

Official suite unchanged: `runtime_official` → `failures=0`.

## Out of scope

Tag imports/exports at instantiation (U4), cross-instance tag identity
beyond the `(instance_id, tag_idx)` pair (U4), host `link_tag` (U4),
vendored corpus (U5).
