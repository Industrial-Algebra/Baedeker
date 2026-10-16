# GC — Unit 2 contract: heap object model + struct/array/i31 execution

Branch `feature/gc-slice` (post-U1, HEAD 7f1fef5). No git commands by
implementer. Scope: the GC heap, its stats, and execution/validation/
lowering of the struct/array/i31 instruction families. Casts, ref.eq,
and extern/any conversions are U3.

## Pinned from the reference (@ `dfa3f32`; quote-trust over intuition)

**Opcodes** (`binary/decode.ml:614–653`, all `0xFB`-prefixed):
struct.new 0x00, struct.new_default 0x01, struct.get 0x02,
struct.get_s 0x03, struct.get_u 0x04, struct.set 0x05;
array.new 0x06, array.new_default 0x07, array.new_fixed 0x08
(+u32 n), array.new_data 0x09 (+typeidx,dataidx), array.new_elem
0x0A (+typeidx,elemidx), array.get 0x0B, array.get_s 0x0C,
array.get_u 0x0D, array.set 0x0E, array.len 0x0F, array.fill 0x10,
array.copy 0x11 (+typeidx,typeidx), array.init_data 0x12
(+typeidx,dataidx), array.init_elem 0x13 (+typeidx,elemidx);
ref.i31 0x1C, i31.get_s 0x1D, i31.get_u 0x1E.

**Validation** (`valid/valid.ml:745–860`, quoted not paraphrased):
- ref.i31: [i32] → [(ref i31)]. i31.get_{s,u}: [(ref null i31)] →
  [i32].
- struct.new{,_default} x: x must be a STRUCT comptype (else unknown
  struct); _default requires every field defaultable (nullable refs /
  num / vec; non-null refs are NOT); new pops fields L→R, pushes
  (ref $x); default pops nothing.
