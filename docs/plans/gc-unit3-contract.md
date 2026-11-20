# GC — Unit 3 contract: cast machinery (test/cast/br_on_cast/eq/conversions)

Branch `feature/gc-slice`, on top of U2 (`f2c8cde` + r2-fix). Scope: the
dynamic type-query instruction family. NOT in scope: full R0 corpus green
(that is U5; several corpus files additionally need U4 surface), GC
const-expr wiring (U4), FFI handles (U4).

## Pinned from the reference (@ `dfa3f32`; quote-trust over intuition)

### Binary rows (interpreter/binary/decode.ml)

```
636:    | 0x14l -> let ht = heaptype s in ref_test (NoNull, ht)
637:    | 0x15l -> let ht = heaptype s in ref_test (Null, ht)
638:    | 0x16l -> let ht = heaptype s in ref_cast (NoNull, ht)
639:    | 0x17l -> let ht = heaptype s in ref_cast (Null, ht)
640:    | 0x18l | 0x19l as opcode ->
641:      let flags = byte s in
642:      require (flags land 0xfc = 0) s (pos + 2) "malformed br_on_cast flags";
643:      let x = at idx s in
644:      let rt1 = ((if bit 0 flags then Null else NoNull), heaptype s) in
645:      let rt2 = ((if bit 1 flags then Null else NoNull), heaptype s) in
646:      (if opcode = 0x18l then br_on_cast else br_on_cast_fail) x rt1 rt2
648:    | 0x1al -> any_convert_extern
649:    | 0x1bl -> extern_convert_any
607:  | 0xd3 -> ref_eq
```

- `ref_test`/`ref_cast` take ONE s33 heaptype (U1's `decode_s33_wrapped`
  reader); 0x14/0x16 = NoNull form, 0x15/0x17 = Null form.
- `br_on_cast`/`br_on_cast_fail`: flags byte with ONLY bits 0/1 legal
  (`flags land 0xfc = 0` required — any other bits set = malformed), then
  labelidx, then TWO s33 heaptypes. Bit0 = rt1 (source) nullness, bit1 =
  rt2 (target) nullness.
- `ref_eq` is opcode 0xd3 (core reference space, not 0xFB-prefixed).

### Validation rows (interpreter/valid/valid.ml)

```
732:  | RefTest rt ->
733:    let (_nul, ht) = rt in
734:    check_reftype c rt e.at;
735:    [RefT (Null, top_of_heaptype c.types ht)] --> [NumT I32T], []

737:  | RefCast rt ->
738:    let (nul, ht) = rt in
739:    check_reftype c rt e.at;
740:    [RefT (Null, top_of_heaptype c.types ht)] --> [RefT (nul, ht)], []

742:  | RefEq ->
743:    [RefT (Null, EqHT); RefT (Null, EqHT)] --> [NumT I32T], []
```

- test/cast OPERAND type is `(ref null top(ht))` — the TOP of the target
  heaptype's hierarchy (any / extern / func / exn), NOT `any` unconditionally:
  `extern.convert_any`-style targets read extern-typed operands. Concrete
  `ht` also reads `top` = any.
- cast RESULT is `(nul, ht)` — nullness from the opcode form, not operand.

```
492:  | BrOnCast (x, rt1, rt2) ->
493:    check_reftype c rt1 e.at; check_reftype c rt2 e.at;
496:    require (match_reftype c.types rt2 rt1) e.at "type mismatch on cast: ..."
498:    require (label c x <> []) e.at "..."
503:    let ts0, t1 = Lib.List.split_last (label c x) in
504:    require (match_valtype c.types (RefT rt2) t1) e.at "..."
506:    (ts0 @ [RefT rt1]) --> (ts0 @ [RefT (diff_reftype rt1 rt2)]), []

508:  | BrOnCastFail (x, rt1, rt2) ->
510:    let rt1' = diff_reftype rt1 rt2 in
512:    require (match_reftype c.types rt2 rt1) e.at "..."
514:    require (label c x <> []) e.at "..."
516:    let ts0, t1 = Lib.List.split_last (label c x) in
517:    require (match_valtype c.types (RefT rt1') t1) e.at "..."
523:    (ts0 @ [RefT rt1]) --> (ts0 @ [RefT rt2]), []
```

