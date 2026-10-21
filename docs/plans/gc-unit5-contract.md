# GC Slice — Unit 5 Contract: R0 Conformance + Articles

Status: APPROVED (operator directed continuation; orchestrator-authored
from measured evidence). Unit 5 of 5. Branch `feature/gc-slice`.

## Context

U1–U4 are converged (review loops closed through r4). The GC surface is
complete: type grammar, heap/object model, cast machinery, linking seam,
const-exprs, runner assertion infrastructure, FFI, smith campaign. What
remains is proof: the official R0 corpus green end-to-end, and the R2
articles + refusal harness in CI.

**Measured baseline (orchestrator probe, 2026-10-17)**: all 22 R0 files
wired into the suite with GC deferrals lifted → `files=201
directives=56321 assertions=54274 failures=40`. Failure census:

- 30 × `ref_cast.wast` + `type-subtyping.wast` `CastFailure`-family:
  `assert_trap` sees `RuntimeErrorKind::CastFailure` and reports
  "got non-trap error". Not a semantics bug — a harness taxonomy gap.
- 3 × `ref_cast.wast` expected-message mismatches overlapping the above
  census (exact-match comparison; the pin uses prefix matching).
- 7 × `type-subtyping.wast` **real semantic failures**:
  `assert_return` expecting `Core(I32(1))` across 1/2/3/8-value shapes —
  the multi-module distinct-ID subtype family.

Post-U5 target: `files=200 deferred=2` (remaining deferrals:
`skip-stack-guard-page.wast`, `names.wast`) **failures=0**.

## Stage 1 — R0 grind

### 1.1 Vendor the corpus

Copy verbatim from the pin (`/tmp/wasm-spec-pin/test/core/gc/`) into
`crates/baedeker-testdata/spec/wast-official/`: the 17 gc/ files
(array, array_copy, array_fill, array_init_data, array_init_elem,
array_new_data, array_new_elem, binary-gc, br_on_cast,
br_on_cast_fail, extern, i31, ref_cast, ref_eq, ref_test, struct,
type-subtyping). The 5 GC-era files (ref_null, type, type-rec,
type-canon, type-equivalence) are already vendored — remove their
`DEFERRED_FILES` entries. Update COMMIT.txt provenance if the crate
records it.

### 1.2 Runner: trap-message prefix matching (pin-verbatim)

`interpreter/script/runner.ml:498-502`:

```ocaml
let assert_message at name msg re =
  if
    String.length msg < String.length re ||
    String.sub msg 0 (String.length re) <> re
  then begin
    print_endline ("Result: \"" ^ msg ^ "\"");
    print_endline ("Expect: \"" ^ re ^ "\"");
    Assert.error at ("wrong " ^ name ^ " error")
```

i.e. the actual message must START WITH the expected string (the
expected string is a prefix; the actual may be longer). Replace the
exact-equality comparison at BOTH assert_trap sites
(`runtime_official.rs` ~line 570 module-level, ~line 922 invoke-level)
with prefix matching. Add tests for both directions (longer actual
passes; divergent actual fails).

### 1.3 Runner: CastFailure as trap message (pin-verbatim)

`interpreter/exec/eval.ml:657`:

```ocaml
vs', [Trapping ("cast failure, expected " ^ ...string_of_heap_type...)]
```

`ref.cast` failure IS a runtime trap whose message starts with
`"cast failure, expected "`. Keep `RuntimeErrorKind::CastFailure` as a
distinct embedder-facing kind (do NOT change the public taxonomy);
in the runner's assert_trap paths, map it to its Display string
(`"cast failure, expected {expected:?}"` — verify the rendered prefix
is exactly `cast failure, expected`) and prefix-match per 1.2. Note our
`Display` renders the expected type with Rust Debug formatting — the
prefix rule only requires `cast failure` from the wast files, which
satisfies every corpus expectation; do not invent OCaml-format parity.

### 1.4 The 7 real type-subtyping failures

