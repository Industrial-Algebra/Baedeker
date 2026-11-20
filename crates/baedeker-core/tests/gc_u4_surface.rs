// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: the GC const-expression surface.
//!
//! `valid.ml:1029-1040` `is_const` is the authority for what a constant
//! expression may contain: constants, `ref.null`/`ref.func`/`ref.i31`,
//! `struct.new`/`array.new` (BOTH `initop` forms — the wildcards match
//! explicit and default alike), `array.new_fixed`, `extern.convert`, and
//! `global.get` of an IMMUTABLE global. Everything else is
//! "constant expression required".
//!
//! Evaluation follows the U2 allocation discipline: heap forms allocate
//! through the store's never-free heap, are stats-counted, and refuse cleanly
//! (a `ResourceLimitExceeded` with no partial instance) instead of aborting.
//!
//! This file owns a (disarmed-by-default) failing global allocator so the
//! refusal window can be armed; every test serializes through one mutex.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeErrorKind, Store, Value, execute_export};
use baedeker_core::validate::error::ValidationErrorKind;

/// Allocations of at least this many bytes fail. `usize::MAX` disarms.
static FAIL_FROM: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Serializes the tests in this file so a failure window never overlaps.
static LOCK: Mutex<()> = Mutex::new(());

struct FailingAlloc;

