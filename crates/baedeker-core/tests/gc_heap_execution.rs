// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 2: struct/array/i31 execution semantics.
//!
//! Full pipeline: WAT → encode → decode → lower → execute against a fresh
//! `Store`. Trap identities match `exec/eval.ml` at the pinned revision.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    RuntimeError, RuntimeErrorKind, RuntimeTrap, Store, Value, execute_export,
};

fn wat_module(source: &str) -> RegModule {
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();
    module.lower().unwrap()
}

fn run(source: &str) -> Result<Vec<Value>, RuntimeError> {
    let module = wat_module(source);
    let store = Store::instantiate(&module).unwrap();
    execute_export(&module, &store, "f", &[])
}

fn run_err(source: &str) -> RuntimeError {
    run(source).expect_err("expected a runtime error")
}

fn trap_of(error: RuntimeError) -> RuntimeTrap {
    match error.kind {
        RuntimeErrorKind::Trap(trap) => trap,
        other => panic!("expected a trap, got {other:?}"),
    }
}

#[test]
fn struct_field_readback_i32_i64() {
    let source = r#"(module
  (type $s (struct (field i32) (field i64)))
  (func (export "f") (result i64)
    i32.const 11 i64.const 22 struct.new $s
    struct.get $s 1))"#;
    assert_eq!(run(source), Ok(vec![Value::I64(22)]));
}

#[test]
fn struct_packed_i8_zero_extends_after_wrap() {
    let source = r#"(module
  (type $s (struct (field (mut i8))))
  (func (export "f") (result i32)
    i32.const 0x1FF struct.new $s
    struct.get_u $s 0))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(0xFF)]));
}

#[test]
fn struct_packed_i8_get_sign_extends() {
    let source = r#"(module
  (type $s (struct (field (mut i8))))
  (func (export "f") (result i32)
    i32.const 0xFF struct.new $s
    struct.get_s $s 0))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(-1)]));
}

#[test]
fn struct_packed_i16_roundtrip() {
    let source = r#"(module
  (type $s (struct (field (mut i16))))
  (func (export "f") (result i32)
    i32.const 0x18000 struct.new $s
    struct.get_u $s 0))"#;
    // Wrapped to 16 bits: 0x8000.
    assert_eq!(run(source), Ok(vec![Value::I32(0x8000)]));
}

#[test]
fn struct_set_writes_field() {
    let source = r#"(module
  (type $s (struct (field i32) (field (mut i64))))
  (func (export "f") (result i64)
    (local $r (ref null $s))
    i32.const 1 i64.const 2 struct.new $s
    local.set $r
    local.get $r i64.const 99 struct.set $s 1
    local.get $r struct.get $s 1))"#;
    assert_eq!(run(source), Ok(vec![Value::I64(99)]));
}

#[test]
fn struct_new_default_zeroes_fields() {
    let source = r#"(module
  (type $s (struct (field i32) (field (mut i64)) (field (mut i8)) (field v128)))
  (func (export "f") (result i64)
    struct.new_default $s
    struct.get $s 1))"#;
    assert_eq!(run(source), Ok(vec![Value::I64(0)]));
}

#[test]
fn array_new_and_get() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    i32.const 42 i32.const 3 array.new $a
    i32.const 2 array.get $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(42)]));
}

#[test]
fn array_new_default_zeroes() {
    let source = r#"(module
  (type $a (array (mut i64)))
  (func (export "f") (result i64)
    i32.const 3 array.new_default $a
    i32.const 1 array.get $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I64(0)]));
}

#[test]
fn array_new_fixed_values_in_order() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    i32.const 10 i32.const 20 i32.const 30 array.new_fixed $a 3
    i32.const 1 array.get $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(20)]));
}

#[test]
fn array_len_returns_length() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    i32.const 0 i32.const 5 array.new $a
    array.len))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(5)]));
}

#[test]
fn array_set_writes_element() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    (local $r (ref null $a))
    i32.const 0 i32.const 2 array.new $a
    local.set $r
    local.get $r i32.const 0 i32.const 77 array.set $a
    local.get $r i32.const 0 array.get $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(77)]));
}

#[test]
fn array_fill_sets_range() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    (local $r (ref null $a))
    i32.const 0 i32.const 4 array.new $a
    local.set $r
    local.get $r i32.const 0 i32.const 9 i32.const 4 array.fill $a
    local.get $r i32.const 3 array.get $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(9)]));
}

#[test]
fn array_copy_forward_overlap() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    (local $r (ref null $a))
    i32.const 0 i32.const 4 array.new $a
    local.set $r
    local.get $r i32.const 0 i32.const 1 array.set $a
    local.get $r i32.const 1 i32.const 2 array.set $a
    local.get $r i32.const 2 i32.const 3 array.set $a
    local.get $r i32.const 3 i32.const 4 array.set $a
    local.get $r i32.const 1 local.get $r i32.const 0 i32.const 3
    array.copy $a $a
    local.get $r i32.const 3 array.get $a))"#;
    // [1,2,3,4] copy src 0..3 to dst 1..4 forward => [1,1,2,3]
    assert_eq!(run(source), Ok(vec![Value::I32(3)]));
}

