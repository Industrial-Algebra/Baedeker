// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 3 review r2/r3 P2 reproduction: module-sized bookkeeping clones
//! and replacement vectors must survive allocation refusal cleanly.
//!
//! The r1 fix made the `br_on_cast` label/leading copies fallible, but two
//! module-sized `Vec` clones still terminated the process when the allocator
//! declined their allocation:
//!
//! * `validate::finish_frame` cloned the whole `ControlFrame` (its
//!   `start_types`, `end_types`, and `local_inits` all scale with
//!   module-controlled result arity / local count).
//! * `lower`'s block-close cloned the frame's `result_types` (scales with
//!   result arity).
//!
//! The reviewer's probe was a cast-branch module with `N` leading `i32`
//! results; declining the clone allocation aborted with `SIGABRT`, requesting
//! 24 bytes at arity 1 and 6156 bytes at arity 512. Both clones are now
//! *eliminated* (the frame's vector is moved out with `mem::take` on the
//! validation side and read without an ownership copy on the lowering side),
//! and the surrounding module-scaled bookkeeping — `local_inits` snapshots,
//! control/block vectors, and the error/result vectors — is reserved
//! fallibly.
//!
//! The r3 fix eliminated the block-close `pending_branches` replacement
//! vector (and the analogous block-close / function-end `pending_catches`
//! replacement vectors) by processing the targeted entries in place and then
//! compacting with `Vec::retain`; a replacement `Vec::with_capacity(len)` was
//! an infallible, module-scaled allocation that aborted on allocator
//! refusal (56 bytes for the reviewer's probe at both arities). The
//! single-clause `try_table` modules below cover the changed catch paths.
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

/// Build the reviewer's module with `arity` leading `i32` block/function
/// results followed by `(ref null any)`, branching through `br_on_cast`
/// (`fail = false`) or `br_on_cast_fail` (`fail = true`).
fn cast_branch_wat(arity: usize, fail: bool) -> String {
    let mut results = String::new();
    for _ in 0..arity {
        results.push_str("i32 ");
    }
    results.push_str("(ref null any)");
    let mut body = String::new();
    for _ in 0..arity {
        body.push_str("      i32.const 7\n");
    }
    let form = if fail {
        "br_on_cast_fail"
    } else {
        "br_on_cast"
    };
    format!(
        "(module\n  (func (param (ref null any)) (result {results})\n    \
         (block $b (result {results})\n{body}      local.get 0\n      \
         {form} $b (ref null any) (ref struct))))"
    )
}

/// A local-heavy variant of the arity-1 module: `local_count` extra `i32`
/// locals exercise the `local_inits` snapshot/copy path.
fn local_heavy_wat(local_count: usize) -> String {
    let mut local_decl = String::from("    (local");
    for _ in 0..local_count {
        local_decl.push_str(" i32");
    }
    local_decl.push_str(")\n");
    format!(
        "(module\n  (func (param (ref null any)) (result i32 (ref null any))\n\
         {local_decl}    (block $b (result i32 (ref null any))\n      i32.const 7\n      \
         local.get 0\n      br_on_cast $b (ref null any) (ref struct))))"
    )
}

/// A single-clause `try_table` module with one pending catch. With
/// `function_level == false` the catch targets an outer non-loop block `$h`,
/// so it is back-patched in the *block-close* path; with
/// `function_level == true` it targets the function's implicit label (`0`),
/// so it is back-patched in the *function-end* path. Both paths were changed
/// from a module-sized replacement vector to in-place processing.
fn try_table_wat(function_level: bool) -> String {
    if function_level {
        String::from(
            "(module\n  (tag $e0 (param i32))\n  \
             (func (export \"f\") (result i32)\n    \
             (try_table (catch $e0 0)\n      \
             i32.const 42\n      throw $e0)\n    \
             i32.const 7))",
        )
    } else {
        String::from(
            "(module\n  (tag $e0 (param i32))\n  \
             (func (export \"f\") (result i32)\n    \
             (block $h (result i32)\n      \
             (try_table (catch $e0 $h)\n        \
             i32.const 42\n        throw $e0)\n      \
             i32.const 7)))",
        )
    }
}

fn decode(wat: &str) -> Module<'static> {
    // Leak the bytes so the borrowed module can outlive this helper; the
    // child process is short-lived.
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut parsed = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes: &'static [u8] = Box::leak(parsed.encode().unwrap().into_boxed_slice());
    Module::decode(bytes).expect("reviewer module decodes")
}

/// Read the child's arity/form/locals configuration and the module it should
/// validate or lower.
fn child_module() -> Module<'static> {
    if let Ok(level) = std::env::var("GC_U3_TRY_TABLE") {
        return decode(&try_table_wat(level == "func"));
    }
    let locals: usize = std::env::var("GC_U3_LOCALS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if locals > 0 {
        return decode(&local_heavy_wat(locals));
    }
    let arity: usize = std::env::var("GC_U3_ARITY")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let fail = std::env::var("GC_U3_FORM").as_deref() == Ok("fail");
    decode(&cast_branch_wat(arity, fail))
}

