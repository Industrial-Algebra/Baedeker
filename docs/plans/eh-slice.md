# WASM 3.0 Slice 3 — Exception Handling (try_table / exnref)

Branch: `feature/eh-slice` off `develop` (695e00b). Target: PR #64.

## Goal

Implement the wasm-3.0 exception-handling surface end-to-end and un-defer the
official corpus:

- `test/core/exceptions/{tag,throw,throw_ref,try_table}.wast` (4 files, at the
  vendoring pin `dfa3f32a881aecc60a8c792da3c25787ccb15572` in
  `crates/baedeker-testdata/spec/wast-official/COMMIT.txt`)
- `instance.wast` and `imports.wast` (currently deferred for exactly this:
  "tag section (exceptions) not implemented")

Suite today: 147 files / 9 deferred / 44,889 assertions / 0 failures.
Target: 153 files / **6 deferred** / 0 failures.

## Survey results (verified in-session, 2026-10-14)

Current state: **nothing exists** — no EH instructions in the decoder, no tag
section, no `exnref` type. Fully greenfield (roadmap: "not started").

Facts pinned from `wast-240.0.0` + `wasm-encoder-0.254.0` sources:

| Item | Encoding |
|---|---|
| `throw` | opcode `0x08`, tagidx immediate |
| `throw_ref` | opcode `0x0a` |
| `try_table` | opcode `0x1f`, blocktype, then vec of catch clauses |
| catch clause kinds | `0x00` catch (tagidx, labelidx) · `0x01` catch_ref (tagidx, labelidx) · `0x02` catch_all (labelidx) · `0x03` catch_all_ref (labelidx) |
| `exnref` valtype | shorthand byte `0x69` = `(ref null exn)`; heap type `exn` |
| tag section | id **13**, ordered after Memory (5), before Global (6) |
| tag entry | attribute byte (must be `0x00`, else malformed) + typeidx; typeidx must resolve to a func type with **empty results** (`assert_invalid "non-empty tag result type"`) |
| legacy EH | `try 0x06 / catch 0x07 / rethrow 0x09 / delegate 0x18 / catch_all 0x19` stay unknown opcodes — the corpus at the pin contains zero legacy tests (verified: `test/core/exceptions/` has exactly the 4 try_table-era files) |

Executor architecture (surveyed, favorable): block-based register IR
(`Vec<RegBlock>`, explicit `RegTerm` terminators), `run()` recurses per wasm
call with tail-call trampolining. Rust `Err` propagation is the unwinding
vehicle; each frame consults its own static protected-region table.

## Design (pinned — implementers do not redesign)

### IR

- `RegTerm::Throw { tag: TagIdx, values: Vec<Reg> }` and
  `RegTerm::ThrowRef { exn: Reg }` — **appended** to `RegTerm` (serde tag
  safety; see AOT note).
- Per-`RegFunc`: `regions: Vec<ProtectedRegion>` where
  `ProtectedRegion { first_block: u32, end_block: u32 /*exclusive*/,
  catches: Vec<RegCatch> }`,
  `RegCatch { tag: Option<TagIdx> /*None = catch_all*/, target_block: u32,
  dst_regs: Vec<Reg>, push_exnref: Option<Reg> }`.
  Try-body blocks lower to a contiguous run (structured control emits blocks
  in order — verify contiguity with a debug assert during lowering dev).
- Runtime consult: when `Err(Throw)` surfaces at any op/terminator in block
  `b`: scan `regions` for `first_block <= b < end_block`, pick the match with
  the **largest `first_block`** (innermost nesting wins), then first matching
  catch clause in declaration order. `tag` comparison is module-local
  (clause tag idx == thrown tag idx; both resolve within one module instance
  — correct even for imported tags). Match ⇒ write payload `Value`s into
  `dst_regs` (+ exnref into `push_exnref`), set `block_idx = target_block`,
  continue the block loop. No match ⇒ propagate `Err` to the Rust caller.

### Runtime values & store

- `Value::ExnRef(Option<u32>)` — index into a store-level arena (mirrors the
  existing `ExternRef(Option<u32>)` host-index pattern; keeps `Value: Copy`).
