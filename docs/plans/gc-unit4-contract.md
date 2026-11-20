# GC — Unit 4 contract: surface completion (types at boundaries, const exprs, runner, FFI, boundary tightening, smith)

Branch `feature/gc-slice`, on top of U3 (`751a500`). Scope: everything
the corpus needs to RUN (U5 grinds R0 to zero; i31.wast/extern.wast/
type-linking files require this unit). NOT in scope: the R0 corpus
itself, articles/harness (U5), collector (5b).

## Pinned from the reference (@ `dfa3f32`; quote-trust over intuition)

### Const-expr eligibility (valid.ml:1029-1040 — U2-era verification, re-quoted)

```
let is_const (c : context) (e : instr) =
  match e.it with
  | Const _ | VecConst _
  | Binary (Value.I32 I32Op.(Add | Sub | Mul))
  | Binary (Value.I64 I64Op.(Add | Sub | Mul))
  | RefNull _ | RefFunc _ | RefI31
  | StructNew _ | ArrayNew _ | ArrayNewFixed _
  | ExternConvert _ -> true
  | GlobalGet x -> let GlobalT (mut, _t) = global c x in mut = Cons
  | _ -> false

let check_const (c : context) (const : const) (t : valtype) =
  require (List.for_all (is_const c) const.it) const.at
    "constant expression required";
  check_block c const.it (InstrT ([], [t], [])) const.at
```

NOTE (corrected pre-dispatch, pin-verified): `StructNew`/`ArrayNew`
carry an `initop` (explicit|implicit) — the wildcard patterns match
BOTH forms, so `struct.new_default`/`array.new_default` ARE
const-eligible (ast.ml:228,231). An earlier draft of this contract
asserted the opposite; the pin wins. `GlobalGet` requires the
immutable kind. `ArrayNewFixed` is separately listed.

### Result patterns for ref results (script/runner.ml:464-483)

```
let assert_ref_pat r p =
  match p, r with
  | RefPat r', r -> Value.eq_ref r r'.it
  | RefTypePat Types.AnyHT, Instance.FuncRef _ -> false
  | RefTypePat Types.AnyHT, _
  | RefTypePat Types.EqHT, (I31.I31Ref _ | Aggr.StructRef _ | Aggr.ArrayRef _)
  | RefTypePat Types.I31HT, I31.I31Ref _
  | RefTypePat Types.StructHT, Aggr.StructRef _
  | RefTypePat Types.ArrayHT, Aggr.ArrayRef _ -> true
  | RefTypePat Types.FuncHT, Instance.FuncRef _
  | RefTypePat Types.ExnHT, Exn.ExnRef _
  | RefTypePat Types.ExternHT, _ -> true
  | NullPat _, Value.NullRef -> true
  | _ -> false
```

- `AnyHT` pattern EXCLUDES funcrefs (but matches host-any/GC/i31/null
  non-func); `ExternHT` matches EVERY ref INCLUDING null (the row is
  `| RefTypePat Types.ExternHT, _ -> true` with no null exclusion —
  r1 corrected the contract's "non-null" phrasing); `EqHT` =
  i31/struct/array only; NullPat matches only null. Ref patterns apply
  ONLY to reference values (numeric values dispatch numeric patterns —
  a ref pattern never matches a number). Host-identity patterns match
  the same zone only (HostRef≡HostRef, wrapped≡wrapped — never
  cross-zone).

### RefHost (script/js.ml:196 `ref.extern`; runner arg wiring)

`ref.extern N` produces the host value N as an externref. Current
runner: args map `RefHost(v) → Value::ExternRef(Some(v))` ✓; RefHost
RESULT patterns return `false` (runtime_official.rs:974) — must become
real matching (eq_ref identity on host handles; RefTypePat rows per
runner.ml above).

### Cross-module type identity (CORRECTED post-implementation — the
original section asserted a type-import/export entity that DOES NOT
EXIST at this pin; caught by the implementer, verified by the
orchestrator at decode.ml:305-313)

```
decode.ml:307-313: 0x00 func | 0x01 table | 0x02 mem | 0x03 global
                    | 0x04 TAG  <- not a type kind
types.ml:46:        five extern kinds only
```

Type identity travels through the `typeuse` INSIDE function (and tag)
import types: `match_externtype` (eval.ml:1187, direction: provided
must match declared) reduces func-vs-func to `match_deftype`
(match.ml:145-150): equal rolled closed forms (rec-group shape and
position included) OR provided's declared-supertype chain reaches
declared. This is NOT the covariant match_comptype relation. U1's
`Imports::with_module_types` + U2's provider-heap registration wire
the seam; a re-export chain is import-then-wrap.

## Current-state gaps (verified by orchestrator recon — confirm, then close)

1. (SUPERSEDED by the correction above: no Type import kind exists at
   this pin. The gap was instead the LINKING seam: imported funcs'
   declared type entries + canonical identity comparison + runner
   wiring — implemented as `RegImport.type_idx` + `defined_type_matches`
   + `with_module_types`/`provider_heap` in the official runner.)
