# Phase 1 Validation and Future Register Lowering

This note records the intended relationship between Baedeker's Phase 1 validation work and the
future Phase 2 register-based execution architecture.

## Core distinction

Baedeker still targets a **register-based internal IR** for execution.

However, WebAssembly validation is defined by the spec in terms of an **abstract operand stack**
and **structured control stack**. That means Phase 1 must model stack-machine semantics even if
Baedeker never executes stack-machine code directly.

These are different concerns:

- **Validation stack** — abstract type stack used to prove a module is well-typed.
- **Control stack** — abstract structured-control model for `block`, `loop`, `if`, labels, and
  stack discipline.
- **Execution IR** — future register-based lowered form used for efficient interpretation or AOT.

## Intended pipeline

The architecture remains:

```text
WASM bytes
  -> DecodedModule
  -> ValidatedModule
  -> RegModule
  -> Execution
```

More concretely:

1. **Decode**
   - Parse sections, types, imports, functions, code bodies, and instruction sequences.
2. **Validate**
   - Check stack-machine typing rules from the WASM spec.
   - Resolve block signatures, local indices, function indices, and label structure.
3. **Lower**
   - Transform validated stack instructions into register-based IR.
4. **Execute**
   - Run register IR, not the original stack machine.

## Why validation still uses a stack

The validator's operand stack is not a runtime value stack. It is a **proof artifact**.

Examples:

- `local.get 0` pushes the local's `ValType`
- `i32.add` pops two `i32` operands and pushes one `i32`
- `br` and `return` trigger stack-polymorphic unreachable regions

This is required by the WebAssembly validation algorithm regardless of the eventual runtime model.

## How validation feeds lowering

The future lowering pass will reuse the same high-level structure as validation, but enrich it with
value identities.

### Validation view

```text
operand stack: [i32, i32]
control frames: block/loop/if typing information
```

### Lowering view

```text
operand stack: [r3:i32, r4:i32]
control frames: block/loop/if typing + register-flow information
```

In other words:

- **validation tracks types**
- **lowering tracks typed value identities**

That makes the Phase 1 validator the semantic front-end for the future register allocator/lowerer.

## Example: simple arithmetic

WASM source semantics:

```text
local.get 0
local.get 1
i32.add
```

Validation tracks:

```text
[] -> [i32] -> [i32, i32] -> [i32]
```

Lowering will later track:

```text
[] -> [r0:i32] -> [r0:i32, r1:i32] -> [r2:i32]
```

and emit something like:

```text
r2 = i32.add r0, r1
```

## Example: structured control flow

Validation answers questions like:

- what result types does this block produce?
- what values must a branch provide to its label?
- when is the operand stack polymorphic because control flow is unreachable?

Lowering will use those same answers to decide:

- what registers cross block boundaries?
- what registers become block results?
- what values a branch transfers to a target frame?

This is why the control-frame machinery built in Phase 1 is directly useful for Phase 2.

## Working principle for implementation

The intended layering is:

- `Instr` = decoded, spec-shaped stack-machine instruction
- `ValidationState` = spec-shaped abstract typing state
- `RegInstr` / `RegBlock` / `RegFunc` = future execution IR

Baedeker should avoid conflating these layers.

## Practical implication for current work

Phase 1 should continue to improve:

- operand stack typing
- control-frame semantics
- block signature resolution
- label typing
- stack polymorphism after unreachable control flow
- `if`/`else` merge behavior

These improvements are not architectural drift toward a stack interpreter.
They are the semantic groundwork required before a correct stack-to-register lowering pass can
exist.

## Spec-suite grounding and deferred cases

Baedeker's Phase 1 validation work is now exercised through two `.wast` lanes:

- curated local validation-focused files under `crates/baedeker-testdata/spec/wast/`
- upstream-derived official-spec subsets under `crates/baedeker-testdata/spec/wast-upstream/`

The upstream-derived lane is intentionally conservative: only cases that currently map cleanly onto
Baedeker's decode/validate boundary are run directly.

When an upstream case is important but does not yet fit cleanly — for example because:

- `wast` canonicalization changes the failure path before Baedeker sees it,
- the current validator does not yet implement the relevant spec rule,
- or the harness boundary does not yet preserve the intended malformed/invalid distinction,

Baedeker now records that explicitly with a sibling `.meta` file using `skip=...`.

This means the spec harness is not just a pass/fail runner. It is also a lightweight ledger of:

- what upstream-derived validation space is already exercised,
- what cases are intentionally deferred,
- and why those deferred cases are not yet expected to pass.

### Decode-vs-validate boundary in the current harness

For the raw `.wasm` fixture lane under `crates/baedeker-testdata/spec/`:

- `invalid-decode/` means `Module::decode(...)` itself must reject the bytes.
- `invalid-validate/` means the module decodes structurally, then validation rejects it.
- Some malformed function-body instruction streams intentionally live in `invalid-validate/`
  because Baedeker keeps code bodies as raw bytes and only decodes instructions during
  validation. Those cases surface as `ValidationErrorKind::Decode { .. }` rather than top-level
  `DecodeError`.

Sibling raw-fixture `.meta` files can therefore pin not just `kind=` and `offset=`, but also:

- current wrapped body-decode fixtures now use exact `offset=` together with `context=` and
  `decode_kind=` for truncated bulk-memory, truncated memarg, unknown SIMD opcode, and unknown
  opcode cases
