# Phase 2 Control Flow — Handoff Document

**Branch:** `feature/phase-2-part-5-control-flow-2`
**Date:** June 2026
**Status:** All Checkpoint 3 control flow complete: block/br/br_if/br_table/if/else/loop/select with WAST coverage

## What's in this branch

### Terminators (`RegTerm`)
| Variant | Status |
|---------|--------|
| `Fallthrough` | ✅ Working |
| `Br { target_block, values }` | ✅ Working (incl. loop back-edges) |
| `BrIf { cond, target_block, values }` | ✅ Working |
| `BrTable { index, targets, default, values }` | ✅ Working |
| `IfFork { cond, then_block, else_block }` | ✅ Working |
| `Return { values }` | ✅ Working (no longer stops lowering) |
| `Trap` | ✅ Working (`unreachable`) |

### Lowering handlers
| Instruction | Status |
|-------------|--------|
| `block`, `end`, `br`, `br_if`, `return` | ✅ Working |
| `if`, `else` | ✅ IfFork with back-patched then/else edges |
| `loop` | ✅ Back-edge `br`/`br_if` to header resolved at lowering time |
| `unreachable`, `nop` | ✅ |
| `br_table` | ✅ Multi-slot back-patching; loop headers resolve immediately |
| `select` / typed select | ✅ `RegOp::Select` |
| `call` etc. | ⬜ Phase 2 C4 |

### Key mechanisms (this branch)

**Polymorphic stack discipline.** `LabelFrame` now carries `height` (entry
stack height) and `unreachable`. Pops at the frame boundary in unreachable
code synthesize *undef registers* of the expected type instead of erroring
(mirrors the spec validation algorithm). `br`/`return`/`unreachable` call
`set_unreachable()`. New frames always start reachable (`unreachable = false`)
— entering a nested frame inside unreachable code resets polymorphism, per
the spec algorithm.

**Branch value delivery (phi lowering).** Continuation blocks read the
registers popped at the frame's `end`. At `end`, every pending branch gets
its target back-patched AND `RegOp::Copy { dst, src }` instructions appended
to its block, delivering branch-site values into the continuation's
registers. Identity copies (dst == src, common for `br_if`) are elided.

**Back-patching.** `pending_branches: Vec<(block_idx, frame_pos, values)>`.
At `end`, matching entries are *removed* (the old code left them, so sibling
frames at the same depth re-patched stale entries) and only the
`target_block` field is overwritten (the old code replaced the whole
terminator with `Br`, silently destroying `BrIf` conditions — this was the
real `br_if` WAST blocker, not the `return` issue).

**if/else.** `if` finishes the current block with `IfFork` (else edge
placeholder). `else` pops the then-body results, records a synthetic pending
branch (so the then-body exit gets the same back-patch + copy treatment as
any branch), and patches the else edge to the else-body start. At `end`
without `else`, the else edge patches to the continuation.

**loop.** Frame kind `Loop { header_block }`. `br`/`br_if` to a loop label
emit terminators targeting the header immediately (no back-patching), and
pop the frame's *parameter* types (empty until multi-value block types) —
all other frames pop *result* types.

**br_table.** `PendingBranch` entries carry a `BranchSlot` (`Single` vs
`Table(i)`) so one terminator's many targets back-patch independently.
Loop-header slots resolve at lowering time. Copies for every targeted
frame accumulate in the dispatch block: each frame's continuation registers
are disjoint allocation ranges and all copies share the same source
registers, so per-frame copies in one block are sound (no trampolines
needed). Loop targets can only appear when all arities are 0, so no copies
are needed for them.

**select.** `RegOp::Select { dst, v1, v2, cond }`. Untyped select discovers
the operand type from the stack; typed select uses the annotation.

### Runtime
- `RegOp::Copy` executes as a register move.
- `RegTerm::Trap` → `RuntimeTrap::Unreachable` (WAST message `"unreachable"`).
- Non-parameter locals are zero-initialized per spec (previously
  `UninitializedLocal`).
- Iteration fuel is now a flat 10M (was `blocks.len() * 100`, far too small
  for loop back-edges). A configurable fuel mechanism is future work.

## File inventory

| File | Purpose |
|------|---------|
| `crates/baedeker-core/src/lower/mod.rs` | IR types, lowering pass |
| `crates/baedeker-core/src/runtime/mod.rs` | Block-walking interpreter |
| `crates/baedeker-core/tests/runtime_wast.rs` | WAST integration harness |
| `crates/baedeker-testdata/spec/runtime/control-block-br.wast` | block/br (incl. value-carrying) |
| `crates/baedeker-testdata/spec/runtime/control-br-if.wast` | br_if cohort |
| `crates/baedeker-testdata/spec/runtime/control-if-else.wast` | if/else cohort + unreachable trap |
| `crates/baedeker-testdata/spec/runtime/control-loop.wast` | loop cohort |
| `crates/baedeker-testdata/spec/runtime/control-br-table.wast` | br_table cohort |
| `crates/baedeker-testdata/spec/runtime/control-select.wast` | select cohort |

## Test baseline
- 564 unit tests: all passing (14 new on this branch)
- Runtime WAST: 16 fixtures, 244 assertions, all passing
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`: clean

## Next steps
1. **Multi-value block types** (`BlockType::TypeIdx`): `block_type_to_vec`
   currently returns `[]`. Needs func-type lookup for params/results; loop
   branch values then use real param types (plumbing already in place).
   Note: br_table copies for loop targets will then need header-param
   registers, not continuation registers — revisit the copy mechanism then.
2. Consider deleting unreachable trailing blocks (after `br`/`return`) to
   slim the IR; currently they are lowered but never execute.
3. Phase 2 C4: calls, memory, globals.
