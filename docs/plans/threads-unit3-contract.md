# Threads — Unit 3 contract: corpus vendoring + smith + closeout (r1)

Branch `feature/threads-slice`. No git commands by implementer. Scope: run
the threads corpus green, extend smith, close the slice. Engine semantics
are DONE (U1 surface, U2 execution) — this unit touches the harness,
testdata, smith config, and docs only unless a corpus case exposes an
engine gap (then: fix the engine per the pin, tests first, record it;
U2's recorded determinations — effective-address alignment, single-agent
wait/notify — remain AUTHORITATIVE over any broader instruction).

Spec pin: `WebAssembly/threads@main-legacy` `33b2dcd6` at
`/tmp/wasm-threads-pin` (COMMIT.txt records the full sha).

**Revision note (r1)**: round-0's byte-patch transform was built on a
false premise — `wast-240` natively supports classic shared memories
(`MemoryType { pub shared: bool }`, `types.rs:649+`; parser consumes the
trailing `shared`; `binary.rs:515-524` forwards it to the encoder). The
premise was disproved by the contract review (r1 findings 1–2, 4–6;
verified by the orchestrator). The patcher is DELETED. This revision also
pins the delta manifest (finding 3), the accounting (finding 5), and the
smith mechanism (finding 2).

## Corpus reality (verified, r1 figures)

- `atomic.wast`: single-agent throughout (own shared memory at :4;
  "unshared memory is OK" at :440; 45 assert_trap incl. the unaligned
  family at :392+). Runs as-is; NO syntax transform needed.
- `atomic_wait_notify.wast`: single-agent directives at :1-62; exactly
  TWO top-level `(thread …)` blocks (:64+) and TWO `(wait …)`
  directives; no directive outside them references their internals.
  Pre-parse strip of those four balanced blocks (string/comment-aware
  paren balancing); the summary prints `threads_directives_skipped=4`.
- `thread.wast`: one module definition (:1-3) and ZERO single-agent
  assertions; all behavioral coverage is multi-agent (`thread`/`wait`,
  `(either …)` matching). **Disposition: DEFERRED**, reason
  "multi-agent behavioral coverage (thread/wait directives) —
  single-agent engine", recorded in the deferral ledger beside
  `skip-stack-guard-page.wast`.
- Delta manifest (directive-level, comments stripped, whitespace
  normalized — pinned by the r1 review):
  - `binary.wast`: ONE threads-related fork-only directive — the
    shared-table rejection at :586-595. Vendor as
    `binary-threads-delta.wast`.
  - `memory.wast`: THREE — :9, :10 (shared grow semantics) and :12
    (`(assert_invalid (module (memory 1 shared)) "shared memory must
    have maximum")`). Vendor as `memory-threads-delta.wast`. The :12
    negative directive is a REGRESSION for the native path: wast-240
    encodes flags `0x02`; our decoder refuses with the exact corpus
    string; `assert_invalid` is satisfied by decode refusal
    (runner:652-666).
  - `linking.wast`: **EMPTY threads delta** — zero shared-memory or
    shared/unshared link cases in the fork's copy (the fork's linking
    differences are older trap-wording/instantiation drift). Record the
    empty delta; add no linking fixtures beyond locally-authored tests,
    clearly labeled as such.
- No shared inline-data memory forms, no shared registry
  (`module definition/instance`) forms occur in these files.

## Deliverables (TDD where code is written)

1. Harness: the four-block strip for `atomic_wait_notify.wast`
   (unit-tested: balanced strings/comments, count, and that stripping is
   a no-op for the other files). NO shared-syntax transform — native
   wast encoding, with focused regressions: defined shared memory,
   imported shared memory, shared-no-max refusal (the :12 directive),
   each asserting decode → `MemType.limits.shared` / the exact error.
2. Vendor `atomic.wast`, `atomic_wait_notify.wast` (strip applies),
   `thread.wast` (deferred), `binary-threads-delta.wast`,
   `memory-threads-delta.wast` (extracted EXACTLY per the pinned
   manifest above). Deferral ledger updated. **Target accounting:
   204 runnable + 3 deferred** (200+2 corpus, +2 deltas; linking delta
   empty). Full-count assertions run ONLY when
   `BAEDEKER_OFFICIAL_FILTER` is unset/empty (the filter legitimately
   narrows runs); `failures=0` asserted unconditionally.
3. Smith: in BOTH configs (`tests/gc_u4_smith.rs` AND
   `fuzz/fuzz_targets/smith_module.rs`): `threads_enabled = true` AND
   add `InstructionKind::MemoryInt` to the allowlist (every atomic is
   MemoryInt-bucketed — `code_builder.rs:597-664`; threads_enabled
   alone generates ZERO atomics under the current allowlist). Adding
   MemoryInt also admits plain loads/stores: acceptable — fully
   supported, and it broadens coverage legitimately. Acceptance
   (decoded == validated == lowered == generated) must remain TOTAL.
   Presence assertion: at least one DECODED atomic instruction (walk
   decoded modules for `Instr::Atomic`, NOT raw-byte 0xfe — immediates
   contain 0xfe incidentally) on pinned deterministic seeds; if the
   pinned seed set yields none, extend seeds/config until presence is
   deterministic and record what it took. Execution stays OUT of smith
   gates (no wait-hang risk).
4. Closeout: CHANGELOG section; book/roadmap threads-slice entry; PR
   body updated with final numbers.

## Gates

- Official suite (unfiltered): `files=204 deferred=3 … failures=0`;
  the pre-existing 200 runnable files unregressed; the two
  pre-existing deferrals remain. CI's smith fuzz job covers the default
  config; the gc-smith test config is verified by the workspace suite.
- Every single-agent directive in the vendored corpus passes (wait
  mismatch→1, epsilon timeout→2, notify→0 — U2 semantics; the corpus
  was written for exactly these).
- Smith total acceptance with atomics present; workspace; clippy ×2;
  fmt; AOT v12 unchanged.

## Ambiguity report rule (rule 7)

Any divergence between corpus expectation and engine behavior: STOP,
quote both (corpus line + pin source), report — no silent fixes. Any
delta-content judgment beyond the pinned manifest: record in closure
notes rather than guessing.
