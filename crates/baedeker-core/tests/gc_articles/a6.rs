// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A6 — `array.new_data` / `array.new_elem` hybrid plus linear-memory pressure.
//!
//! Graded: exact results (byte sum from a data-segment array, non-null count
//! from an element-segment array), GC logical bytes exactly equal to the
//! array formulas, and linear-memory pages accounted **separately** — growing
//! linear memory leaves `GcStats` untouched, and the GC bytes are independent
//! of the page count.

use baedeker_core::runtime::Value;

use crate::harness::{article_count, call_i32, instantiate, memory_pages};

const ELEM_LEN: u32 = 3;
const GROW_PAGES: u32 = 16;

const DATA_LEN: u32 = 8;

const SOURCE: &str = r#"
(module
  (memory 1)
  (type $bytes (array (mut i8)))
  (type $refs (array (mut funcref)))
  (data $d "\01\02\03\04\05\06\07\08")
  (elem $e func $f0 $f1 $f2)
  (func $f0) (func $f1) (func $f2)

  (func $sum_bytes (param $a (ref $bytes)) (result i32)
    (local $i i32) (local $sum i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (i32.const 8)))
        (local.set $sum
          (i32.add (local.get $sum) (array.get_u $bytes (local.get $a) (local.get $i))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $sum))

  (func $count_refs (param $a (ref $refs)) (result i32)
    (local $i i32) (local $n i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (i32.const 3)))
        (local.set $n
          (i32.add
            (local.get $n)
            (i32.eqz (ref.is_null (array.get $refs (local.get $a) (local.get $i))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $n))

  ;; One hybrid build: a data-backed array and an elem-backed array.
  (func (export "build") (param $n i32) (result i32)
    (local $i i32) (local $acc i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $acc
          (i32.add (local.get $acc)
            (call $sum_bytes
              (array.new_data $bytes $d (i32.const 0) (i32.const 8)))))
        (local.set $acc
          (i32.add (local.get $acc)
            (call $count_refs
              (array.new_elem $refs $e (i32.const 0) (i32.const 3)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $acc))

  (func (export "grow") (param $pages i32) (result i32)
    (memory.grow (local.get $pages))))
"#;

#[test]
fn a6_new_data_new_elem_hybrid_keeps_gc_and_memory_accounting_separate() {
    let builds = article_count(100_000);
    let (module, store) = instantiate(SOURCE);
    let pages_before = memory_pages(&store, 0);

    let snapshot = store.gc_snapshot();
    // per build: byte sum 36 + non-null ref count 3.
    let expected_per_build = 36 + i64::from(ELEM_LEN);
    assert_eq!(
        call_i32(&module, &store, "build", &[Value::I32(builds as i32)]) as i64,
        expected_per_build * builds as i64
    );

    let stats = store.gc_stats();
    // array i8[8]  = 12-byte header + 8 * 1  = 20
    // array funcref[3] = 12-byte header + 3 * 4 = 24
    let per_build_bytes = (12 + u64::from(DATA_LEN)) + (12 + 4 * u64::from(ELEM_LEN));
    assert_eq!(
        store.gc_allocations_since(snapshot),
        2 * builds,
        "one data-backed and one elem-backed array per build"
    );
    assert_eq!(stats.live_objects as u64, 2 * builds);
    assert_eq!(
        stats.live_bytes,
        per_build_bytes * builds,
        "GC logical bytes follow the array formula"
    );

    // Linear-memory pressure is accounted separately: growing memory changes
    // no GC accounting at all.
    let grown = call_i32(&module, &store, "grow", &[Value::I32(GROW_PAGES as i32)]);
    assert_eq!(
        grown as u32, pages_before as u32,
        "memory.grow returns the old size"
    );
    assert_eq!(
        memory_pages(&store, 0),
        pages_before + u64::from(GROW_PAGES),
        "linear memory grew by exactly the requested pages"
    );
    assert_eq!(
        store.gc_stats(),
        stats,
        "linear-memory pages are not GC logical bytes"
    );
}