unsafe impl GlobalAlloc for FailingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= FAIL_FROM.load(Ordering::SeqCst) {
            return null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= FAIL_FROM.load(Ordering::SeqCst) {
            return null_mut();
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: FailingAlloc = FailingAlloc;

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn disarm() {
    FAIL_FROM.store(usize::MAX, Ordering::SeqCst);
}

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

fn run(source: &str, export: &str) -> Vec<Value> {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    execute_export(&module, &store, export, &[]).unwrap()
}

fn validation_error(source: &str) -> ValidationErrorKind {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    match Module::decode(&bytes) {
        Ok(module) => match module.lower() {
            Ok(_) => panic!("expected a rejection, module lowered: {source}"),
            Err(_) => module.validate().expect_err("expected invalid module").kind,
        },
        Err(error) => Module::decode(&bytes)
            .err()
            .map(|_| panic!("expected a validation error, got decode error: {error}"))
            .unwrap_or_else(|| unreachable!()),
    }
}

const GLOBALS: &str = r#"
(module
  (type $s (struct (field i32)))
  (type $a (array i32))
  (type $sn (array (ref null $s)))

  ;; Every const-eligible GC form in a global initializer.
  (global $g_struct (ref $s) (struct.new $s (i32.const 11)))
  (global $g_struct_default (ref $s) (struct.new_default $s))
  (global $g_i31 (ref i31) (ref.i31 (i32.const 42)))
  (global $g_array (ref $a) (array.new $a (i32.const 3) (i32.const 4)))
  (global $g_array_default (ref $sn) (array.new_default $sn (i32.const 2)))
  (global $g_array_fixed (ref $a)
    (array.new_fixed $a 3 (i32.const 1) (i32.const 2) (i32.const 3)))
  (global $g_extern externref (extern.convert_any (ref.i31 (i32.const 5))))
  (global $g_any anyref
    (any.convert_extern (extern.convert_any (ref.i31 (i32.const 6)))))
  ;; Immutable `global.get` is const-eligible; the immutable global's own
  ;; initializer already allocated.
  (global $g_alias (ref $s) (global.get $g_struct))

  (func (export "struct_field") (result i32)
    global.get $g_struct
    struct.get $s 0)
  (func (export "struct_default_field") (result i32)
    global.get $g_struct_default
    struct.get $s 0)
  (func (export "i31_payload") (result i32)
    global.get $g_i31
    i31.get_u)
  (func (export "array_len") (result i32)
    global.get $g_array
    array.len)
  (func (export "array_elem") (result i32)
    global.get $g_array
    i32.const 0
    array.get $a)
  (func (export "array_default_len") (result i32)
    global.get $g_array_default
    array.len)
  (func (export "array_default_elem_null") (result i32)
    global.get $g_array_default
    i32.const 0
    array.get $sn
    ref.is_null)
  (func (export "array_fixed_sum") (result i32)
    global.get $g_array_fixed
    i32.const 0
    array.get $a
    global.get $g_array_fixed
    i32.const 1
    array.get $a
    i32.add
    global.get $g_array_fixed
    i32.const 2
    array.get $a
    i32.add)
  (func (export "extern_is_i31") (result i32)
    global.get $g_extern
    any.convert_extern
    ref.test (ref i31))
  (func (export "any_payload") (result i32)
    global.get $g_any
    ref.cast (ref i31)
    i31.get_u)
  (func (export "alias_same") (result i32)
    global.get $g_struct
    global.get $g_alias
    ref.eq)
)"#;

/// Every GC global initializer form evaluates and is observable at run time.
#[test]
fn gc_global_initializers_execute() {
    let _guard = lock();
    assert_eq!(run(GLOBALS, "struct_field"), vec![Value::I32(11)]);
    assert_eq!(run(GLOBALS, "struct_default_field"), vec![Value::I32(0)]);
    assert_eq!(run(GLOBALS, "i31_payload"), vec![Value::I32(42)]);
    // `(array.new $a v n)`: v = 3 fill, n = 4 length.
    assert_eq!(run(GLOBALS, "array_len"), vec![Value::I32(4)]);
    assert_eq!(run(GLOBALS, "array_elem"), vec![Value::I32(3)]);
    assert_eq!(run(GLOBALS, "array_default_len"), vec![Value::I32(2)]);
    assert_eq!(run(GLOBALS, "array_default_elem_null"), vec![Value::I32(1)]);
    assert_eq!(run(GLOBALS, "array_fixed_sum"), vec![Value::I32(6)]);
    assert_eq!(run(GLOBALS, "extern_is_i31"), vec![Value::I32(1)]);
    assert_eq!(run(GLOBALS, "any_payload"), vec![Value::I32(6)]);
    assert_eq!(run(GLOBALS, "alias_same"), vec![Value::I32(1)]);
    disarm();
}

/// Const-expression heap allocations are stats-counted like any other: the
/// module above allocates 5 objects (struct, struct_default, array,
/// array_default, array_fixed; the `global.get` alias reuses the struct), and
/// i31 / conversions allocate nothing.
#[test]
fn const_allocations_are_counted_by_gc_stats() {
    let _guard = lock();
    let module = wat_module(GLOBALS);
    let store = Store::instantiate(&module).unwrap();
    let stats = store.gc_stats();
    assert_eq!(
        stats.live_objects, 5,
        "const-expr allocations must be counted exactly: {stats:?}"
    );
    // No execution happened: every object came from an initializer, and i31 /
    // conversions allocated nothing. Logical bytes (U2 formula): struct =
    // 8 + 4, array[i32] len 4 = 12 + 16, array[(ref null $s)] len 2 = 12 + 8,
    // array.new_fixed len 3 = 12 + 12.
    assert_eq!(stats.live_bytes, 12 + 12 + 28 + 20 + 24);
    disarm();
}

/// A table with a GC-typed element type and a heap-allocating initializer.
#[test]
fn table_with_gc_type_and_init_expr() {
    let _guard = lock();
    let source = r#"
(module
  (type $s (struct (field i32)))
  (table $t 2 (ref $s) (struct.new $s (i32.const 9)))
  (table (export "nulls") 2 (ref null $s) (ref.null $s))
  (func (export "get0") (result i32)
    i32.const 0
    table.get $t
    struct.get $s 0)
  (func (export "shared_fill") (result i32)
    i32.const 0 table.get $t
    i32.const 1 table.get $t
    ref.eq))"#;
    assert_eq!(run(source, "get0"), vec![Value::I32(9)]);
    // The init expression is evaluated once and fills every slot.
    assert_eq!(run(source, "shared_fill"), vec![Value::I32(1)]);
    disarm();
}

/// Element segments accept general const expressions, including GC
/// allocations, evaluated at instantiation.
#[test]
fn element_segments_accept_gc_const_exprs() {
    let _guard = lock();
    let source = r#"
(module
  (type $s (struct (field i32)))
  (type $a (array i32))
  (table $t 3 (ref null any))
  (elem (i32.const 0) (ref null any)
    (item (struct.new $s (i32.const 3)))
    (item (array.new $a (i32.const 7) (i32.const 2)))
    (item (ref.i31 (i32.const 5))))
  (func (export "struct_field") (result i32)
    i32.const 0 table.get $t
    ref.cast (ref $s)
    struct.get $s 0)
  (func (export "array_len") (result i32)
    i32.const 1 table.get $t
    ref.cast (ref $a)
    array.len)
  (func (export "i31_payload") (result i32)
    i32.const 2 table.get $t
    ref.cast (ref i31)
    i31.get_u))"#;
    assert_eq!(run(source, "struct_field"), vec![Value::I32(3)]);
    assert_eq!(run(source, "array_len"), vec![Value::I32(2)]);
    assert_eq!(run(source, "i31_payload"), vec![Value::I32(5)]);
    disarm();
}

/// A mutable `global.get` is not a constant expression (pin: "constant
/// expression required"; the interpreter reports
/// `global initializer references mutable imported global`).
#[test]
fn mutable_global_get_in_init_is_rejected() {
    let _guard = lock();
    let kind = validation_error(
        r#"(module
             (global $m (mut i32) (i32.const 0))
             (global i32 (global.get $m)))"#,
    );
    assert!(
        matches!(kind, ValidationErrorKind::MutableGlobalInInitExpr { .. }),
        "expected MutableGlobalInInitExpr, got {kind:?}"
    );
    disarm();
}

/// A non-const instruction in a global initializer is rejected with the pin's
/// message shape ("constant expression required").
#[test]
fn non_const_instruction_in_init_is_rejected() {
    let _guard = lock();
    let kind = validation_error(
        r#"(module
             (global i32 (i32.ctz (i32.const 1))))"#,
    );
    assert!(
        matches!(kind, ValidationErrorKind::NonConstantGlobalInitExpr),
        "expected NonConstantGlobalInitExpr, got {kind:?}"
    );
    let message = format!("{kind}");
    assert!(
        message.contains("constant expression"),
        "message should mirror the pin: {message}"
    );
    disarm();
}

