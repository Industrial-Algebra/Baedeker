# memory64 — Unit 3 contract: runtime execution

Branch `feature/memory64` at `29bcbe3`. No git commands. Scope: making
validated memory64/table64 modules EXECUTE correctly. IR/lowering is
unchanged by design — the runtime reads address operands by the target's
index type. No linking changes (U4), no corpus (U5).

## Core idea

Add one reader and one bounds helper; thread them through every memory/
table op:

- `fn read_addr(value: Value, is64: bool) -> Result<u64, RuntimeError>`
  (runtime/mod.rs near `expect_addr` :2345): `I32(v) => v as u32 as u64`
  when !is64, `I64(v) => v as u64` when is64; wrong-width register is a
  `TypeMismatch` (validation already prevents it).
- `memory_bounds` (:2320) addr param u32→u64: `ea = addr.checked_add(
  memarg.offset)?` then `.checked_add(width)?`, compare `> mem.len() as
  u64` → OOB trap; before slicing also require `ea <= usize::MAX as u64`
  (guards 32-bit targets; trivially true on 64-bit).
- is64 sources: `store.memory_type(idx)` (:1090) and
  `store.table_type(idx)` (:1169) → `.limits.is64`.

## Per-op changes (crates/baedeker-core/src/runtime/mod.rs)

| Op (site) | Change |
|---|---|
| `Load`/`Store` (+V128 variants, lane ops) :1326+ | `read_addr(..., is64 of memarg.memory)`; u64 through `memory_bounds` |
| `MemorySize` :1579 | result `Value::I64(pages as i64)` when is64, else I32 as today |
| `MemoryGrow` :1589 | delta read by is64; pages math in u64; `max_pages = limits.max.unwrap_or(if is64 { 1<<48 } else { 1<<16 })`; result `I64(old as i64)` / `I64(-1)` when is64 (grow stays fallible: allocation failure → -1, never an error) |
| `MemoryInit` :1613 | dst by is64; src+count ALWAYS read as u32 (`I32`) |
| `MemoryCopy` :1661 | dst by dst-mem is64, src by src-mem is64, count by `dst_is64 && src_is64` |
| `MemoryFill` :1712 | dst+count by is64; value ALWAYS i32→low byte |
| `CallIndirect` eval :1307 + intercept :903, `TailCallIndirect` :1089, `execute_call_indirect` :298 | idx param u32→u64; read by table is64 at every call site |
| `TableGet`/`TableSet` :1737+ | idx u64 by table is64 |
| `TableSize` :1768 | I64 result when table is64 |
| `TableGrow` :1777 | delta by is64; internal u64; result I64/I32 |
| `TableFill`/`TableCopy`/`TableInit` :1798+ | idx/count types per the U2 table (table.copy count = both-is64; table.init src+count always u32) |

## Table internals (crates/baedeker-core/src/runtime/table.rs)

`len` :74, `get` :87, `set` :101, `grow` :125, `fill` :171, `read_slice`
:201, `write_slice` :224 — index/count params u32→u64, bounds vs
`self.values.len() as u64`. Keep allocation paths fallible (`grow`
returns Option; `new` via `try_alloc_n` already exists :217).

## Instantiation offsets (crates/baedeker-core/src/runtime/store.rs)

- Data (block at :987, `let Value::I32(offset) = offset else`): accept
  `I64` when the target memory is64 (read `memory_type(memory.0)`),
  `I32` otherwise; offset math in u64 (`start` as u64, checked_add
  segment len, compare `> mem.len() as u64`, then usize slice).
- Elem (analogous `let Value::I32(offset)` in the element loop ~:909):
  same, by table is64; `write_slice(offset, …)` takes u64 now.
- Memory alloc :719 already `try_alloc_n` (a 2^48-page min fails
  cleanly as `allocation_failed`) — verify, don't rewrite.
- Imported-memory limits synthesis :599 — verify min pages as u64 stays
  consistent (it was widened mechanically in U1).

## Tests — new `crates/baedeker-core/tests/mem64_execution.rs`

Mirror `eh_execution.rs` harness style (wat → instantiate → invoke).
Cases:
1. mem64 load/store round-trip on 1 page (i64 index), incl. store at
   high-but-in-bounds u32+offset address
2. mem64 OOB (index 2^32 on 1 page) traps `OutOfBoundsMemoryAccess`
3. mem64 `memory.size` returns i64; `memory.grow` returns i64 old size,
   respects declared max, returns -1 past max
4. mem64 `memory.fill/init/copy` with i64 dst/count (fill value i32)
5. table64 get/set/size/grow/fill with i64 indexes; OOB i64 traps
6. `call_indirect` through table64 with i64 index
7. active data segment with `(i64.const N)` offset on mem64; i32.const
   offset rejected at validation (already U2 — assert here only the
   positive path)
8. instantiation of `(memory i64 0x1_0000_0000_0000)` (valid limits,
   256 TiB) FAILS CLEANLY with the allocation error — must not abort
9. table64 huge min (`(table i64 0xffff_ffff_ffff funcref)`) likewise

## Gates

fmt; clippy both configs; `cargo test --workspace`; `--features aot`;
official suite stays `153 / 45,135 / 0` (i32 path byte-identical).

## Rules

No git. Anchors from this contract; line numbers may drift a few lines —
match on the quoted code, not the number; if semantics differ, STOP and
report (rule 7). Where this contract's table and the U2 validation table
disagree on a width, report it — do not pick silently.
