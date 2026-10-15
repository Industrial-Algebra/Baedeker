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
3. **Grading = heap-stats API + ulimit harness** (stats hooks designed
   into the slice-5 object model; caps promoted from the review-loop
   recipe into CI).

Spec pin: `dfa3f32a881aecc60a8c792da3c25787ccb15572` (COMMIT.txt at
`/tmp/wasm-spec-pin`). Reference corpus: 17 `test/core/gc/` files +
6 GC-deferred core files (`type.wast`, `type-rec`, `type-canon`,
`type-equivalence`, `ref_null`, and the GC-relevant remainder) ≈ 23
files to vendor; exact inventory pinned at U5 contract time.
Not-deferred typed-ref files (`call_ref`, `return_call_ref`,
`ref_func`, `ref_is_null`, `ref_as_non_null`, `ref.wast`) already pass.

## Scope

- **Type grammar**: `rec` groups over struct/array/func; `sub` /
  `sub final`; abstract heap types `any eq i31 struct array none extern
  func noextern` (+ func sides); full canonicalization with canonical
  indices as RTT identity; type equivalence/subtyping.
- **Heap + instructions**: `GcHeap` object model; struct.new/get/set;
  array.new/new_default/new_fixed/new_data/new_elem/get/set/len/
  fill/copy/init_data/init_elem; i31.new/get; ref.eq; ref.test/ref.cast
  (+ br_on_cast pair); extern.convert_any / any.convert_extern.
- **Out of scope (5)**: collection itself (5b), host-side anyref
  beyond what R0 requires (5b), shared-memory GC (threads slice),
  custom page sizes, multi-memory.

## Design

- **Type system**: generalize the EH rec-group machinery
  (`RecProvenance`) to the full grammar. Canonicalization extends the
  EH r4b normalization (bare ≡ singleton rec) to full structural
  canonical form; `ref.test`/`ref.cast` decide on canonical IDs, never
  deep walks. Unresolvable/cyclic references reject conservatively
  (same posture as EH).
- **Heap**: `GcHeap` on the store, `ObjectId(u32)`-indexed object
  table: struct{type_id, fields} | array{type_id, len, elems} |
  i31(packed). Wasm references are `Value::GcRef(ObjectId)` — copies
  freely, no Rc. Never-free: the table only grows (try_reserve
  discipline, refusal = clean trap with untouched state — the memory64
  resource-discipline contract transplanted). Index-stable so 5b swaps
  allocation strategy behind the interface without touching AOT/FFI.
- **GcStats**: `store.gc_stats() → { live_objects, live_bytes,
  allocations_since(snapshot) }`. Byte accounting is EXACT, from a
  documented formula: per object = header (type_id + discriminant +
  length word) + payload by field type (packed i8/i16 per array
  packing; ref = word). Graders assert exact equality — no tolerance
  bands, no flaky articles.
- **AOT v7** for new instruction forms + GcRef operands (handle ABI
  unchanged underneath). FFI: `WasmValue::GcRef` opaque handle.
  **Lowerer mirror rule**: duplicate operand type-checker gains the
  full ref lattice (`pop_ref_in_heap`).

## The article ladder

| Rung | Articles | Proves | Gates |
|---|---|---|---|
| R0 | Official GC corpus (~23 files) | Spec-observable semantics under any collection schedule | 5 |
| R1 | wasm-smith GC-property modules, differential vs spec interpreter | Robustness on author-blind struct/array graphs, cast cascades | 5 |
| R2 | Hand-built A1–A7 (below), graded by stats deltas + ulimit caps | Allocation accounting, refusal discipline, stats exactness | 5 |
| R3 | Real compilers on wasm-GC: OCaml/wasm_of_ocaml, Dart/dart2wasm, Kotlin/Wasm | Collector under real allocation topology | **5b ticket** |
| R4 | GHC wasm32-wasi substrate stress (no wasm-GC usage — RTS GC runs in linear memory) | Substrate robustness: giant modules, call_indirect storms, RTS memcpy storms, table churn | **5b ticket** |