- representative raw bulk-memory invalid fixtures now also pin exact `offset=` for `memory.init`,
  `memory.copy`, `memory.fill`, and `data.drop` validation failures
- representative raw initialization/const-expression invalid fixtures now also pin exact `offset=`
  for global-init, active-data, element-expression, active-element-table, and `table.init`
  validation failures
- representative raw control invalid fixtures now also pin exact `offset=` for `call_indirect`
  table-type validation failures
- representative raw table/reference invalid fixtures now also pin exact `offset=` for unknown
  exported-table index failures
- the full raw `invalid-decode/` corpus now pins both exact `offset=` and direct decode
  `context=`, and `crates/baedeker-core/tests/spec.rs` now asserts that all raw invalid fixtures
  carry complete metadata (`kind`/`offset`, plus `context` for direct decode failures and nested
  `decode_kind` for decode-preserving validation failures)

- `context=` — the underlying `DecodeContext`
- `decode_kind=` — the underlying `DecodeErrorKind` when validation preserves a decode failure

That boundary is intentional: module/section structure is decoded first, while per-body instruction
stream decoding remains part of the semantic validation front-end.

### Current deferred upstream-derived support matrix

There are currently no explicitly deferred upstream-derived `.wast` cases in the active support
matrix.

Named-label invalid coverage now includes both:

- `crates/baedeker-testdata/spec/wast-upstream/labels-invalid-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/labels-invalid-folded-syntax-subset.wast`

Recent upstream-derived grounding also now covers foundational global/const-expression,
reference-type / table-const-expression, direct table instructions, bulk table ops,
return/block/if result-flow, select/br_if/br_table expression positions, local-variable,
scalar memory, and call argument-flow validation via:

- `crates/baedeker-testdata/spec/wast-upstream/global-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/global-ref-init-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/data-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/data-memory-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/elem-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/elem-table-init-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/ref-null-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/ref-as-non-null-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/ref-select-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/ref-control-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/return-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/block-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/if-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/loop-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/unreachable-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/switch-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/select-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/br-if-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/br-table-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-get-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-set-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-grow-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-size-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-ref-flow-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-grow-ref-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/ref-func-table-call-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/typed-table-ref-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/typed-reference-types-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-init-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-copy-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/table-fill-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/local-get-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/local-set-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/local-tee-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/load-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/store-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/align-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/integer-numeric-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/float-numeric-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/proposal-conversions-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/simd-const-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/simd-memory-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/simd-lane-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/simd-memory-multi-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/memory-init-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/data-drop-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/memory-copy-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/memory-fill-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/call-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/call-indirect-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/call-ref-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/typed-call-ref-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/return-call-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/return-call-indirect-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/return-call-ref-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/typed-return-call-ref-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/br-on-null-subset.wast`
- `crates/baedeker-testdata/spec/wast-upstream/br-on-non-null-subset.wast`

The `call-indirect-subset` lane now also includes broader multi-table / explicit-table-index
interaction coverage rather than only a narrow single-table argument-flow slice.

Bulk-memory grounding now also includes dedicated `memory-init-subset`, `data-drop-subset`,
`memory-copy-subset`, and `memory-fill-subset` files, covering explicit nonzero memory indices,
active/passive data-segment interaction, same-memory and cross-memory copy shapes, and
representative unknown-index / operand-type invalids.

Const-expression / initialization grounding now also includes `global-ref-init-subset`,
`data-memory-subset`, and `elem-table-init-subset`, covering imported immutable ref globals in
new global-initializer positions, explicit-memory active data syntax, and nonzero-table element
initialization with imported-global offsets and ref-valued expressions.

Const-expression / initialization / extended-const hardening now also accepts defined immutable
globals in later constant-expression contexts where the spec allows them and supports the current
extended-const arithmetic subset exercised by the official tests (`i32.add/sub/mul`,
`i64.add/sub/mul`). Grounding now includes additional raw valid fixtures for defined-global and
arithmetic-based global initializers, data offsets, and element offsets, and expanded
`global-subset`, `data-subset`, and `elem-subset` coverage for those cases.

Control-expression grounding now also includes `loop-subset`, `unreachable-subset`, and
`switch-subset`, covering loop-valued expression positions, stack-polymorphic unreachable use in
control/call/memory contexts, and additional `br_table`-driven structured-control nesting.

Call / table / reference interaction grounding now also includes `table-ref-flow-subset`,
`table-grow-ref-subset`, and `ref-func-table-call-subset`, covering `table.get -> table.set`
round-trips, `table.grow` with `ref.null` / `ref.func`, and `ref.func`-driven `call_indirect`
through mutable/global/table flows.

Numeric / SIMD / proposal-surface grounding now also includes `integer-numeric-subset`,
`float-numeric-subset`, `proposal-conversions-subset`, `simd-const-subset`,
`simd-memory-subset`, `simd-lane-subset`, and `simd-memory-multi-subset`, covering integer
unary/binary/compare/sign-extension operators, float compare/unary/binary families, saturating
truncation plus reinterpretation/conversion paths, `v128.const`, SIMD load/store/alignment/lane
validation, and nonzero-memory SIMD lane syntax.

