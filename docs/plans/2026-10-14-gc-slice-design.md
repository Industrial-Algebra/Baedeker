# Slice 5 — GC (staged; collector in 5b)

WASM 3.0 slice 5: the GC proposal's type grammar and heap semantics.
Follows the slice recipe: plan → unit contracts → dispatched
implementation → vendor corpus → grind to zero → review loop to
convergence (breaker 12) → operator merge.

**Operator decisions (2026-10-14 design session):**

1. **Staged collector.** Slice 5 ships full GC *semantics* with a
   never-free heap behind a collector-shaped interface; the real
   collector is slice 5b, gated by the article ladder below.
2. **R0–R2 articles gate slice 5; R3–R4 are 5b's entry ticket.**
3. **Grading = heap-stats API + structured refusal harness**
   (stats hooks designed into the slice-5 object model).

Spec pin: `dfa3f32a881aecc60a8c792da3c25787ccb15572`
(re-certified via `git rev-parse` in the pin clone at
`/tmp/wasm-spec-pin`). Reference corpus to vendor: **exactly 22
files** — 17 in `test/core/gc/` plus the 5 GC-deferred core files
(`type.wast`, `type-rec.wast`, `type-canon.wast`,
`type-equivalence.wast`, `ref_null.wast`). The two remaining deferrals
(`skip-stack-guard-page`, `names`) are non-GC and stay deferred.
Already passing today: `call_ref`, `return_call_ref`, `ref_func`,
`ref_is_null`, `ref_as_non_null`, `ref.wast`.

## Scope

**Type grammar (all four heap-type hierarchies, complete):**

- any-side: `any`, `eq`, `i31`, `struct`, `array`, `none`, concrete
  struct/array types
- func-side: `func`, concrete func types, `nofunc`
- extern-side: `extern`, `noextern`
- exn-side: `exn`, `noexn` (NEW in `HeapType` — current code has
  `Exn` but no `NoExn`; `ref_null.wast` uses `(ref.null noexn)` ×5)

`rec` groups over struct/array/func; `sub` / `sub final` with declared
supertypes. **Recursive references are accepted**: the EH-era
conservative cycle rejection was an exn-tag stopgap; GC requires full
rec-group-aware canonicalization (install the whole group before
checking members — valid.ml:178–189). Rejected: unknown type indices,
supertype cycles, and supertypes not available at declaration point.
Bare≡singleton-rec normalization (EH r4b) carries over.

**Instruction inventory (explicit, frozen before U1):**

- struct: `struct.new`, `struct.new_default`, `struct.get`,
  `struct.get_s`, `struct.get_u`, `struct.set`
- array: `array.new`, `array.new_default`, `array.new_fixed`,
  `array.new_data`, `array.new_elem`, `array.get`, `array.get_s`,
  `array.get_u`, `array.set`, `array.len`, `array.fill`, `array.copy`,
  `array.init_data`, `array.init_elem`
- i31: `ref.i31`, `i31.get_s`, `i31.get_u` (immediate tagged value —
  NEVER a heap allocation; signed/unsigned 31-bit semantics)
- casts: `ref.test`, `ref.test null`, `ref.cast`, `ref.cast null`,
  `br_on_cast`, `br_on_cast_fail` (incl. `null` label forms)
