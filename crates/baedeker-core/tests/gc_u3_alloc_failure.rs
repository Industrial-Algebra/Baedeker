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
//! The 2026-10-19 follow-up sweep (below) fallibilized every remaining
//! allocation on the `Module::validate` + `Module::lower` path — operand,
//! control, and register stacks grow in try-reserved geometric chunks;
//! block/frame bootstraps use fallible constructors; every module-scaled
//! vector reserves before it is filled. Every refusal index in every config
//! this file pins now clean-refuses.
//!
//! The 2026-10-20 static audit closed the last remaining infallible sites on
//! that path (import name/type clones, the `else` frame copies, the appended
//! branch-copy instructions, `array.new_fixed` operands, struct field types,
//! the const-expression operand stack, the function `end_types` copy, and the
//! table-type initializer clone) and replaced the pinned refusal windows with
//! *measured* ones: each config first runs the child once with the allocator
//! disarmed to learn how many allocations the path performs on THIS target,
//! then sweeps every index in `1..=count`. Allocation sequences differ per
//! architecture (size classes and layouts), so a window pinned on x86_64 is
//! not evidence for aarch64 — measuring the count in the same binary keeps
//! the whole sequence covered everywhere, and an index the sweep skips can no
//! longer exist.
//!
//! This file owns a failing global allocator, so it contains only these
//! tests. The refusal sweep runs each allocation index in a child process so
//! a regression that aborts is observed as a failed child rather than killing
//! the parent.
//!
//! Counting is **scoped to the sweeping thread**. libtest runs the test body
//! on its own thread while the harness thread keeps allocating (output
//! formatting, event plumbing); a process-global counter let those harness
//! allocations occupy refusal indices, which aborted the *harness* with
//! `memory allocation of N bytes failed` at a random index — the same
//! signature as an infallible library allocation, and the reason the sweep
//! could flake differently on another architecture. Only the thread that arms
//! the sweep (via `with_armed_sweep`) counts or refuses allocations, so each
//! index names exactly one allocation on the validate/lower path.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

use baedeker_core::binary::module::Module;
use baedeker_core::lower::LowerErrorKind;
use baedeker_core::validate::error::ValidationErrorKind;

/// Total `alloc`/`realloc` calls observed on the armed thread.
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Absolute (1-based) allocation count that returns null. 0 disarms.
static FAIL_AT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Whether the CURRENT thread participates in the sweep. `Cell<bool>` has
    /// no destructor, so this is a const-initialized TLS slot: reading it from
    /// inside the allocator neither allocates nor risks a TLS-destruction
    /// panic.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread is running (or arming) a refusal sweep.
fn armed() -> bool {
    ARMED.try_with(Cell::get).unwrap_or(false)
}

/// Run `body` with this thread's allocations counted, refusing allocation
/// `fail_at` (1-based, relative to the first allocation inside `body`).
/// Returns the number of allocations the body performed and its result.
fn with_armed_sweep<R>(fail_at: usize, body: impl FnOnce() -> R) -> (R, usize) {
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    FAIL_AT.store(fail_at, Ordering::SeqCst);
    ARMED.with(|armed| armed.set(true));
    let result = body();
    let count = ALLOC_COUNT.load(Ordering::SeqCst);
    ARMED.with(|armed| armed.set(false));
    FAIL_AT.store(0, Ordering::SeqCst);
    (result, count)
}

struct FailAt;