Reference/control proposal-edge grounding now also includes `ref-null-subset`,
`ref-as-non-null-subset`, `ref-select-subset`, and `ref-control-subset`, covering `ref.null` in
function/global/control positions, `ref.as_non_null` over nullable and already-non-null
references, typed `select` over `funcref`/`externref`, `ref.func` / `ref.null` joins, and
reference-valued block/if result flow.

Tail-call mini-cluster support now also includes `return_call` and `return_call_indirect`
decoding plus validation grounding via `return-call-subset` and
`return-call-indirect-subset`, along with raw valid fixtures for minimal direct/indirect tail-call
shapes and raw invalid fixtures for result-mismatch and non-`funcref`-table cases.

Function-reference call support now also includes `call_ref` and `return_call_ref` decoding plus
an initial typed-reference groundwork pass: `RefType` now models nullable vs non-null references
and concrete function type indices, binary parsing accepts typed-reference encodings across
function types / globals / tables / locals / block results, and validation accepts concrete
function refs as subtypes of abstract `funcref` where appropriate. Validation now also
canonicalizes structurally equivalent concrete function types for the currently supported typed-
function-reference surface, so equivalent signatures no longer mismatch solely because their raw
`TypeIdx` values differ. Grounding now includes `call-ref-subset`, `return-call-ref-subset`,
`typed-call-ref-subset`, `typed-return-call-ref-subset`, `typed-table-ref-subset`, and
`typed-reference-types-subset`, along with raw valid fixtures for typed global init / typed
`call_ref` / typed table flows, equivalent-signature function-reference flows, and raw invalid
fixtures for non-`funcref` references, concrete-type mismatches, and result-mismatch cases.

Typed-reference / const-init / table-element saturation now also covers typed `global.get` flows
in constant initializers, typed element expressions sourced from typed globals and `ref.func`, and
typed `table.init` against passive typed element segments. Validation now normalizes
element-segment reference types during `table.init`, so structurally equivalent concrete function
signatures no longer mismatch there solely because their raw `TypeIdx` values differ. Grounding
expanded `typed-reference-types-subset`, `elem-subset`, and `table-init-subset`, and added raw
fixtures for imported/defined typed-global initializers, typed element/global flows, passive typed
element segments, and typed `table.init` equivalent-signature and wrong-concrete-type boundaries.

Typed-reference index-validity / malformed-boundary saturation now also validates concrete typed-
reference `TypeIdx` operands across the bounded Phase 1 surface: function-type params/results,
imported typed globals, defined tables, defined globals, element segment types, locals,
`ref.null` immediates, and reference-valued block results. New raw invalid fixtures now pin
`UnknownTypeIdx` for those contexts, and official grounding expanded with
`typed-invalid-typeidx-subset.wast`. Malformed-boundary coverage also now includes raw
unknown-heaptype bytes in import/global/element/local decoding plus decode-preserving body cases
for `ref.null` and block-result immediates. Since Node/V8 supports a broader GC-era heap-type
space than Baedeker currently models, those raw unknown-ref-type byte fixtures are skipped in
strict Node parity while remaining active Baedeker boundary assertions.

Typed-reference proposal-surface saturation now also broadens three adjacent supported areas:
typed `select`, table/global import mixes, and additional block/control/result forms. Grounding
expanded `ref-select-subset`, `ref-control-subset`, `typed-table-ref-subset`, and
`typed-reference-types-subset` with representative equivalent-signature typed-reference cases:
typed `select` over concrete function refs, imported typed-global -> defined/imported typed-table
flows, `ref.func` -> imported mutable typed-global flows, and typed block/if result propagation.
New raw valid fixtures cover those equivalent-signature cases, and new raw invalid fixtures pin
representative wrong-concrete-type boundaries for typed `select`, imported typed-global ->
imported typed-table flow, and typed `if` result propagation.

Typed-reference table/control saturation now also covers `br_if`, `br_table`, and typed
loop-result forms. Grounding expanded `br-if-subset`, `br-table-subset`, and `loop-subset` with
representative equivalent-signature typed-function-reference branches/results plus wrong-concrete-
type invalids. New raw valid fixtures cover typed `br_if`, typed `br_table`, and typed loop-
result equivalent-signature flows, and new raw invalid fixtures pin `BranchTypeMismatch` /
`ControlResultTypeMismatch` boundaries for wrong-concrete-type branch operands and loop
fallthrough results.

Typed branch-target consistency is now also grounded across broader multi-target `br_table`
combinations. `br-table-subset` now includes equivalent-signature and wrong-concrete-type cases
for nested block/block targets and mixed loop/block targets, and new raw fixtures pin
`InconsistentBranchTypes` for mismatched typed multi-target label sets.

Typed control-signature interaction is now also grounded as a broader campaign across block /
loop parameter-result composition and branch-to-loop targets. Grounding expanded `block-subset`,
`loop-subset`, `br-subset`, and `br-if-subset` with representative equivalent-signature typed
function-reference cases for block-result -> loop-param flow, loop-result -> block-result flow,
`br` to param-bearing loops, and `br_if` to param-bearing loops. New raw valid fixtures cover
those composed control-signature flows, and new raw invalid fixtures pin `TypeMismatch`,
`BranchTypeMismatch`, and `ControlResultTypeMismatch` boundaries for wrong-concrete-type loop
entry, branch targets, and enclosing block/result joins.