2. `RegConstInstr` (lower/mod.rs:165+) lacks: `ref.null` with GC
   heaptypes (current `RefNull` is funcref-shorthand ONLY), `RefI31`,
   `StructNew`/`StructNewDefault` (both initop forms), `ArrayNew`/
   `ArrayNewDefault` (both const per is_const — initop wildcard),
   `ArrayNewFixed`, `ExternConvert` both ways.
   Const-expr VALIDATION must accept exactly the is_const set;
   const-expr EVALUATION (globals/tables/elems init) must execute
   them against the heap with U2's allocation discipline (fallible,
   stats counted, refusal = clean ResourceLimitExceeded at
   instantiation; no partial instance).
3. Runner: RefHost results unsupported (`:974`); ref-result patterns
   (RefTypePat/NullPat/RefPat-equivalent) unimplemented.
4. FFI: GC value surface deferred here from U2 (contract amendment
   `371dfa3`). Model: `Value::I31` maps to an i32-typed FFI variant;
   `GcRef`/`HostAny`/`ExternGc` map to OPAQUE u64 handle tokens into a
   per-instance registry (token → Value back-conversion); tokens valid
   for the store's lifetime (no release API in U4 — collector
   interaction is 5b). NO raw heap pointers cross the FFI.
5. Funcref boundary tightening (U2/U3 ledger decision — U4 owns it):
   `value_satisfies`/`concrete_value_matches` for `(ref $ft)` params
   must compare the FuncRef's actual function type (instance+index →
   its FuncType closed form) against the expected declared type
   structurally — the current "any FuncRef passes" is observably
   wrong (r2 evidence: incompatible funcref returned 7). Keep
   `HeapType::Func` (abstract) accepting all funcrefs (correct per
   lattice); tighten only CONCRETE function types.

## Changes

- `types.rs`/`lower/mod.rs`/`runtime/store.rs`: `defined_type_matches`
  (pin match_deftype), `RegImport.type_idx`, canonical func-type
  identity for boundary/call checks; runner `with_module_types` +
  `provider_heap` wiring; link groups register provider heaps per U2
  (now symmetric — see ambiguity #7).
- `validate/mod.rs`: const-expr eligibility per is_const (accept BOTH
  initop forms; reject mutable GlobalGet and non-const instrs in const
  contexts with the pin's message shapes); type-import matching (quote
  the direction!).
- `lower/mod.rs`: RegConstInstr extension + lowering of GC const
  forms; table/elem/global init plumbing unchanged in shape.
- `runtime/mod.rs`: const-expr evaluation for inits (heap-allocating
  forms follow U2 discipline); funcref concrete-boundary tightening;
  FFI handle registry (token minting via fallible map insert).
- `baedeker-ffi/src/value.rs`: I31 + opaque handle variants, match
  arms, roundtrip tests.
- Runner (`tests/runtime_official.rs` + `tests/spec.rs`): RefHost
  result matching (eq_ref on host handles; ExternHT-matches-any row);
  RefTypePat rows per runner.ml:464-483; type-importing module wiring
  in the official corpus runner (instantiate sequence carries module
  types so cross-module concrete linking works end-to-end).
- Smith (R1): extend the wasm-smith-driven fuzz target to emit GC type
  sections + a conservative instruction subset (struct/array/i31
  basics) behind a feature flag; target = decode+validate+lower
  acceptance without panic (no execution differential in U4).
- AOT: new const forms serialize → **AOT v11**; ladder rejects v10.

## Test files

- `gc_u4_type_linking.rs` — type import/export across modules: same
  structural type binds (A=C); different rejects; declared subtype of
  imported binds (direction per pin); re-export chains; heap
  provenance through imported-type-driven allocation.
- `gc_u4_surface.rs` — globals/tables/elems with GC types: immutable
  global initialized by struct.new/struct.new_default/ref.i31/
  any.convert_extern/array.new/array.new_default/array.new_fixed;
  table (ref $t) with init expr; elem segments with GC const exprs;
  const REJECTIONS: mutable GlobalGet, non-const instr — exact pin
  error text (array.new_default in a GLOBAL init is ACCEPTED — pin).
- `gc_u4_runner.rs` — RefHost result matching (identity, ExternHT-any,
  AnyHT-excludes-func, EqHT kinds, NullPat); a module returning
  `(ref.extern N)` asserted equal.
- `gc_u4_ffi.rs` — handle minting: GcRef→token→GcRef identity; I31
  direct; HostAny/ExternGc tokens; unknown token → clean error.
- `gc_u4_funcref_boundary.rs` — concrete `(ref $ft)` param rejects
  incompatible funcref (the r2 "returned 7" case), accepts exact;
  abstract func still accepts all.
- Smith smoke behind feature (fuzz target builds + one seeded run in
  CI-shape).

## Rule-7 ambiguity report

Report: the import-type matching direction as quoted (sub vs super);
any corpus file the runner STILL cannot express after this unit
(gap list for U5); FFI token stability choices.

## Verification gates

`cargo fmt -- --check`; clippy `--all-targets -- -D warnings`
(workspace + `-p baedeker-core --features aot`); `cargo test
--workspace`; `cargo test -p baedeker-core --features aot`; official
suite UNCHANGED `files=178 deferred=7 directives=55439 modules=1854
assertions=53543 failures=0`; EH suites green; AOT v11 roundtrip +
v10 rejected.
