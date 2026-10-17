// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: the FFI surface for GC values.
//!
//! `i31` is an immediate and maps directly to an `i32`-payload tag; `GcRef`,
//! `HostAny` and `ExternGc` are carried as OPAQUE `u64` tokens minted into a
//! per-instance [`HandleRegistry`]. Tokens are stable for the store's lifetime
//! (there is no release API in this slice — collector interaction is 5b), and
//! an unknown token is a clean error. No raw heap pointer ever crosses the
//! boundary.

use baedeker_core::runtime::{ExternPayload, GcHeap, GcTypeKind, Value};
use baedeker_core::types::{FieldType, NumType, StorageType, ValType};
use baedeker_ffi::value::{BaedekerValue, BaedekerValueTag, HandleError, HandleRegistry};

fn mint(value: Value) -> (BaedekerValue, HandleRegistry) {
    let mut handles = HandleRegistry::new();
    let ffi = BaedekerValue::from_core_with_handles(value, &mut handles)
        .expect("GC values are representable")
        .expect("minting does not fail");
    (ffi, handles)
}

fn gc_struct() -> Value {
    let mut heap = GcHeap::new();
    let type_id = heap
        .intern_type(GcTypeKind::Struct(vec![FieldType {
            mutable: false,
            storage: StorageType::Val(ValType::Num(NumType::I32)),
        }]))
        .unwrap();
    let id = heap
        .alloc_struct_typed(type_id, vec![Value::I32(11)])
        .unwrap();
    Value::GcRef {
        heap: heap.heap_id(),
        id,
    }
}

/// A `GcRef` mints a token and resolves back to the identical reference.
#[test]
fn gcref_token_roundtrips_to_the_same_reference() {
    let value = gc_struct();
    let (ffi, handles) = mint(value);
    assert_eq!(ffi.tag, BaedekerValueTag::GcRef as u8);
    assert_eq!(handles.len(), 1);
    let restored = ffi.to_core_with_handles(&handles).expect("token resolves");
    assert_eq!(restored, value);
    // Tokens are opaque: 0 is never valid.
    assert!(handles.resolve(0).is_none());
}

/// `i31` is direct — no token is consumed and the payload survives.
#[test]
fn i31_is_direct_and_allocates_no_token() {
    let mut handles = HandleRegistry::new();
    let ffi = BaedekerValue::from_core_with_handles(Value::I31(-7), &mut handles)
        .unwrap()
        .unwrap();
    assert_eq!(ffi.tag, BaedekerValueTag::I31 as u8);
    assert!(handles.is_empty(), "i31 must not mint a handle token");
    assert_eq!(ffi.to_core_with_handles(&handles).unwrap(), Value::I31(-7));
}

/// `HostAny` and `ExternGc` also travel as tokens, keeping their payloads.
#[test]
fn host_and_extern_gc_values_travel_as_tokens() {
    let host = Value::HostAny(Some(3));
    let external = Value::ExternGc(Some(ExternPayload::I31(9)));

    let mut handles = HandleRegistry::new();
    let host_ffi = BaedekerValue::from_core_with_handles(host, &mut handles)
        .unwrap()
        .unwrap();
    let extern_ffi = BaedekerValue::from_core_with_handles(external, &mut handles)
        .unwrap()
        .unwrap();

    assert_eq!(host_ffi.tag, BaedekerValueTag::HostAny as u8);
    assert_eq!(extern_ffi.tag, BaedekerValueTag::ExternGc as u8);
    assert_eq!(handles.len(), 2);
    assert_eq!(host_ffi.to_core_with_handles(&handles).unwrap(), host);
    assert_eq!(extern_ffi.to_core_with_handles(&handles).unwrap(), external);
}

/// An unminted token is a clean error, never a wild dereference.
#[test]
fn unknown_token_is_a_clean_error() {
    let handles = HandleRegistry::new();
    let bogus = BaedekerValue {
        tag: BaedekerValueTag::GcRef as u8,
        data: baedeker_ffi::value::BaedekerValueData { handle: 12345 },
    };
    assert_eq!(
        bogus.to_core_with_handles(&handles),
        Err(HandleError::UnknownToken(12345))
    );
    assert_eq!(
        HandleError::UnknownToken(12345).to_string(),
        "unknown GC handle token 12345"
    );
}

/// A token minted by ONE registry does not resolve in another instance's
/// registry (per-instance tokens).
#[test]
fn tokens_are_per_registry() {
    let (ffi, mine) = mint(gc_struct());
    let other = HandleRegistry::new();
    assert!(matches!(
        ffi.to_core_with_handles(&other),
        Err(HandleError::UnknownToken(_))
    ));
    assert!(ffi.to_core_with_handles(&mine).is_ok());
}

/// A token resolving to a different reference kind than the tag claims is a
/// clean kind mismatch.
#[test]
fn kind_mismatch_is_a_clean_error() {
    let (mut ffi, handles) = mint(Value::HostAny(Some(1)));
    ffi.tag = BaedekerValueTag::GcRef as u8;
    assert_eq!(
        ffi.to_core_with_handles(&handles),
        Err(HandleError::KindMismatch {
            tag: BaedekerValueTag::GcRef,
            found: BaedekerValueTag::HostAny,
        })
    );
}

/// Scalar values keep converting exactly as before.
#[test]
fn scalars_are_unchanged() {
    for value in [
        Value::I32(-1),
        Value::I64(1 << 40),
        Value::F32(1.5),
        Value::F64(-2.5),
        Value::V128([7; 16]),
    ] {
        let ffi = BaedekerValue::from_core_with_handles(value, &mut HandleRegistry::new())
            .unwrap()
            .unwrap();
        assert_eq!(
            ffi.to_core_with_handles(&HandleRegistry::new()).unwrap(),
            value
        );
    }
}
