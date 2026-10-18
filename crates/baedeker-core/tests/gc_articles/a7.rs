// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A7 — mixed-reference matrix: funcref / externref / `GcRef` in tables,
//! globals and parameters, plus a 10⁶-iteration conversion round-trip storm
//! (`any.convert_extern` / `extern.convert_any`) over mixed identities.
//!
//! Graded: the exact per-iteration success count under every storage kind, and
//! a `GcStats` delta of exactly 0 across the storm — conversions are immediate
//! value-level re-tags, never wrapper allocations.

use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{RuntimeError, Store, Value, execute_export};

use crate::harness::{CHUNK, article_count, call, instantiate};

fn storm_chunk(
    module: &RegModule,
    store: &Store,
    count: u64,
    ext: Value,
    obj: Value,
) -> Result<i32, RuntimeError> {
    let values = execute_export(
        module,
        store,
        "storm",
        &[Value::I32(count as i32), ext, obj],
    )?;
    if let [Value::I32(value)] = values[..] {
        Ok(value)
    } else {
        panic!("storm returned {values:?}, expected one i32")
    }
}

const SUCCESSES_PER_ITERATION: u64 = 8;

const SOURCE: &str = r#"
(module
  (type $s (struct (field i32)))
  (table $tf 4 funcref)
  (table $te 4 externref)
  (table $tg 4 (ref null $s))
  (global $gf (mut funcref) (ref.null func))
  (global $ge (mut externref) (ref.null extern))
  (global $gg (mut (ref null $s)) (ref.null $s))
  (func $f)
  (elem (table $tf) (i32.const 0) func $f)

  (func (export "obj") (result (ref null $s))
    (struct.new $s (i32.const 42)))

  (func (export "storm")
        (param $n i32) (param $ext externref) (param $obj (ref null $s)) (result i32)
    (local $i i32) (local $acc i32)
    (local $i31 (ref i31)) (local $ext2 externref)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $i31 (ref.i31 (local.get $i)))
        ;; 1. i31 identity through the any <- extern round trip.
        (local.set $acc
          (i32.add (local.get $acc)
            (ref.eq
              (ref.cast (ref i31)
                (any.convert_extern (extern.convert_any (local.get $i31))))
              (local.get $i31))))
        ;; 2. i31 stays non-null through the extern round trip.
        (local.set $ext2 (extern.convert_any (local.get $i31)))
        (local.set $acc
          (i32.add (local.get $acc)
            (i32.eqz (ref.is_null (any.convert_extern (local.get $ext2))))))
        ;; 3. struct identity through the any <- extern round trip.
        (local.set $acc
          (i32.add (local.get $acc)
            (ref.eq
              (ref.cast (ref null $s)
                (any.convert_extern (extern.convert_any (local.get $obj))))
              (local.get $obj))))
        ;; 4. struct identity through a GLOBAL slot.
        (global.set $gg (local.get $obj))
        (local.set $acc
          (i32.add (local.get $acc) (ref.eq (global.get $gg) (local.get $obj))))
        ;; 5. struct identity through a TABLE slot.
        (table.set $tg (i32.const 0) (local.get $obj))
        (local.set $acc
          (i32.add (local.get $acc)
            (ref.eq (table.get $tg (i32.const 0)) (local.get $obj))))
        ;; 6. funcref through a TABLE slot (non-null round trip).
        (table.set $tf (i32.const 1) (ref.func $f))
        (local.set $acc
          (i32.add (local.get $acc)
            (i32.eqz (ref.is_null (table.get $tf (i32.const 1))))))
        ;; 7. externref through a GLOBAL slot (converted round trip).
        (global.set $ge (extern.convert_any (any.convert_extern (local.get $ext))))
        (local.set $acc
          (i32.add (local.get $acc)
            (i32.eqz (ref.is_null (global.get $ge)))))
        ;; 8. externref through a TABLE slot.
        (table.set $te (i32.const 0) (local.get $ext))
        (local.set $acc
          (i32.add (local.get $acc)
            (i32.eqz (ref.is_null (table.get $te (i32.const 0))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $acc)))
"#;

#[test]
fn a7_mixed_reference_matrix_results_exact_and_conversions_allocate_nothing() {
    let n = article_count(1_000_000);
    let (module, store) = instantiate(SOURCE);
    // One GcRef and one host externref identity, reused by the whole storm.
    let object = call(&module, &store, "obj", &[]);
    let obj = object[0];
    let ext = Value::ExternRef(Some(7));

    let snapshot = store.gc_snapshot();
    let before = store.gc_stats();
    let mut base = 0u64;
    let mut sum = 0i32;
    loop {
        let this = CHUNK.min(n - base);
        if this == 0 {
            break;
        }
        sum = sum
            .wrapping_add(storm_chunk(&module, &store, this, ext, obj).expect("conversion storm"));
        base += this;
    }
    assert_eq!(
        u64::from(sum as u32),
        SUCCESSES_PER_ITERATION * n,
        "every storage kind must round-trip identity for every iteration"
    );
    assert_eq!(
        store.gc_stats(),
        before,
        "conversions and table/global traffic must not change GcStats"
    );
    assert_eq!(
        store.gc_allocations_since(snapshot),
        0,
        "conversions are immediate re-tags: zero allocations"
    );
}
