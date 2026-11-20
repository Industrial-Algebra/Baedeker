# EH Unit 4 Contract — linking, cross-instance identity, AOT v4

Unit: U4 of the EH ladder (`docs/plans/eh-slice.md`). Depends on: U3
(`809794a`). Followed by: U5 (vendoring + grind).

Corpus drivers (try_table.wast/tag.wast/imports.wast at the pin):
`register` + `(tag $a (import "test" "e0"))` + `(tag $b (import "test"
"e0"))` — two imports of one tag must be the SAME identity, and a throw
from the exporting instance must be caught through either import.
`(assert_unlinkable … "incompatible import type")` for tag type
mismatches.

## Design (pinned)

- **Shared exception arena**: `Store.exceptions` becomes
  `Rc<RefCell<Vec<ExceptionRecord>>>`. Instances in one link group share
  ONE arena (created by the embedder — the harness Registry — and passed
  to every `instantiate_linked` call); standalone `Store::instantiate`
  creates its own. Consequence: `Value::ExnRef(Some(idx))` and
  `Throw { exn }` indices are meaningful across the whole group, and
  `RuntimeErrorKind::Throw` needs NO instance field. Records stay
  resolvable from any member store.
- **Tag identity** remains `(instance_id, module_local_tag_idx)`, but the
  consult no longer assumes the thrower is `store.instance_id()`:
  each Store gains `tag_map: Vec<TagBinding>` (module-local idx →
  binding), filled at instantiation: imported tags bind to the
  PROVIDER's identity, defined tags to `(own_instance_id, own_idx)`.
  The consult compares `record.tag == tag_map[clause_tag].identity`.
- **AOT v4**: `RegModule` gains `imported_tags` (serialized field).

## Files

Modify:
- `crates/baedeker-core/src/lower/mod.rs` — `ImportedTag`, `RegModule.imported_tags`, fill in `lower_module`
- `crates/baedeker-core/src/runtime/store.rs` — arena sharing, `TagBinding`, `tag_map`, `Imports::tag`, `instantiate_linked` arena param, import resolution
- `crates/baedeker-core/src/runtime/mod.rs` — consult uses `tag_map`
- `crates/baedeker-core/src/aot.rs` — AOT_FORMAT_VERSION 3 → 4, `found: 3` rejection case
- `crates/baedeker-core/src/runtime/gpu.rs`, `crates/baedeker-ffi` — only if RegModule literal/exhaustive-match compile fixes are needed (report any)

Create:
- `crates/baedeker-core/tests/eh_linking.rs`

## Contracts

### 1. `lower/mod.rs`

```rust
/// A tag import declared by the lowered module (state-import style).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImportedTag {
    pub module: String,
    pub name: String,
    /// The tag's function type (params; results are empty by validation).
    pub ty: FuncType,
}
```

`RegModule` gains `pub imported_tags: Vec<ImportedTag>,` (LAST field).
In `lower_module`, mirroring how `imported_memories` is collected: walk
`module.imports`; for `ImportDesc::Tag(type_idx)` resolve `type_idx`
through the module's types (the same resolution funcs use) into a
`FuncType` and push `ImportedTag { module, name, ty }`. Initialize
`imported_tags: Vec::new()` in every other `RegModule` construction
site (grep).

### 2. `runtime/store.rs`

(a) Arena sharing:

```rust
    /// Shared exception arena: one `Rc` per link group (cloned into every
    /// member store at instantiation) or per standalone store. Indices in
    /// `Value::ExnRef` / `Throw { exn }` are arena-global within the group.
    exceptions: alloc::rc::Rc<core::cell::RefCell<Vec<ExceptionRecord>>>,
```

All existing uses go through the `Rc` (borrow_mut to allocate,
borrow to read — the existing accessor bodies stay, just deref through
the `Rc`).

`instantiate_linked` signature gains the shared arena:

```rust
    pub fn instantiate_linked(
        module: &RegModule,
        imports: &Imports,
        group: &Rc<RefCell<LinkGroup>>,
        instance_id: u32,
        shared_exceptions: &Rc<RefCell<Vec<ExceptionRecord>>>,
    ) -> Result<Rc<RefCell<Store>>, RuntimeError>
```

(adapt to the real current signature — keep everything else identical;
the new store clones `shared_exceptions`). `Store::instantiate` and
`instantiate_with_imports` create a fresh arena `Rc`. Grep every
caller of the changed constructor and fix (report each).

(b) Tag bindings:

```rust
/// A resolved tag binding: the global identity of the tag plus its type.
#[derive(Debug, Clone, PartialEq)]
pub struct TagBinding {
    /// `(instance_id, module_local_tag_idx)` of the DEFINING instance.
    pub identity: (u32, u32),
    pub ty: FuncType,
}
```

`Store` gains `tag_map: Vec<TagBinding>` (module-local index space:
imported tags first, then defined — mirroring the validator/lowerer).
Accessor: `pub fn tag_binding(&self, module_local_idx: u32) -> Option<&TagBinding>`.

(c) `Imports` gains tag providers (match the memory/global provider
style — private struct + builder):

