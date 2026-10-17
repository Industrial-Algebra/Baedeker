// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3 review r1 P1 reproduction: cast-branch label bookkeeping must
//! survive allocation refusal cleanly.
//!
//! The reviewer armed a global allocator that returns null for a chosen
//! allocation index after decoding, then validated a module whose block
//! result is the `(ref null any)` shorthand and that branches through
//! `br_on_cast`. The 16th allocation (24 bytes — the label-type copy) and the
//! 17th (12 bytes — the leading-type copy) aborted the process because both
//! used infallible `to_vec()`. The fix try-reserves each copy and surfaces a
//! clean [`ValidationErrorKind::ResourceLimitExceeded`].
//!
//! This file owns a failing global allocator, so it contains only these
//! tests. The refusal sweep runs each allocation index in a child process so
//! a regression that aborts is observed as a failed child rather than killing
//! the parent.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

use baedeker_core::binary::module::Module;
use baedeker_core::lower::LowerErrorKind;
use baedeker_core::validate::error::ValidationErrorKind;

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

/// The reviewer's module: a `br_on_cast` branching into a block whose result
/// is the `(ref null any)` shorthand.
const BR_ON_CAST_MODULE: &str = r#"
(module
  (func (param (ref null any)) (result i32 (ref null any))
    (block $b (result i32 (ref null any))
      i32.const 7
      local.get 0
      br_on_cast $b (ref null any) (ref struct))))"#;

/// The same shape with `br_on_cast_fail`.
const BR_ON_CAST_FAIL_MODULE: &str = r#"
(module
  (func (param (ref null any)) (result i32 (ref null any))
    (block $b (result i32 (ref null any))
      i32.const 7
      local.get 0
      br_on_cast_fail $b (ref null any) (ref struct))))"#;

/// Reviewer allocation indices for [`BR_ON_CAST_MODULE`]: the 24-byte
/// label-type copy and the 12-byte leading-type copy (`validate_br_on_cast`
/// lines 5171 and 5199; the reviewer's probe numbered them 16 and 17 0-based).
/// [`BR_ON_CAST_FAIL_MODULE`] has the same shape, and the sweep confirms the
/// same indices.
const CAST_REFUSAL_INDICES: &[usize] = &[17, 18];

/// Lowering allocation window covering the `lower_br_on_cast` bookkeeping:
/// the validation label copies (17-18), then the lowering label-type and
/// leading-type copies, the branch-value vector, and the forwarded copy plus
/// pending-vector reservation (37-41). Pinned empirically for the reviewer
/// module; indices between them are pre-existing lowerer/decode growth.
const LOWER_REFUSAL_INDICES: &[usize] = &[17, 18, 37, 38, 39, 40, 41];

fn decode(wat: &str) -> Module<'static> {
    // Leak the bytes so the borrowed module can outlive this helper; the
    // child process is short-lived.
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes: &'static [u8] = Box::leak(parsed.encode().unwrap().into_boxed_slice());
    Module::decode(bytes).expect("reviewer module decodes")
}

/// Child entry point: validate with allocation `GC_U3_FAIL_AT` refused.
///
/// Prints `CLEAN_REFUSAL` when validation returns the clean resource error,
/// `OK` when the refused allocation was not on the validation path, and
/// exits non-zero on any other error. An infallible refusal aborts the
/// process instead, which the parent observes as a non-zero status.
#[test]
fn refusal_child() {
    let Ok(target) = std::env::var("GC_U3_FAIL_AT") else {
        return; // parent process: no-op
    };
    let target: usize = target.parse().unwrap();
    let wat = match std::env::var("GC_U3_MODULE").as_deref() {
        Ok("fail") => BR_ON_CAST_FAIL_MODULE,
        _ => BR_ON_CAST_MODULE,
    };

    let module = decode(wat);
    let base = ALLOC_COUNT.load(Ordering::SeqCst);
    FAIL_AT.store(base + target, Ordering::SeqCst);
    let result = module.validate();
    FAIL_AT.store(0, Ordering::SeqCst);

    match result {
        Ok(()) => eprintln!("OK"),
        Err(error) => match error.kind {
            ValidationErrorKind::ResourceLimitExceeded { what } => {
                eprintln!("CLEAN_REFUSAL {what}");
            }
            other => {
                eprintln!("OTHER_ERROR {other:?}");
                std::process::exit(3);
            }
        },
    }
}

