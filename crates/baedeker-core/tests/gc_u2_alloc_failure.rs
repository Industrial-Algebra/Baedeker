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

use baedeker_core::runtime::{GcHeap, GcTypeKind, RuntimeErrorKind, Value};
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

fn armed(threshold: usize) -> MutexGuard<'static, ()> {
    let guard = lock();
    FAIL_FROM.store(threshold, Ordering::SeqCst);
    guard
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
    let mut heap = GcHeap::new();
    let before = heap.stats();
    let kind = i8_struct(1024);
    let values = vec![Value::I32(1); 1024];

    let guard = armed(4096);
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
    let mut heap = GcHeap::new();
    let before = heap.stats();
    // Build the layout before arming (its own allocation must succeed).
    let kind = i8_struct(100_000);

    // The key clone reserves `100_000 * size_of::<FieldType>()` bytes; arm a
    // threshold well below that so the try_reserve refuses.
    let threshold = std::mem::size_of::<FieldType>() * 50_000;
    let guard = armed(threshold);
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
    let mut heap = GcHeap::new();
    let kind = i8_struct(1024);
    let values = vec![Value::I32(1); 1024];

    let guard = armed(4096);
    assert!(heap.alloc_struct(kind, values).is_err());
    disarm();
    drop(guard);

    let id = heap
        .alloc_struct(i8_struct(2), vec![Value::I32(3), Value::I32(4)])
        .expect("post-refusal allocation succeeds");
    assert_eq!(heap.len(), 1);
    assert_eq!(
        heap.struct_fields(id).unwrap(),
        &[Value::I32(3), Value::I32(4)]
    );
}
