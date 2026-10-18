// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A1 — ephemeral churn: 10⁶ short-lived pairs in a bounded live window.
//!
//! Graded: schedule-independent result (a triangular sum), exact
//! `allocations_since`, and — in a capped child process — a structured
//! `ResourceLimitExceeded` refusal with the full `GcStats` snapshot untouched
//! by the refused allocation.
//!
//! Slice-5 honesty: the heap never frees, so the churn's "bounded live window"
//! (one wasm global slot) does not bound `live_objects`; the article asserts
//! the never-free accounting (`live_objects == allocations`) rather than a
//! bounded live set, which is the 5b flip.

use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeError, Store, Value, execute_export};

use crate::harness::{
    CHUNK, article_count, child_marker, child_stats, instantiate, is_child, is_refusal,
    spawn_capped_child, sum_chunks, triangular, yes_no,
};

const SOURCE: &str = r#"
(module
  (type $pair (struct (field i32) (field (ref null $pair))))
  (type $big (array (mut i32)))
  ;; The bounded live window: one slot, overwritten every iteration.
  (global $live (mut (ref null $pair)) (ref.null $pair))
  ;; Churn $n pairs starting at value $base; the sum over the whole range is
  ;; chunk-independent, so large campaigns may be split across calls.
  (func (export "churn") (param $n i32) (param $base i32) (result i32)
    (local $i i32) (local $acc i32) (local $value i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $value (i32.add (local.get $base) (local.get $i)))
        (global.set $live
          (struct.new $pair (local.get $value) (global.get $live)))
        (local.set $acc (i32.add (local.get $acc) (local.get $value)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $acc))
  ;; A reservation no process can satisfy: u32::MAX i32 elements.
  (func (export "huge") (result i32)
    (drop (array.new_default $big (i32.const -1)))
    (i32.const 0)))
"#;

/// Call `churn` and reduce its single i32 result, preserving errors as-is.
fn expected_i32(
    module: &RegModule,
    store: &Store,
    count: u64,
    base: i32,
) -> Result<i32, RuntimeError> {
    let values = execute_export(
        module,
        store,
        "churn",
        &[Value::I32(count as i32), Value::I32(base)],
    )?;
    if let [Value::I32(value)] = values[..] {
        Ok(value)
    } else {
        panic!("churn returned {values:?}, expected one i32")
    }
}

const ARTICLE: &str = "a1-capped";
const TEST: &str = "a1::a1_capped_churn_refuses_structurally";

#[test]
fn a1_ephemeral_churn_is_schedule_independent_and_accounted() {
    let n = article_count(1_000_000);
    let (module, store) = instantiate(SOURCE);

    let snapshot = store.gc_snapshot();
    let result =
        sum_chunks(n, CHUNK, |count| expected_i32(&module, &store, count, 0)).expect("churn");
    assert_eq!(
        result,
        i64::from(triangular(n) as i32),
        "churn result must be the schedule-independent triangular sum"
    );

    assert_eq!(
        store.gc_allocations_since(snapshot),
        n,
        "exactly one struct per churned pair"
    );
    let stats = store.gc_stats();
    // pair = 8-byte header + i32 + 4-byte reference handle = 16 logical bytes.
    assert_eq!(
        stats.live_objects, n as u32,
        "never-free honesty: no reclamation"
    );
    assert_eq!(stats.live_bytes, 16 * n, "logical pair accounting");
}

/// Capped run: `ulimit -v` 64 MiB child (the design's 48/64 MiB smoke; 64 MiB
/// is the executable point because a Rust test binary needs address-space
/// headroom). The child churns to 10⁷ pairs (10× the article magnitude), then
/// attempts a reservation no process can satisfy.
#[test]
fn a1_capped_churn_refuses_structurally() {
    if is_child(ARTICLE) {
        return a1_child();
    }
    spawn_capped_child(ARTICLE, TEST).require_structured_refusal(ARTICLE);
}

fn a1_child() {
    let (module, store) = instantiate(SOURCE);
    let capped = crate::harness::process_is_capped();

    // Pressure phase: 10⁷ pairs under the cap, chunked across calls so the
    // engine's per-call backstop is never the thing that stops it.
    let mut base = 0u64;
    let mut pressure_refused = false;
    while base < 10_000_000 {
        match expected_i32(&module, &store, (10_000_000 - base).min(CHUNK), base as i32) {
            Ok(_) => base += CHUNK,
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

    // Atomic refusal: snapshot, attempt the impossible reservation, snapshot.
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
}