/// Run [`refusal_child`] for one index in a fresh process; a clean refusal or
/// a no-op (`OK`) both exit 0, an infallible abort does not.
fn run_child(module: &str, target: usize) -> std::process::Output {
    let exe = std::env::current_exe().expect("test binary path");
    std::process::Command::new(exe)
        .args(["refusal_child", "--exact", "--nocapture"])
        .env("GC_U3_FAIL_AT", target.to_string())
        .env("GC_U3_MODULE", module)
        .output()
        .expect("spawn refusal child")
}

fn assert_child_clean(module: &str, target: usize) -> String {
    let output = run_child(module, target);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "refusing allocation {target} in `{module}` terminated the process \
         (status {:?}); stderr:\n{stderr}",
        output.status
    );
    stderr
}

/// Refusing either label-bookkeeping allocation in `br_on_cast` validation
/// yields a clean resource error (or leaves validation untouched), never a
/// process abort.
#[test]
fn br_on_cast_validation_refusal_sweep_is_clean() {
    for &target in CAST_REFUSAL_INDICES {
        let stderr = assert_child_clean("cast", target);
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "allocation {target} should be the try-reserved label copy, saw:\n{stderr}"
        );
    }
}

/// The same sweep for the `br_on_cast_fail` form; its label bookkeeping is
/// allocation-identical, so the same indices are refused.
#[test]
fn br_on_cast_fail_validation_refusal_sweep_is_clean() {
    for &target in CAST_REFUSAL_INDICES {
        let stderr = assert_child_clean("fail", target);
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "allocation {target} should be the try-reserved label copy, saw:\n{stderr}"
        );
    }
}

// ── lowering: the same module through `Module::lower` ───────────────

/// Child entry point for the lowering sweep: arm allocation
/// `GC_U3_LOWER_FAIL_AT` and lower the module.
#[test]
fn lower_refusal_child() {
    let Ok(target) = std::env::var("GC_U3_LOWER_FAIL_AT") else {
        return; // parent process: no-op
    };
    let target: usize = target.parse().unwrap();
    let wat = match std::env::var("GC_U3_MODULE").as_deref() {
        Ok("fail") => BR_ON_CAST_FAIL_MODULE,
        _ => BR_ON_CAST_MODULE,
    };

    let module = decode(wat);
    let base = ALLOC_COUNT.load(Ordering::SeqCst);
    FAIL_AT.store(base + target, Ordering::SeqCst);
    let result = module.lower();
    FAIL_AT.store(0, Ordering::SeqCst);

    match result {
        Ok(_) => eprintln!("OK"),
        Err(error) => match error.kind {
            LowerErrorKind::Validation(ValidationErrorKind::ResourceLimitExceeded { what }) => {
                eprintln!("CLEAN_REFUSAL {what}");
            }
            other => {
                eprintln!("OTHER_ERROR {other:?}");
                std::process::exit(3);
            }
        },
    }
}

/// Run [`lower_refusal_child`] for one allocation index in a fresh process.
fn run_lower_child(module: &str, target: usize) -> std::process::Output {
    let exe = std::env::current_exe().expect("test binary path");
    std::process::Command::new(exe)
        .args(["lower_refusal_child", "--exact", "--nocapture"])
        .env("GC_U3_LOWER_FAIL_AT", target.to_string())
        .env("GC_U3_MODULE", module)
        .output()
        .expect("spawn lowering refusal child")
}

/// Lowering the `br_on_cast` module is clean for every refusal in the window
/// covering the new `lower_br_on_cast` label bookkeeping (the label/
/// leading-type copies and the forwarded branch values).
#[test]
fn br_on_cast_lowering_refusal_sweep_is_clean() {
    for &target in LOWER_REFUSAL_INDICES {
        let output = run_lower_child("cast", target);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "refusing lowering allocation {target} terminated the process; \
             stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "lowering allocation {target} should be try-reserved, saw:\n{stderr}"
        );
    }
}

#[test]
fn br_on_cast_fail_lowering_refusal_sweep_is_clean() {
    for &target in LOWER_REFUSAL_INDICES {
        let output = run_lower_child("fail", target);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "refusing lowering allocation {target} terminated the process; \
             stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "lowering allocation {target} should be try-reserved, saw:\n{stderr}"
        );
    }
}
