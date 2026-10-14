// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! memory64 unit 3: index-type-driven runtime execution.
//!
//! The IR is unchanged by design: the runtime consults each memory's or
//! table's declared index type to read address operands as `i64` vs `i32`.
//! Each fixture goes through the full pipeline (WAT → encode → decode →
//! lower) and then executes against a fresh `Store`.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    RuntimeError, RuntimeErrorKind, RuntimeTrap, Store, Value, execute_export,
};

/// Parse WAT, encode it to a binary, decode, and lower it.
fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

/// Instantiate `module`, returning the store the invocations share.
fn instantiate(module: &RegModule) -> Store {
    Store::instantiate(module).unwrap()
}

/// Invoke export `name` against an existing store.
fn invoke(
    module: &RegModule,
    store: &Store,
    name: &str,
    args: &[Value],
) -> Result<Vec<Value>, RuntimeError> {
    execute_export(module, store, name, args)
}

#[test]
fn mem64_load_store_roundtrip() {
    // 1 page = 64 KiB. The second function stores through a nonzero `memarg`
    // offset at the top of the page (0xFFF0 + 8 == 0x10000): the effective
    // address is in bounds only because address + offset is computed in u64.
    let module = wat_module(
        r#"
(module
  (memory i64 1)
  (func (export "roundtrip") (result i64)
    (i64.store (i64.const 8) (i64.const 0x1122334455667788))
    (i64.load (i64.const 8)))
  (func (export "high") (result i64)
    (i64.store offset=8 (i64.const 0xFFF0) (i64.const 0x0102030405060708))
    (i64.load (i64.const 0xFFF8))))
"#,
    );
    let store = instantiate(&module);
    assert_eq!(
        invoke(&module, &store, "roundtrip", &[]),
        Ok(vec![Value::I64(0x1122334455667788u64 as i64)])
    );
    assert_eq!(
        invoke(&module, &store, "high", &[]),
        Ok(vec![Value::I64(0x0102030405060708u64 as i64)])
    );
}

#[test]
fn mem64_oob_traps() {
    let module = wat_module(
        r#"
(module
  (memory i64 1)
  (func (export "f") (result i64)
    (i64.load (i64.const 0x1_0000_0000))))
"#,
    );
    let store = instantiate(&module);
    let error = invoke(&module, &store, "f", &[]).unwrap_err();
    assert_eq!(
        error.kind,
        RuntimeErrorKind::Trap(RuntimeTrap::OutOfBoundsMemoryAccess)
    );
}

#[test]
fn mem64_size_and_grow() {
    let module = wat_module(
        r#"
(module
  (memory i64 1 3)
  (func (export "size") (result i64) memory.size)
  (func (export "grow") (param i64) (result i64)
    local.get 0
    memory.grow))
"#,
    );
    let store = instantiate(&module);
    assert_eq!(
        invoke(&module, &store, "size", &[]),
        Ok(vec![Value::I64(1)])
    );
    // Grow returns the previous size.
    assert_eq!(
        invoke(&module, &store, "grow", &[Value::I64(1)]),
        Ok(vec![Value::I64(1)])
    );
    assert_eq!(
        invoke(&module, &store, "size", &[]),
        Ok(vec![Value::I64(2)])
    );
    // Growing exactly to the declared max succeeds.
    assert_eq!(
        invoke(&module, &store, "grow", &[Value::I64(1)]),
        Ok(vec![Value::I64(2)])
    );
    // Past the declared max: -1, size unchanged.
    assert_eq!(
        invoke(&module, &store, "grow", &[Value::I64(1)]),
        Ok(vec![Value::I64(-1)])
    );
    assert_eq!(
        invoke(&module, &store, "size", &[]),
        Ok(vec![Value::I64(3)])
    );
}

