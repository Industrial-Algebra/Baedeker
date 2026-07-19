# Phase 2 — PR Breakdown Plan

**Date:** June 2026
**Status:** Approved by Justin; PRs 25+ in flight

## Sequence

| # | Scope | Size | Depends on |
|---|-------|------|-----------|
| 25 | C3 close-out: if/else/loop/br_table/select, branch-value copies, polymorphic stack. Closes #18 (and effectively #17) | done | — |
| 26 | Direct calls: `RegOp::Call`, runtime call frames, recursion depth limit → `assert_exhaustion` harness support; NaN-pattern WAST support rides along | M | 25 |
| 27 | Linear memory + globals: scalar loads/stores (all widths), memory.size/grow, bounds traps, global.get/set. One runtime-state design | M | 25 |
| 28 | Tables + `call_indirect`: table storage, elem init, table ops, signature check + traps | M | 26, 27 |
| 29 | Multi-value block types (`BlockType::TypeIdx`): frame `param_types`, block-entry param consumption; revisit br_table loop-target copies → header-param regs | S-M | 25 |
| 30 | Saturating truncation (`I32TruncSat*`, `I64TruncSat*`) | XS | anytime |
| 31 | SIMD v128 core: `Value::V128`, v128.const, lane arithmetic subset matched to Borsalino Level 1 needs, v128 load/store | L | 27 |
| 32 | GPU backend slot: `Option<Box<dyn GpuBackend>>` in engine state, trait def, no-op default | S | 31 |
| 33 | Borsalino Level 1: Vulkan backend installed in the slot; bulk SIMD offload with per-platform size thresholds (unified-memory vs discrete GPU) | L | 32 |

## Platform pivot (June 2026)

Baedeker is no longer iOS-first. Borsalino's Vulkan backend runs transparently across
Metal/Nvidia/AMD hardware, removing the only iOS-specific technical dependency. Baedeker
is now a **general cross-platform WASM runtime**: Linux, macOS, iOS, Android, and anything
else Rust targets. Embedding is via C-compatible FFI (Swift interop on Apple platforms);
GPU offload is Vulkan-based everywhere. PR 33 proceeds unchanged in shape, but the backend
is Vulkan, not Metal — and offload thresholds are per-platform (unified-memory vs discrete
GPU crossover differs). Phase 5 of the ROADMAP is now "Platform Integration Layers".

## Guardrails / explicit non-goals

- Reference instructions (`ref.null`, `ref.func`, `ref.as_non_null`, `br_on_null*`,
  `call_ref`, `return_call*`) remain explicitly rejected by lowering — function-references/GC-era,
  not Phase 2.
- Dead-block elimination: skipped until IR size matters for GPU block extraction.
- Imported-function calls fail with a clear runtime error until host-function support exists
  ("keep execution support explicit").
- Multi-value placement is after PR 28 (real-module breadth before spec-surface completeness);
  independent, can slide earlier if needed.

## Test baseline at plan time

564 unit tests, 16 runtime WAST fixtures / 244 assertions, fmt + clippy `-D warnings` clean.
