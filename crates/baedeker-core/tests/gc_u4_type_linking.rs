// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: concrete types at module boundaries.
//!
//! The spec pin (`dfa3f32`) has NO type-import/export kind in `externtype`
//! (`interpreter/syntax/types.ml`: `ExternFuncT`/`ExternTableT`/
//! `ExternMemoryT`/`ExternGlobalT`/`ExternTagT`, with byte `0x04` taken by the
//! tag import kind), so the cross-module type identity the corpus exercises
//! (`type-equivalence.wast`, `type-canon.wast`, `type-rec.wast`) travels
//! through FUNCTION imports carrying concrete reference types. What binds is
//! the exported entity's type against the declared import type:
//!
//! ```text
//! eval.ml:1187   if not (Match.match_externtype [] xt' xt) then Link.error ...
//! ```
//!
//! `xt'` is the PROVIDER's type and `xt` the import DECLARATION, so **the
//! provided type must match the declared type**. For functions
//! `match_externtype` delegates to `match_deftype` (`valid/match.ml:175-186`):
//! the two type ENTRIES must have equal rolled closed forms — rec-group shape
//! and position included — or the provided entry must declare a supertype
//! chain reaching the declared one. So the provided type may be a SUBTYPE of
//! the declared type, never the reverse; and a params/results comparison is
//! NOT the link relation (two `(func)` types in differently shaped rec groups
//! do not match).

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    ExceptionRecord, Imports, LinkGroup, Store, Value, execute_export, link_func,
};
use baedeker_core::types::{TypeIdx, defined_type_matches};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

/// The type index a function import `pos` was declared with.
fn import_type_idx(module: &RegModule, pos: usize) -> TypeIdx {
    module.imported_funcs[pos].type_idx
}

/// The `type_idx` of a module's exported function `name`.
fn exported_func_type_idx(module: &RegModule, name: &str) -> TypeIdx {
    let idx = module
        .exports
        .iter()
        .find_map(|export| match export.desc {
            baedeker_core::lower::RegExportDesc::Func(idx) if export.name == name => Some(idx),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no function export {name:?}"));
    module
        .funcs
        .iter()
        .find(|func| func.idx == idx)
        .map(|func| func.type_idx)
        .expect("exported function is defined")
}

/// Run the link-stage comparison: the provider's type ENTRY against the
/// consumer's declared import type entry (`match_deftype`).
fn links(provider: &RegModule, provided: TypeIdx, consumer: &RegModule, declared: TypeIdx) -> bool {
    defined_type_matches(
        &provider.type_defs,
        &provider.rec_groups,
        provided,
        &consumer.type_defs,
        &consumer.rec_groups,
        declared,
    )
}

/// A provider instance and store, as passed to [`Group::add`].
type Provider<'a> = (&'a str, &'a Rc<RegModule>, &'a Rc<RefCell<Store>>);

/// One link group with its instances and their stores.
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

    /// Instantiate `module` in the group, resolving its function imports
    /// against the already-registered instance named `provider_name` — the
    /// runner's wiring: the provider's type section and GC heap are
    /// registered on the imports so concrete types match structurally and GC
    /// references resolve against the heap that created them.
    fn add(
        &mut self,
        module: &Rc<RegModule>,
        provider_name: Option<Provider<'_>>,
    ) -> Rc<RefCell<Store>> {
        let mut imports = Imports::new();
        if let Some((name, provider_module, provider_store)) = provider_name {
            imports = imports
                .with_module_types(name, provider_module)
                .provider_heap(provider_store.borrow().gc_heap());
        }
        let id = self.next_id;
        self.next_id += 1;
        let store = Store::instantiate_linked(module, &imports, &self.group, id, &self.arena)
            .expect("module instantiates");

        if let Some((name, provider_module, provider_store)) = provider_name {
            for declared in &module.imported_funcs {
                if declared.module != name {
                    continue;
                }
                let func_idx = provider_store
                    .borrow()
                    .export_func(&declared.name)
                    .unwrap_or_else(|| {
                        panic!("provider has no function export {:?}", declared.name)
                    });
                let host = link_func(
                    provider_module.clone(),
                    provider_store.clone(),
                    func_idx,
                    declared.ty.clone(),
                );
                store
                    .borrow_mut()
                    .register_host_func(&declared.module, &declared.name, host)
                    .expect("host registration");
            }
        }

        store
    }
}

/// Two modules declare the SAME struct type at different indices: the import
/// binds by closed-form identity, not by local index.
#[test]
fn same_structural_type_binds_across_modules() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $t (sub (struct (field i32))))
  (func (export "read") (param (ref null $t)) (result i32)
    local.get 0
    struct.get $t 0))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $pad (struct (field f64)))
  (type $t (sub (struct (field i32))))
  (import "a" "read" (func $read (param (ref null $t)) (result i32)))
  (func (export "run") (result i32)
    i32.const 42
    struct.new $t
    call $read))"#,
    ));

    // Provider $t = index 0, consumer $t = index 1.
    assert!(links(
        &provider,
        exported_func_type_idx(&provider, "read"),
        &consumer,
        import_type_idx(&consumer, 0)
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, None);
    let consumer_store = group.add(&consumer, Some(("a", &provider, &provider_store)));
    let result = execute_export(&consumer, &consumer_store.borrow(), "run", &[])
        .expect("consumer-allocated struct crosses into the provider");
    assert_eq!(result, vec![Value::I32(42)]);
}