unsafe impl GlobalAlloc for FailAt {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !armed() {
            return unsafe { System.alloc(layout) };
        }
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
        if !armed() {
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
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

/// A `br_table` module: `arity` leading `i32` block results and a five-entry
/// target list all naming the same outer block. Decoding the target list was
/// an infallible `Vec::with_capacity` in `decode_instr` (reachable lazily from
/// validate/lower), and lowering emits the same list into a `BrTable`
/// terminator.
fn br_table_wat(arity: usize) -> String {
    let arity = arity.max(1);
    let mut results = String::new();
    let mut body = String::new();
    for _ in 0..arity {
        results.push_str("i32 ");
        body.push_str("      i32.const 7\n");
    }
    format!(
        "(module\n  (func (param i32) (result {results})\n    \
         (block $b (result {results})\n{body}      local.get 0\n      \
         br_table $b $b $b $b $b)))"
    )
}

/// A typed-`select` module: the instruction's result-type list is decoded by
/// `parse_result_types`, whose `Vec::with_capacity` was an infallible
/// allocation reachable from validate/lower.
fn select_typed_wat() -> String {
    String::from(
        "(module\n  (func (param i32) (result i32)\n    \
         local.get 0\n    i32.const 1\n    i32.const 2\n    \
         select (result i32)))",
    )
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
    let form = std::env::var("GC_U3_FORM").unwrap_or_default();
    match form.as_str() {
        "br_table" => decode(&br_table_wat(arity)),
        "select_typed" => decode(&select_typed_wat()),
        "fail" => decode(&cast_branch_wat(arity, true)),
        _ => decode(&cast_branch_wat(arity, false)),
    }
}

// ── validation ──────────────────────────────────────────────────────

/// Child entry point: validate with allocation `GC_U3_FAIL_AT` refused.
#[test]
fn validate_child() {
    let Ok(target) = std::env::var("GC_U3_FAIL_AT") else {
        return; // parent process: no-op
    };
    let target: usize = target.parse().unwrap();

    // Probe the harness env BEFORE the armed window: `env::var` allocates,
    // and those allocations must not count as part of the validate sequence.
    let report_count = std::env::var("GC_U3_REPORT_COUNT").is_ok();
    let module = child_module();
    let (result, count) =
        with_armed_sweep(if report_count { 0 } else { target }, || module.validate());

    if report_count {
        eprintln!("COUNT {count}");
        return;
    }

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
    run_validate_child_measured(target, arity, form, locals, false)
}

fn run_validate_child_measured(
    target: usize,
    arity: usize,
    form: &str,
    locals: usize,
    report_count: bool,
) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = std::process::Command::new(exe);
    command
        .args(["validate_child", "--exact", "--nocapture"])
        .env("GC_U3_FAIL_AT", target.to_string())
        .env("GC_U3_ARITY", arity.to_string())
        .env("GC_U3_FORM", form)
        .env("GC_U3_LOCALS", locals.to_string());
    if report_count {
        command.env("GC_U3_REPORT_COUNT", "1");
    }
    let output = command.output().expect("spawn validate refusal child");
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

    // Probe the harness env BEFORE the armed window: `env::var` allocates,
    // and those allocations must not count as part of the lower sequence.
    let report_count = std::env::var("GC_U3_REPORT_COUNT").is_ok();
    let module = child_module();
    let (result, count) =
        with_armed_sweep(if report_count { 0 } else { target }, || module.lower());

    if report_count {
        eprintln!("COUNT {count}");
        return;
    }

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
    run_lower_child_measured(target, arity, form, false)
}

fn run_lower_child_measured(target: usize, arity: usize, form: &str, report_count: bool) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = std::process::Command::new(exe);
    command
        .args(["lower_child", "--exact", "--nocapture"])
        .env("GC_U3_LOWER_FAIL_AT", target.to_string())
        .env("GC_U3_ARITY", arity.to_string())
        .env("GC_U3_FORM", form);
    if report_count {
        command.env("GC_U3_REPORT_COUNT", "1");
    }
    let output = command.output().expect("spawn lowering refusal child");
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
    run_try_lower_child_measured(target, level, false)
}

fn run_try_lower_child_measured(target: usize, level: &str, report_count: bool) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = std::process::Command::new(exe);
    command
        .args(["lower_child", "--exact", "--nocapture"])
        .env("GC_U3_LOWER_FAIL_AT", target.to_string())
        .env("GC_U3_TRY_TABLE", level);
    if report_count {
        command.env("GC_U3_REPORT_COUNT", "1");
    }
    let output = command
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

// ── measured refusal windows ───────────────────────────────────────
//
// Indices are 1-based offsets from the first allocation after decoding.
//
// The number of allocations a config performs depends on the TARGET:
// allocator size classes and value layouts differ per architecture, so a
// window pinned on x86_64 is not evidence for aarch64. Each config therefore
// MEASURES its allocation count in a child process on the running target and
// sweeps `1..=count`: the complete contiguous sequence, with every index
// required to surface a clean `ResourceLimitExceeded` and none permitted to
// abort. Combined with the thread-scoped counting above, an index names
// exactly one allocation on the validate/lower path on every target.
//
// The 2026-10-19 sweep fallibilized *every* allocation on the
// `Module::validate` + `Module::lower` path (operand/control/register stacks
// grow in try-reserved geometric chunks, block and frame bootstraps use
// fallible constructors, every module-scaled vector reserves before filling).
// The 2026-10-20 static audit closed the remaining sites (import name/type
// clones, the `else` frame copies, appended branch-copy instructions,
// `array.new_fixed` operands, struct field types, the const-expression
// operand stack, the `return` result-type copy, and the table-type
// initializer clone); every index of every config below must clean-refuse.

/// The arities the sweep matrix covers.
const SWEEP_ARITIES: [usize; 3] = [1, 64, 512];