- `Store.exceptions: Vec<ExceptionRecord { tag: TagId, payload: Vec<Value> }>`;
  `TagId = u32` into `Store.tags` (instantiation-assigned identities; defined
  tags get fresh ids, imported tags resolve to the exporter's id —
  cross-instance identity via the store, like funcrefs).
- Uncaught exception reaching the host: `RuntimeErrorKind::UncaughtException`
  (trap message `uncaught exception`, matching the corpus).
- Arena growth is unbounded for v1 (records must stay alive while exnref
  values can exist); reclamation belongs to the GC slice.

### Validation

- `try_table bt catches`: type like `block`; each clause's label must be in
  scope (try_table's own label is depth 0, like block); label param types must
  equal: catch ⇒ tag's params; catch_ref ⇒ tag params + `exnref` last;
  catch_all ⇒ `[]`; catch_all_ref ⇒ `[exnref]`. Immediates (label depth,
  tag idx) validated **unconditionally, including unreachable code** (same
  law as the simd lane-index bounds — review r1 confirmed that pattern).
- `throw t`: pops tag params; stack-polymorphic after (like `br`).
- `throw_ref`: pops one `exnref`; polymorphic after.
- Tag type checks: typeidx exists, is a func type, empty results.
- exnref usable in locals/params/results/global types to the extent the
  corpus requires (survey during U2; globals with exnref init from
  `ref.null exn` if the tests demand).

### AOT format

`RegTerm` variants are appended (old tags stable), but `RegFunc`/`RegModule`
gain **struct fields** (`regions`, `tags`) — postcard layouts change.
⇒ `AOT_FORMAT_VERSION` **2 → 3** in U3, with a `found: 2` rejection case in
`rejects_bad_envelope` (the r1 lesson, applied proactively).

## Ladder

| Unit | Scope | Dispatch |
|---|---|---|
| U1 | binary + types surface: `exn` heap type + `0x69` shorthand, tag section decode + order rule, `ImportDesc::Tag`/`ExportDesc::Tag`, `TryTable`/`Throw`/`ThrowRef` decode, `Module.tags` | generation (deepseek) |
| U2 | validation: try_table/throw/throw_ref typing, catch-clause label typing, tag type checks, exnref in signatures | generation (deepseek) |
| U3 | IR + execution: `RegTerm::Throw/ThrowRef`, protected regions, lowering, `Value::ExnRef` + store arenas, run-loop catch/resume, uncaught trap, AOT v3 | generation (deepseek) |
| U4 | linking + instance surface: store tag registry, instantiation resolution (defined/imported), tag exports, harness `link_tag` + `register` identity | generation (deepseek) |
| U5 | vendor 4 files + un-defer (exceptions, instance, imports) + conformance grind to 0 | author-side grind |

## Risk register

1. **Catch-label arity vs blocktype interplay** — the catch label's params are
   NOT the try_table blocktype's params; mis-merging them is the classic bug.
   Pin with exact-type unit tests in U3.
2. **Payload order** — exception payload values map to `dst_regs` in tag-param
   order; exnref appends last for `_ref` clauses.
3. **Regions contiguity** — nested if/block inside try bodies must not break
   the contiguous-run assumption; debug-assert during dev, property-test via
   corpus in U5.
4. **tag.wast rec-group `assert_unlinkable`** — needs structural equivalence
   of rec-group types; the non-rec cases ride existing structural func-type
   equality. Grind-time decision: minimal structural canonicalization vs
   parking individual asserts. Do NOT let it expand into full GC type-canon.
5. **Uncaught-message exactness** — harness `assert_trap` matchers must
   accept `uncaught exception`; verify against corpus text in U5.
6. **AOT v3** — bump lands with the struct-field changes (U3), not after.
7. **`register` + tag identity in the harness** — imports.wast/instance.wast
   link tag exports to later modules; the harness linkage model (built for
   funcs) must extend without regressions to the existing 147 files.

## Out of scope

Legacy `try`/`catch`/`delegate`/`rethrow`; stack switching; GC type
canonicalization beyond what tag.wast strictly needs; exnref introspection
beyond throw_ref; arena reclamation; performance.

## Completion gates (per unit)

```
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
```

Final: `cargo test -p baedeker-core --test runtime_official` →
`files=153 deferred=6 … failures=0`; then the review loop (ia-moment review
class, rounds to convergence, breaker 12) over the cumulative PR diff before
the PR goes ready.
