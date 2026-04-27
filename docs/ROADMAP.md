# Baedeker — Roadmap

> A WASM runtime built in Rust, named for the most cautious and methodical of Pierson's Puppeteers.
> Like its namesake, Baedeker proceeds carefully through unknown territory — but gets there.

## Philosophy

Baedeker is a language-runtime and systems project: its binary decoding, validation, malformed-input
handling, and future robustness testing are all in service of standards-compliant WebAssembly
execution, portability, and implementation quality. This roadmap is therefore framed around spec
compliance, safe execution, and embedding ergonomics rather than offensive security use cases.

This roadmap is structured as a bottom-up traversal of the WebAssembly abstraction stack.
Each phase builds expertise in a specific layer before moving upward. Phases are designed to
produce a working (if incomplete) artifact at each boundary, so the project is always runnable
and testable — never in a state where three more layers need to exist before anything executes.

The target spec is full WebAssembly 3.0 validation, with execution support growing in phases on
top of that semantic front-end, and the primary deployment target is iOS via Rust FFI into Swift.
JIT compilation is explicitly out of scope for the initial architecture; the engine is
interpreter-first with AOT as a future layer.

---

## Phase 0 — Foundation

**Focus:** Project structure, spec familiarity, binary format fundamentals.

- Set up workspace: `baedeker` (top-level), `baedeker-core` (no_std engine), `baedeker-cli`
  (test harness), `baedeker-wasm` (meta — Baedeker compiled to WASM, for bootstrapping tests later).
- Implement LEB128 encoder/decoder with exhaustive edge case handling (overlong encodings,
  unsigned vs signed, maximum bit widths). This is your first contact with the spec's pedantry —
  treat it as calibration for the rigor the rest demands.
- Build the section parser: magic number, version, section IDs, section byte lengths.
  Parse but don't yet interpret section contents — just segment the binary into labeled byte spans.
- Implement a `Module` AST type that represents the parsed-but-not-validated structure.
- **Milestone:** Can ingest any `.wasm` binary and report its section layout without crashing.