Reproduce each (the census shows 1×1-value, 2×2-value, 1×3-value,
1×8-value shapes plus 6 CastFailure-family — after 1.2/1.3, the
remaining 7 are assert_return value mismatches). Hypothesis (verify,
don't assume): multi-module cases where an imported func's typeuse
provides a DISTINCT-ID subtype of the declared type — the linking seam's
`defined_type_matches` (canonical-identity OR provided-reaches-declared)
must accept, and the distinct-ID must flow through the runtime cast
path via the object's own heap (U3 foreign matching). Root-cause per
failing assertion against the pin: quote the governing interpreter
lines (`match_deftype` in `runtime/*.ml` or the subtype check) in the
fix's rationale. If the hypothesis is wrong, report the actual root
cause in the final message — rule-7 ambiguity protocol applies.

### 1.5 parse_init_expr decoder correctness

`binary/datasec.rs` + `binary/elemsec.rs` `parse_init_expr` (3/400 smith
decode failures, bound ≤8 bytes — the U4 probe's documented decoder
bug): the current scan is a 0x0b-hunt that mis-sizes GC-era const
exprs (e.g. `array.new_fixed` with many operands, `struct.new` with
wide fields). Replace with the real const-expr decoder: reuse the
code-section instruction framing the crate already has (proper opcode
+ immediate consumption), fallible throughout (no panics; malformed →
`DecodeError`, never a wrong-length scan). Regression: the 3 failing
smith seeds (recover seed numbers from the campaign's decode-failure
log or regenerate by running the campaign — seeds are deterministic:
0..400 ∪ {4097, 10158, 10334}) now decode; plus a hand-built
array.new_fixed-length-100 const expr in a data/elem offset where
plausible.

## Stage 2 — Articles A1–A8 + refusal harness + CI caps

Per design doc (2026-10-14-gc-slice-design.md R2 table + article
ladder): `tests/gc_articles/` — eight hand-built articles, wast-authorable
where the encoder allows (wast text compiled by our own wat→wasm? NO —
we have no text parser in-crate; author articles as either (a) binary
fixtures built programmatically via the existing test wat encoder
(`wast` crate dev-dependency, as runtime_official does), or (b) direct
IR/store API tests where the assertion targets the harness not the
corpus). Grading = `GcStats` logical accounting + structured refusal:

- A1 ephemeral-churn (10⁶ short-lived pairs): schedule-independent
  results; `allocations_since` exact; capped run → structured
  `ResourceLimitExceeded` (trap-shaped, state untouched).
- A2a persistent-build accounting (provisioned, no caps): `live_bytes`
  exactly = formula × count. Document the formula inline.
- A2b persistent-build refusal under 48/64 MiB caps: structured
  resource trap; stats snapshot shows state untouched (compare full
  GcStats before/after the refused allocation — identical except
  `refusals_since` style counters if present).
- A3 cycle-farm (10⁵ rotating cycles): results correct; slice-5
  honesty: `live_objects` monotonic non-decreasing.
- A4 i31-storm (10⁷ ref.i31/i31.get vs boxed): stats delta exactly 0
  across the i31 phase.
- A5 cast-cascade (10⁶, 30-deep declared subtype chain, mixed outcomes
  incl. distinct-ID subtype successes): results exact; stats delta 0.
- A6 new_data/new_elem hybrid + linear-memory pressure: results
  correct; GC logical bytes and memory pages accounted separately.
- A7 mixed-ref-matrix: funcref/externref/GcRef in tables/globals/params
  + conversion round-trip storm (any.convert_extern/extern.convert_any
  ×10⁶ over mixed identities): results exact; stats delta 0.
- A8 v128-accounting: v128-field structs (8+16) and v128 arrays
  (12+3×16) built/rebuilt in exact counts: `live_bytes` exactly =
  formula × count; `allocations_since` exact.

Runner harness: child-process execution for the capped articles
(structured expectations; the refusing engine surfaces
`ResourceLimitExceeded` as a clean Rust error — exit 0 with a marker,
NEVER abort); snapshots for state-untouched checks. Per-article caps
documented at each test. CI: a dedicated job (or a feature-gated test
target) running the articles under caps — reuse the existing CI job
structure; keep total suite wall-time bounded (shrink article counts in
CI via env knob if the full 10⁶/10⁷ magnitudes blow the budget —
document the CI vs local magnitudes; the FORMULAS must hold at both).

## Constraints

- `no_std` + alloc in crate proper; harness/tests may use std.
- MSRV 1.85. fmt + clippy clean (workspace AND `-p baedeker-core
  --features aot --all-targets`, both `-D warnings`).
- AOT: no IR changes expected (decoder fix is binary-level; runner
  fixes are harness-level) — AOT stays **v11**; bump ONLY if a fix
  touches IR/serialization, and say so in the final message.
- GcStats is LOGICAL accounting (U2 decision) — article formulas grade
  logical bytes/objects, never process RSS.
- No new accepted residuals without disclosure; existing ledger stands.
- Vendored files VERBATIM (byte-identical to the pin).

## Verification matrix (all must appear in the final message)

1. `cargo fmt -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings` and
   `cargo clippy -p baedeker-core --features aot --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `cargo test -p baedeker-core --features aot`
5. Official suite: `files=200 deferred=2 directives=… assertions=… failures=0`
   (directives/assertions totals will shift — report verbatim; the
   invariant is failures=0 with the 22 R0 files IN and 2 non-GC
   deferrals remaining)
6. EH suites green; smith campaign totals unchanged (403/400/400/400
   + 3 decode failures → 3→0 after 1.5; report new totals)
7. Articles: 8/8 green locally at full magnitude; CI-configured job
   green at documented magnitudes

## Ambiguity protocol (rule 7)

Any contract statement that conflicts with pin evidence or repository
reality: STOP that sub-task, record the conflict with file:line +
pin quote, continue the rest. The 1.4 hypothesis is explicitly a
hypothesis — disprove freely with evidence. Do not git commit/push;
files only.
