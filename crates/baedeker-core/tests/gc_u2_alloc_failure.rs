// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2 review r1 P1-4 reproductions: allocation refusal must be clean
//! and atomic at every preparation stage.
//!
//! The reviewer armed a test allocator that returns null for allocations of
//! at least 4096 bytes, then allocated a 1024-field i8 struct. The old
//! payload `.collect()` aborted the process; these tests pin the corrected
//! behavior (a `ResourceLimitExceeded` refusal with no partial object).
//!
//! This file owns a failing global allocator, so it contains only these
//! tests; each serializes through a mutex while the failure window is open.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{GcHeap, GcTypeKind, RuntimeErrorKind, Store, Value, execute_export};
use baedeker_core::types::{FieldType, StorageType};

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

fn i8_struct(field_count: usize) -> GcTypeKind {
    GcTypeKind::Struct(
        (0..field_count)
            .map(|_| FieldType {
                mutable: true,
                storage: StorageType::I8,
            })
            .collect(),
    )
}

/// Reviewer reproduction: a 1024-field i8 struct whose payload is 24,576
/// bytes refuses cleanly when the allocator fails at 4096 bytes, leaving the
/// heap untouched.
#[test]
fn p1_4_1024_field_struct_refuses_atomically() {
    // Hold the lock across setup so a concurrent armed test cannot fail
    // this test's (large) layout build.
    let guard = lock();
    let mut heap = GcHeap::new();
    let before = heap.stats();
    let kind = i8_struct(1024);
    let values = vec![Value::I32(1); 1024];

    FAIL_FROM.store(4096, Ordering::SeqCst);
    let result = heap.alloc_struct(kind, values);
    disarm();
    drop(guard);

    assert!(
        matches!(
            result,
            Err(ref error) if matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
        ),
        "expected a clean ResourceLimitExceeded, got {result:?}"
    );
    assert_eq!(heap.stats(), before);
    assert_eq!(heap.len(), 0);
    // The payload refused before interning: no partial type was committed.
    assert!(
        heap.type_kind(baedeker_core::runtime::GcTypeId(0))
            .is_none()
    );
}

/// A refusal while building the interning key is also fallible: the key
/// clone is try-reserved, so a huge layout refuses instead of aborting.
#[test]
fn p1_4_intern_key_clone_refusal_is_fallible() {
    // Hold the lock across setup so a concurrent armed test cannot fail
    // this test's (large) layout build.
    let guard = lock();
    let mut heap = GcHeap::new();
    let before = heap.stats();
    let kind = i8_struct(100_000);

    // The key clone reserves `100_000 * size_of::<FieldType>()` bytes; arm a
    // threshold well below that so the try_reserve refuses.
    let threshold = std::mem::size_of::<FieldType>() * 50_000;
    FAIL_FROM.store(threshold, Ordering::SeqCst);
    let result = heap.intern_type(kind);
    disarm();
    drop(guard);

    assert!(
        matches!(
            result,
            Err(ref error) if matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
        ),
        "expected a clean ResourceLimitExceeded, got {result:?}"
    );
    assert_eq!(heap.stats(), before);
    assert_eq!(heap.len(), 0);
    assert!(
        heap.type_kind(baedeker_core::runtime::GcTypeId(0))
            .is_none()
    );
}

/// The heap remains fully usable after a refusal.
#[test]
fn p1_4_heap_remains_usable_after_refusal() {
    // Hold the lock across setup and the post-refusal allocation so a
    // concurrent armed test cannot perturb either.
    let _guard = lock();
    let mut heap = GcHeap::new();
    let kind = i8_struct(1024);
    let values = vec![Value::I32(1); 1024];

    FAIL_FROM.store(4096, Ordering::SeqCst);
    assert!(heap.alloc_struct(kind, values).is_err());
    disarm();

    let id = heap
        .alloc_struct(i8_struct(2), vec![Value::I32(3), Value::I32(4)])
        .expect("post-refusal allocation succeeds");
    assert_eq!(heap.len(), 1);
    assert_eq!(
        heap.struct_fields(id).unwrap(),
        &[Value::I32(3), Value::I32(4)]
    );
}

