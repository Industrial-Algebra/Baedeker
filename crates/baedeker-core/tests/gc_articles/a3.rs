// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A3 — cycle farm: 10⁵ rotating cycles.
//!
//! Each cycle builds a fresh ring of `RING_SIZE` nodes (the last node points
//! back at the first) and rotates it exactly `RING_SIZE` steps, summing the
//! visited values. Graded: the schedule-independent sum, plus the slice-5
//! honesty assertion — with a never-free heap `live_objects` is monotonic
//! non-decreasing across the campaign (sampled from the host per chunk). 5b
//! flips this assertion to bounded retention.

use baedeker_core::runtime::Value;

use crate::harness::{article_count, call_i32, instantiate};

const SOURCE: &str = r#"
(module
  (type $node (struct (field i32) (field (mut (ref null $node)))))

  ;; Build a ring of $size nodes with values 1..=size, then rotate it exactly
  ;; one full lap from the last-built node and return the visited sum.
  (func (export "cycle") (param $size i32) (result i32)
    (local $i i32) (local $head (ref null $node))
    (local $first (ref null $node)) (local $cur (ref null $node))
    (local $sum i32) (local $steps i32)
    (block $done
      (loop $build
        (br_if $done (i32.ge_u (local.get $i) (local.get $size)))
        (local.set $head
          (struct.new $node
            (i32.add (local.get $i) (i32.const 1))
            (local.get $head)))
        ;; The first node built has a null `next`: remember it so the ring can
        ;; be closed after the build loop.
        (if (i32.eqz (local.get $i))
          (then (local.set $first (local.get $head))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $build)))
    (if (ref.is_null (local.get $first))
      (then (return (i32.const 0)))
      (else (struct.set $node 1 (local.get $first) (local.get $head))))
    ;; One full lap from the head (values size .. 1).
    (local.set $cur (local.get $head))
    (block $rotated
      (loop $rotate
        (br_if $rotated (i32.ge_u (local.get $steps) (local.get $size)))
        (local.set $sum
          (i32.add (local.get $sum) (struct.get $node 0 (local.get $cur))))
        (local.set $cur (struct.get $node 1 (local.get $cur)))
        (local.set $steps (i32.add (local.get $steps) (i32.const 1)))
        (br $rotate)))
    (local.get $sum)))
"#;

const RING_SIZE: u32 = 8;

/// Full ring of values `1..=size` = `size * (size + 1) / 2`.
fn lap_sum(size: u32) -> i32 {
    (size * (size + 1) / 2) as i32
}

#[test]
fn a3_cycle_farm_results_are_exact_and_live_objects_never_shrink() {
    let cycles = article_count(100_000) as u32;
    let (module, store) = instantiate(SOURCE);

    let snapshot = store.gc_snapshot();
    let mut previous_live = 0u32;
    let chunk = (cycles / 100).max(1);
    let mut done = 0u32;
    while done < cycles {
        let this = chunk.min(cycles - done);
        for _ in 0..this {
            assert_eq!(
                call_i32(&module, &store, "cycle", &[Value::I32(RING_SIZE as i32)]),
                lap_sum(RING_SIZE),
                "one full lap of the ring sums to the triangular number"
            );
        }
        done += this;
        let live = store.gc_stats().live_objects;
        assert!(
            live >= previous_live,
            "slice-5 honesty: live_objects must be monotonic non-decreasing \
             ({live} after {previous_live})"
        );
        previous_live = live;
    }

    let stats = store.gc_stats();
    let expected_objects = u64::from(cycles) * u64::from(RING_SIZE);
    assert_eq!(
        store.gc_allocations_since(snapshot),
        expected_objects,
        "each cycle allocates exactly one object per ring node"
    );
    assert_eq!(stats.live_objects as u64, expected_objects);
    // node = 8 header + i32 + 4-byte reference handle = 16 logical bytes.
    assert_eq!(stats.live_bytes, 16 * expected_objects);
}
