# Baedeker — Roadmap

> A WASM runtime built in Rust, named for the most cautious and methodical of Pierson's Puppeteers.
> Like its namesake, Baedeker proceeds carefully through unknown territory — but gets there.

## Philosophy

This roadmap is structured as a bottom-up traversal of the WebAssembly abstraction stack.
Each phase builds expertise in a specific layer before moving upward. Phases are designed to
produce a working (if incomplete) artifact at each boundary, so the project is always runnable
and testable — never in a state where three more layers need to exist before anything executes.

The target spec is WebAssembly 2.0 (with an eye toward post-MVP proposals), and the primary
deployment target is iOS via Rust FFI into Swift. JIT compilation is explicitly out of scope
for the initial architecture; the engine is interpreter-first with AOT as a future layer.

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

- Parse the Type section: function signatures (`functype`), with the full numtype/vectype/reftype
  taxonomy from WASM 2.0.
- Parse Import and Function sections to build the function index space.
- Implement the **validation algorithm** for instruction sequences. This is the heart of WASM's
  safety model: a type-checking pass over the structured operand stack that enforces:
  - Stack polymorphism after unconditional branches
  - Block signature matching for `block`, `loop`, `if`
  - Correct label indexing for `br`, `br_if`, `br_table`
  - Type-correct `select` with explicit type annotations (2.0)
- Build a validation error type that produces genuinely useful diagnostics (byte offset,
  expected vs actual stack state, surrounding instruction context).
- **Milestone:** Can validate the type-correctness of any WASM 2.0 module and reject malformed ones
  with clear errors. Run against the official spec test suite's `assert_invalid` and
  `assert_malformed` cases.

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

## Ongoing: Creusot Contracts

As each layer stabilizes, add Creusot contracts to the core invariants:
- LEB128 decode/encode roundtrip correctness
- Validation algorithm soundness (well-typed programs don't get stuck)
- Memory bounds checking completeness
- Numeric operation IEEE 754 conformance

This is a long-term investment that compounds: a verified WASM runtime core is
a unique artifact in the ecosystem.
