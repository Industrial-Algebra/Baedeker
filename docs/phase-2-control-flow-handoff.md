# Phase 2 Control Flow — Handoff Document

**Branch:** `feature/phase-2-part-4-control-flow`
**Date:** June 2026
**Status:** Structural work complete; WAST integration in progress

## What's in this branch

### Terminator types (all added to `RegTerm`)
| Variant | Fields | Status |
|---------|--------|--------|
| `Fallthrough` | — | ✅ Working |
| `Br` | `target_block, values` | ✅ Working (block+branch WAST pass) |
| `BrIf` | `cond, target_block, values` | ✅ Structural code; WAST needs debugging |
| `IfFork` | `cond, then_block, else_block` | ✅ Structural code; not wired to `Instr::If` yet |
| `Return` | `values` | ✅ Working |

### Lowering handlers
| Instruction | Handler | Status |
|-------------|---------|--------|
| `block` | Creates label frame, finishes pre-block | ✅ Working |
| `end` | Pops frame, back-patches branches, creates continuation | ✅ Working |
| `br` | Pops values, records pending branch, emits Br terminator | ✅ Working |
| `br_if` | Pops cond + values, records pending branch, emits BrIf terminator | ✅ Structural; needs WAST validation |
| `return` | Pops results, emits Return terminator, stops lowering | ✅ Working |
| `if` | Not yet implemented | ⬜ Needs IfFork wiring |
| `else` | Not yet implemented | ⬜ |
| `loop` | Not yet implemented | ⬜ |
| `br_table` | Not yet implemented | ⬜ |
| `select` | Not yet implemented | ⬜ |

### Back-patching mechanism
Branches (`br`, `br_if`) store a placeholder `target_block: 0` and record
`(block_idx, frame_pos, values)` in `pending_branches`. At `end` time, the
continuation block index is computed and all matching branches are patched.

### Runtime dispatch
```
Fallthrough → block_idx + 1
Br { target } → block_idx = target
BrIf { cond, target } → if cond ≠ 0: block_idx = target; else block_idx + 1
IfFork { cond, then, else } → if cond ≠ 0: block_idx = then; else block_idx = else
Return { values } → return Ok(values)
```

## Known issues

### br_if WAST integration (blocker for green tests)
The `br_if` handler works structurally, but creating a valid WAST test requires
careful handling of `return` inside blocks. The primary issue:

1. `return` stops the lowering loop (returns `Ok(true)` from `lower_instr`).
   This means any code after `return` in the function body is never lowered.
2. For `br_if` tests, `return` inside a block is unreachable when `br_if` is
   taken, but the WAST function body needs code after the block's `end` to be lowered.

**Potential fixes:**
- Change `return` to NOT stop lowering (but this broke existing tests in a
  previous attempt)
- Structure WAST tests so `return` only appears at the very end of the function body
- Handle unreachable regions explicitly in the lowering pass

### If/else not yet wired
The `IfFork` terminator is defined and the runtime dispatches it, but the
`Instr::If` handler in the lowering needs to:
1. Pop the condition
2. Create the IfFork terminator with correct then/else block indices
3. Handle `else` to switch from then-body to else-body
4. Back-patch both paths at `end`

## File inventory

| File | Purpose |
|------|---------|
| `crates/baedeker-core/src/lower/mod.rs` | IR types, lowering pass |
| `crates/baedeker-core/src/runtime/mod.rs` | Block-walking interpreter |
| `crates/baedeker-core/tests/runtime_wast.rs` | WAST integration harness |
| `crates/baedeker-testdata/spec/runtime/control-block-br.wast` | Block + br tests (passing) |
| `docs/ROADMAP.md` | Updated Phase 2 checkpoints |

## Test baseline
- 550 unit tests: all passing
- Runtime WAST: 9 fixtures passing (block+br, integer, float, conversions)
- `cargo fmt`, `cargo clippy`: clean

## Next steps
1. Debug `br_if` WAST test (write a simple passing test)
2. Wire `if`/`else` lowering using `IfFork`
3. Add `loop` lowering (back-edge via `Br`)
4. Add `br_table` (multi-target dispatch)
5. Add `select` (conditional register selection)
6. Expand WAST coverage for all control flow
