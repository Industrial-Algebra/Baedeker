# Threads — Unit 2 contract: execution semantics (single-agent atomics)

Branch `feature/threads-slice`. No git commands by implementer (if dispatched;
in-session fallback follows the standing playbook). Scope: interpreter
execution of the U1 surface. No corpus vendoring, no harness/spectest changes
(U3).

Spec pin: `WebAssembly/threads@main-legacy` `33b2dcd6` at `/tmp/wasm-threads-pin`.
All quotes below are from that clone (eval.ml line numbers from the pin).

## Semantics pinned from the reference (`interpreter/exec/eval.ml`)

**Trap strings (exact, corpus-matched by prefix)**:
- `check_align` (:140-142): `Trap.error at "unaligned atomic memory access"`
  — corpus expects `"unaligned atomic"` (atomic.wast:394+); our harness
  matches by prefix (actual starts with expected), so the reference's full
  string is correct as-is.
- `check_shared` (:144-147): `"expected shared memory"` — wait REQUIRES a
  shared memory, even when it would not block (corpus:
  atomic_wait_notify.wast:58-59 "traps on unshared memory even if it
  wouldn't block").

**Alignment** (:140): `check_align addr ty sz` against the natural width
(`runtime/memory.ml:23-28` `is_aligned`: full widths 4/8 bytes;
`8_u/16_u/32_u` narrow forms 1/2/4 bytes). This is a runtime check in
ADDITION to U1's validation `align ≤ natural` (the recorded ambiguity:
validation allows align below natural; execution then traps unless the
address is naturally aligned).

**RULE-7 RESOLUTION (effective address)**: the spec TEXT is normative on
the EFFECTIVE address — `_exec-atomic.load_` step: `ea = i +
memarg.OFFSET`, then "If `ea` modulo N/8 is not equal to 0, then:
trap" (document/core/exec/instructions.rst). The pin's interpreter checks
the BASE address only (eval.ml:275 passes `addr` before `offset` is
applied inside `load_value`) — an interpreter bug at the pin, invisible
to the corpus (neither atomic.wast nor atomic_wait_notify.wast uses
`offset=` at all, verified). Implement EFFECTIVE-address alignment
(base + offset); code comment cites both sources.

