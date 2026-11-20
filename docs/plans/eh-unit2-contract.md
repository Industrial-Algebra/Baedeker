# EH Unit 2 Contract — validation

Unit: U2 of the EH ladder (`docs/plans/eh-slice.md`). Depends on: U1
(`4f455dd`). Followed by: U3 (IR + execution).

## Reference semantics (pinned from spec interpreter `valid.ml` @ pin)

Verified against `interpreter/valid/valid.ml` at commit
`dfa3f32a881aecc60a8c792da3c25787ccb15572`:

- `TryTable (bt, cs, es)`: check blocktype; **catch clauses are checked in
  the context BEFORE the block's own label is pushed** (`check_catch c …`,
  body uses `c'` with the label); the frame otherwise behaves like `block`.
- `check_catch`:
  - `catch x1 x2`: tag `x1`'s param types must match label `x2`'s types.
  - `catch_ref x1 x2`: tag params **plus `(ref no-null exn)`** (non-null!)
    must match the label's types.
  - `catch_all x`: label types must be empty.
  - `catch_all_ref x`: label types must be exactly `[ (ref no-null exn) ]`.
- `Throw x`: pops tag `x`'s param types; **stack-polymorphic after**
  (`-->...` operator — same class as `br`/`unreachable`).
- `ThrowRef`: pops one **nullable** exnref (`(ref null exn)`); polymorphic
  after.
- Module level: tag declarations (defined and imported) must reference a
  func type with **empty results** ("non-empty tag result type").
- Tag index space: imported tags first, then defined tags (same layout as
  functions/globals).

## Files

Create:
- `crates/baedeker-testdata/spec/eh-validation/eh-validation.wast`
- `crates/baedeker-core/tests/eh_validation.rs`

Modify:
- `crates/baedeker-core/src/validate/state.rs` (ControlKind)
- `crates/baedeker-core/src/validate/error.rs` (new kinds)
- `crates/baedeker-core/src/validate/mod.rs` (instruction + module-level arms)
- `crates/baedeker-core/tests/spec.rs` (kind-name table if the new kinds must
  be listed there — follow `ValidationErrorKind::Unsupported` from U1)

## Contracts

### 1. `state.rs`

Add to `ControlKind` (after `If`):

```rust
    /// A `try_table` block; labels resolve like `Block` (end types).
    TryTable,
```

`label_types` (or wherever `ControlKind::Loop => start_types` selects
loop-label semantics) must return **end types** for `TryTable` — same arm
as `Block`. Grep every `ControlKind::` match and extend each consistently
(`finish_frame` treat it like `Block`; no else-branch).

### 2. `error.rs`

Add (matching existing variant style/derives/Display):

```rust
    /// Tag declaration references a func type with results.
    NonEmptyTagResult {
        type_idx: u32,
    },
    /// Instruction referenced a tag index outside the tag index space.
    UnknownTag {
        idx: u32,
    },
```

