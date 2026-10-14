// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! EH unit 4: cross-instance tag linking and the shared exception arena.
//!
//! Each module goes through the full pipeline (WAT → encode → decode →
//! lower) and instances are linked through one [`LinkGroup`] and one
//! exception arena, mirroring the embedder harness's `register`-based
//! linking.

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::{RegExportDesc, RegModule};
use baedeker_core::runtime::{
    ExceptionRecord, Imports, LinkGroup, RecProvenance, RuntimeError, RuntimeErrorKind, Store,
    Table, TagBinding, Value, execute_export, link_func,
};
use baedeker_core::types::{FuncIdx, FuncType, Limits, NumType, RefType, TableType, ValType};

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

/// The binding of a `(export "name" (tag idx))`, mirroring what the U5
/// harness resolves for `register`ed tag exports.
fn tag_export_binding(store: &Store, module: &RegModule, name: &str) -> Option<TagBinding> {
    module
        .exports
        .iter()
        .find_map(|export| match (&export.name[..], export.desc) {
            (n, RegExportDesc::Tag(idx)) if n == name => store.tag_binding(idx.0).cloned(),
            _ => None,
        })
}

/// Link the function exported by `source` under `name` into `caller_store`'s
/// `(module, name)` import.
fn link_export(
    caller_store: &Rc<RefCell<Store>>,
    source: &Rc<RegModule>,
    source_store: &Rc<RefCell<Store>>,
    module: &str,
    name: &str,
) {
    let idx = source
        .exports
        .iter()
        .find_map(|export| match (&export.name[..], export.desc) {
            (n, RegExportDesc::Func(idx)) if n == name => Some(idx),
            _ => None,
        })
        .expect("function export exists");
    let ty = export_func_type(source, idx);
    let host = link_func(source.clone(), source_store.clone(), idx, ty);
    caller_store
        .borrow_mut()
        .register_host_func(module, name, host)
        .expect("host function registers");
}

/// The declared [`FuncType`] of a defined function index.
fn export_func_type(module: &RegModule, idx: FuncIdx) -> FuncType {
    let func = module
        .funcs
        .iter()
        .find(|func| func.idx == idx)
        .expect("exported function is defined");
    module.types[func.type_idx.0 as usize].clone()
}

#[test]
fn defined_tag_binds_to_own_instance() {
    let module = Rc::new(wat_module(
        r#"
(module
  (tag $e0 (param i32))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch $e0 $h)
        i32.const 42
        throw $e0)
      i32.const 7)))
"#,
    ));
    let mut group = Group::new();
    let store = group.add(&module, &Imports::new());
    assert_eq!(
        execute_export(&module, &store.borrow(), "f", &[]),
        Ok(vec![Value::I32(42)])
    );
    assert_eq!(
        store.borrow().tag_binding(0).map(|b| b.identity),
        Some((1, 0))
    );
}

#[test]
fn cross_instance_catch_through_import() {
    let a = Rc::new(wat_module(
        r#"
(module
  (tag $e0 (param i32))
  (export "e0" (tag $e0))
  (func (export "throw") (param i32)
    local.get 0
    throw $e0))
"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "e0" (tag $e (param i32)))
  (import "a" "throw" (func $a_throw (param i32)))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch $e $h)
        i32.const 42
        call $a_throw)
      i32.const 7)))
"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    assert_eq!(binding.identity, (1, 0));
    assert_eq!(binding.ty.params, vec![ValType::Num(NumType::I32)]);

    let imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    let b_store = group.add(&b, &imports);
    link_export(&b_store, &a, &a_store, "a", "throw");

    assert_eq!(
        execute_export(&b, &b_store.borrow(), "f", &[]),
        Ok(vec![Value::I32(42)])
    );
}

#[test]
fn aliased_tag_imports_share_identity() {
    let a = Rc::new(wat_module(
        r#"
(module
  (tag $e0 (param i32))
  (export "e0" (tag $e0)))
"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "e0" (tag $e_a (param i32)))
  (import "a" "e0" (tag $e_b (param i32)))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch $e_b $h)
        i32.const 5
        throw $e_a)
      i32.const 0)))