/// Structurally DIFFERENT concrete types never link.
#[test]
fn different_concrete_types_do_not_link() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $t (sub (struct (field i32))))
  (func (export "read") (param (ref null $t)) (result i32)
    local.get 0
    struct.get $t 0))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $u (sub (struct (field i64))))
  (import "a" "read" (func $read (param (ref null $u)) (result i32)))
  (func (export "run") (result i32)
    i32.const 0))"#,
    ));

    assert!(
        !links(
            &provider,
            exported_func_type_idx(&provider, "read"),
            &consumer,
            import_type_idx(&consumer, 0)
        ),
        "an i32-field struct must not satisfy an i64-field struct parameter"
    );
}

/// The pin's direction: the PROVIDED type entry may be a SUBTYPE of the
/// declared one (`match_deftype` walks the provided entry's declared
/// supertypes). A provider function type that declares the consumer's
/// declared function type as its supertype links.
#[test]
fn provided_subtype_of_declared_binds() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $super_ft (sub (func (result i32))))
  (type $sub_ft (sub $super_ft (func (result i32))))
  (func (export "f") (type $sub_ft) (result i32) i32.const 7))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  ;; Structurally identical to the provider's $super_ft.
  (type $decl (sub (func (result i32))))
  (import "a" "f" (func $f (type $decl)))
  (func (export "run") (result i32) call $f))"#,
    ));

    assert!(
        links(
            &provider,
            exported_func_type_idx(&provider, "f"),
            &consumer,
            import_type_idx(&consumer, 0)
        ),
        "provided (sub $super_ft (func)) must match declared $super_ft"
    );

    let mut group = Group::new();
    let provider_store = group.add(&provider, None);
    let consumer_store = group.add(&consumer, Some(("a", &provider, &provider_store)));
    let result = execute_export(&consumer, &consumer_store.borrow(), "run", &[]).unwrap();
    assert_eq!(result, vec![Value::I32(7)]);
}

/// The reverse direction is a link error: a declared function type that is a
/// SUBTYPE of the provided one does not match (provided must be the subtype).
#[test]
fn declared_subtype_of_provided_is_rejected() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $super_ft (sub (func (result i32))))
  (func (export "f") (type $super_ft) (result i32) i32.const 7))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $super_ft (sub (func (result i32))))
  (type $sub_ft (sub $super_ft (func (result i32))))
  (import "a" "f" (func $f (type $sub_ft)))
  (func (export "run") (result i32) call $f))"#,
    ));

    assert!(
        !links(
            &provider,
            exported_func_type_idx(&provider, "f"),
            &consumer,
            import_type_idx(&consumer, 0)
        ),
        "declared (sub $super_ft (func)) must NOT match provided $super_ft"
    );
}

/// Signatures are not enough: `match_deftype` compares whole type entries, so
/// two `(func)` types in different SHAPED rec groups are different types
/// (`type-rec.wast` "Link-time matching of recursive function types").
#[test]
fn same_signature_in_different_rec_groups_does_not_link() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (rec (type $f1 (func)) (type (struct)))
  (func (export "f") (type $f1)))"#,
    ));

    // Same group shape (func first): links.
    let same = Rc::new(wat_module(
        r#"
(module
  (rec (type $f2 (func)) (type (struct)))
  (func (import "a" "f") (type $f2)))"#,
    ));
    assert!(
        links(
            &provider,
            exported_func_type_idx(&provider, "f"),
            &same,
            import_type_idx(&same, 0)
        ),
        "identically shaped rec groups must link"
    );

    // Func second in the group: same signature, different type entry.
    let shifted = Rc::new(wat_module(
        r#"
(module
  (rec (type (struct)) (type $f2 (func)))
  (func (import "a" "f") (type $f2)))"#,
    ));
    assert!(
        !links(
            &provider,
            exported_func_type_idx(&provider, "f"),
            &shifted,
            import_type_idx(&shifted, 0)
        ),
        "a different rec-group position is a different type"
    );

    // Bare (singleton) group: also different from a two-member group.
    let bare = Rc::new(wat_module(
        r#"
(module
  (rec (type $f2 (func)))
  (func (import "a" "f") (type $f2)))"#,
    ));
    assert!(
        !links(
            &provider,
            exported_func_type_idx(&provider, "f"),
            &bare,
            import_type_idx(&bare, 0)
        ),
        "a singleton rec group is not the two-member group"
    );
}