**Trap priority (pinned by corpus case pairing)**: for `wait`:
unaligned (atomic.wast-analog :37) beats OOB (:30) beats nothing; and
check_shared sits BETWEEN align and the load: an aligned wait on an
UNSHARED, OOB address traps `"expected shared memory"` (eval.ml:329-335
order: `check_align`, then `check_shared`, then `Memory.load_value`).
For loads/stores/rmw/cmpxchg/notify: `check_align` then the access
(eval.ml:271-357) — no shared requirement anywhere else ("unshared memory
is OK", atomic.wast:440).

**Atomic accesses are plain accesses plus the alignment check** (single
agent; seqcst is trivially preserved):
- load (:271-282): plain load; narrow forms zero-extend (`ZX`).
- store (:283-294): plain store; narrow forms truncate.
- rmw (:295-311): load old (narrow: zero-extend to FULL width), compute
  `eval_rmwop(rmwop, old, operand)` at FULL width (wrapping add/sub,
  bitand/or/xor; xchg = operand), store (narrow: truncate), push OLD value.
- cmpxchg (:312-328): load old (narrow: ZX); `if old = expected` store
  replacement (narrow: truncate); push OLD value in every case.
- fence (:359-360): pure no-op.

**wait32/wait64** (:329-345): `assert sz = None` (no narrow forms exist —
decoding already guarantees this). Order: check_align → check_shared →
load (OOB traps "out of bounds memory access") → compare with expected:
- mismatch → `1`;
- match → timeout handling: `timeout_epsilon = 1_000_000` ns (:38):
  `0 ≤ timeout < 1ms` → `2` immediately; other timeouts → the reference
  SUSPENDS the agent (multi-agent engine).
- **Single-agent determination (documented in code)**: with no second
  agent, no wake source can exist; every matching wait deterministically
  ends in timeout. We return `2` WITHOUT sleeping for ALL timeouts,
  including negative (infinite) ones — observationally identical to the
  reference for a store with one agent, and the only non-hanging choice.
  No corpus case exercises a ≥1ms or negative wait (it would hang a
  single-threaded runner); this rule makes that class unrepresentable
  rather than hangable.

**notify** (:347-357): check_align → load (OOB trap effect ONLY — the value
is discarded) → if count = 0 return 0, else NotifyAction. No shared check,
 no alignment beyond the entry check. **Single-agent determination**: no
waiters can exist → return `0` for every count. (The count-0 short-circuit
and the general case collapse to the same result.)

**Spectest/harness**: untouched in U2 (U3). Shared memories in tests are
hand-built bytes (the U1 module-builder helpers).

## Deliverables (TDD, RED first)

1. Lowering: `Instr::Atomic(op)` lowers from the U1 `UnsupportedInstr`
   posture to executable register ops (design freedom: one `RegOp::Atomic`
   carrying an exec-shaped operand enum, or per-op variants — follow the
   house `RegOp` style; document the choice in the closure note).
   `Instr::AtomicFence` lowers to a no-op op (or is elided at lower time
   — document which).
2. Interpreter: every access per the semantics above; the alignment check
   precedes any bounds check for ALL atomic ops; wait adds the shared
   check between align and bounds.
3. Traps: `"unaligned atomic memory access"` and `"expected shared memory"`
   via the house trap channel (corpus-compatible wast_message mapping).
4. `memory.grow` on shared memories grows normally (no special case; pin
   with a test).
5. Tests (`tests/threads_u2_exec.rs`): per-op value/return checks incl.
   narrow ZX/truncate round-trips; old-value returns for rmw/cmpxchg;
   cmpxchg success and failure; unaligned traps for every family member
   (parameterized); wait mismatch→1 / epsilon-timeouts→2 / negative→2
   (documented determination); notify→0 incl. count=0; OOB wait traps;
   trap-priority pins (unaligned-beats-OOB; shared-beats-OOB);
   fence executes as no-op; shared grow works.

## Gates

- New tests RED before implementation, GREEN after; U1 suites, workspace,
  official 200-file suite, smith — all unregressed. clippy ×2 configs, fmt.
- No new allocation on any atomic path (ops touch existing memory).

## Ambiguity report rule (rule 7)

Any divergence between this contract and the pin sources: STOP, quote both,
report. In particular: if `Memory.is_aligned` at the pin turns out to check
something other than effective-address mod width, that is a P1-class
report, not a judgment call.

## Closure notes (U2 implementation)

Delivered as specified. Decisions the contract left open, recorded here as
its closure note:

- **Lowering shape**: six per-op `RegOp` variants (`AtomicLoad`,
  `AtomicStore`, `AtomicRmw`, `AtomicCmpxchg`, `AtomicWait`, `AtomicNotify`),
  following the house `Load`/`Store`/`V128LoadLane` style rather than one
  packed `Atomic` op. They carry `AtomicWidth`/`AtomicRmw` from
  `binary::instr` (serde derives added to both so the AOT roundtrip covers
  the new terms); `AtomicWidth::byte_width()` is the byte form of the
  existing `natural_align_log2()`.
- **`atomic.fence` is ELIDED at lower time**, not lowered to a no-op op: it
  has no operands, no result and no runtime effect (`eval.ml:359-360`), so a
  dispatch entry would buy nothing.
- **Alignment** is checked on the effective address
  (`base.wrapping_add(memarg.offset)`), before any bounds check, via a code
  comment citing both the normative text and the pin's base-only check.
- **Single-agent determinations** are documented at the trap site: waiting
  returns 2 for every matching wait (all timeouts, including negative),
  notify returns 0 for every count.
- **AOT format version** 11 -> 12 (six new IR terms), version-history comment
  extended.
- U1 latent defect fixed on the way: `validate_atomic`'s
  `debug_assert!(row < ATOMIC_RMW_NAMES.len())` fired for `rmw.xchg` in debug
  builds (the out-of-range row is the documented xchg encoding). Removed.
- U1's opcode-table comment calls the 66-entry list "67 entries"; the 67th
  family member is `atomic.fence`. The parameterized unaligned test covers
  the 66 memory ops (asserting its own count) and fence has its own no-op
  coverage.