(CORRECTED r0: the fail-form OPERAND is the source `rt1` — the original
transcription `rt1'` was the author's error; verified valid.ml:523.)

- Both forms require rt2 <: rt1 (`match_reftype rt2 rt1`).
- BrOnCast: label's LAST type must accept `RefT rt2`; pops `RefT rt1`,
  fall-through pushes `RefT (diff_reftype rt1 rt2)`.
- BrOnCastFail: label's LAST type must accept `RefT rt1'` where
  `rt1' = diff_reftype rt1 rt2`; fall-through pushes `RefT rt2`.
- diff_reftype (valid.ml:234):
  ```
  let diff_reftype (nul1, ht1) (nul2, ht2) =
    match nul2 with
    | Null -> (NoNull, ht1)
    | NoNull -> (nul1, ht1)
  ```
  i.e. NULLABLE TARGET → fall-through is always non-null (nulls would have
  matched and branched); NON-NULL TARGET → fall-through keeps the source
  nullness. (CORRECTED r0: earlier prose inverted this; the row above wins.)

### Execution rows (interpreter/exec/eval.ml)

```
648:      | RefTest rt, Ref r :: vs' ->
649:        let rt' = subst_reftype (subst_of c.frame.inst) rt in
650:        value_of_bool (Match.match_reftype [] (type_of_ref r) rt') :: vs', []

652:      | RefCast rt, Ref r :: vs' ->
653:        let rt' = subst_reftype (subst_of c.frame.inst) rt in
654:        if Match.match_reftype [] (type_of_ref r) rt' then
655:          Ref r :: vs', []
656:        else
657:          vs', [Trapping ("cast failure, expected " ^ ... ) @@ e.at]

661:      | RefEq, Ref r1 :: Ref r2 :: vs' ->
662:        value_of_bool (eq_ref r1 r2) :: vs', []

246:      | BrOnCast (x, _rt1, rt2), Ref r :: vs' ->
247:        let rt2' = subst_reftype (subst_of c.frame.inst) rt2 in
248:        if Match.match_reftype [] (type_of_ref r) rt2' then
249:          Ref r :: vs', [Plain (Br x) @@ e.at]
250:        else
251:          Ref r :: vs', []

253:      | BrOnCastFail (x, _rt1, rt2), Ref r :: vs' ->
254:        ... (mirror: no-match branches)
```

- Runtime match uses `Match.match_reftype [] (type_of_ref r) rt'` — EMPTY
  type context: only the value's runtime type participates; concrete
  targets compare structurally via the value's heap type.
- Cast refusal traps with message prefix `"cast failure, expected ..."`.
- test/br_on_cast NEVER trap on mismatch (they yield 0 / fall through);
  cast traps; the null forms differ ONLY in that null matches the target.

### Equality + conversions (interpreter/runtime/value.ml, i31.ml, eval.ml)

```
value.ml:127:  let eq_ref' = ref (==)             (* physical identity *)
i31.ml:17-21:  | I31Ref i1, I31Ref i2 -> i1 = i2  (* payload equality *)
eval.ml:929-941 (conversions, immediate re-tags — no allocation):
  | ExternConvert Internalize, Ref NullRef :: vs' -> Ref NullRef :: vs', []
  | ExternConvert Internalize, Ref (Extern.ExternRef r) :: vs' -> Ref r :: vs', []
  | ExternConvert Externalize, Ref NullRef :: vs' -> Ref NullRef :: vs', []
  | ExternConvert Externalize, Ref r :: vs' -> Ref (Extern.ExternRef r) :: vs', []
```

- `ref.eq` semantics: i31⊕i31 = payload equality; object⊕object = identity
  (same heap AND ObjectId for us); null⊕null = true; cross-domain = false.
- Conversion VALIDATION (valid.ml:356-359, 855-858 — CORRECTED r0; the
  original text inverted the Externalize domain):
  ```
  356:  let type_externop op = match op with
  358:    | Internalize -> ExternHT, AnyHT
  359:    | Externalize -> AnyHT, ExternHT
  855:  | ExternConvert op ->
  857:    let (nul, _ht) = peek_ref 0 s e.at in
  858:    [RefT (nul, ht1)] --> [RefT (nul, ht2)], []
  ```
  `any.convert_extern` (0x1a): externref → anyref. `extern.convert_any`
  (0x1b): ANYREF → externref — the operand is any ANYREF (peek_ref keeps
  ITS nullness): struct refs, array refs, i31s, internalized hosts ALL
  externalize. eval wraps `Ref r → Ref (Extern.ExternRef r)` for ANY r.
- BOTH are immediate value-level re-tags — NEVER heap entries (recorded
  U-plan decision; re-verified at the pin).

## Runtime model decisions

1. **Domain-crossing value model** (CORRECTED r0 — full bidirectional
   crossing per the pin; the original `AnyExtern(Option<u32>)` model was
   the author's domain inversion, caught by the implementer, verified at
   the pin above). TWO variants:
   - `Value::HostAny(Option<u32>)` — any-domain value holding a host
     handle (internalized raw extern).
   - `Value::ExternGc(Option<ExternPayload>)`,
     `enum ExternPayload { Gc(HeapId, ObjectId), I31(i32) }` —
     extern-domain value holding a GC-domain payload (externalized).
   - `any.convert_extern`: `ExternRef(h)→HostAny(h)`;
     `ExternGc(Gc(heap,id))→GcRef{heap,id}`; `ExternGc(I31(i))→I31(i)`;
     null → null.
   - `extern.convert_any`: `GcRef→ExternGc(Gc(..))`; `I31→ExternGc(I31)`;
     `HostAny(h)→ExternRef(h)` (symmetric with internalize); null → null.
   - Roundtrips are exact identities: internalize∘externalize = id for
     every anyref; externalize∘internalize = id for every externref.
   - Boundary checks: `(ref extern)` accepts `ExternRef` and `ExternGc`;
     `(ref any)` accepts `GcRef` (resolving), `I31`, and `HostAny`.
     Cross-domain values REJECT (a raw `ExternRef` into an any-typed
     param; a `HostAny`/GC value into an extern-typed param).
   - `ref.test`/`ref.cast` targets: `any` matches `GcRef`/`I31`/
     `HostAny` (non-null); `extern` matches `ExternRef`/`ExternGc`;
     `eq` matches `GcRef`/`I31` only; concrete struct/array match only
     `GcRef` of that kind; `i31` matches `I31`; `none` never matches
     non-null.
2. **Runtime type queries via the heap.** Each `GcObject` already records
   its `GcTypeId`. Extend the heap type entry with a precomputed ancestor
   id chain: `ancestry: Vec<GcTypeId>` (declared supertypes, transitively
   resolved at INTERN time by interning each supertype's closed form —
   same closed-form identity as precompute). Fallible (`try_reserve`),
   intern-last ordering preserved (U2 discipline: no committed entry
   around refusal).
3. **match semantics at runtime (`gc_ref_matches_ht`)**, mirroring
   `Match.match_reftype [] (type_of_ref r)`:
   - null operand: matches iff target form is Null.
   - `I31`: matches `i31` (and transitively `eq`, `any` — abstract
     hierarchy bottoms per U1 lattice).
   - `GcRef{heap,id}` (resolving through own or foreign registry;
     unresolvable = boundary rules apply, internal ops validated so this
     is defensive → trap): concrete target: if the object's heap IS the
     executing store's heap, target interned id ∈ {own} ∪ own.ancestry.
     If the object resolves to a FOREIGN heap, the store-side precomputed
     id indexes the WRONG table — resolve by looking the target type's
     CLOSED FORM up in the OBJECT's heap (read-only find in the type-id
     key table; no interning, zero alloc); miss → no match. Then id-walk
     in the object's heap. (r0 correction: per-heap ids are positional;
     cross-heap id comparison only works when intern order aligns — the
     A5 test accidentally aligned; regressions must cover NON-aligned
     intern order.) Abstract targets per kind: `struct`/`array`
     match objects of that kind regardless of concrete type; `eq` matches
     any object + i31; `any` matches every GC-domain value; `none` never
     matches non-null.
   - `AnyExtern`: matches `any` (and its Null form for null) — nothing
     else.
   - `FuncRef`: matches `func` — U3 does not deepen function-type
     matching (funcref boundary tightening is a U4 ledger item).