- misc: `ref.eq`, `any.convert_extern`, `extern.convert_any`
- Constant-expression eligibility for the new forms enumerated in the
  U1 contract (per valid.ml's const rules).

**Out of scope (5):** collection itself (5b), host-side anyref beyond
the R0-required conversion semantics (5b), shared-memory GC (threads
slice), custom page sizes, multi-memory.

## Design

- **Type system**: generalize EH rec-group machinery to the grammar
  above. Canonicalization produces canonical indices; equivalence =
  same canonical index. Cast decisions need more than equivalence:
  **runtime subtype relation = declared supertype chains over
  canonical IDs**, precomputed at validation (each canonical type →
  optional supertype canonical ID); `ref.test`/`ref.cast` walk the
  bounded chain (plus null/abstract-heap cases per valid.ml:732–741).
  A5 exercises distinct-ID subtype successes, not just equivalence.
- **Heap object model** (collector-shaped, never-free in 5):
  `GcHeap` on the store: `ObjectId(u32)`-indexed table of
  struct{type_id, fields} | array{type_id, len, elems}. Wasm
  references: `Value::GcRef(ObjectId)` — copies freely, no Rc.
  i31 is **immediate** (`Value`-level tagged i32), consuming no
  object-table entry. Never-free: table only grows, try_reserve
  discipline, refusal = clean trap with untouched state (the memory64
  resource contract). Index-stable so 5b swaps strategy behind the
  interface without touching AOT/FFI.
- **Extern/any conversions** (representation decided in U2, before
  U3): conversions are **immediate value-level re-tags**, exactly as
  the reference interpreter models them (eval.ml:929–939) —
  `any.convert_extern` re-tags a host reference into the anyref
  hierarchy, `extern.convert_any` re-tags back (wrapping i31/struct/
  array references into the extern side as an immediate carrying the
  inner value); null converts to null. **No wrapper heap entries exist**;
  round trips are identity-preserving by construction (the inner value
  never changes); conversion storms allocate nothing (pinned by A7).
- **GcStats — logical accounting, exact**: `store.gc_stats() →
  { live_objects, live_bytes, allocations_since(snapshot) }`.
  `live_bytes` = Σ logical bytes with the formula frozen in the U2
  contract — **total over every storage type** (valid.ml:131–148):
  struct = 8-byte header + Σ field widths; array = 12-byte header +
  len × elem width; widths **i8=1, i16=2, i32/f32=4, i64/f64=8,
  v128=16**, packed ref handle=4. Wrappers and i31 contribute 0
  (immediates). This is **logical** accounting — explicitly NOT
  reserved capacity, table slack, allocator overhead, or process RSS
  (separate stats expose reserved slots). Graders assert exact
  equality against the formula (A2a, A8). "Untouched state" on
  refusal = the failing allocation is atomic; prior successful
  article allocations stand. Snapshots taken at article boundaries;
  `allocations_since` counts object-table insertions (struct/array
  only).
- **AOT v7** for new instruction forms + reference operands (handle
  ABI unchanged underneath). FFI: GC references as opaque handles.
  **Lowerer mirror rule**: duplicate operand type-checker gains the
  full ref lattice (`pop_ref_in_heap` + nullability + abstract
  hierarchy).
- **Runner**: `runtime_official.rs` needs GC-reference assertion and
  input support (currently `RefHost` inputs map to `ExternRef` host
  indices and `RefHost` results are unconditionally rejected —
  lines ~695/~973) plus hostref round-trip plumbing for `extern.wast`.

## The article ladder

| Rung | Articles | Proves | Gates |
|---|---|---|---|
| R0 | Official GC corpus (exactly 22 files) | Schedule-independent spec semantics | 5 |
| R1 | wasm-smith GC-property modules, differential vs spec interpreter | Robustness on author-blind struct/array graphs, cast cascades | 5 |
| R2 | Hand-built A1–A8 (below), graded by logical-stats deltas + structured refusal | Allocation accounting, refusal discipline, stats exactness | 5 |
| R3 | Real compilers on wasm-GC: OCaml/wasm_of_ocaml, Dart/dart2wasm, Kotlin/Wasm | Collector under real allocation topology | **5b ticket** |
| R4 | GHC wasm32-wasi substrate stress (no wasm-GC usage — RTS GC runs in linear memory) | Substrate robustness: giant modules, call_indirect storms, RTS memcpy storms, table churn | **5b ticket** |

**Assertion classes, kept distinct** (P2.8/P2.10 fix):
- *Schedule-independent*: wasm results — hold under any collection
  schedule; oracle is the never-collecting spec interpreter (= slice
  5's semantics). Every rung.
- *Slice-5 accounting*: never-free stats assertions (A3 monotonic
  growth, A4/A5 zero-delta) — valid only while never-free; 5b
  **replaces** these with bounded-retention assertions (A3 flips to
  reclaim).
- *5b instrumentation*: RSS flatness on bounded-live-set articles.

**Refusal harness (structured, P2.9 fix)**: articles run in a child
process; the runner asserts an *expected structured result* —
`assert_trap` with the resource-exhaustion diagnostic. A plain Rust
panic exit (101) is a FAILURE unless the runner's expected-trap
machinery produced it; abort/SIGKILL (134/137) always fail. Caps:
`ulimit -v 49152/65536` smoke (documented per article); grading
covers every allocation path (struct/array/new_data/new_elem/init),
not just the object table.

GHC placement rationale, recorded honestly: GHC's wasm backend does
not target wasm-GC; compiled Haskell challenges everything *around*
the collector. An early GHC smoke subset (parse + run hello-class
binaries) may land during 5; full R4 with 5b.

### R2 articles

| # | Article | Stresses | Pass criteria |
|---|---|---|---|
| A1 | ephemeral-churn: 10⁶ short-lived pairs, bounded live window | Allocation volume vs live set | schedule-independent results; `allocations_since` exact; capped run → structured resource trap |
| A2a | persistent-build (accounting): 10⁶-node list + tree, all live, **provisioned runner (no caps)** | Logical live-set accounting | `live_bytes` exactly = formula × count |
| A2b | persistent-build (refusal): 10⁷-node construction attempt under 48/64 MiB caps | Refusal discipline at scale | structured resource trap; stats snapshot shows state untouched |
| A3 | cycle-farm: 10⁵ rotating cycles | Never-free honesty (slice-5 class) | schedule-independent results; slice-5: `live_objects` monotonic; 5b: flips to bounded retention |
| A4 | i31-storm: 10⁷ `ref.i31`/`i31.get_*` vs boxed | i31 never allocates | stats delta 0 across run |
| A5 | cast-cascade: 10⁶ tests/casts, 30-deep declared subtype chain, mixed outcomes incl. **distinct-ID subtype successes** | RTT canonical-ID + ancestry path | results exact; stats delta 0 |
| A6 | new_data/new_elem hybrid + linear-memory pressure | Heap × linear-memory interaction | results correct; GC logical bytes and memory pages accounted separately |
| A7 | mixed-ref-matrix: funcref/externref/GcRef in tables/globals/params + conversion round-trip storm (`any.convert_extern`/`extern.convert_any` ×10⁶ over mixed identities) | Reference-kind separation; conversions never allocate | results exact under every storage kind; stats delta 0 across the storm |
| A8 | v128-accounting: v128-field structs (8+16) and v128 arrays (12+3×16) built/rebuilt in exact counts | Formula totality incl. v128 | `live_bytes` exactly = formula × count; `allocations_since` exact |

Articles A1–A8 in `tests/gc_articles/` (wast-authorable where the encoder
allows) + runner harness (child process, snapshots, structured
expectations, caps). Articles self-bound; per-article caps documented.
Throughput/latency benchmarks deliberately deferred to 5b.

## Units

| Unit | Content | Contract |
|---|---|---|
| U1 | Type grammar: all four hierarchies (incl. `noexn`), rec/sub with recursion ACCEPTED, canonicalization + equivalence + declared-supertype chains (precomputed ancestry) in binary+validate | `gc-unit1-contract.md` |
| U2 | Heap + object model: GcHeap/ObjectId, i31 immediate, extern/any wrapper representation + identity rules, GcStats logical formula, struct/array/i31 instruction execution, AOT v7 | `gc-unit2-contract.md` |
| U3 | Cast machinery: ref.test/cast (+null forms), br_on_cast pair, ref.eq, conversions — green on targeted tests (NOT full R0; see U4) | `gc-unit3-contract.md` |
| U4 | Surface completion: GC types in globals/tables/elements/imports/exports, const-expr eligibility, lowerer lattice mirror, runner RefHost/assert-ref support, FFI handles, R1 smith wiring | `gc-unit4-contract.md` |
| U5 | Vendor the 22 files, grind R0 to zero; articles A1–A8 + harness + caps in CI | `gc-unit5-contract.md` |

R0 green end-to-end is delivered at **U5** (U4 supplies GC
globals/tables/elements and runner support that i31.wast/extern.wast
already require — corpus files cannot pass before U4 lands).

## 5b entry criteria

**Ticket (R3 and R4 alike) = curated, executable, baseline-recorded**:
payloads frozen; each runs on post-slice-5 Baedeker with recorded
baseline behavior (schedule-independent results green; resource traps
where they occur). Collector work converges when: A3 flips
accumulate→reclaim (bounded retention), RSS flat on bounded-live-set
articles, every schedule-independent guarantee and refusal-discipline
guarantee still holds.

## Risks

- **(a) Canonicalization is the silent killer** (EH burned two review
  rounds; this plan's r1 review confirmed the recursion-acceptance
  trap). Mitigation: pin canonical-form + supertype-installation
  semantics from valid.ml *before* the U1 contract.
- **(b) wasm-smith GC support**: verify our lockfile's smith generates
  GC modules; else R1 falls back to spec-interpreter differential on
  hand-mutated corpus modules.
- **(c) live_bytes flakiness**: killed by design — logical-only
  formula, exact equality, capacity/RSS explicitly out of scope.
- **(d) Unbounded never-free runs**: articles self-bound; caps
  documented per article.
- **(e) Runner GC-reference plumbing** (RefHost inputs/outputs) is
  load-bearing for R0 — assigned explicitly to U4.