Typed branch/control closure under reachability is now also grounded across `unreachable`,
`return`, dead code after `br`, and control joins with early `return` inside typed `if`.
Grounding expanded `unreachable-subset`, `return-subset`, `ref-control-subset`, and `br-subset`
with representative equivalent-signature typed-function-reference cases for unreachable block
ends, direct typed returns, typed dead-code tails after `br`, and typed `if` joins where one arm
exits via `return`. New raw valid fixtures cover those reachability closures, and new raw invalid
fixtures pin `ControlResultTypeMismatch` boundaries for wrong-concrete-type dead-code tails,
wrong-concrete-type returns, and mismatched typed joins after early exit.

Typed call/control convergence is now also grounded across control-produced typed-function-
references flowing into `call_ref`, `return_call_ref`, `call_indirect`, and
`return_call_indirect`. Grounding expanded `typed-call-ref-subset`,
`typed-return-call-ref-subset`, `call-indirect-subset`, and `return-call-indirect-subset` with
representative equivalent-signature typed cases where block/if results feed direct reference
calls and typed indirect-call parameters. New raw valid fixtures cover block-result ->
`call_ref`, if-result -> `return_call_ref`, block-result -> typed `call_indirect` parameter, and
if-result -> typed `return_call_indirect` parameter. New raw invalid fixtures pin
`TypeMismatch` boundaries for wrong-concrete-type call-site convergence rather than earlier
control-frame failure.

Typed table/global/element flow closure is now also grounded across typed table reads feeding
mutable typed globals and typed globals feeding passive element segments that later initialize
typed tables. Grounding expanded `typed-reference-types-subset`, `typed-table-ref-subset`,
`elem-subset`, and `table-init-subset` with representative equivalent-signature cases for
defined/imported typed `table.get -> global.set` flows plus defined/imported typed
`global.get -> passive elem -> table.init -> table.get` chains. New raw valid fixtures cover
those composed storage/dataflow paths, and new raw invalid fixtures pin `TypeMismatch` on
`global.set` plus `ElementExprTypeMismatch` on wrong-concrete-type passive element
expressions.

Typed nullability-flow saturation is now also grounded across `br_on_null`, `br_on_non_null`,
`ref.as_non_null`, and typed nullable joins. Grounding expanded `br-on-null-subset`,
`br-on-non-null-subset`, `ref-as-non-null-subset`, and `ref-control-subset` with representative
equivalent-signature typed-function-reference cases for `br_on_null` fallthrough feeding
`call_ref`, `br_on_non_null` taken branches feeding typed block results and `call_ref`,
`ref.as_non_null` feeding mutable typed globals, and typed `if` joins combining
`ref.as_non_null` with `ref.null`. New raw valid fixtures cover those nullability-flow
compositions, and new raw invalid fixtures pin `TypeMismatch` at `call_ref`,
`br_on_non_null`, and `global.set` plus `ControlResultTypeMismatch` at typed null joins with
wrong concrete function types.

Malformed/decode boundary completion for typed control/reference encodings is now also grounded
across truncated `call_ref` / `return_call_ref` type immediates, truncated `br_on_null` /
`br_on_non_null` label immediates, truncated `ref.null` heap types, and truncated typed
block-result heap types. Grounding added the upstream-derived
`typed-malformed-control-subset.wast` plus raw invalid fixtures pinning decode-preserving
`ValidationErrorKind::Decode` with exact `CodeSection` / `UnexpectedEof` metadata at the
instruction-boundary offsets for these typed control/reference body-decode failures.

Official spec grounding is now also broadened across additional upstream validation-only and
malformed-binary files that fit Baedeker’s current supported surface. Grounding added the
upstream-derived `forward-subset.wast`, `unreached-invalid-subset.wast`,
`func-ptrs-invalid-subset.wast`, `binary-leb128-subset.wast`, `utf8-import-module-subset.wast`,
and `utf8-import-field-subset.wast`. These extend external grounding for forward mutual
recursion, unreachable-code invalids, classic function-pointer/table/type invalids, non-minimal
vs malformed LEB128 encodings, and malformed UTF-8 import names. Raw fixtures and unit tests now
also pin valid `forward-mutual-recursion.wasm` and `unreached-call-ref.wasm` acceptance plus
unreachable unknown-local/global/function/label failures with exact offsets.

Non-defaultable local initialization tracking is now implemented for function validation.
Parameters and defaultable locals start initialized, non-defaultable locals must be initialized
before `local.get`, and initialization established inside structured control does not escape the
enclosing block/if/loop frame. Grounding added the upstream-derived `local-init-subset.wast` plus
raw valid fixtures for `local.set` / `local.tee` / block-internal flows and raw invalid fixtures
pinning `UninitializedLocal` for direct use, post-block use, `else`-arm use, and post-`if` use of
non-defaultable locals.

Bottom-type / stack-polymorphic unreachable closure is now materially broader across representative
official `unreached-valid.wast` shapes. The validator now carries explicit bottom operands through
unreachable validation so instructions like `select` and `ref.as_non_null` can consume
stack-polymorphic inputs without spuriously underflowing, while frame-end checks still reject
concrete stray or mismatched values that survive in dead code. Grounding added the
upstream-derived `unreached-valid-subset.wast` plus raw valid fixtures for select-heavy
unreachable flows, bottom-heap `ref.as_non_null` / `br_on_null` cases, and a meet-bottom
`br_table` join, along with raw invalid fixtures pinning function-end mismatches for concrete
unreachable select/result leakage.

