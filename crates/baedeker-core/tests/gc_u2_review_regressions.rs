// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2 review r1 reproductions, pinned as named regressions.
//!
//! Each test reproduces one blocking finding from the review and asserts the
//! corrected behavior:
//!  - P1 heap topology: a `GcRef` from one store resolves against the heap
//!    that created it, not the consumer's local object table.
//!  - P1 public argument checking: a concrete aggregate value is checked
//!    module-aware at the public boundary.
//!  - P1 unreachable `array.new_fixed`: a huge count in dead code lowers
//!    without materializing operand registers or aborting.
//!  - P2 structural interning: cross-module equivalent aggregate types intern
//!    equal even when their local type indices differ.

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    ExceptionRecord, GcHeap, GcTypeId, GcTypeKind, Imports, LinkGroup, RuntimeErrorKind, Store,
    Value, execute_export,
};
use baedeker_core::types::{ClosedDefinedType, CompType, TypeIdx};

/// Parse WAT, encode it to a binary, decode, and lower it.
fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

/// Decode + validate + lower, surfacing the validation decision.
fn lower_checked(source: &str) -> Result<RegModule, String> {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.validate().map_err(|error| format!("{error:?}"))?;
    module.lower().map_err(|error| format!("{error:?}"))
}

/// A link group plus the shared exception arena its members share.
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
        let id = self.next_id;
        self.next_id += 1;
        Store::instantiate_linked(module, imports, &self.group, id, &self.arena)
            .expect("module instantiates")
    }
}

// ── P1: heap topology ────────────────────────────────────────────────

const PROVIDER: &str = r#"
(module
  (type $s (struct (field (mut i32))))
  (global (export "g") (mut (ref null $s)) (ref.null $s))
  (func (export "init")
    i32.const 11 struct.new $s global.set 0)
  (func (export "read") (result i32)
    global.get 0 ref.as_non_null struct.get $s 0))"#;

const CONSUMER: &str = r#"
(module
  (type $s (struct (field (mut i32))))
  (import "a" "g" (global (mut (ref null $s))))
  (func (export "init")
    i32.const 99 struct.new $s drop)
  (func (export "read") (result i32)
    global.get 0 ref.as_non_null struct.get $s 0)
  (func (export "set")
    global.get 0 ref.as_non_null i32.const 7 struct.set $s 0))"#;

/// Reviewer reproduction: without heap ownership, B's own `ObjectId(0)` is
/// misread as A's reference and `B.read` returns 99 instead of 11. With the
/// provider heap registered, the reference resolves against A's heap.
#[test]
fn p1_1_shared_global_gcref_resolves_against_provider_heap() {
    let a = Rc::new(wat_module(PROVIDER));
    let b = Rc::new(wat_module(CONSUMER));

    let a_store = Store::instantiate(&a).unwrap();
    execute_export(&a, &a_store, "init", &[]).unwrap();
    let (ty, shared) = a_store.export_global("g").expect("global export");

    let imports = Imports::new()
        .with_module_types("a", &a)
        .provider_heap(a_store.gc_heap())
        .shared_global("a", "g", ty, shared);
    let b_store = Store::instantiate_with_imports(&b, &imports).unwrap();

    // B allocates its own object 0, which would alias A's object 0 if a raw
    // `ObjectId` were used.
    execute_export(&b, &b_store, "init", &[]).unwrap();
    assert_eq!(
        execute_export(&b, &b_store, "read", &[]),
        Ok(vec![Value::I32(11)])
    );
}

/// Cross-store mutation: B mutates A's object through the shared foreign
/// heap; the `Rc<RefCell<GcHeap>>` borrow must not alias.
#[test]
fn p1_1_cross_store_struct_set_targets_provider_heap() {
    let a = Rc::new(wat_module(PROVIDER));
    let b = Rc::new(wat_module(CONSUMER));

    let a_store = Store::instantiate(&a).unwrap();
    execute_export(&a, &a_store, "init", &[]).unwrap();
    let (ty, shared) = a_store.export_global("g").expect("global export");
    let imports = Imports::new()
        .with_module_types("a", &a)
        .provider_heap(a_store.gc_heap())
        .shared_global("a", "g", ty, shared);
    let b_store = Store::instantiate_with_imports(&b, &imports).unwrap();

    execute_export(&b, &b_store, "init", &[]).unwrap();
    execute_export(&b, &b_store, "set", &[]).unwrap();
    // A's heap object now holds 7; both consumers observe it.
    assert_eq!(
        execute_export(&a, &a_store, "read", &[]),
        Ok(vec![Value::I32(7)])
    );
    assert_eq!(
        execute_export(&b, &b_store, "read", &[]),
        Ok(vec![Value::I32(7)])
    );
}

