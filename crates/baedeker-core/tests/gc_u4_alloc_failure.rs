// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4 review r1 P1-1 reproduction: const-expression evaluation must be
//! fallible at EVERY temporary, not only at the heap allocation itself.
//!
//! The reviewer armed an allocator that declines a single allocation and
//! instantiated
//!
//! ```wat
//! (module (type $s (struct (field i32)))
//!         (global (ref $s) (struct.new_default $s)))
//! ```
//!
//! Declining the zero-based allocations 13 and 16 terminated the process
//! (exit 134, `SIGABRT`): `ConstEnv::eval`'s operand stack was a plain `Vec`
//! (`stack.push`) and the struct-layout copy was an infallible `.collect()`.
//! Both are now fallible, so a refusal is a clean
//! `ResourceLimitExceeded` at instantiation with no partial instance.
//!
//! The refusal sweep runs each allocation index in a child process, so a
//! regression that aborts is observed as a failed child rather than killing
//! the parent. This file owns a failing global allocator, so it contains only
//! these tests.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeErrorKind, Store, Value};

/// Total `alloc`/`realloc` calls observed in this process.
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Absolute (1-based) allocation count that returns null. 0 disarms.
static FAIL_AT: AtomicUsize = AtomicUsize::new(0);

struct FailAt;

unsafe impl GlobalAlloc for FailAt {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let n = ALLOC_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if FAIL_AT.load(Ordering::SeqCst) == n {
            return null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let n = ALLOC_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if FAIL_AT.load(Ordering::SeqCst) == n {
            return null_mut();
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: FailAt = FailAt;

/// The reviewer's probe: one heap-allocating const expression evaluated while
/// the instance's globals are initialized.
const PROBE: &str = "(module (type $s (struct (field i32))) \
                     (global (ref $s) (struct.new_default $s)))";

fn decode(wat: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = parsed.encode().unwrap();
    Module::decode(&bytes)
        .expect("probe module decodes")
        .lower()
        .expect("probe module lowers")
}

/// The probe's allocation indices that aborted before the fix, expressed
/// zero-based relative to the first allocation after lowering (the reviewer's
/// numbering, reproduced exactly).
const PINNED_ABORTS: &[usize] = &[13, 16];

/// The const-expression refusal window: every allocation reachable from
/// `ConstEnv::eval` for this probe — the operand-stack growths (the former
/// abort at 13), the struct-layout copy (the former abort at 16) and the
/// heap's own object reservation (15). Indices 14 and 16 both land on the
/// layout reservation; 13 lands on the operand stack.
///
/// Allocations OUTSIDE this window belong to instantiation scaffolding that
/// no `try_reserve` can express (stdlib `Rc::new` aborts on refusal) or to
/// per-section `Vec::with_capacity` bookkeeping, and are not part of the
/// const-expression evaluator this fix makes fallible.
const REFUSAL_WINDOW: &[usize] = &[13, 14, 15, 16];

/// Child entry point: arm allocation `GC_U4_FAIL_AT` (zero-based) and
/// instantiate the probe.
#[test]
fn instantiate_child() {
    let Ok(target) = std::env::var("GC_U4_FAIL_AT") else {
        return; // parent process: no-op
    };
    let target: usize = target.parse().unwrap();

    let module = decode(PROBE);
    let base = ALLOC_COUNT.load(Ordering::SeqCst);
    FAIL_AT.store(base + target + 1, Ordering::SeqCst);
    let result = Store::instantiate(&module);
    FAIL_AT.store(0, Ordering::SeqCst);

    match result {
        Ok(_) => {
            eprintln!("OK");
            return;
        }
        Err(error) => match error.kind {
            RuntimeErrorKind::ResourceLimitExceeded { what } => {
                eprintln!("CLEAN_REFUSAL {what}");
            }
            other => {
                eprintln!("OTHER_ERROR {other:?}");
                std::process::exit(3);
            }
        },
    }

    // No partial instance: with the allocator disarmed the same module
    // instantiates and publishes exactly its one global, whose const
    // expression allocated exactly one heap object.
    let store = Store::instantiate(&module).expect("post-refusal instantiation succeeds");
    let value = store.get_global(0).expect("the probe's one global exists");
    let Value::GcRef { .. } = value else {
        panic!("global holds a GC reference, got {value:?}");
    };
    assert!(store.get_global(1).is_none(), "no extra globals published");
    assert_eq!(
        store.gc_stats().live_objects,
        1,
        "exactly one heap object allocated"
    );
    eprintln!("REINSTANTIATED");
}

fn run_child(target: usize) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args(["instantiate_child", "--exact", "--nocapture"])
        .env("GC_U4_FAIL_AT", target.to_string())
        .output()
        .expect("spawn instantiation refusal child");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "refusing zero-based allocation {target} terminated the process \
         (status {:?}); stderr:\n{stderr}",
        output.status
    );
    stderr
}

/// Every allocation reachable from the probe's const-expression evaluation
/// refuses cleanly: the process never aborts, and the two indices that
/// aborted before the fix (`13`, `16`) surface a `ResourceLimitExceeded`
/// with no partial instance.
#[test]
fn const_expr_instantiation_refusal_sweep_is_clean() {
    for &target in REFUSAL_WINDOW {
        let stderr = run_child(target);
        if PINNED_ABORTS.contains(&target) {
            assert!(
                stderr.contains("CLEAN_REFUSAL"),
                "zero-based allocation {target} should clean-refuse, saw:\n{stderr}"
            );
            assert!(
                stderr.contains("REINSTANTIATED"),
                "zero-based allocation {target} must leave no partial instance, saw:\n{stderr}"
            );
        }
    }
}
