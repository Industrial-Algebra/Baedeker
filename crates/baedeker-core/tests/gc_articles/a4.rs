// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A4 — i31 storm: 10⁷ `ref.i31` / `i31.get_s` / `i31.get_u` operations.
//!
//! i31 is an immediate: it never enters the object table and contributes zero
//! logical bytes. Graded: the exact (wrapping) sums, and a `GcStats` delta of
//! exactly 0 across the i31 phase, contrasted with a boxed-struct phase whose
//! delta equals the box count.

use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeError, Store, Value, execute_export};

use crate::harness::{CHUNK, article_count, call_i32, instantiate};

fn i31_chunk(
    module: &RegModule,
    store: &Store,
    count: u64,
    base: i64,
) -> Result<i32, RuntimeError> {
    let values = execute_export(
        module,
        store,
        "i31",
        &[Value::I32(count as i32), Value::I32(base as i32)],
    )?;
    if let [Value::I32(value)] = values[..] {
        Ok(value)
    } else {
        panic!("i31 returned {values:?}, expected one i32")
    }
}

const SOURCE: &str = r#"
(module
  (type $box (struct (field i32)))

  (func (export "i31") (param $n i32) (param $base i32) (result i32)
    (local $i i32) (local $acc i32) (local $value i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $value (i32.add (local.get $base) (local.get $i)))
        ;; ref.i31 -> i31.get_s round trip, then the unsigned accessor.
        (local.set $acc
          (i32.add
            (local.get $acc)
            (i31.get_s (ref.i31 (local.get $value)))))
        (local.set $acc
          (i32.add
            (local.get $acc)
            (i31.get_u (ref.i31 (local.get $value)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $acc))

  (func (export "boxed") (param $n i32) (result i32)
    (local $i i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (drop (struct.new $box (local.get $i)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $i)))
"#;

#[test]
fn a4_i31_storm_allocates_nothing() {
    let n = article_count(10_000_000);
    let (module, store) = instantiate(SOURCE);

    let snapshot = store.gc_snapshot();
    let before = store.gc_stats();
    let mut base = 0u64;
    let mut sum = 0i32;
    loop {
        let this = CHUNK.min(n - base);
        if this == 0 {
            break;
        }
        sum = sum.wrapping_add(i31_chunk(&module, &store, this, base as i64).expect("i31"));
        base += this;
    }
    let result = sum;
    let after = store.gc_stats();

    // Each iteration adds two copies of its value (signed and unsigned
    // accessors), so the oracle is a wrapping doubling of the running sum.
    let expected =
        2i32.wrapping_mul((0..n as i32).fold(0i32, |acc, value| acc.wrapping_add(value)));
    assert_eq!(result, expected, "i31 storm result");
    assert_eq!(after, before, "i31 operations must not change GcStats");
    assert_eq!(
        store.gc_allocations_since(snapshot),
        0,
        "i31 is an immediate: zero allocations"
    );

    // Contrast: the same count of BOXED i32s allocates one struct each.
    let boxed = article_count(100_000);
    let boxed_snapshot = store.gc_snapshot();
    let boxed_before = store.gc_stats();
    assert_eq!(
        call_i32(&module, &store, "boxed", &[Value::I32(boxed as i32)]) as u64,
        boxed
    );
    let boxed_after = store.gc_stats();
    assert_eq!(
        store.gc_allocations_since(boxed_snapshot),
        boxed,
        "one struct per boxed integer"
    );
    assert_eq!(
        boxed_after.live_objects - boxed_before.live_objects,
        boxed as u32
    );
    // box = 8 header + i32 = 12 logical bytes.
    assert_eq!(
        boxed_after.live_bytes - boxed_before.live_bytes,
        12 * boxed,
        "boxed integers account 12 logical bytes each"
    );
}
