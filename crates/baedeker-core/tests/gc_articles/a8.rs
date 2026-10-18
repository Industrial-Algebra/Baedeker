// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A8 — v128 accounting: structs with a `v128` field and arrays of `v128`,
//! built and rebuilt in exact counts.
//!
//! Graded: `live_bytes` exactly equals the logical formula times the count and
//! `allocations_since` is exact. The formula (unit-2 contract):
//!
//! ```text
//! struct with a v128 field: 8-byte header + v128(16)      = 24
//! array  v128[3]:          12-byte header + 3 × v128(16)  = 60
//! ```

use baedeker_core::runtime::Value;

use crate::harness::{article_count, call_i32, instantiate};

const STRUCT_BYTES: u64 = 8 + 16;
const ARRAY_BYTES: u64 = 12 + 3 * 16;
const ARRAY_LEN: u32 = 3;

const SOURCE: &str = r#"
(module
  (type $vs (struct (field v128)))
  (type $va (array (mut v128)))

  (func (export "build") (param $structs i32) (param $arrays i32) (result i32)
    (local $i i32) (local $acc i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $structs)))
        (drop (struct.new $vs (v128.const i32x4 1 2 3 4)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (block $done2
      (loop $loop2
        (br_if $done2 (i32.ge_u (local.get $acc) (local.get $arrays)))
        (drop (array.new_fixed $va 3
          (v128.const i32x4 1 2 3 4)
          (v128.const i32x4 5 6 7 8)
          (v128.const i32x4 9 10 11 12)))
        (local.set $acc (i32.add (local.get $acc) (i32.const 1)))
        (br $loop2)))
    (i32.const 0)))
"#;

#[test]
fn a8_v128_structures_and_arrays_account_exactly() {
    let structs = article_count(200_000);
    let arrays = article_count(200_000);
    let (module, store) = instantiate(SOURCE);

    // Phase 1: build in exact counts.
    let snapshot = store.gc_snapshot();
    assert_eq!(
        call_i32(
            &module,
            &store,
            "build",
            &[Value::I32(structs as i32), Value::I32(arrays as i32)]
        ),
        0
    );
    let after_build = store.gc_stats();
    let expected_objects = structs + arrays;
    let expected_bytes = STRUCT_BYTES * structs + ARRAY_BYTES * arrays;
    assert_eq!(
        store.gc_allocations_since(snapshot),
        expected_objects,
        "one struct and one array per requested build"
    );
    assert_eq!(after_build.live_objects as u64, expected_objects);
    assert_eq!(
        after_build.live_bytes, expected_bytes,
        "v128 fields count 16 logical bytes; headers are 8 (struct) and 12 (array)"
    );

    // Phase 2: rebuild the same counts; every object stays live (never-free),
    // so the totals are the formula times the TOTAL count.
    assert_eq!(
        call_i32(
            &module,
            &store,
            "build",
            &[Value::I32(structs as i32), Value::I32(arrays as i32)]
        ),
        0
    );
    let after_rebuild = store.gc_stats();
    assert_eq!(
        store.gc_allocations_since(snapshot),
        2 * expected_objects,
        "allocations_since is exact across both phases"
    );
    assert_eq!(after_rebuild.live_objects as u64, 2 * expected_objects);
    assert_eq!(
        after_rebuild.live_bytes,
        2 * expected_bytes,
        "live_bytes is the formula times the total build count"
    );
    assert_eq!(
        ARRAY_LEN, 3,
        "the array formula above assumes three v128 elements"
    );
}