/// Extract the allocation count a child reported under `GC_U3_REPORT_COUNT`.
fn parse_count(stderr: &str) -> usize {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("COUNT "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("child did not report an allocation count:\n{stderr}"))
}

fn measure_validate_allocations(arity: usize, form: &str, locals: usize) -> usize {
    parse_count(&run_validate_child_measured(0, arity, form, locals, true))
}

fn measure_lower_allocations(arity: usize, form: &str) -> usize {
    parse_count(&run_lower_child_measured(0, arity, form, true))
}

fn measure_try_lower_allocations(level: &str) -> usize {
    parse_count(&run_try_lower_child_measured(0, level, true))
}

/// Sweep every allocation index the validation path performs on this target,
/// asserting each one clean-refuses.
fn assert_validate_sweep(arity: usize, form: &str, locals: usize) {
    let count = measure_validate_allocations(arity, form, locals);
    assert!(
        count > 0,
        "validation path performed no allocations (arity {arity}, form {form}, \
         locals {locals}) — the sweep would be vacuous"
    );
    for target in 1..=count {
        let stderr = run_validate_child(target, arity, form, locals);
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "validate allocation {target} of {count} (arity {arity}, form {form}, \
             locals {locals}) should clean-refuse, saw:\n{stderr}"
        );
    }
}

/// Sweep every allocation index the lowering path performs on this target,
/// asserting each one clean-refuses.
fn assert_lower_sweep(arity: usize, form: &str) {
    let count = measure_lower_allocations(arity, form);
    assert!(
        count > 0,
        "lowering path performed no allocations (arity {arity}, form {form})"
    );
    for target in 1..=count {
        let stderr = run_lower_child(target, arity, form);
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "lowering allocation {target} of {count} (arity {arity}, form {form}) \
             should clean-refuse, saw:\n{stderr}"
        );
    }
}

fn assert_try_lower_sweep(level: &str) {
    let count = measure_try_lower_allocations(level);
    assert!(
        count > 0,
        "try_table lowering path performed no allocations ({level})"
    );
    for target in 1..=count {
        let stderr = run_try_lower_child(target, level);
        assert!(
            stderr.contains("CLEAN_REFUSAL"),
            "try_table {level} allocation {target} of {count} should \
             clean-refuse, saw:\n{stderr}"
        );
    }
}

/// `finish_frame` no longer clones the module-sized `ControlFrame`: refusing
/// any of the validator's reservations on these modules (every allocation
/// index is fallibly reserved) never terminates the process and always
/// surfaces a clean `ResourceLimitExceeded`.
#[test]
fn finish_frame_refusal_windows_are_clean() {
    for form in ["cast", "fail"] {
        for arity in SWEEP_ARITIES {
            assert_validate_sweep(arity, form, 0);
        }
    }
}

/// The lowering block-close no longer clones the frame's module-sized
/// `result_types` (r2) and no longer allocates a `pending_branches`
/// replacement vector (r3). Every allocation the validate + lower path makes
/// for these modules — validation reservations, the block result-register
/// reservation, the control-block/func reservations, and the `register type
/// table` reservation — is fallibly reserved, so refusing any index never
/// terminates the process and always surfaces a clean `ResourceLimitExceeded`.
#[test]
fn block_close_refusal_windows_are_clean() {
    for form in ["cast", "fail"] {
        for arity in SWEEP_ARITIES {
            assert_lower_sweep(arity, form);
        }
    }
}

/// The `try_table` pending-catch vectors no longer allocate a replacement
/// `Vec`: refusing any allocation index on the lowering path (including the
/// fallible `catch landing registers` and `catch value types` reservations)
/// never terminates the process, for both the block-close and function-end
/// catch paths.
#[test]
fn pending_catch_refusal_windows_are_clean() {
    assert_try_lower_sweep("block");
    assert_try_lower_sweep("func");
}

/// The `br_table` target list and the typed-`select` result-type list are
/// decoded lazily from validate/lower, so their reservations must be fallible
/// on both paths: every allocation index clean-refuses and none aborts.
#[test]
fn br_table_and_typed_select_refusal_windows_are_clean() {
    for arity in SWEEP_ARITIES {
        assert_validate_sweep(arity, "br_table", 0);
        assert_lower_sweep(arity, "br_table");
    }
    assert_validate_sweep(1, "select_typed", 0);
    assert_lower_sweep(1, "select_typed");
}

/// A 1024-local module exercises the `local_inits` copies; refusing any
/// allocation index on the validation path (the flag vector, either
/// block-frame reservation, or the operand-stack chunk growth) clean-refuses.
#[test]
fn local_heavy_local_inits_refusal_window_is_clean() {
    assert_validate_sweep(1, "cast", 1024);
}
