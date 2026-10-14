# Slice 4 — memory64 (and table64)

WASM 3.0 slice 4: 64-bit memories and tables. Follows the EH-slice recipe:
plan → unit contracts → dispatched implementation (deepseek) → vendor corpus
→ grind to zero → review loop to convergence (breaker 12) → operator merge.

Spec pin: `dfa3f32a881aecc60a8c792da3c25787ccb15572` (COMMIT.txt).
Reference corpus: `test/core/memory64/` — **25 files, ~8,282 assertions**
(5,703 return / 2,116 trap / 373 invalid / 60 malformed / 30 unlinkable).
Expected post-slice: 178 files, ~53.4k assertions, 0 failures, deferred still 7.

## Scope

- **memory64**: memories whose index type is i64 — limits flag bit 0x04,
  u64 min/max LEBs, i64 addressing for every memory op, i64 `memory.size` /
  `memory.grow`, i64 bulk-memory operands, i64 data-segment offsets.
- **table64** (part of the proposal at our pin): i64 index type for
  `table.get/set/size/grow/fill/copy/init`, `call_indirect`, i64 elem
  offsets.
- **Out of scope**: shared memories (threads slice), custom page sizes
  (`pagesize`, flag bit 0x08 — not in corpus at pin), multi-memory.

## Semantics pinned from the reference (valid.ml @ pin)

- `check_limits`: `min ≤ max ≤ range`, both unsigned.
- Memory range: i32 → `0x1_0000` (2^16 pages); i64 → `0x1_0000_0000_0000`
  (2^48 pages, 256 TiB).
- Table range: i32 → `0xffff_ffff` (2^32-1); i64 → unbounded (2^64-1).
- **memarg offset (new rule)**: for i32 memories `offset < 2^32`
  ("offset out of range"); i64 memories unrestricted.
- wast-240 exposes `Limits { is64, min: u64, max: Option<u64> }`; encoder
  sets `memory64`/`table64` flag. Binary flags: 0x00 min-only, 0x01 min+max
  (existing), 0x04 i64 min-only, 0x05 i64 min+max. 0x02 (shared) and 0x08
  (custom page size) still rejected.

## Design

- `Limits { min: u64, max: Option<u64>, is64: bool }` (one struct, both
  sections + imports).
- IR shape unchanged: same `RegTerm`s for mem/table ops. At runtime the
  store consults `memory_types[idx]` / `table_types[idx]` to read address
  registers as i64 vs i32 — one code path, dispatch by index type.
- Addressing: `ea = index(u64) + memarg.offset(u64)`, checked_add; trap on
  `ea + width > len` or overflow. On 32-bit targets also guard
  `ea > usize::MAX` → trap (armv7 CI compiles but can't address it).
- Instantiate with huge-but-valid min must fail cleanly (no OOM abort):
  cap allocation attempts, return a RuntimeError.
- AOT v6 (limits serialization changes shape).
- FFI: already u64 — the one ABI-shape change was pre-paid.

## Units

| Unit | Content | Contract |
|---|---|---|
| U1 | Binary surface: u64+is64 limits (0x04/0x05), u64 LEBs, `MemArg.offset`→u64, AOT v6 (limits/memarg serialization shape), i64 const offsets in data/elem flow-through, fixtures | `docs/plans/memory64-unit1-contract.md` |
| U2 | Validation: limit ranges, memarg-offset rule, index-type-aware operand checks for all mem/table ops, segment offset types | `docs/plans/memory64-unit2-contract.md` |
| U3 | IR + runtime: index-type-driven addressing, 64-bit bounds/grow, OOM-safe instantiate, table64 indexing, call_indirect64 | `docs/plans/memory64-unit3-contract.md` |
| U4 | Linking compat (is64 must match), FFI verify, CLI/GPU sweep | `docs/plans/memory64-unit4-contract.md` |
| U5 | Vendor 25 corpus files, grind to zero failures | `docs/plans/memory64-unit5-contract.md` |

Review loop after U5, per ia-moment doctrine (reviewer on gpt-6.1-sol+,
settled-stays-settled, files-only contracts, no git by implementers).
