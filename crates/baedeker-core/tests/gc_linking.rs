// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC linking: cross-module structural matching of concrete reference
//! types for globals, tables, and tags.
//!
//! Review r2 P1 reproductions. Two defined types are equivalent exactly
//! when their rolled closed forms are structurally equal, so a provider
//! type index and a consumer type index that happen to share a number
//! must NOT link unless the underlying kinds/structures agree. Each
//! module goes through the full pipeline (WAT → encode → decode → lower)
//! and instances link through one [`LinkGroup`].

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::{RegExportDesc, RegModule};
use baedeker_core::runtime::{
    ExceptionRecord, Imports, LinkGroup, RecProvenance, RuntimeError, RuntimeErrorKind, Store,
    TagBinding,
};

/// Parse WAT, encode it to a binary, decode, and lower it.
fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

/// A link group plus the shared exception arena its members share. Instance
/// ids start at 1 (0 is reserved for standalone stores).
struct Group {
    group: Rc<RefCell<LinkGroup>>,
    arena: Rc<RefCell<Vec<ExceptionRecord>>>,
    next_id: u32,
}

impl Group {
    fn new() -> Self {
        Self {
            group: Rc::new(RefCell::new(LinkGroup::new())),
            arena: Rc::new(RefCell::new(Vec::new())),
            next_id: 1,
        }
    }

    fn add(&mut self, module: &Rc<RegModule>, imports: &Imports) -> Rc<RefCell<Store>> {
        self.try_add(module, imports).expect("module instantiates")
    }

    fn try_add(
        &mut self,
        module: &Rc<RegModule>,
        imports: &Imports,
    ) -> Result<Rc<RefCell<Store>>, RuntimeError> {
        let id = self.next_id;
        self.next_id += 1;
        Store::instantiate_linked(module, imports, &self.group, id, &self.arena)
    }
}

fn assert_import_type_mismatch(result: Result<Rc<RefCell<Store>>, RuntimeError>) {
    let error = result.expect_err("expected an import type mismatch");
    assert!(
        matches!(error.kind, RuntimeErrorKind::ImportTypeMismatch { .. }),
        "expected ImportTypeMismatch, got {error:?}"
    );
}

/// The binding of a `(export "name" (tag idx))`.
fn tag_export_binding(store: &Store, module: &RegModule, name: &str) -> Option<TagBinding> {
    module
        .exports
        .iter()
        .find_map(|export| match (&export.name[..], export.desc) {
            (n, RegExportDesc::Tag(idx)) if n == name => store.tag_binding(idx.0).cloned(),
            _ => None,
        })
}

// ── P1 #2: global linking ────────────────────────────────────────────

/// Provider global `(ref null $struct)` must not link into a consumer
/// expecting `funcref` — a concrete struct is not a function reference.
#[test]
fn global_struct_ref_mismatches_funcref() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (global (export "g") (ref null $s) (ref.null $s)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "g" (global funcref))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let (ty, shared) = a_store.borrow().export_global("g").expect("global export");

    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_global("a", "g", ty, shared);
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

/// Positive control: structurally identical struct global types link even
/// at different type indices (raw-index comparison would reject).
#[test]
fn identical_struct_globals_link_across_modules() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (global (export "g") (ref null $s) (ref.null $s)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $dummy (func))
  (type $s (struct (field i32)))
  (import "a" "g" (global (ref null $s)))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let (ty, shared) = a_store.borrow().export_global("g").expect("global export");

    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_global("a", "g", ty, shared);
    group
        .try_add(&b, &imports)
        .expect("structurally identical struct globals must link");
}

/// A mutable global is invariant: same-shape structs link, but a struct
/// must not link against a mutable func.
#[test]
fn mutable_global_struct_mismatches_func() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (global (export "g") (mut (ref null $s)) (ref.null $s)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $f (func))
  (import "a" "g" (global (mut (ref null $f))))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let (ty, shared) = a_store.borrow().export_global("g").expect("global export");
    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_global("a", "g", ty, shared);
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

// ── P1 #3: table linking ─────────────────────────────────────────────

/// Provider table element type `(ref null $struct)` (provider idx 0 =
/// struct) must not link against a consumer `(ref null $func)` (consumer
/// idx 0 = func): raw `TypeIdx` equality would alias the two.
#[test]
fn table_struct_elem_mismatches_func_elem() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (table (export "t") 1 (ref null $s)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $f (func))
  (import "a" "t" (table 1 (ref null $f)))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let (ty, shared) = a_store.borrow().export_table("t").expect("table export");
    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_table("a", "t", ty, shared);
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

