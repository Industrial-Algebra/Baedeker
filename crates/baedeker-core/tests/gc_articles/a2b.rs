// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A2b — persistent build (refusal): a 10⁷-node construction attempt under a
//! 48/64 MiB address-space cap.
//!
//! Graded in a child process (see `harness::spawn_capped_child`): the refusal
//! is a structured `ResourceLimitExceeded` (never a panic or abort), and the
//! full `GcStats` snapshot taken immediately before the refused allocation is
//! identical afterwards — prior successful article allocations stand, and the
//! failing allocation is atomic.
//!
//! The cap is `ulimit -v` 64 MiB (the design's 48/64 MiB smoke; 64 MiB is the
//! executable point because the test binary itself needs address space). The
//! article additionally attempts a reservation no process can satisfy
//! (`u32::MAX` i32 array elements), so the structured-refusal and
//! stats-untouched assertions hold on hosts without an enforceable RLIMIT_AS
//! (macOS), where the pressure phase is reported as not enforced.

use baedeker_core::runtime::{Value, execute_export};

use crate::harness::{
    CHUNK, article_count, child_marker, child_stats, instantiate, is_child, is_refusal,
    process_is_capped, spawn_capped_child, yes_no,
};

const SOURCE: &str = r#"
(module
  (type $node (struct (field i32) (field i64) (field (ref null $node))))
  (type $big (array (mut i32)))

  (func (export "build") (param $n i32) (result i32)
    (local $i i32) (local $head (ref null $node))
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $head
          (struct.new $node
            (local.get $i)
            (i64.const 7)
            (local.get $head)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (drop (local.get $head))
    (local.get $i))

  (func (export "huge") (result i32)
    (drop (array.new_default $big (i32.const -1)))
    (i32.const 0)))
"#;

const ARTICLE: &str = "a2b-capped";
const TEST: &str = "a2b::a2b_persistent_build_refuses_under_caps";

#[test]
fn a2b_persistent_build_refuses_under_caps() {
    if is_child(ARTICLE) {
        return a2b_child();
    }
    spawn_capped_child(ARTICLE, TEST).require_structured_refusal(ARTICLE);
}

fn a2b_child() {
    let (module, store) = instantiate(SOURCE);
    let capped = process_is_capped();

    // Provisioned base: a real workload before the caps make themselves felt,
    // so "state untouched" is graded against non-trivial prior state. Kept
    // small: under the 64 MiB address-space cap the test binary itself holds
    // most of the headroom.
    let base = article_count(5_000) as i32;
    execute_export(&module, &store, "build", &[Value::I32(base)])
        .expect("provisioned base build must succeed");

    // Pressure phase: 10⁷ nodes (the design's magnitude) under the cap,
    // chunked so the engine's per-call backstop is never what stops it.
    // When NO cap is enforceable (macOS: `ulimit -v` is a no-op there), the
    // workload must not run to its full magnitude — two uncapped children
    // at 10⁷ live nodes each OOM-killed the hosted MacCloud runner (CI
    // incident, 2026-10-19). The refusal itself is only gradeable under a
    // real cap; uncapped, two chunks preserve the schedule-independence
    // evidence at bounded memory and the grader already skips the
    // pressure-refusal assertion when `capped=no`.
    let pressure_target: u64 = if capped { 10_000_000 } else { 2 * CHUNK };
    let mut built = 0u64;
    let mut pressure_refused = false;
    while built < pressure_target {
        match execute_export(
            &module,
            &store,
            "build",
            &[Value::I32((pressure_target - built).min(CHUNK) as i32)],
        ) {
            Ok(_) => built += CHUNK,
            Err(error) => {
                assert!(
                    is_refusal(&error),
                    "pressure refusal must be ResourceLimitExceeded, got {}",
                    error.kind
                );
                pressure_refused = true;
                break;
            }
        }
    }

    // Atomic refusal of a single allocation, with the full snapshot compared
    // immediately before and after.
    let before = store.gc_stats();
    let huge_refused = match execute_export(&module, &store, "huge", &[]) {
        Ok(_) => false,
        Err(error) => {
            assert!(
                is_refusal(&error),
                "over-large refusal must be ResourceLimitExceeded, got {}",
                error.kind
            );
            true
        }
    };
    let after = store.gc_stats();
    child_stats("before-huge", before);
    child_stats("after-huge", after);
    child_marker(&format!(
        "article={ARTICLE} capped={} pressure_refused={} huge_refused={} stats_untouched={}",
        yes_no(capped),
        yes_no(pressure_refused),
        yes_no(huge_refused),
        yes_no(before == after),
    ));
    assert_eq!(
        before, after,
        "a refused allocation must leave the full GcStats snapshot identical"
    );
    assert!(
        before.live_objects > 0,
        "the provisioned base must still be accounted after the refusal"
    );
}