/// Linked-group consumers register already-present members' heaps
/// automatically, so the shared-global case works without an explicit
/// `provider_heap` call.
#[test]
fn p1_1_linked_group_registers_member_heaps() {
    let a = Rc::new(wat_module(PROVIDER));
    let b = Rc::new(wat_module(CONSUMER));

    let mut group = Group::new();
    let a_store = group.add(&a, &Imports::new());
    execute_export(&a, &a_store.borrow(), "init", &[]).unwrap();
    let (ty, shared) = a_store.borrow().export_global("g").expect("global export");
    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_global("a", "g", ty, shared);
    let b_store = group.add(&b, &imports);

    execute_export(&b, &b_store.borrow(), "init", &[]).unwrap();
    assert_eq!(
        execute_export(&b, &b_store.borrow(), "read", &[]),
        Ok(vec![Value::I32(11)])
    );
}

/// A foreign reference whose heap was never registered is a clean runtime
/// error — never a panic, never a wrong local read.
#[test]
fn p1_1_unregistered_foreign_heap_is_a_clean_error() {
    let a = Rc::new(wat_module(PROVIDER));
    let b = Rc::new(wat_module(CONSUMER));

    let a_store = Store::instantiate(&a).unwrap();
    execute_export(&a, &a_store, "init", &[]).unwrap();
    let (ty, shared) = a_store.export_global("g").expect("global export");

    // No `provider_heap`: the consumer has no handle for A's heap.
    let imports = Imports::new()
        .with_module_types("a", &a)
        .shared_global("a", "g", ty, shared);
    let b_store = Store::instantiate_with_imports(&b, &imports).unwrap();
    execute_export(&b, &b_store, "init", &[]).unwrap();

    let error = execute_export(&b, &b_store, "read", &[]).expect_err("unregistered heap");
    assert!(
        matches!(error.kind, RuntimeErrorKind::GcHeapNotFound { .. }),
        "expected GcHeapNotFound, got {error:?}"
    );
}

// ── P1: public argument checking ─────────────────────────────────────

const ARG_CHECK: &str = r#"
(module
  (type $i (struct (field i32)))
  (type $f (struct (field f64)))
  (type $a (array (mut i32)))
  (func (export "make") (result (ref $f))
    f64.const 3.5 struct.new $f)
  (func (export "make_array") (result (ref $a))
    i32.const 1 i32.const 2 array.new $a)
  (func (export "read") (param (ref $i)) (result i32)
    local.get 0 struct.get $i 0)
  (func (export "read_f") (param (ref $f)) (result f64)
    local.get 0 struct.get $f 0))"#;

/// Reviewer reproduction: a struct value returned for `(ref $f)` passed into
/// a `(ref $i)` parameter must be rejected before execution. Under the former
/// laxness any `GcRef` satisfied any concrete `(ref $t)`.
#[test]
fn p1_2_wrong_struct_type_rejected_at_boundary() {
    let m = wat_module(ARG_CHECK);
    let store = Store::instantiate(&m).unwrap();
    let value = execute_export(&m, &store, "make", &[]).unwrap()[0];

    let error = execute_export(&m, &store, "read", &[value]).expect_err("wrong struct type");
    assert!(
        matches!(error.kind, RuntimeErrorKind::TypeMismatch { .. }),
        "expected TypeMismatch, got {error:?}"
    );

    // Wrong kind: an array reference where a struct was declared.
    let array = execute_export(&m, &store, "make_array", &[]).unwrap()[0];
    let error = execute_export(&m, &store, "read", &[array]).expect_err("wrong ref kind");
    assert!(
        matches!(error.kind, RuntimeErrorKind::TypeMismatch { .. }),
        "expected TypeMismatch, got {error:?}"
    );
}

/// Positive control: the same concrete type passes through the boundary.
#[test]
fn p1_2_matching_struct_type_passes_through() {
    let m = wat_module(ARG_CHECK);
    let store = Store::instantiate(&m).unwrap();
    let value = execute_export(&m, &store, "make", &[]).unwrap()[0];
    assert_eq!(
        execute_export(&m, &store, "read_f", &[value]),
        Ok(vec![Value::F64(3.5)])
    );
}

// ── P1: unreachable array.new_fixed ──────────────────────────────────

const HUGE_FIXED_DEAD: &str = r#"
(module
  (type (array i32))
  (func
    unreachable
    array.new_fixed 0 4294967295
    drop))"#;

fn lower_huge_fixed_dead() -> Result<(), String> {
    lower_checked(HUGE_FIXED_DEAD).map(|_| ())
}

