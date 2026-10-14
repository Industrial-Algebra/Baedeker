# memory64 — Unit 2 contract: validation semantics

Branch `feature/memory64` at `cdbbe83` (U1 landed: `Limits { min: u64,
max: Option<u64>, is64 }`, `MemArg.offset: u64`, memarg-offset rule
already in). No git commands. Scope: index-type-aware validation only —
no runtime/lowering changes (U3), no linking (U4).

All operand types below are pinned from the reference interpreter
(valid.ml @ dfa3f32a, `numtype_of_addrtype`; `min at1 at2` picks the
NARROWER addr type: i32 unless both are i64).

## Changes (crates/baedeker-core/src/validate/mod.rs)

Helper first:

- `fn addr_num_type(limits: &Limits) -> NumType` — `is64 ? I64 : I32`.

### 1. Limit ranges (`validate_memory_limits` at :185; table equivalent)

- memory: `max_allowed = if is64 { 1 << 48 } else { 1 << 16 }` pages;
  existing `MemorySizeOutOfRange` + `MemoryMinExceedsMax` kinds reused.
- table limits (find the table-type validation path): i32 → min/max ≤
  `0xffff_ffff`; i64 → unbounded (u64; only min≤max applies). Reuse
  appropriate existing kinds (`TableTypeMismatch` is NOT it — if no
  fitting kind exists, report under rule 7 and propose one).

### 2. Operand types per addr type

| Op (site) | Operands (pop order listed in source order) |
|---|---|
| all loads/stores (`validate_load` :3068, `validate_store` :3088, simd lane :3113/:3140 — address pop in each) | address = addr type of `memarg.memory` instead of hardcoded I32 |
| `MemorySize` :2053 | result = addr type |
| `MemoryGrow` :2059 | delta = addr type; result = addr type |
| `MemoryFill` :1911 | dst = addr; **value = I32 always**; count = addr |
| `MemoryCopy` :1886 | dst = dst-mem addr; src = src-mem addr; count = `min(at1,at2)` |
| `MemoryInit` :1858 | dst = addr; **src = I32 always; count = I32 always** |
| `CallIndirect` :1121 | index = addr type of table (after args) |
| `TableGet` :1310 / `TableSet` | index = table addr type |
| `TableSize` :2029 | result = addr type |
| `TableGrow` :2015 | delta = addr; result = addr (fill value = elem reftype, unchanged) |
| `TableFill` :2035 | index = addr; value = reftype (unchanged); count = addr |
| `TableCopy` :1978 | dst = at1; src = at2; count = `min(at1,at2)` |
| `TableInit` :1935 | dst = at; **src = I32 always; count = I32 always** |

`resolve_memory_type` / `resolve_table_type_with_context` already
return the type — read `.limits.is64` from it.

### 3. Segment offset const-expr types

`validate_data_segments` (:348) and `validate_elements` (:701) call
`validate_const_i32_expr` on active offsets. These become
addr-type-aware: the expected const type is the target memory's /
table's addr type. Locate `validate_const_i32_expr` and generalize
(e.g. `validate_const_num_expr(module, expr, ty, offset)`) or add an
i64 sibling; `i64.const 0` on an i64 memory must validate, and
`i32.const 0` on an i64 memory must be rejected (type mismatch).

### 4. Fixture + tests

- New fixture `crates/baedeker-testdata/spec/mem64-validation/mem64-validation.wast`
  (mirror `eh-validation` layout incl. any `.meta` conventions used by
  `tests/spec.rs` — inspect that harness for how fixtures map to
  assertions) covering: mem64 valid load/store/size/grow types, mem64
  i32-address rejection, table64 get/set/size/grow/fill/copy/init types
  + call_indirect64 index type, mixed copy count-type (mem32→mem64 copy
  count is i32; mem64→mem64 copy count is i64), fill value i32 on mem64,
  init src/count i32 on mem64, data/elem i64 offset accepted + i32
  offset on mem64 rejected, limit ranges (i64 min > 2^48 rejected,
  i64 min ≤ 2^48 accepted-by-validation, i32 table min > 2^32-1 rejected).
- New integration test `crates/baedeker-core/tests/mem64_validation.rs`
  driving the fixture, mirroring `eh_validation.rs`.

## Gates

`cargo fmt --all`; clippy both configs `-D warnings`; `cargo test
--workspace`; `cargo test -p baedeker-core --features aot`; official
suite must stay `153 files / 45,135 assertions / 0 failures` (existing
i32 modules are unaffected — every rule branches on `is64`).

## Rules

Files only as listed; anchors from this contract — if a site moved or
differs materially, STOP and report (rule 7). Compute nothing from
memory: every operand type above is quoted from valid.ml; if you find
conflicting evidence in our corpus fixtures, report rather than choose.
No git. Report exact gate outputs and every deviation.