// ── validation ──────────────────────────────────────────────────────

/// Child entry point: validate with allocation `GC_U3_FAIL_AT` refused.
#[test]
fn validate_child() {
    let Ok(target) = std::env::var("GC_U3_FAIL_AT") else {
        return; // parent process: no-op
    };
    let target: usize = target.parse().unwrap();

    let module = child_module();
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

fn run_validate_child(target: usize, arity: usize, form: &str, locals: usize) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args(["validate_child", "--exact", "--nocapture"])
        .env("GC_U3_FAIL_AT", target.to_string())
        .env("GC_U3_ARITY", arity.to_string())
        .env("GC_U3_FORM", form)
        .env("GC_U3_LOCALS", locals.to_string())
        .output()
        .expect("spawn validate refusal child");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "refusing allocation {target} (arity {arity}, form {form}, locals \
         {locals}) terminated the process (status {:?}); stderr:\n{stderr}",
        output.status
    );
    stderr
}

// ── lowering ────────────────────────────────────────────────────────

/// Child entry point for the lowering sweep: arm allocation
/// `GC_U3_LOWER_FAIL_AT` and lower the module.
#[test]
fn lower_child() {
    let Ok(target) = std::env::var("GC_U3_LOWER_FAIL_AT") else {
        return; // parent process: no-op
    };
    let target: usize = target.parse().unwrap();

    let module = child_module();
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

fn run_lower_child(target: usize, arity: usize, form: &str) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args(["lower_child", "--exact", "--nocapture"])
        .env("GC_U3_LOWER_FAIL_AT", target.to_string())
        .env("GC_U3_ARITY", arity.to_string())
        .env("GC_U3_FORM", form)
        .output()
        .expect("spawn lowering refusal child");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "refusing lowering allocation {target} (arity {arity}, form {form}) \
         terminated the process (status {:?}); stderr:\n{stderr}",
        output.status
    );
    stderr
}

/// Like [`run_lower_child`], but for the single-clause `try_table` module
/// (one pending catch back-patched at the outer block's `end`).
fn run_try_lower_child(target: usize, level: &str) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args(["lower_child", "--exact", "--nocapture"])
        .env("GC_U3_LOWER_FAIL_AT", target.to_string())
        .env("GC_U3_TRY_TABLE", level)
        .output()
        .expect("spawn try_table lowering refusal child");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "refusing try_table lowering allocation {target} ({level}) \
         terminated the process (status {:?}); stderr:\n{stderr}",
        output.status
    );
    stderr
}

// ── pinned refusal windows ──────────────────────────────────────────
//
// Each window covers the former clone allocation and the module-scaled
// bookkeeping that replaced it. Indices are 1-based offsets from the first
// allocation after decoding, pinned empirically for these exact modules: the
// windows shift with result arity because decoding a larger result list
// allocates more.
//
// Every index in a window is asserted not to abort. Indices listed as
// `clean` additionally must surface a `ResourceLimitExceeded` (the fallible
// reservation that now replaces the clone); indices listed only in the
// window were consumed by the fix (`mem::take` / moved data) and therefore no
// longer exist, so refusing them is a clean no-op.

/// Validation window for the reviewer's arity-1 module: the fallible
/// `br_on_cast` copies plus the former `finish_frame` clone index (19).
const VALIDATE_WINDOW_ARITY1: &[usize] = &[16, 17, 18, 19];
const VALIDATE_CLEAN_ARITY1: &[usize] = &[16, 17];

/// Validation window for arity 512: the fallible copies plus the former
/// `finish_frame` clone index (42).
const VALIDATE_WINDOW_ARITY512: &[usize] = &[38, 39, 40, 41, 42];
const VALIDATE_CLEAN_ARITY512: &[usize] = &[38, 39, 40];

/// Lowering window for arity 1: the function-setup copies, the
/// `br_on_cast` lowering copies, the block result registers, the
/// function-end result copies, the control-block / lowered-function
/// reservations, and the fallible `register type table` reservation. The
/// block-close `pending_branches` replacement vector (former index 37) is
/// now eliminated in place, so refusing that index rolls to the `function
/// result types` reservation; every window index at or after 37 shifted
/// down by one relative to the pre-elimination pinning.
const LOWER_WINDOW_ARITY1: &[usize] = &[
    21, 22, 23, 24, 25, 26, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
];
const LOWER_CLEAN_ARITY1: &[usize] = LOWER_WINDOW_ARITY1;

/// Lowering window for arity 512: the validation `br_on_cast` copies, the
/// function-setup copies, the lowering `br_on_cast` copies, the block result
/// registers, the function-end result copies, the control-block /
/// lowered-function reservations, the fallible `register type table`
/// reservation, and the former block-close clone index (now beyond the
/// allocation count, so refusing it is a clean no-op). The block-close
/// `pending_branches` replacement vector (former index 91) is now
/// eliminated in place, so refusing that index rolls to the `function
/// result types` reservation; every window index at or after 91 shifted
/// down by one relative to the pre-elimination pinning.
const LOWER_WINDOW_ARITY512: &[usize] = &[
    38, 39, 40, 51, 52, 53, 54, 55, 56, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 111,
];
const LOWER_CLEAN_ARITY512: &[usize] = &[
    38, 39, 40, 51, 52, 53, 54, 55, 56, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94,
];

