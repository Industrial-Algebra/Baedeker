# GC — Unit 1 contract: type grammar (binary + validate)

Branch `feature/gc-slice`. No git commands by implementer. Scope: the
type-section grammar, its validation, and canonicalization. No new
instructions, no heap objects, no runtime changes (U2/U3).

## Semantics pinned from the reference (@ `dfa3f32`, files below quoted,
not paraphrased — trust them over intuition)

**Binary grammar** (`interpreter/binary/decode.ml`):

| Byte | Meaning |
|---|---|
| `0x4E` | rec group: `vec subtype` |
| `0x50` | `sub`: vec typeidx supertypes + comptype (not final) |
| `0x4F` | `sub final`: vec typeidx supertypes + comptype |
| bare | comptype alone = final, no supertypes |
| `0x60` | func comptype (params, results) |
| `0x5F` | struct comptype: vec fieldtype |
| `0x5E` | array comptype: fieldtype |
| `0x78`/`0x77` | packtype i8 / i16 |
| `0x74`–`0x69` | abstract heaptypes: `noexn nofunc noextern none func extern any eq i31 struct array exn` |
| `0x63`/`0x64` | `ref null ht` / non-null `ref ht` |

fieldtype = storagetype + mut byte (0=const, 1=var). Multiple
supertypes per `sub` decode and validate (vec iterated — no count
restriction at this pin).

**Roll & install** (`valid.ml:178–189` check_rectype; `syntax/types.ml`
roll_rectype/unroll): roll = substitute in-group indices with RELATIVE
rec-refs, external indices with their CLOSED deftype forms; append the
whole rolled group to the context; only then check members. References
to later groups are unknown-index errors. A bare subtype is a
singleton rec group by construction (canonicalization inherits
bare≡singleton automatically).

**Supertype rules** (`valid.ml:161–177` check_subtype_sub): for each
supertype `xi` of member `x` in a group starting at `x0`:
(1) `xi < x0` — no self, no in-group, no forward reference;
(2) supertype must NOT be final;
(3) member comptype must MATCH the supertype comptype.

**Heap lattice** (`valid/match.ml:77–104`):
eq/struct/array/i31 ≤ any; i31/struct/array ≤ eq; struct ≤ struct,
array ≤ array (abstract); concrete-to-abstract only via its comptype
kind (struct→any/eq/struct, array→any/eq/array, func→func); bottoms
`none/nofunc/noexn/noextern` match every non-bottom of their own
hierarchy top; concrete-vs-concrete via deftype matching.

**Deftype matching** (`match.ml:151–159`): canonical/structural
equality OR the declared-supertype list (unrolled) contains the other.
**Comptype matching** (`match.ml:140–149`): struct = width ≥ and
prefix fields match; array = field matches; func = params
CONTRAVARIANT, results covariant. **Field** (`match.ml:134–139`):
mutability equal; const fields covariant (match both directions),
mutable fields invariant. Nullability: nullable matches only nullable.

**Canonicalization semantics** = OCaml structural equality on rolled
closed forms (`syntax/types.ml:264–283`): deftype = (rolled rectype,
member index); `dt1 ≡ dt2 ⟺ rolled1 = rolled2 ∧ i1 = i2`. All
external refs point to EARLIER groups (enforced by sequential
checking), so closed forms are finite trees — canonicalization
terminates on all inputs.

## Changes

1. **`types.rs`**: `HeapType` gains `Any None Eq I31 Struct Array
   NoFunc NoExtern NoExn` (keep `Func Extern Exn Type(TypeIdx)`). New
   `StorageType { Val(ValType), I8, I16 }`, `FieldType { mutable,
   storage }`, `CompType { Struct(Vec<FieldType>), Array(FieldType),
   Func(FuncType) }`, `SubType { is_final, supertypes: Vec<u32>,
   comp }`. `Module.types: Vec<FuncType>` → flat `Vec<SubType>` +
   `Vec<(u32,u32)>` rec-group spans. Existing `types()` consumers get
   a `func_type(idx) -> Option<&FuncType>` view (func comps only);
   fix workspace fallout mechanically; EH paths keep identical
   behavior (their tests are the regression bar).
2. **`binary/typesec.rs`**: decode the full grammar above (all rows).
   Malformed bytes → existing error kinds (`UnexpectedByte` shape).
3. **`validate/mod.rs`**: type-section validation per roll&install +
   supertype rules; replace `HeapType::is_subtype_of` with the full
   lattice (valtype/reftype/heaptype/comptype match fns, rules above);
   update every existing call site.
4. **Canonicalization** (`validate` or new `types.rs` module):
   hash-consed registry — rolled group key = (external refs as
   canonical member ids, in-group refs as relative idx); group canon
   id → member canon id; equivalence = canon equality. Must agree
   with reference structural equality (property test below).
5. **No IR change** (RegTerms untouched). AOT: if any serialized shape
   changes, bump v7 HERE and note it; verify by roundtrip test. If
   unchanged, v7 stays in U2. Ambiguity → report, don't guess.
6. **Corpus grind (validation level)**: vendor the 4 files (`type`,
   `type-rec`, `type-canon`, `type-equivalence`) into testdata; add a
   validation-mode runner to the spec harness that processes
   `assert_malformed`/`assert_invalid` and validates runnable modules
   without instantiating feature-dependent bodies (count skips,
   report). Suite total stays 178/0 — full inclusion is U5's grind.

## Tests (exact, TDD — red before green)

- `gc_type_binary.rs` (≥12): every grammar row + malformed variants
  (mut byte 2, unknown heaptype 0x68, bad packtype 0x79, `0x4E` with
  trailing garbage, empty rec group `0x4E 0x00` VALID, sub vec then
  bad comptype byte).
- `gc_type_validation.rs` (≥10): supertype forward-use invalid;
  in-group supertype invalid; final supertype invalid; width
  violation; mutable-field variance violation; param covariance
  rejected (contravariant required); nullability mismatch; unknown
  type index in field; bottom-of-wrong-hierarchy mismatch; valid
  multi-supertype module (per pin).
- `gc_type_canon.rs` (≥8): same group declared twice → same canon;
  bare ≡ singleton; in-group vs external ref distinction;
  cross-group external inlining equivalence; non-equivalent
  permutations distinct; property sweep over every group in the 4
  vendored files: canon-eq classes match hand-derived reference
  classes (author derives from rolled forms).
- EH regression: full EH suite green (RecProvenance seam intact).
- Official suite unchanged: 178 files / 53,543 assertions / 0 failures.

## Rule-7 ambiguity report

Anything the reference leaves genuinely ambiguous, or any deviation
forced by our architecture (e.g., `Vec<FuncType>` fallout reaching
public API), gets REPORTED in the dispatch result, not decided
silently.

## Verification gates

`cargo fmt --all`; `cargo clippy --workspace --all-targets
--all-features -- -D warnings`; `cargo clippy -p baedeker-core
--all-targets --no-default-features -- -D warnings`; `cargo test
--workspace`; `cargo test -p baedeker-core --features aot`; official
suite run (178/0, deferred still 7).