/// Reviewer reproduction: validation succeeds and lowering must not abort
/// trying to materialize 4,294,967,295 operand registers.
#[test]
fn p1_3_unreachable_huge_new_fixed_lowers() {
    lower_huge_fixed_dead().expect("dead-code huge array.new_fixed lowers");
}

/// Helper run in a child process under `ulimit -v 600000`.
#[test]
fn p1_3_lower_helper_under_memory_limit() {
    lower_huge_fixed_dead().expect("dead-code huge array.new_fixed lowers");
    eprintln!("P1_3_LOWER_OK");
}

/// Lowering the reviewer's module under `ulimit -v 600000` (the constraint
/// from the review) exits cleanly instead of aborting.
#[test]
fn p1_3_unreachable_huge_new_fixed_under_ulimit() {
    let exe = std::env::current_exe().expect("test binary path");
    let status = std::process::Command::new("bash")
        .arg("-c")
        .arg("ulimit -v 600000; exec \"$0\" p1_3_lower_helper_under_memory_limit --exact --nocapture")
        .arg(exe)
        .status()
        .expect("spawn child test");
    assert!(
        status.success(),
        "lowering under ulimit -v 600000 must not abort"
    );
}

// ── P2: structural interning ─────────────────────────────────────────

/// Reviewer reproduction: the final struct's cloned layout carries a
/// module-local `TypeIdx`, so raw interning conflates A with B and separates
/// A from C. Interning the closed forms must give A≠B and A=C.
#[test]
fn p2_cross_module_struct_interning_uses_closed_identity() {
    // A: type 0 = struct(i32); type 1 = struct(ref null $0).
    let a = wat_module(
        r#"(module
  (type (struct (field i32)))
  (type (struct (field (ref null 0)))))"#,
    );
    // B: type 0 = struct(f64); type 1 = struct(ref null $0).
    let b = wat_module(
        r#"(module
  (type (struct (field f64)))
  (type (struct (field (ref null 0)))))"#,
    );
    // C: type 0 = func; type 1 = struct(i32); type 2 = struct(ref null $1).
    let c = wat_module(
        r#"(module
  (type (func))
  (type (struct (field i32)))
  (type (struct (field (ref null 1)))))"#,
    );

    let mut heap = GcHeap::new();
    let a_id = intern_final_struct(&mut heap, &a);
    let b_id = intern_final_struct(&mut heap, &b);
    let c_id = intern_final_struct(&mut heap, &c);

    assert_ne!(a_id, b_id, "A and B reference different structures");
    assert_eq!(a_id, c_id, "A and C are structurally identical");
}

/// Intern a module's final struct type into `heap` by its closed form.
fn intern_final_struct(heap: &mut GcHeap, module: &RegModule) -> GcTypeId {
    let idx = TypeIdx(module.type_defs.len() as u32 - 1);
    let kind = match &module.type_defs[idx.0 as usize].comp {
        CompType::Struct(fields) => GcTypeKind::Struct(fields.clone()),
        CompType::Array(field) => GcTypeKind::Array(*field),
        CompType::Func(_) => panic!("final type must be an aggregate"),
    };
    let closed = ClosedDefinedType::of(&module.type_defs, &module.rec_groups, idx)
        .expect("closed form resolves");
    heap.intern_closed(closed, kind)
        .expect("interning succeeds")
}

/// Sanity: the old raw key really does conflate A/B and separate A/C, which
/// is why the runtime path must use closed interning.
#[test]
fn p2_raw_interning_still_distinguishes_on_layout_only() {
    let a = wat_module(
        r#"(module
  (type (struct (field i32)))
  (type (struct (field (ref null 0)))))"#,
    );
    let c = wat_module(
        r#"(module
  (type (func))
  (type (struct (field i32)))
  (type (struct (field (ref null 1)))))"#,
    );
    let mut heap = GcHeap::new();
    let a_id = intern_final_struct_raw(&mut heap, &a);
    let c_id = intern_final_struct_raw(&mut heap, &c);
    // Raw local indices differ (0 vs 1): a demonstration that the raw path
    // alone cannot deliver the cross-module identity.
    assert_ne!(a_id, c_id);
}

fn intern_final_struct_raw(heap: &mut GcHeap, module: &RegModule) -> GcTypeId {
    let idx = TypeIdx(module.type_defs.len() as u32 - 1);
    let kind = match &module.type_defs[idx.0 as usize].comp {
        CompType::Struct(fields) => GcTypeKind::Struct(fields.clone()),
        CompType::Array(field) => GcTypeKind::Array(*field),
        CompType::Func(_) => panic!("final type must be an aggregate"),
    };
    heap.intern_type(kind).expect("interning succeeds")
}