/// Positive control: a table of a structurally identical struct element
/// type links across modules even at different type indices.
#[test]
fn identical_struct_tables_link_across_modules() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (table (export "t") 1 (ref null $s)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $dummy (func))
  (type $s (struct (field i32)))
  (import "a" "t" (table 1 (ref null $s)))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let (ty, shared) = a_store.borrow().export_table("t").expect("table export");
    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_table("a", "t", ty, shared);
    group
        .try_add(&b, &imports)
        .expect("structurally identical struct tables must link");
}

/// Plain `funcref` table linking is unchanged (no type context needed).
#[test]
fn plain_funcref_tables_still_link() {
    let a = Rc::new(wat_module(
        r#"
(module
  (table (export "t") 1 funcref))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "t" (table 1 funcref))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let (ty, shared) = a_store.borrow().export_table("t").expect("table export");
    let imports = Imports::new().shared_table("a", "t", ty, shared);
    group
        .try_add(&b, &imports)
        .expect("plain funcref tables must link");
}

// ── P1 #4: tag linking ───────────────────────────────────────────────

/// Provider tag parameter `(ref null $struct)` must not link against a
/// consumer parameter `(ref null $func)`: a struct-typed parameter
/// formerly expanded into the same empty `FuncType` placeholder.
#[test]
fn tag_struct_param_mismatches_func_param() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (tag (export "e") (param (ref null $s))))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $f (func))
  (import "a" "e" (tag (param (ref null $f))))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

/// Positive control: a tag parameter referencing a structurally identical
/// struct type links across modules at different type indices.
#[test]
fn identical_struct_tag_params_link_across_modules() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (tag (export "e") (param (ref null $s))))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $dummy (func))
  (type $s (struct (field i32)))
  (import "a" "e" (tag (param (ref null $s))))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    group
        .try_add(&b, &imports)
        .expect("structurally identical struct tag params must link");
}

/// Two structurally different structs must not link even though both
/// params are struct-typed (cross-module structural identity is exact).
#[test]
fn different_struct_tag_params_do_not_link() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (tag (export "e") (param (ref null $s))))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $s (struct (field i64)))
  (import "a" "e" (tag (param (ref null $s))))
  (func (export "f")))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

// ── Review r3b P1: complete defined-type identity ───────────────────

/// Provider tag parameter referencing a NON-FINAL `(sub (struct (field
/// i32)))` must not link against a consumer parameter referencing a plain
/// FINAL `(struct (field i32))`. The referenced defined types differ in
/// finality, so their complete closed forms differ even though the expanded
/// struct shapes are identical.
#[test]
fn tag_non_final_param_mismatches_final() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type (sub (struct (field i32))))
  (tag (export "e") (param (ref null 0))))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type (struct (field i32)))
  (import "a" "e" (tag (param (ref null 0)))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

/// The same finality mismatch must still be rejected after the CONSUMER has
/// been AOT serialized and deserialized: the closed form is recomputed from
/// the serialized `type_defs`, not stored in the artifact.
#[cfg(feature = "aot")]
#[test]
fn tag_non_final_param_mismatches_final_after_consumer_aot() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type (sub (struct (field i32))))
  (tag (export "e") (param (ref null 0))))"#,
    ));
    let consumer = wat_module(
        r#"
(module
  (type (struct (field i32)))
  (import "a" "e" (tag (param (ref null 0)))))"#,
    );
    let artifact = baedeker_core::aot::serialize(&consumer);
    let b =
        Rc::new(baedeker_core::aot::deserialize(&artifact).expect("consumer artifact roundtrips"));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    assert_import_type_mismatch(group.try_add(&b, &imports));
}

/// Positive control: two identical non-final `(sub (struct (field i32)))`
/// declarations must still link.
#[test]
fn identical_non_final_struct_tag_params_link() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type (sub (struct (field i32))))
  (tag (export "e") (param (ref null 0))))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type (sub (struct (field i32))))
  (import "a" "e" (tag (param (ref null 0)))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    group
        .try_add(&b, &imports)
        .expect("identical non-final struct tag params must link");
}

/// Two non-final struct params whose declared supertype lists differ (one
/// extends a base struct, the other has no supertypes) must not link: the
/// compared closed form includes the declared supertype list.
#[test]
fn tag_param_differing_supertypes_mismatch() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $base (sub (struct (field i32))))
  (type $s (sub $base (struct (field i32))))
  (tag (export "e") (param (ref null $s))))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $s (sub (struct (field i32))))
  (import "a" "e" (tag (param (ref null $s)))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    assert_import_type_mismatch(group.try_add(&b, &imports));
}