Official grounding expansion has resumed now that the concrete post-audit semantic blockers are
closed. Curated official additions broadened active coverage in `local-init-subset`,
`unreached-valid-subset`, `ref-as-non-null-subset`, `select-subset`, and `br-table-subset`,
including extra `local_init.wast` tee-init grounding, additional unreachable-valid select shapes,
an official unreachable `ref.as_non_null` case, richer select placement coverage, and additional
numeric `br_table` value forms. Representative raw valid fixtures and unit tests now also pin
those official shapes directly.

To avoid drifting back into tiny one-off expansions, the next official zero-skip batch widened a
denser `select.wast` control-consumer slice in one pass. `select-subset` now covers broader
official placement shapes including loop-first/last, if-condition, call-indirect operand
positions, store operands, memory.grow, call/return/branch/local/global consumers, load,
unary/binary/test/compare, and conversion contexts. Representative raw valid fixtures and unit
tests now pin `select-as-call-indirect-last`, `select-as-memory-grow-value`,
`select-as-global-set-value`, `select-as-convert-operand`, and `select-as-if-condition`.

The next official sweep kept that denser cadence by broadening `br-table-subset` across a much
larger official branch-consumer slice rather than a single placement. Active coverage now spans
block/loop placements, branch consumers (`br`, `br_if`, nested `br_table`), if/select/call /
call_indirect positions, local/global consumers, memory address/value consumers, arithmetic /
compare / conversion consumers, and `memory.grow`. Representative raw valid fixtures and unit
tests now pin `br-table-as-br-if-value-cond`, `br-table-as-call-indirect-func`,
`br-table-as-local-set-value`, `br-table-as-load-address`, `br-table-as-store-value`,
`br-table-as-compare-left`, and `br-table-as-memory-grow-size`.

A comparable branch-adjacent sweep then broadened `br-if-subset` rather than starting another tiny
file. Active official coverage now spans `br_if` result typing, block/loop placements, nested
branch consumers, if/select/call/call_indirect consumers, local/global consumers, memory
address/value consumers, arithmetic/compare consumers, and `memory.grow`. Representative raw valid
fixtures and unit tests now pin `br-if-as-br-if-value-cond`, `br-if-as-select-cond`,
`br-if-as-call-indirect-last`, `br-if-as-local-tee-value`, `br-if-as-load-address`,
`br-if-as-storeN-value`, and `br-if-as-memory-grow-size`.

The next comparable densification broadened `br-subset` across a much larger official direct-
branch consumer slice. Active official coverage now spans result typing, block/loop placements,
nested branch consumers (`br`, `br_if`, `br_table`), if/select/call/call_indirect consumers,
local/global consumers, memory address/value consumers, arithmetic/compare/conversion consumers,
and `memory.grow`, while retaining the existing typed-reference branch grounding in the same file.
Representative raw valid fixtures and unit tests now pin `br-as-br-if-value-cond`,
`br-as-select-all`, `br-as-call-indirect-all`, `br-as-local-tee-value`, `br-as-load-address`,
`br-as-storeN-value`, and `br-as-memory-grow-size`.

A follow-on control-consumer closure sweep then widened the remaining smaller structured-control
subsets rather than opening another narrow lane. `if-subset`, `block-subset`, `loop-subset`, and
`return-subset` now cover additional consumer placements spanning `call_indirect`, `memory.grow`,
`select`, loads, `local.tee`, direct calls, and nested branch values. This closes more of the
official expression-position surface around structured control without new validator algorithms.
Representative raw valid fixtures and unit tests now pin `if-as-call-indirect-last`,
`if-as-memory-grow-size`, `block-as-select-cond`, `block-as-load-address`,
`loop-as-local-tee-value`, `loop-as-memory-grow-size`, `return-as-call-value`, and
`return-as-br-value`.

A broader official audit pass then selected `call-subset` as the best dense zero-skip target, and
its follow-on batch widened that file aggressively rather than scattering tiny additions elsewhere.
Active official direct-call grounding now spans richer type/result cases plus a broad
producer/consumer slice across `select`, `if`, `br_if`, `br_table`, `call_indirect`, stores,
`memory.grow`, `return`, `drop`, `br`, local/global writes, loads, unary/binary/test/compare
operators, and conversion contexts, alongside additional representative invalid arity/type cases.
Representative raw valid fixtures and unit tests now pin `call-as-call-all-operands`,
`call-as-br-table-last`, `call-as-call-indirect-last`, `call-as-memory-grow-value`,
`call-as-local-tee-value`, `call-as-load-operand`, `call-as-compare-right`, and
`call-as-convert-operand`.

A comparable dense follow-on then broadened `call-indirect-subset` across its own official
producer/consumer surface rather than leaving indirect-call coverage comparatively sparse. Active
official indirect-call grounding now spans richer type/result cases plus placement coverage across
`select`, `if`, `br_if`, `br_table`, stores, `memory.grow`, `return`, `drop`, `br`, local/global
writes, loads, unary/binary/test/compare operators, and conversion contexts, while preserving the
existing multi-table / explicit-table-index and typed-reference call-indirect grounding already
present in the file. The invalid slice also broadened with more representative official
arity/type mismatches. Representative raw valid fixtures and unit tests now pin
`call-indirect-as-select-last`, `call-indirect-as-br-if-first`, `call-indirect-as-store-last`,
`call-indirect-as-memory-grow-value`, `call-indirect-as-local-tee-value`,
`call-indirect-as-load-operand`, `call-indirect-as-compare-right`, and
`call-indirect-as-convert-operand`.