#[test]
fn mem64_bulk_ops() {
    // fill: dst/count are i64, the value is i32 (low byte).
    // init: dst is i64, src/count are always i32.
    // copy: dst/src/count are all i64 for an i64 memory.
    let module = wat_module(
        r#"
(module
  (memory i64 1)
  (data $d "abcd")
  (func (export "fill") (result i32)
    (memory.fill (i64.const 0) (i32.const 0x41) (i64.const 4))
    (i32.load8_u (i64.const 0)))
  (func (export "init") (result i32)
    (memory.init $d (i64.const 10) (i32.const 0) (i32.const 4))
    (i32.load8_u (i64.const 10)))
  (func (export "copy") (result i32)
    (memory.copy (i64.const 20) (i64.const 10) (i64.const 4))
    (i32.load8_u (i64.const 20))))
"#,
    );
    let store = instantiate(&module);
    assert_eq!(
        invoke(&module, &store, "fill", &[]),
        Ok(vec![Value::I32(0x41)])
    );
    assert_eq!(
        invoke(&module, &store, "init", &[]),
        Ok(vec![Value::I32(i32::from(b'a'))])
    );
    assert_eq!(
        invoke(&module, &store, "copy", &[]),
        Ok(vec![Value::I32(i32::from(b'a'))])
    );
}

#[test]
fn table64_get_set_size_grow_fill() {
    let module = wat_module(
        r#"
(module
  (table $t i64 4 8 funcref)
  (elem func $f)
  (func $f)
  (func (export "roundtrip") (result i32)
    (table.set $t (i64.const 2) (ref.func $f))
    (ref.is_null (table.get $t (i64.const 2))))
  (func (export "size") (result i64) (table.size $t))
  (func (export "grow") (param i64) (result i64)
    (table.grow $t (ref.null func) (local.get 0)))
  (func (export "fill") (result i32)
    (table.fill $t (i64.const 0) (ref.null func) (i64.const 4))
    i32.const 1)
  (func (export "oob") (result i32)
    (table.get $t (i64.const 8))
    drop
    i32.const 0))
"#,
    );
    let store = instantiate(&module);
    // A non-null funcref survives the set/get round-trip.
    assert_eq!(
        invoke(&module, &store, "roundtrip", &[]),
        Ok(vec![Value::I32(0)])
    );
    assert_eq!(
        invoke(&module, &store, "size", &[]),
        Ok(vec![Value::I64(4)])
    );
    assert_eq!(
        invoke(&module, &store, "grow", &[Value::I64(2)]),
        Ok(vec![Value::I64(4)])
    );
    assert_eq!(
        invoke(&module, &store, "size", &[]),
        Ok(vec![Value::I64(6)])
    );
    // Past the declared max (8): -1.
    assert_eq!(
        invoke(&module, &store, "grow", &[Value::I64(3)]),
        Ok(vec![Value::I64(-1)])
    );
    assert_eq!(
        invoke(&module, &store, "fill", &[]),
        Ok(vec![Value::I32(1)])
    );
    let error = invoke(&module, &store, "oob", &[]).unwrap_err();
    assert_eq!(
        error.kind,
        RuntimeErrorKind::Trap(RuntimeTrap::OutOfBoundsTableAccess)
    );
}

#[test]
fn table64_call_indirect() {
    let module = wat_module(
        r#"
(module
  (type $t (func (result i32)))
  (table i64 2 4 funcref)
  (elem (i64.const 0) func $f)
  (func $f (type $t) (result i32) i32.const 7)
  (func (export "f") (result i32)
    (call_indirect (type $t) (i64.const 0))))
"#,
    );
    let store = instantiate(&module);
    assert_eq!(invoke(&module, &store, "f", &[]), Ok(vec![Value::I32(7)]));
}