### Spec sections to internalize
- [Binary Format](https://webassembly.github.io/spec/core/binary/index.html) — all of it.
- [Values](https://webassembly.github.io/spec/core/binary/values.html) — LEB128 specifics.

---

## Phase 1 — Type System & Validation

**Focus:** The WASM type system, structured control flow, and the validation algorithm.

### Progress checklist

#### Done
- [x] Parse the Type section: function signatures (`functype`), with the current decoded subset of
  WASM 2.0 value types used by Baedeker so far.
- [x] Parse Import and Function sections to build the function index space.
- [x] Parse the Code section and decode a growing instruction subset on demand.
- [x] Parse the Global section, including defined globals and raw initializer expressions.
- [x] Parse the Memory section, including defined memory limits.
- [x] Implement a first substantial slice of the **validation algorithm** for instruction
  sequences, including:
  - [x] stack polymorphism after unconditional branches
  - [x] block signature matching for `block`, `loop`, `if`
  - [x] correct label indexing for `br`, `br_if`, `br_table`
  - [x] type-correct `select` with explicit type annotations (2.0)
  - [x] function type index resolution
  - [x] call target resolution across imported and defined functions
  - [x] local index validation
  - [x] global index validation across imported and defined globals
  - [x] memory index validation across imported and defined memories
  - [x] final function result stack checking
- [x] Validate a useful current instruction subset including structured control flow, locals,
  calls, globals, `memory.size`, `memory.grow`, `select`, typed `select`, `i32.add`, `i64.add`,
  `i32.eqz`, and a small constant/comparison subset.
- [x] Validate defined global initializer expressions for the current const-expression subset:
  `i32.const`, `i64.const`, `f32.const`, `f64.const`, and `global.get` of imported immutable
  globals.
- [x] Build a validation error type that produces genuinely useful diagnostics, including:
  - [x] precise byte offsets for failing instructions
  - [x] decode-preserving validation errors
  - [x] operation-aware stack underflow diagnostics
  - [x] operation-aware type mismatch diagnostics
  - [x] richer branch/control result mismatch diagnostics
  - [x] final result mismatch diagnostics with full stack context
  - [x] index-space diagnostics with available-count context for globals and memories

#### In progress / partial
- [~] Expand instruction coverage from the current strong subset toward full WebAssembly 3.0
  validation.
  - Current support now includes structured control flow, direct calls and `call_indirect`,
    locals, globals, multi-memory / multi-table module validation, scalar memory load/store
    families, SIMD/vector memory operations and lane checks, explicit nonzero memory-index
    validation across `memory.size` / `memory.grow`, bulk-memory instructions, scalar memory
    ops, and SIMD memory ops, typed `select`, `br_table`, a substantially broader numeric
    operator subset across i32/i64/f32/f64 comparisons and arithmetic, the core conversion /
    reinterpretation families, integer sign-extension operators, saturating float-to-int
    truncation variants, and an expanded reference/const-expression subset including `ref.null`,
    `ref.func`, `ref.is_null`, imported immutable `global.get` in more const-expression
    positions, and declared-function-reference checking for `ref.func` in function bodies with
    declaration sources currently grounded in exports, global initializer `ref.func`, and
    element segments.
  - Major remaining gaps are now broader reference-type-driven validation paths beyond the
    current subset, additional proposal-era completeness, and external spec-suite
    integration/backfill rather than the main scalar numeric families.
- [~] Enrich module-level validation toward full spec-shaped coverage.
  - Type/import/function/code/global/memory/data/data-count/export/table/start/element sections are
    now parsed and validated in a useful Phase 1 base.
  - Remaining work is mostly semantic breadth and proposal-era completeness rather than missing the
    core module skeleton.
- [~] Run the growing validator against a disciplined external corpus rather than only crate-local
  tests.
  - Baedeker now has a filesystem-backed spec fixture harness with `valid`, `invalid-decode`, and
    `invalid-validate` buckets plus optional `.meta` files for exact error `kind`/`offset`
    assertions, plus `context=` and `decode_kind=` for cases where validation intentionally
    preserves an underlying body-decode failure as `ValidationErrorKind::Decode`.
  - Baedeker also now has a separate `wast` integration path with curated local files plus a broad
    `wast-upstream/` directory of small upstream-derived official-spec subsets.
  - Upstream-derived `.wast` coverage now uses sibling `.meta` files with `skip=...` to record
    intentionally deferred or currently mismatched cases explicitly, so the harness acts as both a
    runner and a lightweight support ledger for spec-suite friction points.
  - There are currently no explicitly deferred upstream-derived cases in the support matrix.

  - Active upstream-derived subsets now include:
    - `wast-upstream/labels-invalid-subset.wast`
    - `wast-upstream/labels-invalid-folded-syntax-subset.wast`
    - `wast-upstream/ref-null-subset.wast`
    - `wast-upstream/ref-as-non-null-subset.wast`
    - `wast-upstream/ref-select-subset.wast`
    - `wast-upstream/ref-control-subset.wast`
    - `wast-upstream/ref-func-undeclared-reference-subset.wast`
    - `wast-upstream/imports-unknown-type-subset.wast`
    - `wast-upstream/global-subset.wast`
    - `wast-upstream/global-ref-init-subset.wast`
    - `wast-upstream/data-subset.wast`
    - `wast-upstream/data-memory-subset.wast`
    - `wast-upstream/elem-subset.wast`
    - `wast-upstream/elem-table-init-subset.wast`
    - `wast-upstream/local-get-subset.wast`
    - `wast-upstream/local-set-subset.wast`
    - `wast-upstream/local-tee-subset.wast`
    - `wast-upstream/load-subset.wast`
    - `wast-upstream/store-subset.wast`
    - `wast-upstream/align-subset.wast`
    - `wast-upstream/integer-numeric-subset.wast`
    - `wast-upstream/float-numeric-subset.wast`
    - `wast-upstream/proposal-conversions-subset.wast`
    - `wast-upstream/simd-const-subset.wast`
    - `wast-upstream/simd-memory-subset.wast`
    - `wast-upstream/simd-lane-subset.wast`
    - `wast-upstream/simd-memory-multi-subset.wast`
    - `wast-upstream/memory-init-subset.wast`
    - `wast-upstream/data-drop-subset.wast`
    - `wast-upstream/memory-copy-subset.wast`
    - `wast-upstream/memory-fill-subset.wast`
    - `wast-upstream/call-subset.wast`
    - `wast-upstream/call-indirect-subset.wast`
    - `wast-upstream/call-ref-subset.wast`
    - `wast-upstream/typed-call-ref-subset.wast`
    - `wast-upstream/return-call-subset.wast`
    - `wast-upstream/return-call-indirect-subset.wast`
    - `wast-upstream/return-call-ref-subset.wast`
    - `wast-upstream/typed-return-call-ref-subset.wast`
    - `wast-upstream/br-on-null-subset.wast`
    - `wast-upstream/br-on-non-null-subset.wast`
    - `wast-upstream/return-subset.wast`
    - `wast-upstream/block-subset.wast`
    - `wast-upstream/if-subset.wast`
    - `wast-upstream/loop-subset.wast`
    - `wast-upstream/unreachable-subset.wast`
    - `wast-upstream/switch-subset.wast`
    - `wast-upstream/select-subset.wast`
    - `wast-upstream/br-if-subset.wast`
    - `wast-upstream/br-table-subset.wast`
    - `wast-upstream/table-get-subset.wast`
    - `wast-upstream/table-set-subset.wast`
    - `wast-upstream/table-grow-subset.wast`
    - `wast-upstream/table-size-subset.wast`
    - `wast-upstream/table-ref-flow-subset.wast`
    - `wast-upstream/table-grow-ref-subset.wast`
    - `wast-upstream/ref-func-table-call-subset.wast`
    - `wast-upstream/typed-table-ref-subset.wast`
    - `wast-upstream/typed-reference-types-subset.wast`
    - `wast-upstream/table-init-subset.wast`
    - `wast-upstream/table-copy-subset.wast`
    - `wast-upstream/table-fill-subset.wast`
    - `wast-upstream/utf8-custom-section-id-subset.wast`

  - Historical active upstream files have been normalized to `-subset.wast` names; no active
    upstream-derived coverage currently retains the old `-skip` suffix.

  - Remaining work is to continue integrating official `wast`/spec-suite cases while tightening the
    support/defer boundary and tracking unsupported areas explicitly.
  - Recent upstream grounding has strengthened scalar memory structure/alignment coverage across
    `memory-subset`, `load-subset`, `store-subset`, and `align-subset`.
  - Recent upstream grounding has also broadened bulk memory coverage via new
    `memory-init-subset`, `data-drop-subset`, `memory-copy-subset`, and `memory-fill-subset`
    files, including explicit nonzero memory selection, active/passive data-segment use, same-
    memory and cross-memory copies, and representative unknown-index / missing-data-count /
    operand-type invalid cases.
  - Recent upstream grounding has also broadened direct/indirect call argument-flow coverage via
    `call-subset` and an expanded `call-indirect-subset`.
  - Recent upstream grounding has also broadened global / const-expression coverage via expanded
    `global-subset`, `data-subset`, and `elem-subset` files, including imported-const-global
    offsets, reference-valued constant initializers, immutable-global writes, and representative
    invalid constant-expression forms.
  - Const-expression / initialization / extended-const hardening now also accepts defined immutable
    globals in later constant-expression contexts where the spec allows them and supports the
    current extended-const arithmetic subset used by the official tests (`i32.add/sub/mul`,
    `i64.add/sub/mul`). Grounding now includes additional valid raw fixtures for defined-global and
    arithmetic-based global initializers, data offsets, and element offsets, while upstream-derived
    `global-subset`, `data-subset`, and `elem-subset` now include representative defined-global and
    arithmetic constant-expression cases.
  - Recent upstream grounding has also broadened const-expression / initialization coverage via new
    `global-ref-init-subset`, `data-memory-subset`, and `elem-table-init-subset` files, including
    imported immutable ref globals in global initializers, explicit memory selection in active data
    segments, and nonzero-table element initialization with imported-global offsets and ref-valued
    expressions.
  - Recent upstream grounding has also broadened reference-type / table-const-expression coverage
    via expanded `table-subset` and `elem-subset` cases, including inline table element syntax,
    `ref.func` / `ref.null` table initializers in supported sugar forms, imported `externref` /
    `funcref` globals as constant element expressions, and corresponding mismatch / non-constant
    invalid cases.
  - Recent upstream grounding has also broadened direct table-instruction coverage via expanded
    `table-get-subset`, `table-set-subset`, and `table-grow-subset` files plus new
    `table-size-subset` coverage, including multi-table index selection, ref-typed operand/result
    flow, and representative wrong-arity / wrong-type / wrong-result invalid cases.
  - Recent upstream grounding has also broadened return / block / if result-flow coverage via
    expanded `return-subset`, `block-subset`, and `if-subset` files, including nested result
    propagation through structured control, `return` flowing through block/loop/if/call contexts,
    and representative empty-vs-valued / void-vs-valued / malformed inline-type invalid cases.
  - Recent upstream grounding has also broadened select / `br_if` / `br_table` expression-position
    coverage via expanded `select-subset`, `br-if-subset`, and `br-table-subset` files, including
    use inside loop/if/branch contexts and representative arity / operand-type / label-result
    mismatch invalid cases.
  - Recent upstream grounding has also broadened control-expression saturation via new
    `loop-subset`, `unreachable-subset`, and `switch-subset` files, including loop-valued
    expression positions, stack-polymorphic unreachable use in control/call/memory contexts, and
    additional `br_table`-driven structured-control nesting.
  - Recent upstream grounding has also broadened bulk table-op coverage via new
    `table-init-subset`, `table-copy-subset`, and `table-fill-subset` files, including nonzero
    table/element indices, `elem.drop`, same-table and cross-table copies, and representative
    unknown-index / operand-type / result-shape invalid cases.
  - Recent upstream grounding has also broadened `call_indirect` / table interaction coverage via
    an expanded `call-indirect-subset` with multi-table valid modules, explicit nonzero table
    selection, and representative no-table / wrong-result / wrong-argument invalid cases.
  - Recent upstream grounding has also broadened call / table / reference interaction coverage via
    new `table-ref-flow-subset`, `table-grow-ref-subset`, and `ref-func-table-call-subset` files,
    including `table.get -> table.set` round-trips, `table.grow` with `ref.null` / `ref.func`,
    `ref.func`-driven `call_indirect` through mutable/global/table flows, and representative
    ref-type mismatch invalid cases.
  - Recent upstream grounding has also broadened numeric / SIMD / proposal-surface completeness via
    new `integer-numeric-subset`, `float-numeric-subset`, `proposal-conversions-subset`,
    `simd-const-subset`, `simd-memory-subset`, `simd-lane-subset`, and
    `simd-memory-multi-subset` files, including integer unary/binary/compare/sign-extension
    operators, float compare/unary/binary families, saturating truncations, reinterpretation and
    conversion paths, `v128.const`, SIMD load/store/alignment/lane validation, and nonzero-memory
    SIMD lane syntax.
  - Recent upstream grounding has also broadened reference/control proposal-edge coverage via new
    `ref-null-subset`, `ref-as-non-null-subset`, `ref-select-subset`, and `ref-control-subset`
    files, including `ref.null` in function/global/control positions, `ref.as_non_null` over
    nullable and already-non-null references, typed `select` over `funcref`/`externref`,
    `ref.func` / `ref.null` joins, and reference-valued block/if result flow with representative
    ref-type mismatch invalid cases.
  - Tail-call mini-cluster support now also includes `return_call` and `return_call_indirect`
    decoding plus validation grounding via new `return-call-subset` and
    `return-call-indirect-subset` upstream files, new raw valid fixtures
    `valid/return-call-minimal.wasm` and `valid/return-call-indirect-funcref-table.wasm`, and new
    raw invalid fixtures for result-mismatch and non-`funcref`-table cases.
  - Function-reference call support now also includes `call_ref` and `return_call_ref` decoding
    plus the first typed-reference groundwork pass: `RefType` now models nullable vs non-null
    references and concrete function type indices, binary parsing accepts typed-reference encodings
    across types/tables/globals/locals/block results, and validation accepts concrete function refs
    as subtypes of abstract `funcref` where appropriate. Validation now also canonicalizes
    structurally equivalent concrete function types for the currently supported typed-function-
    reference surface, so equivalent signatures no longer mismatch solely because their raw
    `TypeIdx` values differ. Grounding now includes new `call-ref-subset`,
    `return-call-ref-subset`, `typed-call-ref-subset`, `typed-return-call-ref-subset`,
    `typed-table-ref-subset`, and `typed-reference-types-subset` upstream files, new raw valid
    fixtures `valid/call-ref-minimal.wasm`, `valid/return-call-ref-minimal.wasm`,
    `valid/typed-ref-global-init-from-ref-func.wasm`, `valid/typed-call-ref-null-concrete.wasm`,
    `valid/typed-table-set-get-concrete.wasm`, `valid/typed-call-ref-equivalent-signature.wasm`,
    `valid/typed-return-call-ref-equivalent-signature.wasm`,
    `valid/typed-table-set-get-equivalent-signature.wasm`, and
    `valid/typed-ref-global-init-equivalent-signature.wasm`, plus raw invalid fixtures for
    non-`funcref` references, concrete-type mismatches, and result-mismatch cases.
  - Typed-reference / const-init / table-element saturation now also covers typed `global.get`
    flows in constant initializers, typed element expressions sourced from typed globals and
    `ref.func`, and typed `table.init` against passive typed element segments. Validation now
    normalizes element-segment reference types during `table.init`, so structurally equivalent
    concrete function signatures no longer mismatch there solely because their raw `TypeIdx`
    values differ. Grounding expanded `typed-reference-types-subset`, `elem-subset`, and
    `table-init-subset`, and added raw fixtures for imported/defined typed-global initializers,
    typed element/global flows, passive typed element segments, and typed `table.init`
    equivalent-signature and wrong-concrete-type boundaries.
  - Typed-reference index-validity / malformed-boundary saturation now also validates concrete
    typed-reference `TypeIdx` operands across the bounded Phase 1 surface: function-type params /
    results, imported typed globals, defined tables, defined globals, element segment types,
    locals, `ref.null` immediates, and reference-valued block results. New raw invalid fixtures pin
    `UnknownTypeIdx` at these boundaries. Malformed-boundary coverage also now includes raw
    unknown-heaptype bytes in import/global/element/local decoding plus decode-preserving body
    cases for `ref.null` and block-result immediates. Because Node/V8 supports a broader GC-era
    heap-type space than Baedeker currently models, those raw unknown-ref-type byte fixtures are
    explicitly skipped in strict Node parity while remaining active Baedeker boundary assertions.
    Official grounding expanded via new `typed-invalid-typeidx-subset.wast`.
  - Typed-reference proposal-surface saturation now also broadens three adjacent supported areas:
    typed `select`, table/global import mixes, and additional block/control/result forms.
    Grounding expanded `ref-select-subset`, `ref-control-subset`, `typed-table-ref-subset`, and
    `typed-reference-types-subset` with representative equivalent-signature typed-reference cases:
    typed `select` over concrete function refs, imported typed-global -> defined/imported typed-
    table flows, `ref.func` -> imported mutable typed-global flows, and typed block/if result
    propagation. New raw valid fixtures cover those equivalent-signature cases, and new raw invalid
    fixtures pin representative wrong-concrete-type boundaries for typed `select`, imported typed-
    global -> imported typed-table flow, and typed `if` result propagation.
  - Typed-reference table/control saturation now also covers `br_if`, `br_table`, and typed
    loop-result forms. Grounding expanded `br-if-subset`, `br-table-subset`, and `loop-subset`
    with representative equivalent-signature typed-function-reference branches/results plus wrong-
    concrete-type invalids. New raw valid fixtures cover typed `br_if`, typed `br_table`, and
    typed loop-result equivalent-signature flows, and new raw invalid fixtures pin
    `BranchTypeMismatch` / `ControlResultTypeMismatch` boundaries for wrong-concrete-type branch
    operands and loop fallthrough results.
  - Typed branch-target consistency is now also grounded across broader multi-target `br_table`
    combinations. `br-table-subset` now includes equivalent-signature and wrong-concrete-type
    cases for nested block/block targets and mixed loop/block targets, and new raw fixtures pin
    `InconsistentBranchTypes` for mismatched typed multi-target label sets.
  - Typed control-signature interaction is now also grounded as a broader campaign across block /
    loop parameter-result composition and branch-to-loop targets. Grounding expanded
    `block-subset`, `loop-subset`, `br-subset`, and `br-if-subset` with representative
    equivalent-signature typed function-reference cases for block-result -> loop-param flow,
    loop-result -> block-result flow, `br` to param-bearing loops, and `br_if` to param-bearing
    loops. New raw valid fixtures cover those composed control-signature flows, and new raw
    invalid fixtures pin `TypeMismatch`, `BranchTypeMismatch`, and
    `ControlResultTypeMismatch` boundaries for wrong-concrete-type loop entry, branch targets, and
    enclosing block/result joins.
  - Typed branch/control closure under reachability is now also grounded across `unreachable`,
    `return`, dead code after `br`, and control joins with early `return` inside typed `if`.
    Grounding expanded `unreachable-subset`, `return-subset`, `ref-control-subset`, and
    `br-subset` with representative equivalent-signature typed-function-reference cases for
    unreachable block ends, direct typed returns, typed dead-code tails after `br`, and typed
    `if` joins where one arm exits via `return`. New raw valid fixtures cover those reachability
    closures, and new raw invalid fixtures pin `ControlResultTypeMismatch` boundaries for wrong-
    concrete-type dead-code tails, wrong-concrete-type returns, and mismatched typed joins after
    early exit.
  - Typed call/control convergence is now also grounded across control-produced typed-function-
    references flowing into `call_ref`, `return_call_ref`, `call_indirect`, and
    `return_call_indirect`. Grounding expanded `typed-call-ref-subset`,
    `typed-return-call-ref-subset`, `call-indirect-subset`, and
    `return-call-indirect-subset` with representative equivalent-signature typed cases where
    block/if results feed direct reference calls and typed indirect-call parameters. New raw valid
    fixtures cover block-result -> `call_ref`, if-result -> `return_call_ref`, block-result ->
    typed `call_indirect` parameter, and if-result -> typed `return_call_indirect` parameter.
    New raw invalid fixtures pin `TypeMismatch` boundaries for wrong-concrete-type call-site
    convergence rather than earlier control-frame failure.
  - Typed table/global/element flow closure is now also grounded across typed table reads feeding
    mutable typed globals and typed globals feeding passive element segments that later initialize
    typed tables. Grounding expanded `typed-reference-types-subset`, `typed-table-ref-subset`,
    `elem-subset`, and `table-init-subset` with representative equivalent-signature cases for
    defined/imported typed `table.get -> global.set` flows plus defined/imported typed
    `global.get -> passive elem -> table.init -> table.get` chains. New raw valid fixtures cover
    those composed storage/dataflow paths, and new raw invalid fixtures pin `TypeMismatch` on
    `global.set` plus `ElementExprTypeMismatch` on wrong-concrete-type passive element
    expressions.
  - Typed nullability-flow saturation is now also grounded across `br_on_null`,
    `br_on_non_null`, `ref.as_non_null`, and typed nullable joins. Grounding expanded
    `br-on-null-subset`, `br-on-non-null-subset`, `ref-as-non-null-subset`, and
    `ref-control-subset` with representative equivalent-signature typed-function-reference cases
    for `br_on_null` fallthrough feeding `call_ref`, `br_on_non_null` taken branches feeding typed
    block results and `call_ref`, `ref.as_non_null` feeding mutable typed globals, and typed `if`
    joins combining `ref.as_non_null` with `ref.null`. New raw valid fixtures cover those
    nullability-flow compositions, and new raw invalid fixtures pin `TypeMismatch` at
    `call_ref`, `br_on_non_null`, and `global.set` plus `ControlResultTypeMismatch` at typed null
    joins with wrong concrete function types.
  - Malformed/decode boundary completion for typed control/reference encodings is now also
    grounded across truncated `call_ref` / `return_call_ref` type immediates, truncated
    `br_on_null` / `br_on_non_null` label immediates, truncated `ref.null` heap types, and
    truncated typed block-result heap types. Grounding added a new upstream-derived
    `typed-malformed-control-subset` plus raw invalid fixtures pinning decode-preserving
    `ValidationErrorKind::Decode` with exact `CodeSection` / `UnexpectedEof` metadata at the
    instruction boundary offsets for these typed control/reference body-decode failures.
  - Official spec grounding is now also broadened across additional upstream validation-only and
    malformed-binary files that fit Baedeker’s current supported surface. Grounding added new
    upstream-derived `forward-subset`, `unreached-invalid-subset`, `func-ptrs-invalid-subset`,
    `binary-leb128-subset`, `utf8-import-module-subset`, and `utf8-import-field-subset` files.
    These expand coverage for forward mutual recursion, unreachable-code invalids, classic
    function-pointer/table/type invalids, non-minimal vs malformed LEB128 encodings, and malformed
    UTF-8 import names. Raw fixtures and unit tests now also pin valid `forward-mutual-recursion`
    and `unreached-call-ref` acceptance plus unreachable unknown-local/global/function/label
    failures with exact offsets.
  - Non-defaultable local initialization tracking is now implemented for function validation.
    Parameters and defaultable locals start initialized, non-defaultable locals must be initialized
    before `local.get`, and initialization established inside structured control does not escape
    the enclosing block/if/loop frame. Grounding added new upstream-derived
    `local-init-subset` plus raw valid fixtures for `local.set` / `local.tee` / block-internal
    flows and raw invalid fixtures pinning `UninitializedLocal` for direct use, post-block use,
    `else`-arm use, and post-`if` use of non-defaultable locals.
  - Bottom-type / stack-polymorphic unreachable closure is now materially broader across
    representative official `unreached-valid.wast` shapes. The validator now carries explicit
    bottom operands through unreachable validation so instructions like `select` and
    `ref.as_non_null` can consume stack-polymorphic inputs without spuriously underflowing, while
    frame-end checks still reject concrete stray or mismatched values that survive in dead code.
    Grounding added new upstream-derived `unreached-valid-subset` plus raw valid fixtures for
    select-heavy unreachable flows, bottom-heap `ref.as_non_null` / `br_on_null` cases, and a
    meet-bottom `br_table` join, along with raw invalid fixtures pinning function-end mismatches
    for concrete unreachable select/result leakage.
  - Official grounding expansion has resumed now that the concrete post-audit semantic blockers are
    closed. Curated official additions broadened active coverage in `local-init-subset`,
    `unreached-valid-subset`, `ref-as-non-null-subset`, `select-subset`, and `br-table-subset`,
    including extra `local_init.wast` tee-init grounding, additional unreachable-valid select
    shapes, an official unreachable `ref.as_non_null` case, richer select placement coverage, and
    additional numeric `br_table` value forms. Representative raw valid fixtures and unit tests now
    also pin those official shapes directly.
  - To avoid drifting back into tiny one-off expansions, the next official zero-skip batch widened a
    denser `select.wast` control-consumer slice in one pass. `select-subset` now covers broader
    official placement shapes including loop-first/last, if-condition, call-indirect operand
    positions, store operands, memory.grow, call/return/branch/local/global consumers, load,
    unary/binary/test/compare, and conversion contexts. Representative raw valid fixtures and unit
    tests now pin `select-as-call-indirect-last`, `select-as-memory-grow-value`,
    `select-as-global-set-value`, `select-as-convert-operand`, and `select-as-if-condition`.
  - The next official sweep kept that denser cadence by broadening `br-table-subset` across a much
    larger official branch-consumer slice rather than a single placement. Active coverage now spans
    block/loop placements, branch consumers (`br`, `br_if`, nested `br_table`), if/select/call /
    call_indirect positions, local/global consumers, memory address/value consumers, arithmetic /
    compare / conversion consumers, and `memory.grow`. Representative raw valid fixtures and unit
    tests now pin `br-table-as-br-if-value-cond`, `br-table-as-call-indirect-func`,
    `br-table-as-local-set-value`, `br-table-as-load-address`, `br-table-as-store-value`,
    `br-table-as-compare-left`, and `br-table-as-memory-grow-size`.
  - A comparable branch-adjacent sweep then broadened `br-if-subset` rather than starting another
    tiny file. Active official coverage now spans `br_if` result typing, block/loop placements,
    nested branch consumers, if/select/call/call_indirect consumers, local/global consumers,
    memory address/value consumers, arithmetic/compare consumers, and `memory.grow`. Representative
    raw valid fixtures and unit tests now pin `br-if-as-br-if-value-cond`, `br-if-as-select-cond`,
    `br-if-as-call-indirect-last`, `br-if-as-local-tee-value`, `br-if-as-load-address`,
    `br-if-as-storeN-value`, and `br-if-as-memory-grow-size`.
  - The next comparable densification broadened `br-subset` across a much larger official direct-
    branch consumer slice. Active official coverage now spans result typing, block/loop placements,
    nested branch consumers (`br`, `br_if`, `br_table`), if/select/call/call_indirect consumers,
    local/global consumers, memory address/value consumers, arithmetic/compare/conversion
    consumers, and `memory.grow`, while retaining the existing typed-reference branch grounding in
    the same file. Representative raw valid fixtures and unit tests now pin `br-as-br-if-value-cond`,
    `br-as-select-all`, `br-as-call-indirect-all`, `br-as-local-tee-value`, `br-as-load-address`,
    `br-as-storeN-value`, and `br-as-memory-grow-size`.
  - A follow-on control-consumer closure sweep then widened the remaining smaller structured-control
    subsets rather than opening another narrow lane. `if-subset`, `block-subset`, `loop-subset`,
    and `return-subset` now cover additional consumer placements spanning `call_indirect`,
    `memory.grow`, `select`, loads, `local.tee`, direct calls, and nested branch values. This
    closes more of the official expression-position surface around structured control without new
    validator algorithms. Representative raw valid fixtures and unit tests now pin
    `if-as-call-indirect-last`, `if-as-memory-grow-size`, `block-as-select-cond`,
    `block-as-load-address`, `loop-as-local-tee-value`, `loop-as-memory-grow-size`,
    `return-as-call-value`, and `return-as-br-value`.
  - The next broader official audit pass selected `call-subset` as the best dense zero-skip target,
    and the follow-on batch widened it aggressively inside a single existing file. Active official
    direct-call grounding now spans richer type/result cases plus a broad producer/consumer slice
    across `select`, `if`, `br_if`, `br_table`, `call_indirect`, stores, `memory.grow`, `return`,
    `drop`, `br`, local/global writes, loads, unary/binary/test/compare operators, and conversion
    contexts, alongside additional representative invalid arity/type cases. Representative raw
    valid fixtures and unit tests now pin `call-as-call-all-operands`, `call-as-br-table-last`,
    `call-as-call-indirect-last`, `call-as-memory-grow-value`, `call-as-local-tee-value`,
    `call-as-load-operand`, `call-as-compare-right`, and `call-as-convert-operand`.
  - A comparable dense follow-on then broadened `call-indirect-subset` across its own official
    producer/consumer surface rather than leaving indirect-call coverage comparatively sparse.
    Active official indirect-call grounding now spans richer type/result cases plus placement
    coverage across `select`, `if`, `br_if`, `br_table`, stores, `memory.grow`, `return`, `drop`,
    `br`, local/global writes, loads, unary/binary/test/compare operators, and conversion
    contexts, while preserving the existing multi-table / explicit-table-index and typed-reference
    call-indirect grounding already present in the file. The invalid slice also broadened with more
    representative official arity/type mismatches. Representative raw valid fixtures and unit tests
    now pin `call-indirect-as-select-last`, `call-indirect-as-br-if-first`,
    `call-indirect-as-store-last`, `call-indirect-as-memory-grow-value`,
    `call-indirect-as-local-tee-value`, `call-indirect-as-load-operand`,
    `call-indirect-as-compare-right`, and `call-indirect-as-convert-operand`.
  - `ref.as_non_null` support now also includes decoding plus validation grounding via new
    `ref-as-non-null-subset` upstream coverage, raw valid
    `valid/ref-as-non-null-call-ref.wasm`, and raw invalid
    `invalid-validate/ref-as-non-null-non-ref-input.wasm`. Within the current bounded model,
    `ref.as_non_null` accepts reference operands and produces the non-null form of that reference
    type.
  - Null-branch support now also includes `br_on_null` and `br_on_non_null` decoding plus
    validation grounding via new `br-on-null-subset` and `br-on-non-null-subset` upstream files.
    Raw fixtures now include valid `valid/br-on-null-fallthrough-narrow.wasm` and
    `valid/br-on-non-null-branch-result.wasm`, plus invalid
    `invalid-validate/br-on-null-non-ref-input.wasm` and
    `invalid-validate/br-on-non-null-non-ref-target.wasm`. `br_on_null` now narrows the
    fallthrough reference to non-null, while `br_on_non_null` requires the target label to end in a
    reference type and routes the tested value through that branch target.
  - Raw invalid body-fixture metadata is now tighter for decode-preserving validation failures:
    the current wrapped `ValidationErrorKind::Decode` cases for truncated bulk-memory, truncated
    memarg, and unknown SIMD opcode bodies now pin exact `offset=` alongside `context=` and
    `decode_kind=`.
  - Raw bulk-memory invalid fixtures now also pin exact `offset=` for representative
    `memory.init`, `memory.copy`, `memory.fill`, and `data.drop` validation failures.
  - Raw initialization/const-expression invalid fixtures now also pin exact `offset=` for
    representative global-init, active-data, element-expression, active-element-table, and
    `table.init` validation failures.
  - Raw control invalid fixtures now also pin exact `offset=` for representative
    `call_indirect` table-type validation failures.
  - Raw table/reference invalid fixtures now also pin exact `offset=` for representative unknown
    exported-table index failures.
  - Raw malformed/decode fixtures now pin both exact `offset=` and direct decode `context=` across
    the full `invalid-decode/` corpus, and `spec.rs` now asserts that all raw invalid fixtures
    carry complete metadata (`kind`/`offset`, plus `context` for direct decode failures and nested
    `decode_kind` for decode-preserving validation failures).

  - Current decode-vs-validate boundary in the raw fixture harness is:
    - `invalid-decode`: `Module::decode(...)` itself must fail.
    - `invalid-validate`: module decoding succeeds, then validation fails.
    - malformed function-body instruction streams that are only decoded during validation belong in
      `invalid-validate` and should assert `kind=Decode` plus nested `context=` / `decode_kind=`
      metadata where useful.

#### Revised Phase 1 completion definition
Phase 1 is complete only when Baedeker provides a robust, diagnostics-oriented validation front-end
for the full intended WebAssembly 3.0 surface, sufficient to serve as the semantic foundation for
later register-based lowering and execution.

More concretely, Phase 1 is done when all of the following are true:
1. **Validation coverage**
   - The decoder and validator cover the intended WebAssembly 3.0 instruction and module surface,
     or any explicitly excluded areas are documented as out of scope for the current release.
2. **Semantic reliability**
   - Control-flow typing, operand stack typing, label/result propagation, const-expression rules,
     and index-space resolution are stable enough that later lowering does not need to rediscover
     spec semantics on its own.
3. **Diagnostics quality**
   - Validation and malformed-input failures preserve precise byte offsets where feasible and report
     stable structured error kinds suitable for regression testing.
4. **Spec-test grounding**
   - Baedeker is exercised against both its internal fixture corpus and official spec-suite style
     invalid/malformed validation inputs, with compliance status tracked explicitly.
5. **Architectural clarity**
   - The validator remains a spec-facing proof/type layer, clearly separated from the future
     register-based IR and interpreter core.

#### Checkpoint 8 — Phase 1 closure audit

**Audit verdict:** Phase 1 is materially advanced and well-grounded, but **not yet complete** under
Baedeker’s own revised definition.

**What the audit says is already true**
- Validation diagnostics are now precise and regression-pinned across a broad raw corpus with exact
  `offset=` metadata and decode-vs-validate separation.
- The active upstream-derived lane remains **zero-skip** while covering **87** curated
  `wast-upstream` files and **492** upstream directives.
- Compile-time external parity is in the regular loop via Node/V8 for the active supported raw and
  upstream-derived surface.
- Typed function references, tail calls, null branches, `ref.as_non_null`, const/init flows, and a
  large control/table/global/reference matrix are grounded well enough that remaining work is now
  mostly closure-oriented rather than foundational.

**What the audit found is still missing**
- [ ] **Official validation-only grounding can still broaden now that the concrete post-audit
  semantic blockers are closed.** The current lane is strong, but additional official files and
  curated slices of `unreached-valid.wast` should continue to become active as Baedeker widens its
  supported validation surface.
- [ ] **Some broader WebAssembly 3.0 validation surface still remains beyond the current bounded
  typed-reference model.** The current model is intentionally sufficient for the active supported
  subsets, but Phase 1 closure still requires continued audit against remaining proposal-era and
  niche validation cases.

### Recommended next steps
1. Continue the **official spec grounding lane**, widening the active zero-skip upstream surface
   with additional curated official validation cases rather than reintroducing skips.
2. Continue the **broader WebAssembly 3.0 audit pass** against remaining proposal-era and niche
   validation cases beyond the current bounded typed-reference model.
3. Keep the architectural boundary explicit: validation state remains proof/type state; register IR
   design and lowering stay in Phase 2.

### Current verification snapshot
- `cargo fmt -- --check`
- `cargo test -p baedeker-core --test spec`
- `cargo test -p baedeker-core --test spec_wast`
- `cargo test -p baedeker-core --test spec_node`
- `cargo test -p baedeker-core`
- `cargo clippy -p baedeker-core --all-targets -- -D warnings`
- Current `baedeker-core` unit test count: **396 passing**
- Current spec-harness integration tests: **8 passing** (`spec`: 3, `spec_wast`: 2, `spec_node`: 3)
- Current corpus snapshot:
  - `wast-upstream`: **87** active files / **492** directives
  - raw `valid`: **163** fixtures
  - raw `invalid-validate`: **116** fixtures
  - raw `invalid-decode`: **34** fixtures
  - custom `spec/wast`: **17** files
- `spec_node` adds a Node/V8 compile-time cross-check over the raw fixture corpus plus the active
  upstream-derived `wast-upstream` subset lane. Custom `spec/wast` cases remain Baedeker-shaped
  boundary coverage and are not enforced against Node/V8.

### Current branch snapshot
Current Phase 1 closure work continues on `feat/phase-1-part-3-larger-test-batches`, with the
validator and fixture/docs support matrix kept in sync as upstream-derived skips are narrowed or
retired.

### Phase 1 note
The validator stack remains the spec-facing abstract operand/control stack used for proof of
well-typedness. This is intentionally distinct from the future execution architecture, which still
flows through decode → validate → lower to register IR → execute.

Because Baedeker's long-term goal is full WebAssembly 3.0 validation, Phase 1 should not be read
as a short-lived subset-only milestone. It is the semantic front-end of the runtime: the layer that
must eventually make the full intended WASM surface precise, diagnosable, and trustworthy before
execution and lowering broaden on top of it.

### Spec sections to internalize
- [Types](https://webassembly.github.io/spec/core/syntax/types.html)
- [Validation](https://webassembly.github.io/spec/core/valid/index.html) — especially instruction validation.
- [Appendix: Validation Algorithm](https://webassembly.github.io/spec/core/appendix/algorithm.html)

---

## Phase 2 — The Interpreter Core

**Focus:** Execution semantics, stack frames, the operational heart of the runtime.

- Design the internal IR. Two options, with a strong recommendation:
  - **Register-based IR** (recommended): Transform WASM's stack machine into a register-based
    representation during a lowering pass after validation. This is the wasm3 / wasm-micro-runtime
    approach and yields 30–50% speedup over naive stack interpretation. More complex to implement
    but far more instructive and performant.
  - Stack-based direct interpretation: Simpler, useful as a reference implementation for
    differential testing against the register-based path.
- Implement the core execution loop: instruction dispatch, operand handling, control flow
  (block entry/exit, branch, return).
- Implement all numeric instructions: i32/i64/f32/f64 arithmetic, comparisons, conversions,
  reinterpretations. This is ~120 opcodes of mostly mechanical work, but the IEEE 754 edge
  cases (NaN propagation, rounding modes, min/max semantics) will test your patience and
  your understanding of the spec's determinism requirements.
- Implement v128 (SIMD) instructions — these matter on iOS/ARM where NEON is available and
  your geometric algebra workloads will benefit directly.
- Implement call/return, including indirect calls through tables.
- **Milestone:** Can execute `(module (func (export "add") (param i32 i32) (result i32) (local.get 0) (local.get 1) (i32.add)))` and return the correct result through the embedding API. Then: pass the full spec test suite for numeric and control flow instructions.

### Spec sections to internalize
- [Execution](https://webassembly.github.io/spec/core/exec/index.html) — the reduction rules.
- [Numerics](https://webassembly.github.io/spec/core/exec/numerics.html) — every edge case.
- [Instructions](https://webassembly.github.io/spec/core/syntax/instructions.html)

### Where your toolkit applies
This is where Orlando's transducer philosophy becomes relevant. The stack-to-register lowering
pass is a transformation of transformations: you're rewriting a sequence of stack effects into
a sequence of register transfers. If you can express this as a composable transducer pipeline,
you get a clean architecture for layering optimization passes later.

---

## Phase 3 — Memory, Tables, Globals

**Focus:** The mutable state model — linear memory, tables, global variables.

- Implement linear memory: allocation, bounds checking, grow semantics. Pay close attention
  to the 32-bit address space and page granularity (64KiB). Memory access must be correct
  for unaligned loads/stores and must trap on out-of-bounds — no UB.
- Implement load/store instructions for all width and signedness combinations (i32.load8_s,
  i64.load32_u, etc.), including the alignment hints and their actual semantics (they're
  hints, not requirements, but misalignment has performance implications on ARM).
- Implement tables (funcref and externref), `table.get`, `table.set`, `table.grow`, `table.fill`,
  `table.copy`, `table.init`, `elem.drop`.
- Implement globals (mutable and immutable, imported and defined).
- Implement data segments and element segments, including passive segments and the
  `memory.init` / `data.drop` bulk memory operations.
- **Milestone:** Can run modules that allocate memory, perform pointer arithmetic, and use
  indirect function calls through tables. This is where "real" programs start working.

### Spec sections to internalize
- [Memory Instances](https://webassembly.github.io/spec/core/exec/runtime.html#memory-instances)
- [Table Instances](https://webassembly.github.io/spec/core/exec/runtime.html#table-instances)
- Bulk memory operations proposal (now merged into 2.0)

---

## Phase 4 — Module Instantiation & Linking

**Focus:** The full module lifecycle — imports, exports, instantiation, multi-module linking.

- Implement the instantiation algorithm: resolve imports, allocate memories/tables/globals,
  evaluate global initializer expressions, run the start function.
- Build the host function interface: the Rust API for registering callable functions that
  WASM modules can import. This is the primary embedding API and its ergonomics matter
  enormously — it's what you'll use to bridge Baedeker into Swift and to expose system
  capabilities (Metal compute, file I/O, etc.).
- Implement multi-module linking: one module importing another module's exports.
- Implement the `call_indirect` + table machinery that makes dynamic dispatch and
  function pointers work.
- **Milestone:** Can instantiate a module that imports `env.print_i32` from the host,
  calls it, and produces output. Can link two modules together. This is where Baedeker
  becomes a usable embedding runtime.

### Design consideration
The host function API is where you decide Baedeker's personality as an embeddable runtime.
Consider a trait-based approach where host functions are statically typed against WASM
signatures, avoiding the runtime type-checking overhead that plagues some runtimes.

---

## Phase 5 — iOS Integration Layer

**Focus:** Making Baedeker a first-class iOS citizen.

- Build `baedeker-ios`: a crate that compiles to a static library with C-compatible FFI.
- Produce a Swift package that wraps the FFI in idiomatic Swift (async/await for long-running
  WASM computations, value types for WASM values, closures for host functions).
- Implement an AOT pipeline: compile WASM to Baedeker's internal IR at build time (on macOS),
  serialize the IR, bundle it into the iOS app, deserialize and execute on device.
  This sidesteps the JIT prohibition entirely.
- Implement a Metal compute host module: a standard set of importable functions that let
  WASM modules dispatch GPU compute kernels, pass buffers, and read results. This is
  purpose-built for running Amari/Cliffy workloads on iPad GPU hardware.
- **Milestone:** A Swift iOS app that loads a WASM module compiled from Amari, performs a
  geometric algebra computation, and displays the result. The demo that proves the thesis.

---

## Phase 6 — Post-MVP Proposals

**Focus:** The evolving spec — GC, threads, tail calls, exception handling, component model.

These are ordered by relevance to your use cases:

1. **Tail calls** — relatively simple, high value for functional patterns in Cliffy.
2. **Exception handling** — needed for robust interop with code compiled from languages
   that use exceptions.
3. **Threads and atomics** — shared memory, `memory.atomic.*` instructions, `wait`/`notify`.
   Critical for parallel geometric algebra on multi-core iPad chips. Careful: this interacts
   with the memory model in subtle ways.
4. **GC proposal** — struct and array types managed by the runtime's GC. This is a massive
   addition that fundamentally changes what WASM can express efficiently. It's also where the
   "interaction between linear memory and GC'd references" lives — the hardest conceptual
   territory in modern WASM.
5. **Component Model** — the higher-level module linking and interface type system. This is
   where Flynn contracts could map onto WASM's own interface validation. A component that
   declares "this function takes a blade of grade 2" using component model interface types,
   validated at link time, is the synthesis of Baedeker and Amari's contract system.

---

## Ongoing: Spec Test Suite Compliance

The official [WebAssembly spec test suite](https://github.com/AnisBoss/WebAssembly-spec-testsuite)
is the ground truth. Every phase should be accompanied by running the relevant subset of spec
tests. Track compliance percentage as a first-class project metric. The goal is 100% on
WASM 2.0 core before moving to proposals.

## Ongoing: Differential Testing

Once the interpreter is functional, set up differential testing against Wasmtime or Wasmer:
feed the same modules to both runtimes and compare outputs. This catches spec misunderstandings
that the official test suite might not cover.

## Ongoing: Robustness Testing

As parser and validator coverage grows, add fuzzing and malformed-input testing specifically to
harden decoding, validation, and error reporting against truncated or invalid WebAssembly modules.
This work is strictly for standards compliance, runtime robustness, and implementation quality.

## Ongoing: Creusot Contracts

As each layer stabilizes, add Creusot contracts to the core invariants:
- LEB128 decode/encode roundtrip correctness
- Validation algorithm soundness (well-typed programs don't get stuck)
- Memory bounds checking completeness
- Numeric operation IEEE 754 conformance

This is a long-term investment that compounds: a verified WASM runtime core is
a unique artifact in the ecosystem.
