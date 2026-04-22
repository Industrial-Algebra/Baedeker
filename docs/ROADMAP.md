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
    - `wast-upstream/memory-init-subset.wast`
    - `wast-upstream/data-drop-subset.wast`
    - `wast-upstream/memory-copy-subset.wast`
    - `wast-upstream/memory-fill-subset.wast`
    - `wast-upstream/call-subset.wast`
    - `wast-upstream/call-indirect-subset.wast`
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

#### Still to do before Phase 1 can be called complete
- [ ] Cover the remaining major instruction families and backfill gaps, especially additional
  reference-type/proposal-era validation paths, any still-missing niche numeric/proposal
  variants, and official spec-suite-driven completeness gaps.
- [ ] Continue broadening reference-type, const-expression, and proposal-era validation coverage in
  line with the WebAssembly 3.0 target surface, building beyond the current `ref.null` /
  `ref.func` / `ref.is_null` and imported-const-`global.get` subset.
- [ ] Integrate more official spec-suite `assert_invalid` / `assert_malformed` style coverage and
  keep compliance/defer status explicit.
- [ ] Continue converting any newly discovered upstream friction points into either explicit tracked
  deferred cases (`.meta` `skip=...`) or implemented support, so Phase 2 builds on a crisp
  semantic contract rather than assumptions.

### Recommended next steps
1. Prioritize the highest-leverage remaining semantic gaps for both full validation and future
   lowering: remaining numeric coverage/variants, broader reference-type/proposal-era
   validation, and external spec-suite integration.
2. Keep expanding the fixture corpus in lockstep with each new instruction family, including exact
   `.meta` assertions for representative diagnostics.
3. Begin wiring in official spec-suite inputs so Phase 1 progress is measured against external
   ground truth as well as internal fixtures.
4. Keep the architectural boundary explicit: validation state remains proof/type state; register IR
   design and lowering stay in Phase 2.

### Current verification snapshot
- `cargo fmt -- --check`
- `cargo test -p baedeker-core --test spec`
- `cargo test -p baedeker-core --test spec_wast`
- `cargo test -p baedeker-core`
- `cargo clippy -p baedeker-core --all-targets -- -D warnings`
- Current `baedeker-core` unit test count: **203 passing**
- Current spec-harness integration tests: **5 passing** (`spec`: 3, `spec_wast`: 2)

### Current branch snapshot
Current Phase 1 closure work continues on `feat/phase-1-part-2-closures`, with the validator and
fixture/docs support matrix kept in sync as upstream-derived skips are narrowed or retired.

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