#[test]
fn array_copy_backward_overlap() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    (local $r (ref null $a))
    i32.const 0 i32.const 4 array.new $a
    local.set $r
    local.get $r i32.const 0 i32.const 1 array.set $a
    local.get $r i32.const 1 i32.const 2 array.set $a
    local.get $r i32.const 2 i32.const 3 array.set $a
    local.get $r i32.const 3 i32.const 4 array.set $a
    local.get $r i32.const 0 local.get $r i32.const 1 i32.const 3
    array.copy $a $a
    local.get $r i32.const 0 array.get $a))"#;
    // [1,2,3,4] copy src 1..4 to dst 0..3 backward => [2,3,4,4]
    assert_eq!(run(source), Ok(vec![Value::I32(2)]));
}

#[test]
fn array_new_data_reads_bytes() {
    let source = r#"(module
  (type $a (array (mut i8)))
  (data $d "\01\02\03")
  (func (export "f") (result i32)
    i32.const 0 i32.const 3 array.new_data $a $d
    i32.const 2 array.get_u $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(3)]));
}

#[test]
fn array_init_data_writes_range() {
    let source = r#"(module
  (type $a (array (mut i8)))
  (data $d "\05\06\07")
  (func (export "f") (result i32)
    (local $r (ref null $a))
    i32.const 0 i32.const 4 array.new $a
    local.set $r
    local.get $r i32.const 1 i32.const 0 i32.const 3 array.init_data $a $d
    local.get $r i32.const 3 array.get_u $a))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(7)]));
}

#[test]
fn array_new_elem_reads_segment() {
    let source = r#"(module
  (type $a (array (mut funcref)))
  (elem $e func $g)
  (func $g (result i32) i32.const 1)
  (func (export "f") (result i32)
    i32.const 0 i32.const 1 array.new_elem $a $e
    i32.const 0 array.get $a
    ref.is_null))"#;
    assert_eq!(run(source), Ok(vec![Value::I32(0)]));
}

#[test]
fn ref_i31_sign_and_zero() {
    let source = r#"(module
  (func (export "f") (result i32)
    i32.const -1 ref.i31 i31.get_u
    i32.const -1 ref.i31 i31.get_s
    i32.add))"#;
    // get_u of -1 (masked 0x7FFFFFFF) = 0x7FFFFFFF; get_s = -1.
    assert_eq!(run(source), Ok(vec![Value::I32(0x7FFF_FFFE)]));
}

#[test]
fn i31_get_null_traps() {
    let source = r#"(module
  (func (export "f") (result i32)
    ref.null i31 i31.get_s))"#;
    assert_eq!(trap_of(run_err(source)), RuntimeTrap::NullI31Reference);
}

#[test]
fn struct_get_null_traps_before_access() {
    let source = r#"(module
  (type $s (struct (field i32)))
  (func (export "f") (result i32)
    ref.null $s struct.get $s 0))"#;
    assert_eq!(trap_of(run_err(source)), RuntimeTrap::NullStructReference);
}

#[test]
fn array_get_null_traps() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    ref.null $a i32.const 0 array.get $a))"#;
    assert_eq!(trap_of(run_err(source)), RuntimeTrap::NullArrayReference);
}

#[test]
fn array_len_null_traps() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    ref.null $a array.len))"#;
    assert_eq!(trap_of(run_err(source)), RuntimeTrap::NullArrayReference);
}

#[test]
fn array_get_out_of_bounds_traps() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f") (result i32)
    i32.const 0 i32.const 2 array.new $a
    i32.const 5 array.get $a))"#;
    assert_eq!(
        trap_of(run_err(source)),
        RuntimeTrap::OutOfBoundsArrayAccess
    );
}

#[test]
fn array_init_data_array_bounds_before_segment_bounds() {
    // A zero-length array with a non-empty data segment and a huge range:
    // the ARRAY bounds check must fire first.
    let source = r#"(module
  (type $a (array (mut i8)))
  (data $d "\01\02\03\04\05\06")
  (func (export "f")
    i32.const 0 i32.const 0 array.new $a
    i32.const 0 i32.const 0 i32.const 5 array.init_data $a $d))"#;
    assert_eq!(
        trap_of(run_err(source)),
        RuntimeTrap::OutOfBoundsArrayAccess
    );
}

#[test]
fn array_init_data_segment_bounds_after_array() {
    // The array is long enough but the data segment is too short: the
    // segment bounds trap is the MEMORY trap, not the array trap.
    let source = r#"(module
  (type $a (array (mut i8)))
  (data $d "\01\02")
  (func (export "f")
    i32.const 0 i32.const 8 array.new $a
    i32.const 0 i32.const 0 i32.const 5 array.init_data $a $d))"#;
    assert_eq!(
        trap_of(run_err(source)),
        RuntimeTrap::OutOfBoundsMemoryAccess
    );
}

#[test]
fn array_copy_null_source_checked_first() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f")
    ref.null $a i32.const 0
    ref.null $a i32.const 0 i32.const 1
    array.copy $a $a))"#;
    assert_eq!(trap_of(run_err(source)), RuntimeTrap::NullArrayReference);
}

#[test]
fn array_fill_zero_length_on_null_still_traps() {
    let source = r#"(module
  (type $a (array (mut i32)))
  (func (export "f")
    ref.null $a i32.const 0 i32.const 0 i32.const 0 array.fill $a))"#;
    assert_eq!(trap_of(run_err(source)), RuntimeTrap::NullArrayReference);
}