```rust
struct TagProvider {
    module: String,
    name: String,
    identity: (u32, u32),
    ty: FuncType,
}
```

builder on `Imports`:

```rust
    /// Provide an imported tag `(module, name)` bound to the defining
    /// instance's tag identity (from that instance's `tag_binding`).
    pub fn tag(mut self, module: &str, name: &str, identity: (u32, u32), ty: FuncType) -> Self
```

(d) In `instantiate_internal` (eager, alongside memories/globals/
tables): for each `module.imported_tags[i]` (in declaration order):
find a provider with matching `(module, name)` — else
`unknown_import` (same error the memory path uses). Type check:
`provider.ty.params == declared.ty.params` (results are empty on both
sides by validation; compare params exactly) — else the same
`import_type_mismatch` error the memory path uses. Push
`TagBinding { identity: provider.identity, ty: provider.ty }`.
Then for each defined tag `i` (module-local index continues after the
imports): resolve `module.tags[i]` through the module's types (the
lowerer's `RegModule` carries the types the same way `lower_module`'s
function typing does — find where instantiate already reads module
types, e.g. for call_indirect signatures, and reuse) and push
`TagBinding { identity: (instance_id, imported_count + i as u32), ty }`.
Also register defined-tag types via the existing `register_tag_type`.

### 3. `runtime/mod.rs`

`consult_catch`: replace `record.tag != (store.instance_id(), tag.0)`
with a lookup through the binding:

```rust
Some(tag) => {
    let Some(binding) = store.tag_binding(tag.0) else {
        continue;
    };
    if record.tag != binding.identity {
        continue;
    }
    ...
}
```

No other runtime changes (the shared arena makes cross-instance records
resolvable unchanged).

### 4. `aot.rs`

`AOT_FORMAT_VERSION` 3 → 4 + a `found: 3` rejection case (comment: v3
predates `RegModule.imported_tags`).

### 5. Tests — `tests/eh_linking.rs`

License header. Helpers: the `wat_module` WAT pipeline from
`eh_execution.rs`; plus a two-instance linker:

```rust
struct Group {
    group: Rc<RefCell<LinkGroup>>,
    arena: Rc<RefCell<Vec<ExceptionRecord>>>,
    next_id: u32,
}
impl Group {
    fn new() -> Self { /* id 1.. */ }
    fn add(&mut self, module: &RegModule, imports: &Imports) -> Rc<RefCell<Store>> {
        /* instantiate_linked with shared arena; assign fresh id; */
    }
}
fn tag_export_binding(store: &Store, module: &RegModule, name: &str) -> Option<(TagBinding-like)>
```

(The export helper scans `module.exports` for `RegExportDesc::Tag(idx)`
with matching name and reads `store.tag_binding(idx)` — this mirrors
what the U5 harness will do for `register`ed tag exports.)

Tests (all spec-text modules):

1. `defined_tag_binds_to_own_instance` — one module, one defined tag,
   catch works (regression), and `tag_binding(0).identity == (1, 0)`.
2. `cross_instance_catch_through_import` — module A exports tag `$e0`
   (param i32) + a throwing function; module B imports the tag and the
   function; B's try_table catches A's throw via the imported tag;
   returns the payload.
3. `aliased_tag_imports_share_identity` — module B imports the same
   exported tag TWICE (two names/same source); a throw of one is
   caught by a clause naming the other.
4. `importer_throw_caught_in_exporter` — module B imports A's tag and
   throws it; A's function (called by B through an imported funcref)
   catches it. (Direction: thrower = importer, catcher = exporter.)
5. `incompatible_tag_type_unlinkable` — provider type `(param i32)`,
   declared `(param f64)` → `instantiate_linked` errors with the
   import-type-mismatch message.
6. `unknown_tag_import_fails` — no provider for the declared tag →
   unknown-import error.
7. `exnref_across_instances` — B catch_ref's A's exception and
   rethrows via throw_ref; A's caller catches (arena shared, exnref
   index group-global).
8. `multi_instance_ids_distinct` — two groups (or two instances)
   throwing structurally identical tags; a catch clause for one does
   not match the other's tag (identity distinctness).

Enumerate every assertion in the test bodies: results compared with
`assert_eq!` on `Value`s; errors compared by `RuntimeError` Display
substring (`contains`) for the link failures.

## Constraints

- No new dependencies; `no_std` + alloc in the crate.
- serde enums/structs: new fields LAST only.
- Official suite unchanged (`failures=0`); all prior EH tests stay green
  (eh_binary 8, eh_validation 1, eh_execution 12/13).

## Completion

```
cargo test -p baedeker-core 2>&1 | grep -E 'test result|FAILED|error' | tail -25
cargo test -p baedeker-core --features aot 2>&1 | grep -E 'test result|FAILED' | tail -10
cargo clippy -p baedeker-core --all-targets -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo fmt --all -- --check && echo FMT-OK
```

## Out of scope

`runtime_official` harness changes and the `assert_exception` arm (U5),
`spectest` tags (the corpus defines none), arena reclamation.