"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let imports = Imports::new()
        .tag(
            "a",
            "e0",
            binding.identity,
            binding.ty.clone(),
            binding.expanded_params.clone(),
            RecProvenance::Bare,
        )
        .tag(
            "a",
            "e0",
            binding.identity,
            binding.ty.clone(),
            binding.expanded_params.clone(),
            RecProvenance::Bare,
        );
    let b_store = group.add(&b, &imports);

    let b_guard = b_store.borrow();
    let first = b_guard
        .tag_binding(0)
        .expect("first import binding")
        .identity;
    let second = b_guard
        .tag_binding(1)
        .expect("second import binding")
        .identity;
    assert_eq!(first, binding.identity);
    assert_eq!(second, binding.identity);
    assert_eq!(
        execute_export(&b, &b_guard, "f", &[]),
        Ok(vec![Value::I32(5)])
    );
}

#[test]
fn importer_throw_caught_in_exporter() {
    let a = Rc::new(wat_module(
        r#"
(module
  (import "shared" "t" (table $t 1 funcref))
  (tag $e0 (param i32))
  (export "e0" (tag $e0))
  (type $cb (func (param i32)))
  (func (export "wrap") (result i32)
    (block $h (result i32)
      (try_table (catch $e0 $h)
        i32.const 42
        i32.const 0
        call_indirect $t (type $cb))
      i32.const 7)))
"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "shared" "t" (table $t 1 funcref))
  (import "a" "e0" (tag $e (param i32)))
  (import "a" "wrap" (func $wrap (result i32)))
  (func $boom (param i32)
    local.get 0
    throw $e)
  (elem (i32.const 0) $boom)
  (func (export "f") (result i32)
    call $wrap))
"#,
    ));

    let mut group = Group::new();
    let table_ty = TableType {
        elem: RefType::FuncRef,
        limits: Limits {
            min: 1,
            max: None,
            is64: false,
        },
        init: None,
    };
    let table = Rc::new(RefCell::new(
        Table::new(table_ty.clone(), Value::FuncRef(None)).expect("table allocates"),
    ));
    let a_store = group.add(
        &a,
        &Imports::new().shared_table("shared", "t", table_ty.clone(), table.clone()),
    );
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let b_imports = Imports::new()
        .shared_table("shared", "t", table_ty, table)
        .tag(
            "a",
            "e0",
            binding.identity,
            binding.ty.clone(),
            binding.expanded_params.clone(),
            RecProvenance::Bare,
        );
    let b_store = group.add(&b, &b_imports);
    link_export(&b_store, &a, &a_store, "a", "wrap");

    assert_eq!(
        execute_export(&b, &b_store.borrow(), "f", &[]),
        Ok(vec![Value::I32(42)])
    );
}

#[test]
fn incompatible_tag_type_unlinkable() {
    let a = Rc::new(wat_module(
        r#"
(module
  (tag $e0 (param i32))
  (export "e0" (tag $e0)))
"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "e0" (tag $e (param f64)))
  (func (export "f")))
"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    // The provided tag is `(param i32)`; the declaration wants `(param f64)`.
    let imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    let error = group
        .try_add(&b, &imports)
        .expect_err("type mismatch unlinkable");
    let message = format!("{error:?}");
    assert!(
        message.contains("ImportTypeMismatch"),
        "expected import-type-mismatch, got {message}"
    );
}

#[test]
fn unknown_tag_import_fails() {
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "e0" (tag $e (param i32)))
  (func (export "f")))
"#,
    ));

    let mut group = Group::new();
    let error = group
        .try_add(&b, &Imports::new())
        .expect_err("unknown import unlinkable");
    let message = format!("{error:?}");
    assert!(
        message.contains("UnknownImport"),
        "expected unknown-import, got {message}"
    );
}