#[test]
fn mem64_active_data_segment_i64_offset() {
    // Positive path only: the i32-offset-on-i64-memory rejection is a U2
    // validation concern (see `mem64_validation.rs`).
    let module = wat_module(
        r#"
(module
  (memory i64 1)
  (data (i64.const 4) "hi")
  (func (export "f") (result i32)
    (i32.load8_u (i64.const 4))))
"#,
    );
    let store = instantiate(&module);
    assert_eq!(
        invoke(&module, &store, "f", &[]),
        Ok(vec![Value::I32(i32::from(b'h'))])
    );
}

#[test]
fn huge_mem64_instantiation_fails_cleanly() {
    // 2^48 pages (256 TiB) is a valid limit but cannot be allocated; it must
    // surface as a RuntimeError, never abort the process.
    let module = wat_module(r#"(module (memory i64 0x1_0000_0000_0000))"#);
    let error = Store::instantiate(&module).unwrap_err();
    assert!(
        matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. }),
        "expected clean allocation failure, got {error:?}"
    );
}

#[test]
fn huge_table64_instantiation_fails_cleanly() {
    // ~2^48 entries is a valid table64 limit but not allocatable.
    let module = wat_module(r#"(module (table i64 0xffff_ffff_ffff funcref))"#);
    let error = Store::instantiate(&module).unwrap_err();
    assert!(
        matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. }),
        "expected clean allocation failure, got {error:?}"
    );
}