4. **Allocator discipline** (house rule, as U2): every new runtime path
   here (ancestor-chain walks are reads; no allocation needed in
   test/br_on_cast AT EXECUTION — chains precomputed at intern). Decoder
   vectors and validator/label bookkeeping reserve fallibly where growth
   is attacker-sized.

## Changes

- `types.rs` / `binary/instr.rs`: `RefTest{null_form, ht}`,
  `RefCast{null_form, ht}`, `BrOnCast{fail, flags, label, src, tgt}`,
  `RefEq`, `AnyConvertExtern`, `ExternConvertAny` instruction shapes
  (`RefEq` if not already present — check first; opcode 0xd3 may already
  exist from the reference-types base).
- `binary/instr.rs` decode: rows per the pin above; flags mask check
  `flags & 0xfc == 0` (malformed otherwise, decoder error not trap);
  two s33 heaptype reads for br_on_cast.
- `validate/mod.rs`: rows verbatim per pin — operand `top(ht)` types,
  cast result nullness, rt2<:rt1, label-last checks BOTH forms,
  fall-through diff type. Label machinery: our validator's label types
  must incorporate the pushed ref (mirror how br checks label tails
  today; if our labels are untyped positions, encode the check as the
  structural equivalent — REPORT in ambiguity report which form was
  taken).