The next dense sweep then expanded `unreachable-subset` itself rather than leaving the
stack-polymorphic lane represented by only a handful of official placements. Active official
unreachable grounding now spans result typing, function/block/loop placements, branch/branch-table
positions, `if`/`select` consumers, direct and indirect calls, local/global writes, load/store
address and value positions, unary/binary/test/compare/conversion consumers, and `memory.grow`,
while keeping the typed-reference unreachable join case already grounded in the same file.
Representative raw valid fixtures and unit tests now pin `unreachable-as-func-mid`,
`unreachable-as-block-value`, `unreachable-as-br-table-value-index`,
`unreachable-as-if-then-no-else`, `unreachable-as-call-indirect-first`,
`unreachable-as-local-tee-value`, `unreachable-as-storeN-value`, and
`unreachable-as-convert-operand`.

A follow-on typed/control adjacency sweep then densified the typed reference/control frontier
around those now-broader call/branch/unreachable lanes instead of leaving `call_ref`,
`return_call_ref`, `br_on_null`, `br_on_non_null`, and `ref.as_non_null` coverage relatively
isolated. Active official grounding now includes nested and recursive `call_ref`, recursive
`return_call_ref` under `if`, explicit unreachable typing around `call_ref`, `br_on_null`, and
`br_on_non_null`, `br_on_non_null` + `ref.as_non_null` joins, and direct-call-to-typed-ref
producer/consumer flow in `ref-as-non-null-subset`. Representative raw valid fixtures and unit
tests now pin `call-ref-run-nested`, `call-ref-unreachable-ref-func`,
`call-ref-unreachable-call-drop`, `return-call-ref-count`, `br-on-null-unreachable`,
`br-on-non-null-ref-as-non-null`, `br-on-non-null-unreachable`, and
`ref-as-non-null-direct-call-ref-func`.

The next invalid-pressure pass then deepened the same typed/control frontier rather than only
adding more happy-path official grounding. Active official invalid coverage now includes extra
`call_ref` / `return_call_ref` non-funcref cases, `return_call_ref` result-arity pressure, and
broader raw exact-offset regression fixtures for `call_ref`, `return_call_ref`, `br_on_null`,
`br_on_non_null`, and `ref.as_non_null` consumer/stack-shape failures. Representative raw invalid
fixtures and unit tests now pin `call-ref-non-funcref-externref`,
`call-ref-non-funcref-funcref`, `return-call-ref-non-funcref-externref`,
`return-call-ref-non-funcref-funcref`, `return-call-ref-multi-result`,
`br-on-null-stack-mismatch`, `br-on-non-null-stack-mismatch`, and
`ref-as-non-null-null-to-nonnull-call`.

A follow-on typed result-lattice pressure pass then focused specifically on reference-result
subtyping and nullability joins around `call_ref`, `return_call_ref`, and structured-control
result boundaries. Active official invalid grounding now includes the broader `return_call_ref`
result-lattice slice from upstream (`ref $t` vs `ref null $t`, `ref func`, and `ref null func`
result interactions), while new raw exact-offset fixtures pin `call_ref`-produced
wrong-concrete-type failures at function, block, and `if` result boundaries plus
`return_call_ref` wrong-concrete-type and nullability result mismatches. Representative raw
invalid fixtures and unit tests now pin `typed-call-ref-function-result-wrong-concrete-type`,
`typed-call-ref-block-result-wrong-concrete-type`,
`typed-call-ref-if-result-wrong-concrete-type`,
`typed-return-call-ref-result-wrong-concrete-type`, and
`typed-return-call-ref-result-nullability-mismatch`.

The next typed select / ref-control result-pressure pass then widened the same result-boundary
scrutiny across `select`, `ref.as_non_null`, `br_on_null`, and `br_on_non_null` rather than
leaving that lattice pressure concentrated only around `call_ref` / `return_call_ref`. Active
official invalid grounding now includes additional result-boundary failures in
`ref-select-subset`, `ref-as-non-null-subset`, `br-on-null-subset`, and
`br-on-non-null-subset`, while new raw exact-offset fixtures pin typed-select function/block/`if`
result mismatches, typed-select nullability-result mismatch, `ref.as_non_null`
function-result mismatch, and `br_on_null` / `br_on_non_null` block-result mismatches.
Representative raw invalid fixtures and unit tests now pin
`typed-select-function-result-wrong-concrete-type`,
`typed-select-block-result-wrong-concrete-type`,
`typed-select-if-result-wrong-concrete-type`,
`typed-select-function-result-nullability-mismatch`,
`typed-ref-as-non-null-function-result-wrong-concrete-type`,
`typed-br-on-null-block-result-wrong-concrete-type`, and
`typed-br-on-non-null-block-result-wrong-concrete-type`.

