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
`ref-select-subset`, and `ref-control-subset`, covering `ref.null` in function/global/control
positions, typed `select` over `funcref`/`externref`, `ref.func` / `ref.null` joins, and
reference-valued block/if result flow.

Tail-call mini-cluster support now also includes `return_call` and `return_call_indirect`
decoding plus validation grounding via `return-call-subset` and
`return-call-indirect-subset`, along with raw valid fixtures for minimal direct/indirect tail-call
shapes and raw invalid fixtures for result-mismatch and non-`funcref`-table cases.

Function-reference call support now also includes `call_ref` and `return_call_ref` decoding plus
an initial typed-reference groundwork pass: `RefType` now models nullable vs non-null references
and concrete function type indices, binary parsing accepts typed-reference encodings across
function types / globals / tables / locals / block results, and validation accepts concrete
function refs as subtypes of abstract `funcref` where appropriate. Grounding now includes
`call-ref-subset`, `return-call-ref-subset`, `typed-call-ref-subset`,
`typed-return-call-ref-subset`, `typed-table-ref-subset`, and
`typed-reference-types-subset`, along with raw valid fixtures for typed global init / typed
`call_ref` / typed table flows and raw invalid fixtures for non-`funcref` references,
concrete-type mismatches, and result-mismatch cases.

Null-branch support now also includes `br_on_null` and `br_on_non_null` decoding plus validation
via `br-on-null-subset` and `br-on-non-null-subset`, along with raw valid fixtures
`br-on-null-fallthrough-narrow.wasm` and `br-on-non-null-branch-result.wasm` and raw invalid
fixtures `br-on-null-non-ref-input.wasm` and `br-on-non-null-non-ref-target.wasm`.
`br_on_null` now narrows the fallthrough reference to non-null, while `br_on_non_null` requires a
reference-typed target label suffix and routes the tested value through that branch.

Historical active upstream files have also been normalized to `-subset.wast` names; for example,
`ref-func-undeclared-reference-subset.wast` is active coverage rather than a deferred skip.

That bookkeeping matters for Phase 2 because the register-lowering work should inherit a semantic
front-end with known boundaries, not an ambiguous notion of "probably enough validation."