- `lower/mod.rs`: 7 new RegOps mirroring validation; br_on_cast pair
  compiles to test + conditional branch (NO new control-flow primitive;
  the value stays on the operand stack either way per eval rows —
  `Ref r :: vs'` precedes the branch).
- `runtime/gcheap.rs`: ancestry chains on interned types (fallible,
  intern-last preserved); read-only query helpers.
- `runtime/mod.rs`: execution per eval rows; `ref.eq` per eq_ref;
  conversions as re-tags; cast trap message prefix exactly
  `"cast failure, expected "`; `Value::AnyExtern` wired through
  `value_type`, boundary `value_satisfies`, null/ref handling.
- `runtime/store.rs`: precompute extends to cover types reachable ONLY
  as supertypes (they are type-section entries already interned — verify
  no gap; REPORT).
- AOT: serialized instruction forms are new → **AOT v10** (ladder rejects
  v9), roundtrip tests updated.

## Test files (targeted; R0 corpus stays at U5)

- `gc_u3_binary.rs` — decode/encode roundtrips: all 6 opcodes × null
  forms; br_on_cast flags bits {0,1,2,3} legal / bit2-set (0x04) and
  high-bits (0xfc) malformed; two-heaptype operand order; ref.eq 0xd3.
- `gc_u3_validation.rs` — pin rows: test/cast operand `top(ht)` (extern
  target accepts externref operand, rejects anyref operand); rt2 <: rt1
  both forms; label-last acceptance both forms (including null-diff
  label refinement); fall-through type is the DIFF (nullable source,
  non-null target → non-null fall-through; nullable target → nullable
  source fall-through); ref.eq operand domain (eq only: rejects externref
  operand); conversions' type effects.
- `gc_u3_execution.rs` — runtime matrix: test 0/1 across {struct object,
  sub-typed struct object (declared supertype chain ≥2 deep), array
  object, i31, null, converted extern, funcref} × targets {concrete hit,
  concrete miss, declared-supertype hit (A5-style distinct-ID subtype:
  two modules, structurally identical supertype, distinct concrete ids —
  the SUBTYPE test must succeed), struct/array/i31/eq/any/none/extern,
  null forms}; cast: hits pass through, misses trap with exact message
  prefix, null traps non-null form / passes Null form (result null);
  br_on_cast/br_on_cast_fail: branch + fall-through on hit/miss with the
  value retained (per eval row: value pushed BEFORE branch), flags ×4;
  ref.eq: i31⊕i31 payload equal/unequal, object identity same/different,
  null⊕null = 1, i31⊕object = 0; conversions roundtrip
  (extern.convert_any ∘ any.convert_extern = id, null passthrough both
  ways), converted extern refused by eq-typed consumers (validation).
- `gc_u3_stats.rs` — conversions allocate ZERO logical bytes (stats
  delta 0 across both conversions, incl. roundtrip).
- `gc_u3_resource.rs` — armed-allocator-class checks ONLY if any new
  execution path allocates (goal: none — chains precomputed; assert
  stats/heap unchanged under repeat test/cast).
- AOT roundtrip: new opcodes serialize/deserialize/execute (v10 bump,
  v9 rejected).

## Rule-7 ambiguity report

Report, do not silently decide: label-check encoding if our validator
lacks typed label tails; supertype-only-reachable intern gaps; ref.eq
0xd3 prior existence; any place the pin requires `subst` of instance
types at runtime that our single-module substitution cannot express.

## Verification gates

`cargo fmt -- --check`; clippy `--all-targets -- -D warnings` (workspace
and `-p baedeker-core --features aot`); `cargo test --workspace` green;
`cargo test -p baedeker-core --features aot` green; official suite
UNCHANGED from `files=178 deferred=7 directives=55439 modules=1854
assertions=53543 failures=0`; EH suites green. No regression in any
existing test file.