/// memory64 U4: index type is part of the entity type — an i64 memory
/// import never links against an i32 provider, in either direction;
/// matching index types link and share the backing storage.
#[test]
fn memory64_index_type_mismatch_is_unlinkable() {
    use baedeker_core::runtime::{Imports, LinkGroup, Store};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn wat_module(source: &str) -> Rc<baedeker_core::lower::RegModule> {
        Rc::new(crate::wat_module(source))
    }

    let provider32 = wat_module(r#"(module (memory (export "m") 1))"#);
    let provider64 = wat_module(r#"(module (memory (export "m") i64 1))"#);
    let importer32 = wat_module(
        r#"(module (import "a" "m" (memory 1)) (func (export "f") (result i32) (memory.size)))"#,
    );
    let importer64 = wat_module(
        r#"(module (import "a" "m" (memory i64 1)) (func (export "f") (result i64) (memory.size)))"#,
    );

    let group: Rc<RefCell<LinkGroup>> = Rc::new(RefCell::new(BTreeMap::new()));
    let arena = Rc::new(RefCell::new(Vec::new()));

    // 64-bit provider, 32-bit importer: unlinkable.
    let p64 = Store::instantiate_linked(&provider64, &Imports::new(), &group, 1, &arena).unwrap();
    let (ty64, shared64) = {
        let guard = p64.borrow();
        guard.export_memory("m").expect("memory export")
    };
    assert!(ty64.limits.is64);
    let imports = Imports::new().shared_memory("a", "m", ty64, shared64.clone());
    let err = Store::instantiate_linked(&importer32, &imports, &group, 2, &arena)
        .expect_err("i64 memory must not link against i32 import");
    assert!(format!("{err:?}").contains("ImportTypeMismatch"), "{err:?}");

    // 32-bit provider, 64-bit importer: unlinkable.
    let p32 = Store::instantiate_linked(&provider32, &Imports::new(), &group, 3, &arena).unwrap();
    let (ty32, shared32) = {
        let guard = p32.borrow();
        guard.export_memory("m").expect("memory export")
    };
    assert!(!ty32.limits.is64);
    let imports = Imports::new().shared_memory("a", "m", ty32, shared32.clone());
    let err = Store::instantiate_linked(&importer64, &imports, &group, 4, &arena)
        .expect_err("i32 memory must not link against i64 import");
    assert!(format!("{err:?}").contains("ImportTypeMismatch"), "{err:?}");

    // Matching index types link and share storage.
    let imports = Imports::new().shared_memory("a", "m", ty64, shared64.clone());
    let linked = Store::instantiate_linked(&importer64, &imports, &group, 5, &arena)
        .expect("matching i64 memories must link");
    let result = execute_export(&importer64, &linked.borrow(), "f", &[]).unwrap();
    assert_eq!(result, vec![baedeker_core::runtime::Value::I64(1)]);
    drop(linked);
}

/// table64: index-type mismatch is unlinkable; matching i64 tables link.
#[test]
fn table64_index_type_mismatch_is_unlinkable() {
    use baedeker_core::runtime::{Imports, LinkGroup, Store};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn wat_module(source: &str) -> Rc<baedeker_core::lower::RegModule> {
        Rc::new(crate::wat_module(source))
    }

    let provider32 = wat_module(
        r#"(module (table (export "t") 1 funcref) (func $f (export "f")) (elem (i32.const 0) $f))"#,
    );
    let provider64 = wat_module(
        r#"(module (table (export "t") i64 1 funcref) (func $f (export "f")) (elem (i64.const 0) $f))"#,
    );
    let importer32 = wat_module(
        r#"(module (import "a" "t" (table 1 funcref)) (func (export "f") (result i32) (table.size)))"#,
    );
    let importer64 = wat_module(
        r#"(module (import "a" "t" (table i64 1 funcref)) (func (export "f") (result i64) (table.size)))"#,
    );

    let group: Rc<RefCell<LinkGroup>> = Rc::new(RefCell::new(BTreeMap::new()));
    let arena = Rc::new(RefCell::new(Vec::new()));

    let p64 = Store::instantiate_linked(&provider64, &Imports::new(), &group, 1, &arena).unwrap();
    let (ty64, shared64) = {
        let guard = p64.borrow();
        guard.export_table("t").expect("table export")
    };
    assert!(ty64.limits.is64);
    let imports = Imports::new().shared_table("a", "t", ty64.clone(), shared64.clone());
    let err = Store::instantiate_linked(&importer32, &imports, &group, 2, &arena)
        .expect_err("i64 table must not link against i32 import");
    assert!(format!("{err:?}").contains("ImportTypeMismatch"), "{err:?}");

    let p32 = Store::instantiate_linked(&provider32, &Imports::new(), &group, 3, &arena).unwrap();
    let (ty32, shared32) = {
        let guard = p32.borrow();
        guard.export_table("t").expect("table export")
    };
    assert!(!ty32.limits.is64);
    let imports = Imports::new().shared_table("a", "t", ty32, shared32.clone());
    let err = Store::instantiate_linked(&importer64, &imports, &group, 4, &arena)
        .expect_err("i32 table must not link against i64 import");
    assert!(format!("{err:?}").contains("ImportTypeMismatch"), "{err:?}");

    let imports = Imports::new().shared_table("a", "t", ty64.clone(), shared64.clone());
    let linked = Store::instantiate_linked(&importer64, &imports, &group, 5, &arena)
        .expect("matching i64 tables must link");
    let result = execute_export(&importer64, &linked.borrow(), "f", &[]).unwrap();
    assert_eq!(result, vec![baedeker_core::runtime::Value::I64(1)]);
    drop(linked);
}

/// Review r1 P1-1: growing a table by millions of NON-default entries
/// must not allocate per-entry — uniform fills are ranges, and growth
/// beyond the dense limit succeeds without materializing entries.
#[test]
fn table64_huge_nondefault_grow_is_range_based() {
    let module = wat_module(
        r#"
(module
  (table i64 0 funcref)
  (func $f (result i32) (i32.const 7))
  (elem declare func $f)
  (func (export "grow") (result i64)
    (table.grow 0 (ref.func $f) (i64.const 4000000)))
  (func (export "size") (result i64) (table.size 0))
  (func (export "peek") (param i64) (result i32)
    (call_indirect 0 (type 0) (local.get 0))))"#,
    );
    let store = instantiate(&module);
    let old = invoke(&module, &store, "grow", &[]).unwrap();
    assert_eq!(old, vec![Value::I64(0)]);
    assert_eq!(
        invoke(&module, &store, "size", &[]).unwrap(),
        vec![Value::I64(4_000_000)]
    );
    // The grown entries hold the fill value.
    assert_eq!(
        invoke(&module, &store, "peek", &[Value::I64(3_999_999)]).unwrap(),
        vec![Value::I32(7)]
    );
    // And an explicit set punches a hole through the fill range.
    assert!(invoke(&module, &store, "peek", &[Value::I64(0)]).is_ok());
}

/// Review r1 P1-2: table.copy with a huge count against a short
/// destination traps BEFORE any allocation.
#[test]
fn table64_copy_huge_count_onto_empty_dst_traps() {
    let module = wat_module(
        r#"
(module
  (table $src i64 4294967295 funcref)
  (table $dst i64 0 funcref)
  (func (export "copy")
    (table.copy $dst $src (i64.const 0) (i64.const 0) (i64.const 16000000))))"#,
    );
    let store = instantiate(&module);
    let error =
        invoke(&module, &store, "copy", &[]).expect_err("copy onto empty destination must trap");
    assert!(
        matches!(error.kind, RuntimeErrorKind::Trap { .. })
            || format!("{error:?}").contains("OutOfBoundsTableAccess"),
        "expected OOB trap, got {error:?}"
    );
}

/// Same-table overlapping copy preserves source data (direction), and
/// uniform runs stay range-based after the copy.
#[test]
fn table64_overlapping_copy_preserves_source() {
    let module = wat_module(
        r#"
(module
  (type $t (func (result i32)))
  (table i64 8 funcref)
  (func $a (result i32) (i32.const 1))
  (func $b (result i32) (i32.const 2))
  (elem (i64.const 0) $a $a $a $b)
  (func (export "shift")
    (table.copy (i64.const 1) (i64.const 0) (i64.const 4)))
  (func (export "at") (param i64) (result i32)
    (call_indirect (type $t) (local.get 0))))"#,
    );
    let store = instantiate(&module);
    invoke(&module, &store, "shift", &[]).unwrap();
    // [a a a b _ _ _ _] -> copy to offset 1 -> [a a a a b _ _ _]
    for i in 0..4 {
        assert_eq!(
            invoke(&module, &store, "at", &[Value::I64(i)]).unwrap(),
            vec![Value::I32(1)],
            "index {i}"
        );
    }
    assert_eq!(
        invoke(&module, &store, "at", &[Value::I64(4)]).unwrap(),
        vec![Value::I32(2)]
    );
}

/// Review r2 P1-1: setting a value over a fill range and then setting
/// the default must land on the default — the hole rule is
/// unconditional, whether or not an explicit entry existed before.
#[test]
fn fill_range_set_then_default_lands_on_default() {
    use baedeker_core::runtime::{Table, Value};
    use baedeker_core::types::{Limits, RefType, TableType};
    let ty = TableType {
        elem: RefType::ExternRef,
        limits: Limits {
            min: 0,
            max: None,
            is64: true,
        },
        init: None,
    };
    let mut table = Table::new(ty, Value::ExternRef(None)).unwrap();
    let a = Value::ExternRef(Some(1));
    let b = Value::ExternRef(Some(2));
    // Grow into a uniform non-default fill covering [0, 10).
    assert_eq!(table.grow(10, a), Some(0));
    assert_eq!(table.get(3), Some(a));
    // Explicit non-default entry, then explicit default: must stay
    // default — not the fill value.
    assert!(table.set(3, b));
    assert_eq!(table.get(3), Some(b));
    assert!(table.set(3, Value::ExternRef(None)));
    assert_eq!(
        table.get(3),
        Some(Value::ExternRef(None)),
        "hole must stay default"
    );
    // Repeating the default assignment is idempotent.
    assert!(table.set(3, Value::ExternRef(None)));
    assert_eq!(table.get(3), Some(Value::ExternRef(None)));
    // Sibling indices keep the fill value.
    assert_eq!(table.get(2), Some(a));
    assert_eq!(table.get(4), Some(a));
}

/// Review r2 P1-3: dense-to-sparse migration of a fragmented table
/// (alternating values, all singleton runs) must not abort — the grow
/// either succeeds with the entries preserved or fails cleanly.
#[test]
fn fragmented_dense_migration_is_fallible_or_exact() {
    use baedeker_core::runtime::{Table, Value};
    use baedeker_core::types::{Limits, RefType, TableType};
    let ty = TableType {
        elem: RefType::ExternRef,
        limits: Limits {
            min: 0,
            max: None,
            is64: true,
        },
        init: None,
    };
    let mut table = Table::new(ty, Value::ExternRef(None)).unwrap();
    let a = Value::ExternRef(Some(1));
    let b = Value::ExternRef(Some(2));
    // 1M-entry dense table with alternating values: every run is a
    // singleton, the worst case for migration compression.
    assert_eq!(table.grow(1 << 20, a), Some(0));
    for idx in 0..(1u64 << 20) {
        if idx % 2 == 1 {
            assert!(table.set(idx, b));
        }
    }
    // Growing past the dense limit forces migration; with sufficient
    // memory it must succeed and preserve every other entry exactly.
    let result = table.grow(8, a);
    assert!(
        result.is_some(),
        "migration must succeed under normal memory, got None"
    );
    for idx in 0..(1u64 << 20) {
        let expected = if idx % 2 == 0 { a } else { b };
        assert_eq!(table.get(idx), Some(expected), "index {idx}");
    }
    assert_eq!(table.len(), (1 << 20) + 8);
}

/// Review r2 P1-2: importing the SAME shared table at two indices and
/// copying between the aliases must not double-borrow the RefCell — it
/// takes the same-table path and behaves like a same-table copy.
#[test]
fn table_copy_between_aliased_imports_is_same_table() {
    use baedeker_core::runtime::{Imports, LinkGroup, Store};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn wat_module(source: &str) -> Rc<baedeker_core::lower::RegModule> {
        Rc::new(crate::wat_module(source))
    }

    let provider = wat_module(
        r#"(module
  (type $t (func (result i32)))
  (table (export "tbl") i64 4 funcref)
  (func $f (result i32) (i32.const 9))
  (elem (i64.const 0) $f))"#,
    );
    let importer = wat_module(
        r#"(module
  (type $t (func (result i32)))
  (import "m" "tbl" (table $a i64 4 funcref))
  (import "m" "tbl" (table $b i64 4 funcref))
  (func (export "shift") (table.copy $b $a (i64.const 1) (i64.const 0) (i64.const 2)))
  (func (export "at") (param i64) (result i32)
    (call_indirect $b (type $t) (local.get 0))))"#,
    );

    let group: Rc<RefCell<LinkGroup>> = Rc::new(RefCell::new(BTreeMap::new()));
    let arena = Rc::new(RefCell::new(Vec::new()));
    let provider_store = Store::instantiate_linked(&provider, &Imports::new(), &group, 1, &arena)
        .expect("provider instantiates");
    let (ty, shared) = {
        let guard = provider_store.borrow();
        guard.export_table("tbl").expect("table export")
    };
    let imports = Imports::new().shared_table("m", "tbl", ty, shared);
    let importer_store = Store::instantiate_linked(&importer, &imports, &group, 2, &arena)
        .expect("importer instantiates");

    // Copy [0,2) of the alias to offset 1 — must succeed, not panic.
    execute_export(&importer, &importer_store.borrow(), "shift", &[])
        .expect("aliased copy executes");
    // at(1) now holds the entry from index 0.
    let result =
        execute_export(&importer, &importer_store.borrow(), "at", &[Value::I64(1)]).unwrap();
    assert_eq!(result, vec![Value::I32(9)]);
}
