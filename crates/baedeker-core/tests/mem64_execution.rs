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