- struct.get{,_s,_u} x i: i < field count else unknown field; the
  extension marker must PRESENT iff the field is packed ("field is
  packed"/"unpacked"); operand [(ref null $x)] → field's unpacked
  type. struct.set x i: field must be mutable ("immutable field");
  [(ref null $x); t] → [].
- array.new{,_default} x: x must be an ARRAY comptype; default
  requires defaultable field; new pops [t; i32(len)] (len on TOP);
  pushes (ref $x).
- array.new_fixed x n: n values of field type → (ref $x).
- array.new_elem x y: elem segment's reftype must MATCH the array's
  field type (module-aware); [i32 i32] → (ref $x).
- array.new_data x y: field must be num-or-vec ("array type is not
  numeric or vector"); [i32 i32] → (ref $x).
- array.get{,_s,_u} x: packedness rule as struct.get; [(ref null $x)
  i32] → t. array.set x: mutable; [(ref null $x) i32 t] → [].
- array.len: [(ref null array)] → i32 (ABSTRACT array heap type, not
  $x).
- array.fill x: mutable; [(ref null $x) i32 t i32] → [].
- array.copy x y: destination mutable; source storage MATCHES dest
  storage (module-aware); [(ref null $x) i32 (ref null $y) i32 i32]
  → [].
- array.init_data x y: mutable, num-or-vec; [(ref null $x) i32 i32
  i32] → []. array.init_elem x y: mutable; elem reftype matches
  field; same shape.

**Execution** (`exec/eval.ml:660–890`):
- i31: `I31.of_i32` truncates to 31 bits at ref.i31; get_s sign-
  extends bit 30, get_u zero-extends; null i31 traps
  ("null i31 reference").
- EVERY access (struct.get/set, array.get/set/len/fill/copy/init_*)
  traps on a null operand BEFORE bounds checks.
- Bounds: `array_oob a i n` = i+n > len unsigned; checked before any
  effect; n=0 is a no-op AFTER the null check (null still traps).
- array.copy: null-checks both refs, then bounds both ranges, then
  element-wise in the overlap-safe direction (implementation may use
  a direct loop with identical observable order: no partial state is
  observable on trap because bounds precede effects).
- array.init_data/init_elem: ARRAY bounds checked FIRST, then segment
  bounds ("out of bounds array access" vs memory error — trap
  identity matters); data loads use storage widths i8=1/i16=2 bytes,
  little-endian; src offset is u32 read as u64 for byte math.
- struct/array values: alloc at struct.new/array.new*; fields store
  packed (i8/i16) or full values.

**Resource discipline** (house rule, memory64 contract): every heap
allocation path try_reserves BEFORE building; refusal = clean
RuntimeError (ResourceLimitExceeded), never abort; huge-but-valid
array.new (e.g. len 2^31) fails cleanly; failed allocation leaves no
partial object.

## Changes

1. **`runtime/gcheap.rs` (new)**: `GcHeap { objects: Vec<GcObject>,
   reserved stats }` with `GcObject = Struct { type_id, fields:
   Vec<FieldValue> } | Array { type_id, len: u32, elems:
   ElemStorage }`. `ElemStorage`: packed `Vec<u8>`/`Vec<u16>` or
   full `Vec<Value>`-per-width (implementer's choice; report).
   `ObjectId(u32)`; fallible `alloc_struct/alloc_array/alloc_fixed`
   (count-first try_reserve of both table slot and payload);
   `grow`-style bounds (usize::MAX guard on 32-bit).
2. **`Value`**: gains `GcRef(ObjectId)` and `I31(i32)` (31-bit-masked
   payload). Both Copy-cheap; no allocation to hold. FFI: opaque
   handles (GcRef) / i32 (I31) — extend `WasmValue` mechanically.
3. **`GcStats`** on the store: `live_objects`, `live_bytes`
   (LOGICAL: struct = 8 + Σ widths; array = 12 + len × width; widths
   i8=1 i16=2 i32/f32=4 i64/f64=8 v128=16 refhandle=4; i31/I31
   contribute 0), `allocations_since(snapshot)`;
   `GcHeap::stats()/snapshot()`. Exact equality grading (plan §3).
4. **Types/validate**: instruction validation per the tables above
   (new Instr variants, immediate checks, module-aware matching for
   elem/data field compat). Error kinds for: unknown struct/array
   type index, unknown field, immutable field/array, packedness
   mismatch, non-defaultable, elem/data type mismatch.
5. **Lower/IR**: new RegTerms (StructNew{Default,Get{,S,U},Set},
   ArrayNew{,Default,Fixed,Data,Elem}, ArrayGet{,S,U}, ArraySet,
   ArrayLen, ArrayFill, ArrayCopy, ArrayInitData, ArrayInitElem,
   RefI31, I31Get{S,U}) with immediates; lowerer operand checks
   MIRROR the validator tables (pop_ref_in_heap-class helpers);
   GcRef/I31 RegConst values for const-expr eligibility limited to
   what U2 supports (struct.new/array.new*/ref.i31 are NOT const
   eligible at this pin — verify against valid.ml const rules;
   report).
6. **Execution** (`runtime/mod.rs`): dispatch per eval.ml tables —
   null-before-bounds ordering, trap identities, packed loads
   (libm-free), direction-safe copy, init_* ordering. AOT **v9**
   (new terms + value shapes — layout-true bump; ladder 1..=8).
7. **Tests (exact, TDD)**:
   - `gc_heap_binary.rs` (≥10): every opcode decodes; malformed
     variants (0xFB 0x05 missing immediates, bad LEBs, 0xFB 0x14/
     0x15+ reserved → unknown opcode since casts are U3).
   - `gc_heap_validation.rs` (≥14): one per validation row above
     (positive + negative each where applicable).
   - `gc_heap_execution.rs` (≥16): field readback (packed s/u
     roundtrips), struct.set, array fill/copy/init_data/init_elem
     happy + every trap identity (null-before-bounds, bounds-before-
     segment), i31 s/u semantics + null, new_default zero values,
     array.len, elem/data inits incl. packed widths.
   - `gc_heap_stats.rs` (≥5): exact live_bytes for a v128 struct
     (8+16=24) and 3-elem v128 array (12+48=60); allocations_since
     exact; i31 zero-delta; refusal under ulimit-style pressure via
     direct API (huge array.new fails cleanly, stats unchanged).
   - `gc_heap_resource.rs` (≥3): try_reserve-before-build audit
     tests (huge new_fixed, huge new, failed alloc leaves no partial
     entry: live_objects unchanged after refusal).
   - EH + existing suites green; official suite 178/53,543/0
     (no new files in the official runner yet — corpus grind is U5).

## Rule-7 ambiguity report

array.new_fixed's u32 n builds an n-value operand stack at
validation; the reference has no cap (it OOMs). Our validator must
handle huge n without abort — if you impose an internal operand-
stack bound, REPORT the bound and its justification. Anything else
ambiguous: report, don't decide silently.

## Verification gates

fmt; clippy both configs; `cargo test --workspace`; `cargo test -p
baedeker-core --features aot`; official suite 178/0 deferred 7.