/// `array.new_default` in a GLOBAL initializer is ACCEPTED (the pin's
/// `initop` wildcard) — the earlier contract draft asserted the opposite.
#[test]
fn array_new_default_is_const_eligible() {
    let _guard = lock();
    let source = r#"
(module
  (type $a (array i32))
  (global (export "g") (ref $a) (array.new_default $a (i32.const 4)))
  (func (export "len") (result i32)
    global.get 0
    array.len))"#;
    assert_eq!(run(source, "len"), vec![Value::I32(4)]);
    disarm();
}

/// `struct.new_default` on a type with a non-nullable reference field has no
/// default value and is rejected ("field type is not defaultable").
#[test]
fn struct_new_default_rejects_non_defaultable_field() {
    let _guard = lock();
    let kind = validation_error(
        r#"(module
             (type $s (struct (field i32)))
             (type $bad (struct (field (ref $s))))
             (global (ref $bad) (struct.new_default $bad)))"#,
    );
    assert!(
        matches!(kind, ValidationErrorKind::NonDefaultable { .. }),
        "expected NonDefaultable, got {kind:?}"
    );
    disarm();
}

/// An immutable global initializer may reference an earlier immutable global
/// (`global.get`), including a GC-typed one.
#[test]
fn immutable_global_get_in_init_is_accepted() {
    let _guard = lock();
    let source = r#"
(module
  (type $a (array i32))
  (global $first (ref $a) (array.new_fixed $a 2 (i32.const 4) (i32.const 5)))
  (global $second (ref $a) (global.get $first))
  (func (export "same") (result i32)
    global.get $first
    global.get $second
    ref.eq))"#;
    assert_eq!(run(source, "same"), vec![Value::I32(1)]);
    disarm();
}

/// A refused const-expr allocation is a clean `ResourceLimitExceeded` at
/// instantiation with no partial instance — never an abort.
#[test]
fn refused_const_allocation_is_clean_and_atomic() {
    let _guard = lock();
    // 256 `i32` array elements: the payload reservation is a few KiB, so an
    // allocator that refuses >= 4096-byte requests makes it refuse.
    let source = r#"
(module
  (type $a (array i32))
  (global (ref $a) (array.new_default $a (i32.const 256))))"#;
    let module = wat_module(source);

    FAIL_FROM.store(4096, Ordering::SeqCst);
    let result = Store::instantiate(&module);
    disarm();

    let error = result.expect_err("the armed allocator must make instantiation refuse");
    assert!(
        matches!(
            error.kind,
            RuntimeErrorKind::ResourceLimitExceeded { .. } | RuntimeErrorKind::InvalidConstExpr
        ),
        "expected a clean resource refusal, got {error:?}"
    );
    // Nothing was published: instantiating again (allocator disarmed) yields a
    // fresh, fully-initialized instance.
    let store =
        Store::instantiate(&module).expect("instantiation succeeds when the allocator recovers");
    assert_eq!(store.gc_stats().live_objects, 1);
}

/// AOT: the new const forms and the general element initializer serialize,
/// roundtrip identically, and execute identically (AOT v11).
#[cfg(feature = "aot")]
#[test]
fn gc_const_forms_roundtrip_through_aot_v11() {
    let _guard = lock();
    let module = wat_module(GLOBALS);
    let artifact = baedeker_core::aot::serialize(&module);
    assert_eq!(
        &artifact[..baedeker_core::aot::AOT_MAGIC.len()],
        &baedeker_core::aot::AOT_MAGIC
    );
    let restored = baedeker_core::aot::deserialize(&artifact).expect("v11 artifact roundtrips");
    assert_eq!(restored, module, "roundtrip changed the const-expr IR");

    let before = {
        let store = Store::instantiate(&module).unwrap();
        execute_export(&module, &store, "array_fixed_sum", &[]).unwrap()
    };
    let after = {
        let store = Store::instantiate(&restored).unwrap();
        execute_export(&restored, &store, "array_fixed_sum", &[]).unwrap()
    };
    assert_eq!(before, after);
    assert_eq!(before, vec![Value::I32(6)]);

    // The element-segment Expr form roundtrips too.
    let segment = wat_module(
        r#"
(module
  (type $s (struct (field i32)))
  (table 1 (ref null any))
  (elem (i32.const 0) (ref null any) (item (struct.new $s (i32.const 1))))
  (func (export "f") (result i32) i32.const 0 table.get 0 ref.is_null))"#,
    );
    let restored = baedeker_core::aot::deserialize(&baedeker_core::aot::serialize(&segment))
        .expect("element Expr artifacts roundtrip");
    assert_eq!(restored, segment);
    disarm();
}