Display texts: `"non-empty tag result type"` and
`"unknown tag {idx}"` (match the crate's existing Display phrasing style).

### 3. `validate/mod.rs`

(a) **Tag index space helper** (near the other index-space helpers — find
how `resolve_global_type`/function-index helpers are structured and mirror):

```rust
/// Resolve a tag index to its (params, results) func type.
/// Index space: imported tags first, then module-defined tags.
fn resolve_tag_type(
    module: &Module<'_>,
    function: FuncIdx,
    idx: TagIdx,
    offset: usize,
) -> Result<(Vec<ValType>, Vec<ValType>), ValidationError> {
    let idx_usize = idx.0 as usize;
    let imported = module.imports.iter().filter(|import| {
        matches!(import.desc, ImportDesc::Tag(_))
    }).count();
    let type_idx = if idx_usize < imported {
        match &module.imports.iter()
            .filter(|import| matches!(import.desc, ImportDesc::Tag(_)))
            .nth(idx_usize)
            .expect("count checked above")
            .desc
        {
            ImportDesc::Tag(type_idx) => *type_idx,
            _ => unreachable!("filtered to tag imports"),
        }
    } else {
        let defined = idx_usize - imported;
        match module.tags.get(defined) {
            Some(type_idx) => *type_idx,
            None => {
                return Err(ValidationError {
                    offset: ByteOffset(offset),
                    function: Some(function),
                    kind: ValidationErrorKind::UnknownTag { idx: idx.0 },
                });
            }
        }
    };
    let func_type = <resolve type_idx into module.types like existing
        type-resolution helpers do, with their unknown-type error>;
    Ok((func_type.params.clone(), func_type.results.clone()))
}
```

Adapt to the crate's real helper names/idioms — the shape above is the
contract (imported-first index space, `UnknownTag` on OOB, type resolution
reusing the existing path).

(b) **Instruction arms** — replace the U1 `Unsupported` arms:

```rust
Instr::TryTable(try_table) => {
    let sig = resolve_block_type(module, function, try_table.block_type, offset)?;
    pop_exact(function, state, &sig.params, offset)?;
    for catch in &try_table.catches {
        validate_catch(module, function, state, catch, offset)?;
    }
    state.push_frame(ControlKind::TryTable, try_table.block_type, sig.params, sig.results);
}
Instr::Throw(tag) => {
    let (params, _results) = resolve_tag_type(module, function, *tag, offset)?;
    pop_exact(function, state, &params, offset)?;
    state.enter_unreachable();
}
Instr::ThrowRef => {
    pop_expect(
        function,
        state,
        ValType::Ref(RefType::from_parts(true, HeapType::Exn)),
        offset,
        "throw_ref",
    )?;
    state.enter_unreachable();
}
```

(Adapt enum-path spelling — `ControlKind::Block` vs bare `Block` — to the
file's idiom.)

```rust
/// Validate one catch clause of a try_table. Labels resolve in the
/// CURRENT frame stack — before the try_table's own frame is pushed
/// (reference: check_catch uses the outer context `c`).
fn validate_catch(
    module: &Module<'_>,
    function: FuncIdx,
    state: &ValidatorState<'_>,
    catch: &TryTableCatch,
    offset: usize,
) -> Result<(), ValidationError> {
    let label_types = validate_label(function, state, catch.label, offset)?.to_vec();
    let expected: Vec<ValType> = match catch.kind {
        TryTableCatchKind::Catch | TryTableCatchKind::CatchRef => {
            let tag = catch.tag.expect("decoder guarantees Some for catch/catch_ref");
            let (params, _results) = resolve_tag_type(module, function, tag, offset)?;
            let mut expected = params;
            if matches!(catch.kind, TryTableCatchKind::CatchRef) {
                expected.push(ValType::Ref(RefType::from_parts(false, HeapType::Exn)));
            }
            expected
        }
        TryTableCatchKind::CatchAll => Vec::new(),
        TryTableCatchKind::CatchAllRef => {
            alloc::vec![ValType::Ref(RefType::from_parts(false, HeapType::Exn))]
        }
    };
    if label_types.len() != expected.len()
        || label_types.iter().zip(expected.iter())
            .any(|(found, want)| !<the crate's operand-compat predicate used
                by pop_branch_types for matching values to label types>)
    {
        return Err(ValidationError {
            offset: ByteOffset(offset),
            function: Some(function),
            kind: ValidationErrorKind::<the existing label/type-mismatch kind
                used when br value types disagree with label types>,
        });
    }
    Ok(())
}
```

Find how `pop_branch_types` compares operand types to label types and reuse
THAT predicate and THAT mismatch error kind verbatim (this keeps unreachable-
polymorphic and subtyping behavior identical to `br`).

(c) **Module-level validation** — in `validate_module` (or where imports/
exports are checked):

- For every import `ImportDesc::Tag(type_idx)` and every defined
  `module.tags[i]`: resolve `type_idx`; if it does not name a func type,
  use the existing wrong-type error for that; if the func type has
  non-empty results, error `NonEmptyTagResult { type_idx }`.
- Export arm `ExportDesc::Tag(TagIdx(idx))`: bounds-check against
  `imported_tags + module.tags.len()` using `UnknownTag { idx }` (the
  fixture expects an "unknown tag" message). Replace the U1 `Unsupported`
  arm.

### 4. Fixture `eh-validation.wast` (exact content)

```wast
;; EH validation contract fixtures (unit 2).
;; Valid modules instantiate; invalid ones must be rejected.

(module
  (type (func (param i32)))
  (tag)              ;; type 1 implicit? no — see note below
)
```

NO — the final content is exactly this (authored spec-text, do not
reorder):

```wast
;; Exception-handling validation fixtures (EH unit 2).

(module
  (type (func))
  (type (func (param i32)))
  (tag (type 1))
  (tag $t2 (type 0))
  (export "t2" (tag $t2))
)

(module
  (import "" "" (tag (type 0)))
)

;; tag type must be a func type with empty results
(assert_invalid
  (module (type (func (result i32))) (tag (type 0)))
  "non-empty tag result type"
)
(assert_invalid
  (module (type (func (result i32))) (import "" "" (tag (type 0))))
  "non-empty tag result type"
)

;; unknown tag referenced by throw
(assert_invalid
  (module (tag) (func (throw 1)))
  "unknown tag"
)

;; catch label arity mismatch: label wants i32, tag has no params
(assert_invalid
  (module
    (tag)
    (func (result i32)
      (block $h (result i32)
        (try_table (catch 0 $h) (nop))
        (i32.const 1)
      )
    )
  )
  "type mismatch"
)

;; catch label arity mismatch: label wants nothing, tag has i32 param
(assert_invalid
  (module
    (tag (param i32))
    (func
      (block $h
        (try_table (catch 0 $h) (i32.const 1) (throw 0))
      )
    )
  )
  "type mismatch"
)

;; catch_ref label arity mismatch: missing the exnref slot
(assert_invalid
  (module
    (tag (param i32))
    (func (result i32)
      (block $h (result i32)
        (try_table (catch_ref 0 $h) (i32.const 1) (throw 0))
        (i32.const 7)
      )
    )
  )
  "type mismatch"
)

;; catch_all label must have empty types
(assert_invalid
  (module
    (func (result i32)
      (block $h (result i32)
        (try_table (catch_all $h) (nop))
        (i32.const 1)
      )
    )
  )
  "type mismatch"
)

;; catch_all_ref label must be exactly [exnref]
(assert_invalid
  (module
    (func
      (block $h
        (try_table (catch_all_ref $h) (nop))
      )
    )
  )
  "type mismatch"
)

;; catch label type mismatch: label [f32] vs tag param [i32]
(assert_invalid
  (module
    (tag (param i32))
    (func (result f32)
      (block $h (result f32)
        (try_table (catch 0 $h) (nop))
        (f32.const 1)
      )
    )
  )
  "type mismatch"
)

;; unknown catch label
(assert_invalid
  (module (tag) (func (try_table (catch 0 3))))
  "unknown label"
)

;; throw parameter mismatches
(assert_invalid
  (module (tag (param i32)) (func (throw 0)))
  "type mismatch"
)
(assert_invalid
  (module (tag (param i32)) (func (f32.const 1) (throw 0)))
  "type mismatch"
)

;; throw_ref operand must be exnref
(assert_invalid
  (module (func (i32.const 1) (throw_ref)))
  "type mismatch"
)
(assert_invalid
  (module (func (local exnref) (local.get 0) (drop) (i32.const 2) (throw_ref)))
  "type mismatch"
)

;; export bounds
(assert_invalid
  (module (tag) (export "t" (tag 1)))
  "unknown tag"
)

;; valid: exnref locals, catch into matching label, throw_ref on nullable exnref
(module
  (tag (param i32))
  (func (param exnref) (result i32)
    (local exnref)
    (block $h (result i)
      (try_table (catch 0 $h) (i32.const 1) (throw 0))
      (i32.const 2)
    )
  )
  (func (param exnref) (local.get 0) (throw_ref))
)
```

NOTE (author-audited): the first catch case is deliberately a clause-only
error — the try body is `(nop)` or ends in `throw` (polymorphic), so the
ONLY error is the clause/label disagreement. The final module's first
function: label `$h` wants `[i32]` = tag params ✓ clause valid; body
pushes i32 then throws (polymorphic) ✓; `(i32.const 2)` supplies the
block result after the (empty-typed) try_table ✓.

Also: tag.wast at the pin shows `(tag)` with implicit `(func)` type — the
fixture relies on the same shorthand.

### 5. Runner `tests/eh_validation.rs`

Mirror `runtime_wast.rs`'s machinery, restricted to the new subdir:

- Iterate `baedeker_testdata::spec_wast_cases("eh-validation")`.
- Same directive handling as `runtime_wast.rs` for: `Module` (decode →
  validate → lower → instantiate — the two valid EH-free-of-execution
  modules must go through the FULL pipeline; the final "valid" module
  contains `try_table`/`throw` which the LOWERER does not support yet —
  see (b) below), `AssertInvalid` (decode+validate must fail with a
  validation error), `AssertMalformed` (decode error).
- (b) The final valid module cannot lower yet (U3). Two options, pick
  ONE and report which: (1) for any module where `module_uses_eh(&Module)`
  is true — defined as: any `TryTable`/`Throw`/`ThrowRef` in any function
  body, **or any `ImportDesc::Tag` import** (tag imports cannot link
  until U4) — run only decode+validate, skip lower/instantiate, and count
  it as `validated_only` in the summary; or (2) move that module into
  comments until U3. Prefer (1). Module 1 of the fixture (tags defined,
  no EH instructions, no tag imports) MUST go through the full pipeline.
- Summary line format:
  `eh validation summary: files=1 directives=N modules=M assertions=K validated_only=J`
  and fail the test on any assertion failure or unexpected success.
- License header, no_std-irrelevant (integration test, std fine).

## Tests

The fixture IS the test corpus (21 directives: 2 module + 1 tag-import
module + 1 final valid module = 4 modules, 16 assert_invalid — recount
from the final text you land on and assert exactly that count in the
runner's summary expectation). Plus extend `tests/spec.rs`'s kind-name
table if required for compilation.

## Constraints

- No new dependencies. Do not touch `lower/` or `runtime/` beyond what
  compilation requires (nothing should be required this unit).
- Only the files listed.
- The fixture text is verbatim from this contract (modulo the rule-7
  escape hatch above).

## Completion

```
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
```

Plus official suite unchanged: `runtime_official` → `failures=0`.

## Out of scope

Lowering/execution (U3), tag linking/identity (U4), assert_unlinkable
semantics (U4), `assert_exception` harness directives (U5).
