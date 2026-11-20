# Slice 6 — threads (classic: shared memory + atomics)

**Provenance correction (operator-approved 2026-10-21):** threads is NOT part
of WASM 3.0 — the pinned spec (`dfa3f32a`) and the official `wasm-3.0` ref
contain zero threads content. With slice 5 merged, WASM 3.0 as pinned is
complete. This slice implements the **classic threads proposal** (shared
linear memory + sequentially-consistent atomics + wait/notify + fence): the
frozen, browser-shipped state of `WebAssembly/threads` at branch
**`main-legacy`, commit `33b2dcd6` (2025-09-14)**. Shared-everything threads
(`main` @ `cc535ada`: `thread.spawn`, shared GC, ordering params on all ops)
is explicitly out of scope and stays gated behind the GC-5b collector work.

Spec pin: `WebAssembly/threads@main-legacy`, `33b2dcd6…` (full sha to be
recorded in COMMIT.txt when vendored). Reference corpus delta vs the 3.0 pin:
**3 added files** — `atomic.wast`, `atomic_wait_notify.wast`, `thread.wast` —
plus shared-memory deltas in overlapping core files (`binary.wast`,
`memory.wast`, `linking.wast` at minimum; exact per-file directive diff is a
U3 task). Expected post-slice: 203 files, 0 failures.

## Scope

- **Shared memory types**: `limits` shared flag (binary flag bit 0x02/0x03
  encoding per the proposal); shared memories REQUIRE a declared `max`;
  import/export/link matching requires shared≡shared (a shared memory
  never links against a non-shared one, and vice versa).
- **Atomics instruction family (`0xFE` prefix)**: loads/stores at
  i8/i16/i32/i64 widths, RMW (add/sub/and/or/xor/xchg) and cmpxchg at
  i32/i64, `memory.atomic.wait32/64`, `memory.atomic.notify`, `memory.atomic.fence`.
- **Alignment**: natural alignment is REQUIRED for every atomic — it is a
  validation error (not a trap) to encode `align` below the access width's
  natural alignment.
- **Execution (single-agent semantics, per the proposal)**: every atomic
  access is sequentially consistent; in a single-agent store an atomic
  access is observationally its plain access with the alignment/OOB rules of
  the memory instruction family. `fence` is a no-op. `wait`: value mismatch
  → 1 (loaded value differed); value match with no other agents → the agent
  is woken only by timeout → 2 after `timeout` ns. `notify` → 0 (no
  waiters ever). These are deterministic in our interpreter.
- **Host/embedder surface**: shared memories are shareable across instances
  in a link group (imported memory reuse — our existing mechanism); the
  `shared` flag is carried on `MemType` so embedders can allocate and inject
  shared memories. Cross-thread execution is an embedder concern (wasi-threads
  shape later); the engine remains single-agent.

## Out of scope

- `thread.spawn`, shared tables/globals/GC heaps, per-op ordering params
  (shared-everything `main`).
- The `wait` blocking-with-wakers path (no other agents exist to wake us;
  timeout-only is the complete single-agent semantics).
- Making `skip-stack-guard-page.wast` runnable (needs ≥2 agents; stays
  deferred), unless `thread.wast` proves likewise runner-dependent — its
  disposition is decided in U3 with a recorded reason either way.

## Units

- **U1 — surface**: `MemType`/`Limits` shared flag end-to-end (binary ↔
  types ↔ validate ↔ link matching, max-required-for-shared, grow rules for
  shared); `0xFE` family decode (all opcodes, memargs, i32/i64 operand
  shapes); natural-alignment validation; malformed-binary corpus from the
  proposal's `binary.wast` deltas. Contract: `docs/plans/threads-unit1-contract.md`.
- **U2 — execution**: interpreter paths for every atomic access (ordered
  plain-op semantics, OOB/misalignment traps per memory family), cmpxchg
  equality check, `fence` no-op, `wait32/64`/`notify` single-agent returns;
  official-harness support for shared-memory directives (`memory.wast`/
  `linking.wast` deltas, spectest shared-memory provider if the corpus
  requires one). Contract: `docs/plans/threads-unit2-contract.md`.
- **U3 — R0 + smith + closeout**: vendor the corpus delta from the pin
  (`atomic.wast`, `atomic_wait_notify.wast`, `thread.wast`, per-file shared
  deltas), grind to zero failures; smith extension generating `0xFE` ops
  against shared memories (decode/validate/lower acceptance); book/roadmap
  updates; deferral ledger re-issued. Contract: `docs/plans/threads-unit3-contract.md`.

## Gates

- R0 = vendored threads corpus green: **203 files, 0 failures** (or the
  recorded-disposition equivalent if `thread.wast` is runner-dependent).
- Existing 200-file official suite stays green (no regressions from the
  `Limits` shape change — it is a public type; semver-clean additive change).
- `cargo fmt` + clippy `-D warnings` both feature configs; workspace suites;
  smith acceptance unchanged-or-better; AOT surface untouched (v11) unless a
  `0xFE` lowering is required for smith's lower stage (decided in U3).

## Risks

- `Limits` is a public, widely-used type — adding `shared` touches many
  construction sites; mitigate with `Default`/builder discipline per
  ia-coding-standards (review focus in U1).
- `atomic.wast` includes multi-agent-shaped directives (e.g. `assert_trap` on
  wait over non-shared memory, and alignment traps) — the harness mapping is
  a U2/U3 review focus; single-agent determinism must hold for every
  assertion we run.
- The threads repo's core files are a full fork of the suite; vendoring must
  be a delta against OUR pin, not a wholesale copy (the fork may be older
  than `dfa3f32a` in unrelated files).