// ── r2: runtime allocation preparation under `execute_export` ──────
//
// The r1 fix made the direct heap API fallible, but the path from
// `execute_export` still cloned the layout, rebuilt the closed form, and
// `.collect()`ed the payload before the heap call. The regression forces
// that preparation to be fallible end-to-end: every large allocation
// reachable from execution must be a clean `ResourceLimitExceeded`, never
// an allocator abort, and the heap must be untouched and still usable.

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    Module::decode(&bytes).unwrap().lower().unwrap()
}

fn runtime_1024_field_struct_module() -> RegModule {
    let fields: String = (0..1024).map(|_| "(field (mut i8))").collect();
    wat_module(&format!(
        "(module (type $s (struct {fields})) (func (export \"f\") struct.new_default $s drop))"
    ))
}

fn array_module(body: &str) -> RegModule {
    wat_module(&format!(
        "(module (type $a (array (mut i32))) (func (export \"f\") {body} drop))"
    ))
}

/// Run the module's `f` export with the allocator armed at `threshold`,
/// holding the file lock from module construction so no concurrent armed
/// test can fail the (large) setup allocations. Asserts a clean refusal and
/// untouched heap statistics.
fn assert_runtime_refusal_clean(threshold: usize, build: impl FnOnce() -> RegModule) {
    let guard = lock();
    let module = build();
    let store = Store::instantiate(&module).expect("module instantiates");
    let before = store.gc_stats();

    FAIL_FROM.store(threshold, Ordering::SeqCst);
    let result = execute_export(&module, &store, "f", &[]);
    FAIL_FROM.store(usize::MAX, Ordering::SeqCst);
    drop(guard);

    assert!(
        matches!(
            result,
            Err(ref error) if matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
        ),
        "expected a clean ResourceLimitExceeded, got {result:?}"
    );
    assert_eq!(store.gc_stats(), before);
    assert_eq!(store.gc_stats().live_objects, 0);
}

/// Reviewer reproduction (through the runtime, not the direct API): a
/// 1024-field i8 `struct.new_default` must refuse cleanly under an allocator
/// that fails >= 4096-byte allocations. The `collect()` that used to abort
/// now reserves fallibly.
#[test]
fn p1_runtime_1024_field_new_default_refuses_cleanly() {
    let guard = lock();
    let module = runtime_1024_field_struct_module();
    let store = Store::instantiate(&module).expect("module instantiates");
    let before = store.gc_stats();

    FAIL_FROM.store(4096, Ordering::SeqCst);
    let result = execute_export(&module, &store, "f", &[]);
    FAIL_FROM.store(usize::MAX, Ordering::SeqCst);

    assert!(
        matches!(
            result,
            Err(ref error) if matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
        ),
        "expected a clean ResourceLimitExceeded, got {result:?}"
    );
    assert_eq!(store.gc_stats(), before);
    assert_eq!(store.gc_stats().live_objects, 0);

    // The heap is still usable after the refusal.
    let ok = execute_export(&module, &store, "f", &[]);
    assert!(ok.is_ok(), "post-refusal execution succeeds: {ok:?}");
    assert_eq!(store.gc_stats().live_objects, 1);
    drop(guard);
}

#[test]
fn p1_runtime_array_new_refuses_cleanly() {
    assert_runtime_refusal_clean(4096, || {
        array_module("i32.const 0 i32.const 1024 array.new $a")
    });
}

#[test]
fn p1_runtime_array_new_default_refuses_cleanly() {
    assert_runtime_refusal_clean(4096, || array_module("i32.const 1024 array.new_default $a"));
}

#[test]
fn p1_runtime_array_new_fixed_refuses_cleanly() {
    assert_runtime_refusal_clean(4096, || {
        let operands: String = (0..1024).map(|_| "i32.const 0 ").collect();
        array_module(&format!("{operands}array.new_fixed $a 1024"))
    });
}