A follow-on typed branch/select join-pressure sweep then widened nullability-flow pressure
specifically across `br`, `br_if`, `br_table`, and `select` interactions with typed refs. Active
official grounding now includes select-fed branch cases in `br-subset`, `br-if-subset`, and
`br-table-subset`, plus a direct typed-select nullability invalid in `ref-select-subset`. New raw
fixtures now pin valid nullable-target select-to-branch flows for `br`, `br_if`, and `br_table`,
along with exact-offset invalids for direct typed-select nullability mismatch, select-fed `br` /
`br_if` / `br_table` non-null target failures, and mixed nullable/non-null `br_table` target
inconsistency. Representative raw fixtures and unit tests now pin
`typed-select-to-br-nullable`, `typed-select-to-br-if-nullable`,
`typed-select-to-br-table-nullable`, `typed-select-nullability-mismatch`,
`typed-select-to-br-nullability-mismatch`, `typed-select-to-br-if-nullability-mismatch`,
`typed-select-to-br-table-nullability-mismatch`, and
`typed-br-table-nullability-targets-mismatch`.

A further typed branch-result pressure sweep then widened nullability-specific coverage around loop
params, block results, and `if` joins. Active official grounding now includes nullable
branch-to-loop-param cases in `br-subset` and `br-if-subset`, nullable block/loop result cases in
`block-subset` and `loop-subset`, and nullable/non-null join pressure in `if-subset`. New raw
fixtures now pin valid non-null→nullable flows for block results, loop results, `if` joins, `br`
to loop params, and `br_if` to loop params, along with exact-offset invalids for non-null
block/loop/`if` result expectations fed by nullable typed refs and for nullable branch values
targeting non-null loop params. Representative raw fixtures and unit tests now pin
`typed-block-result-nullable-from-nonnull`, `typed-loop-result-nullable-from-nonnull`,
`typed-if-result-nullable-from-join`, `typed-br-to-loop-param-nullable`,
`typed-br-if-to-loop-param-nullable`, `typed-block-result-nullability-mismatch`,
`typed-loop-result-nullability-mismatch`, `typed-if-result-nullability-mismatch`,
`typed-br-to-loop-param-nullability-mismatch`, and
`typed-br-if-to-loop-param-nullability-mismatch`.

A follow-on typed nullability-consumer sweep then widened pressure across plain `return`,
`call_ref` consumers, and table/global storage boundaries under mixed nullable/non-null structured
joins. Active official grounding now includes nullable structured-join return cases in
`return-subset`, nullable `if`-fed `call_ref` cases in `typed-call-ref-subset`, and nullable
`if`-fed `global.set` / `table.set` cases in `typed-table-ref-subset`. New raw fixtures now pin
valid nullable joins flowing into `return`, `call_ref`, `global.set`, and `table.set`, along with
exact-offset invalids for non-null return/global/table expectations fed by nullable structured
joins and for an abstract `funcref` structured join consumed by `call_ref`. Representative raw
fixtures and unit tests now pin `typed-return-if-nullable`, `typed-call-ref-if-nullable`,
`typed-global-set-if-nullable`, `typed-table-set-if-nullable`,
`typed-return-if-nullability-mismatch`, `typed-call-ref-if-abstract-nullability-mismatch`,
`typed-global-set-if-nullability-mismatch`, and
`typed-table-set-if-nullability-mismatch`.

A further typed nullability storage/dataflow sweep then widened structured-join coverage across
locals, globals, and tables, while extending nullability pressure through passive element and
`table.init` flows. Active official grounding now includes nullable structured joins stored via
`local.set` in `local-set-subset`, carried through `local` / `global` / `table` storage into later
`call_ref` consumers in `typed-call-ref-subset`, fed through non-null function-result boundaries
in `return-subset`, and widened passive-element / `table.init` nullability coverage in
`elem-table-init-subset` and `table-init-subset`. New raw fixtures now pin valid structured-join
storage/dataflow through locals, globals, and tables into later `call_ref`, plus passive-element
and `table.init` nullable flows from non-null producers. Exact-offset invalids now pin nullable
structured-join values escaping local/global/table storage into non-null function results,
nullable structured joins stored into non-null locals, nullable passive-element expressions
consumed as non-null typed refs, and nullable typed element segments targeting non-null tables.
Representative raw fixtures and unit tests now pin `typed-local-set-if-nullable`,
`typed-local-if-nullable-to-call-ref`, `typed-global-if-nullable-to-call-ref`,
`typed-table-if-nullable-to-call-ref`, `typed-passive-element-nullable-from-nonnull`,
`typed-table-init-nullable-to-call-ref`, `typed-local-set-if-nullability-mismatch`,
`typed-local-if-to-return-nullability-mismatch`,
`typed-global-if-to-return-nullability-mismatch`,
`typed-table-if-to-return-nullability-mismatch`,
`typed-passive-element-nullability-mismatch`, and
`typed-table-init-nullability-mismatch`.