/// `try_table` catch window. Each single-clause module exercises one of the
/// block-close / function-end `pending_catches` processing paths that were
/// changed from a module-sized replacement vector to in-place processing.
/// Indices are the post-elimination positions of the removed vectors (which
/// now fall on fallible reservations):
///
/// * `block` module: index 39 is the `try_table` frame's `end` (where the
///   pending catch does not yet match the frame and is retained) and index
///   42 is the targeted block's `end` (where it matches and is processed);
///   both now land on the fallible `control blocks` / `catch landing
///   registers` reservations.
/// * `func` module: index 41 is the function-end path; it now lands on the
///   fallible `catch landing registers` reservation.
const TRY_TABLE_BLOCK_WINDOW: &[usize] = &[39, 42];
const TRY_TABLE_FUNC_WINDOW: &[usize] = &[41];

/// Local-heavy `local_inits` window: the `local-init flags` flag vector and
/// the two `block frame` reservations (the frame's local-init snapshot).
const LOCAL_WINDOW: &[usize] = &[10, 14, 15];

/// Sweep `window` for `form`, asserting no index aborts; additionally assert
/// every index in `clean` clean-refuses.
fn assert_validate_window(arity: usize, form: &str, window: &[usize], clean: &[usize]) {
    for &target in window {
        let stderr = run_validate_child(target, arity, form, 0);
        if clean.contains(&target) {
            assert!(
                stderr.contains("CLEAN_REFUSAL"),
                "validate allocation {target} (arity {arity}, form {form}) \
                 should clean-refuse, saw:\n{stderr}"
            );
        }
    }
}

fn assert_lower_window(arity: usize, form: &str, window: &[usize], clean: &[usize]) {
    for &target in window {
        let stderr = run_lower_child(target, arity, form);
        if clean.contains(&target) {
            assert!(
                stderr.contains("CLEAN_REFUSAL"),
                "lowering allocation {target} (arity {arity}, form {form}) \
                 should clean-refuse, saw:\n{stderr}"
            );
        }
    }
}

/// `finish_frame` no longer clones the module-sized `ControlFrame`: refusing
/// either the fallible copies or the clone's former index (arity 1 index 19,
/// arity 512 index 42) never terminates the process.
#[test]
fn finish_frame_refusal_windows_are_clean() {
    for form in ["cast", "fail"] {
        assert_validate_window(1, form, VALIDATE_WINDOW_ARITY1, VALIDATE_CLEAN_ARITY1);
        assert_validate_window(512, form, VALIDATE_WINDOW_ARITY512, VALIDATE_CLEAN_ARITY512);
    }
}

/// The lowering block-close no longer clones the frame's module-sized
/// `result_types` (r2) and no longer allocates a `pending_branches`
/// replacement vector (r3): refusing the block result-register reservation,
/// the control-block/func reservations, the `pending_branches` vector's
/// former index (arity 1 index 37 -> the fallible `function result types`
/// reservation; arity 512 index 91 -> the same reservation), or the r2
/// clone's former position (arity 1 index 41 -> the fallible `register type
/// table` reservation; arity 512 index 111 -> beyond the allocation count)
/// never terminates the process.
#[test]
fn block_close_refusal_windows_are_clean() {
    for form in ["cast", "fail"] {
        assert_lower_window(1, form, LOWER_WINDOW_ARITY1, LOWER_CLEAN_ARITY1);
        assert_lower_window(512, form, LOWER_WINDOW_ARITY512, LOWER_CLEAN_ARITY512);
    }
}

/// The `try_table` pending-catch vectors no longer allocate a replacement
/// `Vec`: refusing the former replacement-vector indices (or the fallible
/// reservations that now occupy those positions) never terminates the
/// process, for both the block-close and function-end catch paths.
#[test]
fn pending_catch_refusal_windows_are_clean() {
    for &target in TRY_TABLE_BLOCK_WINDOW {
        let stderr = run_try_lower_child(target, "block");
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "try_table block allocation {target} should clean-refuse, saw:\n{stderr}"
        );
    }
    for &target in TRY_TABLE_FUNC_WINDOW {
        let stderr = run_try_lower_child(target, "func");
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "try_table func allocation {target} should clean-refuse, saw:\n{stderr}"
        );
    }
}

/// A 1024-local module exercises the `local_inits` copies; refusing the flag
/// vector or either block-frame reservation clean-refuses.
#[test]
fn local_heavy_local_inits_refusal_window_is_clean() {
    for &target in LOCAL_WINDOW {
        let stderr = run_validate_child(target, 1, "cast", 1024);
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "local-heavy allocation {target} should clean-refuse, saw:\n{stderr}"
        );
    }
}
