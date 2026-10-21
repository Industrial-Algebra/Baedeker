// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! A2a — persistent build (accounting): a 10⁶-node list plus a binary tree,
//! all live, in a provisioned runner (no caps).
//!
//! Graded: `live_bytes` exactly equals the logical formula times the actual
//! count. The formula (unit-2 contract, design doc "GcStats"):
//!
//! ```text
//! struct bytes = 8-byte header + Σ field widths
//! node  $node  = 8 + i32(4) + ref(4)                 = 16
//! node  $tnode = 8 + i32(4) + ref(4) + ref(4)        = 20
//! ```

use baedeker_core::runtime::Value;

use crate::harness::{article_count, call, call_i32, instantiate};

const SOURCE: &str = r#"
(module
  (type $node (struct (field i32) (field (ref null $node))))
  (type $tnode
    (struct (field i32) (field (ref null $tnode)) (field (ref null $tnode))))

  (func (export "list") (param $n i32) (result i32)
    (local $i i32) (local $head (ref null $node))
    (block $done
      (loop $loop
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $head (struct.new $node (local.get $i) (local.get $head)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (drop (local.get $head))
    (local.get $i))

  (func $tree (param $depth i32) (param $v i32) (result (ref null $tnode))
    (if (result (ref null $tnode)) (i32.eqz (local.get $depth))
      (then (ref.null $tnode))
      (else
        (struct.new $tnode
          (local.get $v)
          (call $tree (i32.sub (local.get $depth) (i32.const 1)) (local.get $v))
          (call $tree
            (i32.sub (local.get $depth) (i32.const 1))
            (i32.add (local.get $v) (i32.const 1)))))))

  (func (export "tree") (param $depth i32) (result i32)
    (drop (call $tree (local.get $depth) (i32.const 0)))
    (i32.const 0)))
"#;

const LIST_NODE_BYTES: u64 = 8 + 4 + 4;
const TREE_NODE_BYTES: u64 = 8 + 4 + 4 + 4;
const TREE_DEPTH: u32 = 10;

#[test]
fn a2a_persistent_build_live_bytes_is_the_exact_formula() {
    let list_nodes = article_count(1_000_000);
    let tree_nodes = (1u64 << TREE_DEPTH) - 1;
    let (module, store) = instantiate(SOURCE);

    let snapshot = store.gc_snapshot();
    assert_eq!(
        call_i32(&module, &store, "list", &[Value::I32(list_nodes as i32)]) as u64,
        list_nodes
    );
    assert_eq!(
        call(&module, &store, "tree", &[Value::I32(TREE_DEPTH as i32)]),
        [Value::I32(0)]
    );

    let stats = store.gc_stats();
    let expected_objects = list_nodes + tree_nodes;
    let expected_bytes = LIST_NODE_BYTES * list_nodes + TREE_NODE_BYTES * tree_nodes;
    assert_eq!(
        store.gc_allocations_since(snapshot),
        expected_objects,
        "one object per list node plus one per tree node"
    );
    assert_eq!(
        stats.live_objects as u64, expected_objects,
        "every node stays live (never-free)"
    );
    assert_eq!(
        stats.live_bytes, expected_bytes,
        "live_bytes must equal the logical formula, not reserved capacity"
    );
}