#[test]
fn exnref_across_instances() {
    let a = Rc::new(wat_module(
        r#"
(module
  (tag $e0 (param i32))
  (export "e0" (tag $e0))
  (import "b" "relay" (func $relay))
  (func (export "outer") (result i32)
    (block $h (result i32)
      (try_table (catch $e0 $h)
        call $relay)
      i32.const 0)))
"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (import "a" "e0" (tag $e (param i32)))
  (func (export "relay")
    (local $exn exnref)
    (block $l (result i32 (ref exn))
      (try_table (result i32 (ref exn)) (catch_ref $e $l)
        i32.const 99
        throw $e))
    local.set $exn
    local.get $exn
    throw_ref))
"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let b_imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    let b_store = group.add(&b, &b_imports);
    // A calls B's relay, which catch_ref's A's tag into an exnref, rethrows
    // it, and A's outer frame catches the rethrown (arena-global) exnref.
    link_export(&a_store, &b, &b_store, "b", "relay");

    assert_eq!(
        execute_export(&a, &a_store.borrow(), "outer", &[]),
        Ok(vec![Value::I32(99)])
    );
}

#[test]
fn multi_instance_ids_distinct() {
    let a = Rc::new(wat_module(
        r#"
(module
  (tag $a (param i32))
  (export "a" (tag $a))
  (func (export "throw_a") (param i32)
    local.get 0
    throw $a))
"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (tag $b (param i32))
  (export "b" (tag $b))
  (func (export "throw_b") (param i32)
    local.get 0
    throw $b))
"#,
    ));
    let c = Rc::new(wat_module(
        r#"
(module
  (import "a" "a" (tag $ea (param i32)))
  (import "b" "b" (tag $eb (param i32)))
  (import "a" "throw_a" (func $ta (param i32)))
  (import "b" "throw_b" (func $tb (param i32)))
  (func (export "f") (result i32)
    (block $h (result i32)
      (try_table (catch $ea $h)
        i32.const 42
        call $tb)
      i32.const 0))
  (func (export "g") (result i32)
    (block $h (result i32)
      (try_table (catch $ea $h)
        i32.const 42
        call $ta)
      i32.const 0)))
"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let a_binding = tag_export_binding(&a_store.borrow(), &a, "a").expect("tag export binding");
    let b_store = group.add(&b, &Imports::new());
    let b_binding = tag_export_binding(&b_store.borrow(), &b, "b").expect("tag export binding");
    assert_eq!(a_binding.identity, (1, 0));
    assert_eq!(b_binding.identity, (2, 0));
    assert_ne!(a_binding.identity, b_binding.identity);

    let c_imports = Imports::new()
        .tag(
            "a",
            "a",
            a_binding.identity,
            a_binding.ty,
            a_binding.expanded_params.clone(),
            RecProvenance::Bare,
        )
        .tag(
            "b",
            "b",
            b_binding.identity,
            b_binding.ty,
            b_binding.expanded_params.clone(),
            RecProvenance::Bare,
        );
    let c_store = group.add(&c, &c_imports);
    link_export(&c_store, &a, &a_store, "a", "throw_a");
    link_export(&c_store, &b, &b_store, "b", "throw_b");

    let c_guard = c_store.borrow();
    // A's own tag is caught.
    assert_eq!(
        execute_export(&c, &c_guard, "g", &[]),
        Ok(vec![Value::I32(42)])
    );
    // B's structurally-identical tag has a distinct identity and slips past
    // the clause naming A's tag.
    let error = execute_export(&c, &c_guard, "f", &[]).expect_err("distinct tag not caught");
    assert!(
        matches!(error.kind, RuntimeErrorKind::UncaughtException { .. }),
        "expected uncaught exception, got {error:?}"
    );
}

/// Review r1 P1-1 (false-positive direction): tags whose parameters
/// reference concrete types must not compare equal by raw index — the
/// two modules below declare structurally DIFFERENT `$p` types that a
/// numeric-index comparison would alias. Linking must fail.
#[test]
fn different_concrete_ref_params_do_not_link() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $p (func (param i32)))
  (tag $e0 (param (ref null $p)))
  (export "e0" (tag $e0)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $p (func (param f64)))
  (import "a" "e0" (tag $e (param (ref null $p)))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    let error = group
        .try_add(&b, &imports)
        .expect_err("structurally different typed-ref params must not link");
    let msg = format!("{error:?}");
    assert!(
        msg.contains("ImportTypeMismatch"),
        "expected import type mismatch, got {msg}"
    );
}

/// Review r1 P1-1 (false-negative direction): identical typed-tag
/// structures declared at different type indices must compare EQUAL
/// after expansion — this link must succeed.
#[test]
fn identical_typed_tags_at_different_indices_link() {
    // `$p` sits at type index 1 in A (a dummy type pads it) but index 0
    // in B: raw-index comparison would wrongly fail this link.
    let a = Rc::new(wat_module(
        r#"
(module
  (type $d (func))
  (type $p (func (param i32)))
  (tag $e0 (param (ref null $p)))
  (export "e0" (tag $e0)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $p (func (param i32)))
  (import "a" "e0" (tag $e (param (ref null $p)))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty.clone(),
        binding.expanded_params.clone(),
        RecProvenance::Bare,
    );
    group
        .try_add(&b, &imports)
        .expect("structurally identical typed tags must link");
}

/// Review r2 P1-1: a reference to a type inside a rec group must not
/// link-equal a reference to a bare type — nested rec-group identity
/// needs canonicalization we do not have, so such tags reject
/// conservatively.
#[test]
fn nested_rec_reference_rejects_against_bare() {
    let a = Rc::new(wat_module(
        r#"
(module
  (rec (type $t1 (func)) (type $t2 (func)))
  (tag $e0 (param (ref null $t1)))
  (export "e0" (tag $e0)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $t (func))
  (import "a" "e0" (tag $e (param (ref null $t)))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty,
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    let error = group
        .try_add(&b, &imports)
        .expect_err("rec-referencing tag must not link against a bare type");
    let msg = format!("{error:?}");
    assert!(
        msg.contains("ImportTypeMismatch"),
        "expected import type mismatch, got {msg}"
    );
}

/// Review r2 P1-2: a tag whose rec group contains an unexpandable
/// (cyclic) sibling must itself be Unresolvable — it must not link as
/// if it were a bare type.
#[test]
fn cyclic_sibling_group_tag_rejects_against_bare() {
    let a = Rc::new(wat_module(
        r#"
(module
  (rec
    (type $a (func (param (ref null $a))))
    (type $b (func)))
  (tag $e0 (type $b))
  (export "e0" (tag $e0)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $t (func))
  (import "a" "e0" (tag $e (type $t))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "e0").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "e0",
        binding.identity,
        binding.ty,
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    let error = group
        .try_add(&b, &imports)
        .expect_err("tag with cyclic sibling must not link as bare");
    let msg = format!("{error:?}");
    assert!(
        msg.contains("ImportTypeMismatch"),
        "expected import type mismatch, got {msg}"
    );
}

/// Review r3 P1: bare types are implicit singleton rec groups in the
/// reference's canonicalization — a bare tag type and a one-member rec
/// group of the same shape are canonically EQUAL and must link.
#[test]
fn singleton_rec_group_links_against_bare() {
    let a = Rc::new(wat_module(
        r#"
(module
  (rec (type $f (func (param i32))))
  (tag (export "t") (type $f)))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $f (func (param i32)))
  (import "a" "t" (tag $e (type $f))))"#,
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    let binding = tag_export_binding(&a_store.borrow(), &a, "t").expect("tag export binding");
    let imports = Imports::new().tag(
        "a",
        "t",
        binding.identity,
        binding.ty,
        binding.expanded_params.clone(),
        binding.rec.clone(),
    );
    group
        .try_add(&b, &imports)
        .expect("canonically equal singleton tag types must link");
}
