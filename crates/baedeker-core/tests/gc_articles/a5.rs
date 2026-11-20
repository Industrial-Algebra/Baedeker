// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A5 — cast cascade: 10⁶ `ref.test`/`ref.cast` operations over a 30-deep
//! declared subtype chain, with mixed outcomes **including distinct-ID subtype
//! successes**.
//!
//! Three parallel chains are generated: `$a0..$a29` (each level declares the
//! previous as its supertype and adds one `i32` field), `$b0..$b29` (identical
//! shapes in separate type ENTRIES — canonical closed-form identity, so a cast
//! from the `$a` object to a `$b` target must succeed), and `$c0..$c29`
//! (different field storage, so the cast must fail). Graded: exact per-probe
//! results and a `GcStats` delta of exactly 0 (the ancestry chains are
//! precomputed ids; deciding a cast allocates nothing).

use baedeker_core::runtime::{Store, Value};

use crate::harness::{article_count, call, call_i32, wat_module};

const DEPTH: usize = 30;

fn chain_source() -> String {
    let mut source = String::from("(module\n");
    // Field lists grow by one i32 per level, so the deepest object is the most
    // specific subtype and every shallower level is an ancestor.
    for level in 0..DEPTH {
        let fields: String = (0..=level).map(|_| "(field i32)").collect();
        let supertype = if level == 0 {
            String::new()
        } else {
            format!(" $a{}", level - 1)
        };
        source.push_str(&format!(
            "  (type $a{level} (sub{supertype} (struct {fields})))\n"
        ));
    }
    for level in 0..DEPTH {
        let fields: String = (0..=level).map(|_| "(field i32)").collect();
        let supertype = if level == 0 {
            String::new()
        } else {
            format!(" $b{}", level - 1)
        };
        source.push_str(&format!(
            "  (type $b{level} (sub{supertype} (struct {fields})))\n"
        ));
    }
    for level in 0..DEPTH {
        let fields: String = (0..=level).map(|_| "(field i64)").collect();
        let supertype = if level == 0 {
            String::new()
        } else {
            format!(" $c{}", level - 1)
        };
        source.push_str(&format!(
            "  (type $c{level} (sub{supertype} (struct {fields})))\n"
        ));
    }

    let deepest = DEPTH - 1;
    source.push_str(&format!(
        "  (func (export \"build\") (result (ref null $a{deepest}))\n    {}struct.new $a{deepest})\n",
        "(i32.const 7) ".repeat(DEPTH)
    ));

    for level in 0..DEPTH {
        source.push_str(&format!(
            "(func (export \"probe{level}\") (param $n i32) (param $obj (ref null $a{deepest})) \
             (result i32)\n    (local $i i32) (local $acc i32)\n    (block $done\n      \
             (loop $loop\n        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))\n        \
             ;; declared-supertype hit\n        \
             (local.set $acc (i32.add (local.get $acc) \
             (ref.test (ref $a{level}) (local.get $obj))))\n        \
             ;; distinct-ID success: identical structure, separate type entry\n        \
             (local.set $acc (i32.add (local.get $acc) \
             (ref.test (ref $b{level}) (local.get $obj))))\n        \
             ;; negative: different field storage\n        \
             (local.set $acc (i32.add (local.get $acc) \
             (ref.test (ref $c{level}) (local.get $obj))))\n        \
             ;; the cast must succeed and keep the reference non-null\n        \
             (local.set $acc (i32.add (local.get $acc)\n          \
             (i32.eqz (ref.is_null (ref.cast (ref $a{level}) (local.get $obj))))))\n        \
             (local.set $i (i32.add (local.get $i) (i32.const 1)))\n        \
             (br $loop)))\n    (local.get $acc))\n"
        ));
    }
    source.push_str(")\n");
    source
}

#[test]
fn a5_cast_cascade_results_are_exact_and_allocate_nothing() {
    let total_probes = article_count(1_000_000);
    let per_probe = (total_probes / DEPTH as u64).max(1);
    let module = wat_module(&chain_source());
    let store = Store::instantiate(&module).expect("instantiate generated cascade");

    let object = call(&module, &store, "build", &[]);
    assert_eq!(object.len(), 1);

    let snapshot = store.gc_snapshot();
    let before = store.gc_stats();
    for level in 0..DEPTH {
        let result = call_i32(
            &module,
            &store,
            &format!("probe{level}"),
            &[Value::I32(per_probe as i32), object[0]],
        );
        // 3 successes per iteration: the declared-supertype hit, the
        // distinct-ID equivalent hit, and the successful cast; the `$c` probe
        // must contribute 0.
        assert_eq!(
            result as u64,
            3 * per_probe,
            "probe{level} must see the declared-supertype hit, the distinct-ID \
             equivalent hit, no $c hit, and a successful cast"
        );
    }
    assert_eq!(
        store.gc_stats(),
        before,
        "cast decisions must not change GcStats"
    );
    assert_eq!(
        store.gc_allocations_since(snapshot),
        0,
        "cast decisions must not allocate heap objects"
    );
}