/// A re-export chain: A's concrete type binds into B (different local index)
/// and B's wrapper binds into C. Every hop compares closed forms, and the
/// consumer-allocated object traverses both hops.
#[test]
fn reexport_chain_links_across_modules() {
    let a = Rc::new(wat_module(
        r#"
(module
  (type $t (struct (field i32)))
  (func (export "f") (param (ref null $t)) (result i32)
    local.get 0
    struct.get $t 0))"#,
    ));
    let b = Rc::new(wat_module(
        r#"
(module
  (type $pad (func))
  (type $t (struct (field i32)))
  (import "a" "f" (func $f (param (ref null $t)) (result i32)))
  (func (export "g") (param (ref null $t)) (result i32)
    local.get 0
    call $f))"#,
    ));
    let c = Rc::new(wat_module(
        r#"
(module
  (type $pad (struct (field f32)))
  (type $t (struct (field i32)))
  (import "a" "g" (func $g (param (ref null $t)) (result i32)))
  (func (export "run") (result i32)
    i32.const 9
    struct.new $t
    call $g))"#,
    ));

    assert!(links(
        &a,
        exported_func_type_idx(&a, "f"),
        &b,
        import_type_idx(&b, 0)
    ));
    assert!(links(
        &b,
        exported_func_type_idx(&b, "g"),
        &c,
        import_type_idx(&c, 0)
    ));

    let mut group = Group::new();
    let a_store = group.add(&a, None);
    let b_store = group.add(&b, Some(("a", &a, &a_store)));
    let c_store = group.add(&c, Some(("a", &b, &b_store)));

    let result = execute_export(&c, &c_store.borrow(), "run", &[]).unwrap();
    assert_eq!(result, vec![Value::I32(9)]);
}

/// Heap provenance through an imported-type-driven allocation: the CONSUMER
/// allocates the object and the PROVIDER stores it. The provider's store
/// learns the consumer's heap through the shared link group, so the reference
/// stored in the provider's table still resolves.
#[test]
fn heap_provenance_through_imported_type_allocation() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $t (struct (field i32)))
  (table (export "t") 1 (ref null $t))
  (func (export "store") (param (ref null $t))
    i32.const 0
    local.get 0
    table.set 0))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $t (struct (field i32)))
  (import "a" "store" (func $store (param (ref null $t))))
  (func (export "run") (result i32)
    i32.const 77
    struct.new $t
    call $store
    i32.const 1))"#,
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, None);
    let consumer_store = group.add(&consumer, Some(("a", &provider, &provider_store)));

    let result = execute_export(&consumer, &consumer_store.borrow(), "run", &[])
        .expect("provider accepts and stores a consumer-owned GC reference");
    assert_eq!(result, vec![Value::I32(1)]);

    let (ty, table) = provider_store.borrow().export_table("t").unwrap();
    assert!(ty.elem.is_nullable());
    let stored = table.borrow().get(0).expect("table slot 0 is set");
    let Value::GcRef { heap, id } = stored else {
        panic!("expected a GC reference, got {stored:?}");
    };
    // The object lives in the CONSUMER's heap; the provider resolves it
    // because group heap registration is symmetric.
    assert!(
        provider_store
            .borrow()
            .with_heap(heap, |h| h.object_type_id(id).is_some())
            .unwrap_or(false),
        "the provider must resolve the consumer-owned heap"
    );
    let field = provider_store
        .borrow()
        .with_heap(heap, |h| h.struct_get(id, 0, None))
        .expect("heap registered")
        .expect("field read");
    assert_eq!(field, Value::I32(77));
}

/// A concrete function-typed import is matched structurally too: an
/// incompatible function type is a link error, the exact one a matching body
/// signature links.
#[test]
fn concrete_function_typed_import_matches_structurally() {
    let provider = Rc::new(wat_module(
        r#"
(module
  (type $f (func (param i32) (result i32)))
  ;; Stack order for call_ref is [args..., funcref]: the reference is on top.
  (func (export "apply") (param (ref null $f)) (result i32)
    i32.const 5
    local.get 0
    call_ref $f))"#,
    ));
    let consumer = Rc::new(wat_module(
        r#"
(module
  (type $c (func (param i32) (result i32)))
  (import "a" "apply" (func $apply (param (ref null $c)) (result i32)))
  (func $double (type $c) (param i32) (result i32)
    local.get 0
    local.get 0
    i32.add)
  (elem declare func $double)
  (func (export "run") (result i32)
    ref.func $double
    call $apply))"#,
    ));

    assert!(links(
        &provider,
        exported_func_type_idx(&provider, "apply"),
        &consumer,
        import_type_idx(&consumer, 0)
    ));

    let mut group = Group::new();
    let provider_store = group.add(&provider, None);
    let consumer_store = group.add(&consumer, Some(("a", &provider, &provider_store)));
    let result = execute_export(&consumer, &consumer_store.borrow(), "run", &[]).unwrap();
    assert_eq!(result, vec![Value::I32(10)]);
}
