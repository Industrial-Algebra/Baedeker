# Threads — Unit 1 contract: surface (shared memory types + 0xFE decode + validation)

Branch `feature/threads-slice`. No git commands by implementer. Scope: the
type/binary/validation surface for classic threads. No interpreter execution
(U2), no corpus vendoring (U3).

Spec pin: `WebAssembly/threads@main-legacy`, `33b2dcd6a933f5e3c4b860fbbc4d89077256a2aa`
(cloned at `/tmp/wasm-threads-pin`, COMMIT.txt records the sha). All quotes
below are from that clone — trust them over intuition. Where the pin's .rst
and its reference interpreter/corpus disagree, the INTERPRETER + CORPUS win
(recorded ambiguity below).

## Semantics pinned from the reference

**Memory type binary** (`document/core/binary/types.rst`, `_binary-limits_` +
`_binary-memtype_`): the limits flags byte carries the shared bit —

| Flags | Fields | shared |
|---|---|---|
| `0x00` | min | 0 |
| `0x01` | min max | 0 |
| `0x02` | min | 1 |
| `0x03` | min max | 1 |

`_binary-memtype_` production: `lim,1 ⇒ lim SHARED (iff lim.MAX ≠ ε)` — a
shared memory WITHOUT a maximum is a **decode error** at memtype level
(because flags `0x02` carries no max). Cross-check: `interpreter/valid/valid.ml:375-379`
(`shared memory must have maximum`) — validation ALSO enforces it, so both a
decoder rule (flags 0x02 in a memtype) and a validator rule (invariant
`shared ⇒ max.is_some()`) exist in the reference. Implement both layers.

**Table types stay unshared** at this pin: tabletype reuses the plain limits
(`0x00/0x01`) — flags `0x02/0x03` in a TABLE type are malformed. Verify
against the pin's `_binary-tabletype_` + `decode.ml` while implementing; if
the fork's tabletype accepts the shared bit, STOP and report (rule 7).

**Atomic instruction family** (`document/core/binary/instructions.rst`,
`_binary-instr-atomic-memory_`; opcode table cross-checked against
`wast-240.0.0/src/core/expr.rs:791+` which encodes this exact classic set):

- `0xFE 0x00` `memory.atomic.notify` — memarg, `[i32 i32] → [i32]`
- `0xFE 0x01` `memory.atomic.wait32` — memarg, `[i32 i32 i64] → [i32]`
- `0xFE 0x02` `memory.atomic.wait64` — memarg, `[i32 i64 i64] → [i32]`
- `0xFE 0x03` `memory.atomic.fence` — **one reserved byte `0x00`** (memarg-free but not immediate-free; verify in decode.ml / wast crate)
- `0xFE 0x10`–`0x17` atomic loads (i32/i64 full + `8_u/16_u/32_u` packed narrow)
- `0xFE 0x18`–`0x1E` atomic stores (i32/i64 full + narrow 8/16/32)
- `0xFE 0x1F`–`0x2E` atomic RMW add/and/cmpxchg/or/sub/xchg/xor (i32 + i64; cmpxchg keeps both old-value result and two same-type operands)
- Extract the EXACT byte-per-instruction table from `wast-240` `expr.rs` and
  the pin's instructions.rst while implementing; the list above is the shape,
  not a substitute for the table.

**Validation** (`interpreter/valid/valid.ml:151-160` `check_memop` shared by
ALL atomic cases at :295-335): `C.mems[0]` defined; `align ≤ natural`
("alignment must not be larger than natural") — the SAME rule as plain memory
ops; NO shared-memory requirement on the context (atomics validate against
any memory at this pin). Operand shapes per case (valid.ml):
load `[i32] → [t]`; store `[i32 t] → []`; rmw `[i32 t] → [t]`;
cmpxchg `[i32 t t] → [t]`; notify/wait/fence as listed above.

**RECORDED AMBIGUITY (decided, corpus-grounded)**: the pin's
`valid/instructions.rst` `_valid-atomic-load_` says align must EQUAL natural
at validation, but its own interpreter enforces `≤` and the corpus
(`test/core/atomic.wast:392+`) makes unaligned access a RUNTIME TRAP
"unaligned atomic" via `assert_trap` on modules that VALIDATE fine.
Implement: validation `align ≤ natural` (check_memop rule); runtime
alignment check belongs to U2. Cite both sources in the code comment.

**Runtime trap message (U2's to implement, pinned here for the message
string)**: `"unaligned atomic"` — exact corpus string, `atomic.wast:394+`.

**Link/matching**: import of a memory requires shared≡shared equality
(`exec/modules.rst` matching is `limits equal ∧ shared equal`; corpus:
`linking.wast` delta cases + `atomic.wast`'s shared import modules). Grow of
a shared memory (`memory.grow` on shared) validates/executes the same as
unshared at this pin (max is mandatory; corpus memory.wast delta decides any
quirk — report if it disagrees).

## Deliverables (TDD, RED first, one commit per red-green pair where natural)

1. `types.rs`: `Limits`/`MemType` shared flag — additive, public-API clean
   (all existing construction sites compile unchanged via
   `shared: false`-defaulting constructor discipline per ia-coding-standards;
   audit every `MemType {` literal).
2. `binary/`: memtype decode per the flags table (both error layers); 0xFE
   family decode with the full opcode table, memargs (incl. multi-memory
   memarg encoding at our pin's rules), fence reserved byte; malformed
   corpus cases from the proposal's `binary.wast` delta (extract the
   atomic/shared-specific malformed cases into unit tests).
3. `validate/`: the rules above; malformed/invalid tests for: shared without
   max (both layers), shared tabletype, align > natural per atomic op shape,
   operand shapes, fence reserved byte ≠ 0x00.
4. Link matching: shared≡shared for memory imports/exports (spectest
   provider unaffected — spectest declares no shared memories at this pin;
   verify).
5. No interpreter/`lower` execution changes: U1 instructions decode + lower
   to an `Unsupported`-style refusal or lower normally IF that is cheaper —
   pick one, document it in the contract closure note, keep smith/official
   suites green (nothing new reaches execution in U1's tests).

## Gates

- All new tests RED before implementation, GREEN after; existing workspace
  suites, official 200-file suite, smith acceptance unchanged.
- `cargo fmt` + clippy `--all-targets -- -D warnings` (both feature configs).
- Corpus cross-check: every `assert_malformed`/`assert_invalid` in
  `binary.wast`'s atomic/shared delta that belongs to U1's surface passes
  (the delta extraction itself may land in U3; U1's obligation is unit-test
  coverage of each RULE, demonstrated on hand-built bytes).

## Ambiguity report rule (rule 7)

Any divergence between pin sources (rst vs interpreter vs corpus vs
`wast-240`), or between this contract and the pin: STOP, quote both sources,
report. Do not adjudicate silently.