A follow-on typed nullability transport sweep then widened indirect storage/initialization coverage
across `global.get -> passive elem -> table.init -> table.get` chains, with blame pinned at the
final semantic consumer when intermediate transport remains valid. Active official grounding now
includes nullable and non-null global transport into passive elements in `elem-table-init-subset`,
indirect nullable transport into `call_ref` consumers in `typed-call-ref-subset` and
`table-init-subset`, indirect nullable transport into non-null function-result boundaries in
`return-subset`, and indirect nullable transport into non-null `global.set` in
`typed-table-ref-subset`. New raw fixtures now pin valid defined/imported global transport through
passive elements and `table.init` into later `call_ref` consumers, including non-null global
values widened through nullable passive-element / table boundaries. Exact-offset invalids now pin
nullable transport arriving at non-null function results, nullable transport arriving at non-null
`global.set`, and nullable passive-element segments targeting non-null tables. Representative raw
fixtures and unit tests now pin
`typed-defined-global-passive-element-table-init-nullable-to-call-ref`,
`typed-imported-global-passive-element-table-init-nullable-to-call-ref`,
`typed-defined-nonnull-global-passive-element-nullable`,
`typed-imported-nonnull-global-passive-element-table-init-nullable-to-call-ref`,
`typed-defined-global-passive-element-table-init-to-return-nullability-mismatch`,
`typed-imported-global-passive-element-table-init-to-return-nullability-mismatch`,
`typed-defined-global-passive-element-table-init-to-global-set-nullability-mismatch`, and
`typed-imported-global-passive-element-table-init-nullability-mismatch`.

A further typed nullability consumer-lattice sweep then widened terminal-consumer pressure across
`return_call_ref`, `return_call_indirect`, and branch-target transport after table initialization.
Active official grounding now includes table-initialized function-reference transport into
`return_call_ref` result boundaries in `typed-return-call-ref-subset`, table-initialized
indirect-call-table transport into `return_call_indirect` result boundaries in
`return-call-indirect-subset`, and table-initialized branch-value transport into `br`, `br_if`,
and `br_table` target/result boundaries in `br-subset`, `br-if-subset`, and `br-table-subset`.
New raw fixtures now pin valid nullable consumer flows for table-initialized `return_call_ref`,
`return_call_indirect`, `br`, `br_if`, and `br_table` cases. Exact-offset invalids now pin where
the terminal consumer rejects nullable transport: `ResultTypeMismatch` at `return_call_ref` /
`return_call_indirect`, and `BranchTypeMismatch` at `br`, `br_if`, and `br_table`.
Representative raw fixtures and unit tests now pin
`typed-table-init-to-return-call-ref-result-nullable`,
`typed-table-init-to-return-call-indirect-result-nullable`,
`typed-table-init-to-br-nullable`, `typed-table-init-to-br-if-nullable`,
`typed-table-init-to-br-table-nullable`,
`typed-table-init-to-return-call-ref-result-nullability-mismatch`,
`typed-table-init-to-return-call-indirect-result-nullability-mismatch`,
`typed-table-init-to-br-nullability-mismatch`,
`typed-table-init-to-br-if-nullability-mismatch`, and
`typed-table-init-to-br-table-nullability-mismatch`.

`ref.as_non_null` now also has dedicated grounding via `ref-as-non-null-subset`, along with raw
valid fixture `ref-as-non-null-call-ref.wasm` and raw invalid fixture
`ref-as-non-null-non-ref-input.wasm`. Within the current bounded typed-reference model it accepts
reference operands and produces the non-null form of the same reference type.

Null-branch support now also includes `br_on_null` and `br_on_non_null` decoding plus validation
via `br-on-null-subset` and `br-on-non-null-subset`, along with raw valid fixtures
`br-on-null-fallthrough-narrow.wasm` and `br-on-non-null-branch-result.wasm` and raw invalid
fixtures `br-on-null-non-ref-input.wasm` and `br-on-non-null-non-ref-target.wasm`.
`br_on_null` now narrows the fallthrough reference to non-null, while `br_on_non_null` requires a
reference-typed target label suffix and routes the tested value through that branch.

Historical active upstream files have also been normalized to `-subset.wast` names; for example,
`ref-func-undeclared-reference-subset.wast` is active coverage rather than a deferred skip.

## Phase 1 closure audit snapshot

Current audit verdict: **Phase 1 is not yet complete**, even though the validation front-end is now
broadly grounded and structurally stable.

### What is already strong enough to carry into Phase 2 later
- The validator/harness boundary is crisp: raw `invalid-decode` fails at `Module::decode(...)`, raw
  `invalid-validate` fails after decoding, and decode-preserving body failures are pinned through
  `ValidationErrorKind::Decode { context, kind }`.
- The active upstream-derived lane is zero-skip and now covers **87** curated upstream subset files
  with **573** directives, all enforced in both `spec_wast` and `spec_node`.
- The raw corpus is now large enough to act as a real regression floor:
  **206** valid fixtures, **165** invalid-validate fixtures, and **34** invalid-decode fixtures.
- The typed-function-reference / tail-call / nullability / const-init / table-global-element
  campaigns all broadened coverage without forcing architecture drift away from the current
  spec-facing validator model.

### What the audit says still blocks calling Phase 1 complete
1. **Official spec grounding should continue.** The next high-value official files to activate are
   additional curated slices of `unreached-valid.wast` and adjacent official validation-only
   files.
2. **Broader WebAssembly 3.0 audit pressure still remains beyond the current bounded typed-ref
   model.** The current model is appropriate for the active supported subsets, but Phase 1 closure
   still requires continued auditing against remaining proposal-era and niche validation cases.

### Most direct post-audit path
1. Continue widening the official zero-skip upstream lane with more curated `unreached-valid`
   subsets and adjacent validation-only files.
2. Continue auditing the remaining WebAssembly 3.0 validation surface beyond the currently bounded
   typed-reference model.

That bookkeeping matters for Phase 2 because the register-lowering work should inherit a semantic
front-end with known boundaries, not an ambiguous notion of "probably enough validation."