**GC-timing tolerance**: every assertion in every rung holds under any
collection schedule — oracle is the (never-collecting) spec
interpreter, which is exactly slice 5's semantics.
**Refusal-first**: every rung runs under 48/64 MiB caps; exit 101
clean, exit 134 = P1.

GHC placement rationale, recorded honestly: GHC's wasm backend does
not target wasm-GC; compiled Haskell challenges everything *around*
the collector (module size, indirect calls, bulk-memory storms), which
is where slice-5 regressions will hide. An early GHC smoke subset
(parse + run hello-class binaries) may land during 5; full R4 with 5b.

### R2 articles

| # | Article | Stresses | Pass criteria |
|---|---|---|---|
| A1 | ephemeral-churn: 10⁶ short-lived pairs, bounded live window | Allocation volume vs live set | results correct; `allocations_since` exact; caps → clean trap |
| A2 | persistent-build: 10⁷-node list + tree, all live | Live-set accounting | `live_bytes` exactly = formula × count; refusal leaves state untouched |
| A3 | cycle-farm: 10⁵ rotating cycles | Never-free honesty | `live_objects` monotonically grows; becomes 5b's primary convergence probe |
| A4 | i31-boxing-storm: 10⁷ tagged ops vs boxed | i31 must never allocate | stats delta 0 across run |
| A5 | cast-cascade: 10⁶ tests/casts, 30-deep subtype chain | RTT canonical-ID path | results exact; stats delta 0 |
| A6 | new_data/new_elem hybrid + linear-memory pressure | Heap × linear-memory interaction | results correct; combined accounting exact |
| A7 | mixed-ref-matrix: funcref/externref/GcRef in tables/globals/params | Reference-kind separation | results exact under every storage kind |

Articles live in `tests/gc_articles/` (wast-authorable where the
encoder allows; encoder-first discipline stays) + a runner harness
that snapshots stats and applies caps. Articles bound their own loops
(never-free can't reclaim); per-article caps documented.

Deliberately missing: throughput/latency benchmarks — 5b instruments.

## Units

| Unit | Content | Contract |
|---|---|---|
| U1 | Type grammar: rec/sub/canonicalization in binary+validate; type-canon/equivalence/rec/type.wast green at validation level | `gc-unit1-contract.md` |
| U2 | Heap + object model: GcHeap, ObjectId, GcStats (exact formula), struct/array/i31 instructions, AOT v7 | `gc-unit2-contract.md` |
| U3 | Casts: RTTs on canonical IDs, ref.test/cast, br_on_cast pair, conversions; R0 green end-to-end | `gc-unit3-contract.md` |
| U4 | Linkage/surface: GC types across imports/exports/globals/tables, lowerer lattice mirror, FFI handle, R1 smith wiring | `gc-unit4-contract.md` |
| U5 | Articles A1–A7 + harness + caps in CI + corpus grind to zero | `gc-unit5-contract.md` |

## 5b entry criteria

R3 payloads curated and frozen (binaries + expected traces); R0–R2
green. Collector converges when: A3 flips accumulate→reclaim (stats
flat), RSS flat on A1/A2 at 10⁷ scale, every R0–R2 guarantee holds.

## Risks

- **(a) Canonicalization is the silent killer** (EH burned two review
  rounds there). Mitigation: pin canonical-form semantics from the
  reference *before* the U1 contract; valid.ml discipline.
- **(b) wasm-smith GC support**: verify our lockfile's smith generates
  GC modules; else R1 falls back to spec-interpreter differential on
  hand-mutated corpus modules.
- **(c) live_bytes flakiness**: killed by design — exact formula, exact
  equality, no tolerances.
- **(d) Unbounded never-free runs**: articles self-bound; caps
  documented per article.
