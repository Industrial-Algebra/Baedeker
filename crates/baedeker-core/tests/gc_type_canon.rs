// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 1: type canonicalization.
//!
//! Canonical equivalence must agree with the reference's structural equality
//! on rolled closed forms (`syntax/types.ml` `roll_deftype`). The property
//! sweep compares the production canonicalizer against an independent
//! closed-form reference built directly from the spec's roll/substitution.

use std::collections::{BTreeMap, BTreeSet};

use baedeker_core::binary::module::Module;
use baedeker_core::types::{
    CompType, HeapType, NumType, RefType, StorageType, SubType, TypeIdx, ValType, VecType,
};
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastDirective};

fn wat_bytes(mut wat: QuoteWat<'_>) -> Vec<u8> {
    wat.encode()
        .unwrap_or_else(|error| panic!("failed to encode wat: {error}"))
}

fn decode(text: &str) -> Module<'static> {
    // Leak the encoded bytes so the returned module can borrow them; tests are
    // short-lived and the leak is bounded by the corpus size.
    let buf = ParseBuffer::new(text).unwrap_or_else(|error| panic!("bad buffer: {error}"));
    let mut wat = parse::<wast::Wat<'_>>(&buf).unwrap_or_else(|error| panic!("bad wat: {error}"));
    let bytes = wat
        .encode()
        .unwrap_or_else(|error| panic!("bad encode: {error}"));
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let module = Module::decode(leaked).unwrap_or_else(|error| panic!("bad decode: {error}"));
    module
        .validate()
        .unwrap_or_else(|error| panic!("bad module: {error}"));
    module
}

fn equivalent(module: &Module<'_>, lhs: TypeIdx, rhs: TypeIdx) -> bool {
    module.type_canon.equivalent(lhs, rhs)
}

// ---------------------------------------------------------------------------
// Independent closed-form reference.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefHeap {
    Abstract(HeapType),
    Rec(u32),
    Def(Box<RefDeftype>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefVal {
    Num(NumType),
    Vec(VecType),
    Ref(bool, RefHeap),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefStorage {
    Val(RefVal),
    I8,
    I16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefField {
    mutable: bool,
    storage: RefStorage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefComp {
    Struct(Vec<RefField>),
    Array(RefField),
    Func(Vec<RefVal>, Vec<RefVal>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefMember {
    is_final: bool,
    supers: Vec<RefDeftype>,
    comp: RefComp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefDeftype {
    group: Vec<RefMember>,
    index: u32,
}

fn group_spans(module: &Module<'_>) -> BTreeMap<u32, u32> {
    let mut by_start: BTreeMap<u32, u32> = module
        .rec_groups
        .iter()
        .filter(|&&(_, len)| len > 0)
        .copied()
        .collect();
    let mut spans = BTreeMap::new();
    let mut start = 0u32;
    while (start as usize) < module.types().len() {
        let len = by_start.remove(&start).unwrap_or(1);
        spans.insert(start, len);
        start += len;
    }
    spans
}

fn span_of(spans: &BTreeMap<u32, u32>, idx: u32) -> (u32, u32) {
    for (&start, &len) in spans.iter().rev() {
        if idx >= start && idx < start + len {
            return (start, len);
        }
    }
    panic!("type index {idx} is not in any group")
}

fn closed_ref_val(
    module: &Module<'_>,
    spans: &BTreeMap<u32, u32>,
    val: ValType,
    start: u32,
    len: u32,
) -> RefVal {
    match val {
        ValType::Num(num) => RefVal::Num(num),
        ValType::Vec(vec) => RefVal::Vec(vec),
        ValType::Ref(RefType::FuncRef) => RefVal::Ref(true, RefHeap::Abstract(HeapType::Func)),
        ValType::Ref(RefType::ExternRef) => RefVal::Ref(true, RefHeap::Abstract(HeapType::Extern)),
        ValType::Ref(RefType::Typed { nullable, heap }) => {
            RefVal::Ref(nullable, closed_ref_heap(module, spans, heap, start, len))
        }
    }
}

fn closed_ref_heap(
    module: &Module<'_>,
    spans: &BTreeMap<u32, u32>,
    heap: HeapType,
    start: u32,
    len: u32,
) -> RefHeap {
    match heap {
        HeapType::Type(idx) if idx.0 >= start && idx.0 < start + len => RefHeap::Rec(idx.0 - start),
        HeapType::Type(idx) => RefHeap::Def(Box::new(closed_deftype(module, spans, idx.0))),
        other => RefHeap::Abstract(other),
    }
}

fn closed_ref_storage(
    module: &Module<'_>,
    spans: &BTreeMap<u32, u32>,
    storage: StorageType,
    start: u32,
    len: u32,
) -> RefStorage {
    match storage {
        StorageType::Val(val) => RefStorage::Val(closed_ref_val(module, spans, val, start, len)),
        StorageType::I8 => RefStorage::I8,
        StorageType::I16 => RefStorage::I16,
    }
}

fn closed_ref_field(
    module: &Module<'_>,
    spans: &BTreeMap<u32, u32>,
    field: crate::RefFieldSource,
    start: u32,
    len: u32,
) -> RefField {
    RefField {
        mutable: field.mutable,
        storage: closed_ref_storage(module, spans, field.storage, start, len),
    }
}

struct RefFieldSource {
    mutable: bool,
    storage: StorageType,
}

fn closed_ref_comp(
    module: &Module<'_>,
    spans: &BTreeMap<u32, u32>,
    comp: &CompType,
    start: u32,
    len: u32,
) -> RefComp {
    match comp {
        CompType::Struct(fields) => RefComp::Struct(
            fields
                .iter()
                .map(|field| {
                    closed_ref_field(
                        module,
                        spans,
                        RefFieldSource {
                            mutable: field.mutable,
                            storage: field.storage,
                        },
                        start,
                        len,
                    )
                })
                .collect(),
        ),
        CompType::Array(field) => RefComp::Array(closed_ref_field(
            module,
            spans,
            RefFieldSource {
                mutable: field.mutable,
                storage: field.storage,
            },
            start,
            len,
        )),
        CompType::Func(func) => RefComp::Func(
            func.params
                .iter()
                .map(|val| closed_ref_val(module, spans, *val, start, len))
                .collect(),
            func.results
                .iter()
                .map(|val| closed_ref_val(module, spans, *val, start, len))
                .collect(),
        ),
    }
}

fn closed_ref_member(
    module: &Module<'_>,
    spans: &BTreeMap<u32, u32>,
    idx: u32,
    start: u32,
    len: u32,
) -> RefMember {
    let sub: &SubType = &module.types()[idx as usize];
    RefMember {
        is_final: sub.is_final,
        supers: sub
            .supertypes
            .iter()
            .map(|&s| closed_deftype(module, spans, s))
            .collect(),
        comp: closed_ref_comp(module, spans, &sub.comp, start, len),
    }
}

fn closed_deftype(module: &Module<'_>, spans: &BTreeMap<u32, u32>, idx: u32) -> RefDeftype {
    let (start, len) = span_of(spans, idx);
    let group = (start..start + len)
        .map(|k| closed_ref_member(module, spans, k, start, len))
        .collect();
    RefDeftype {
        group,
        index: idx - start,
    }
}

fn reference_equal(module: &Module<'_>, spans: &BTreeMap<u32, u32>, lhs: u32, rhs: u32) -> bool {
    closed_deftype(module, spans, lhs) == closed_deftype(module, spans, rhs)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn same_group_declared_twice_has_same_canon() {
    let module = decode(
        "(module
           (rec (type $a (func (param i32 (ref $b)))) (type $b (func (result (ref $a)))))
           (rec (type $c (func (param i32 (ref $d)))) (type $d (func (result (ref $c))))))",
    );
    assert!(equivalent(&module, TypeIdx(0), TypeIdx(2)));
    assert!(equivalent(&module, TypeIdx(1), TypeIdx(3)));
    assert!(!equivalent(&module, TypeIdx(0), TypeIdx(1)));
}

#[test]
fn bare_type_equals_singleton_rec() {
    let module = decode("(module (type (func (param i32))) (rec (type (func (param i32)))))");
    assert!(equivalent(&module, TypeIdx(0), TypeIdx(1)));
}

#[test]
fn in_group_and_external_refs_are_distinct() {
    let module = decode(
        "(module
           (rec (type $f (func (param (ref $f)))))
           (rec (type $g (func (param (ref $f))))))",
    );
    assert!(!equivalent(&module, TypeIdx(0), TypeIdx(1)));
}

#[test]
fn cross_group_external_reference_is_inlined() {
    let module = decode(
        "(module
           (type $s (struct))
           (rec (type $a (func (param (ref $s)))))
           (rec (type $b (func (param (ref $s))))))",
    );
    assert!(equivalent(&module, TypeIdx(1), TypeIdx(2)));
}

#[test]
fn permuted_groups_are_distinct() {
    let module = decode(
        "(module
           (rec (type $a (func (param (ref $b)))) (type $b (func (result (ref $a)))))
           (rec (type $c (func (result (ref $d)))) (type $d (func (param (ref $c))))))",
    );
    assert!(!equivalent(&module, TypeIdx(0), TypeIdx(2)));
    assert!(!equivalent(&module, TypeIdx(1), TypeIdx(3)));
}

#[test]
fn identical_recursive_groups_are_equal() {
    let module = decode(
        "(module
           (rec (type $t1 (func (param i32 (ref $t2)))) (type $t2 (func (result (ref $t1)))))
           (rec (type $u1 (func (param i32 (ref $u2)))) (type $u2 (func (result (ref $u1))))))",
    );
    assert!(equivalent(&module, TypeIdx(0), TypeIdx(2)));
    assert!(equivalent(&module, TypeIdx(1), TypeIdx(3)));
    assert!(!equivalent(&module, TypeIdx(0), TypeIdx(1)));
}

#[test]
fn bare_self_recursive_equals_rec_singleton() {
    let module = decode(
        "(module
           (type $t (func (param (ref 0))))
           (rec (type $u (func (param (ref $u))))))",
    );
    assert!(equivalent(&module, TypeIdx(0), TypeIdx(1)));
}

#[test]
fn empty_rec_group_adds_no_types() {
    let module = decode("(module (rec))");
    assert!(module.types().is_empty());
    assert!(module.type_canon.canonical_id(TypeIdx(0)).is_none());
}

#[test]
fn bare_type_after_empty_rec_group_stays_distinct() {
    // An empty rec group consumes no index; the following bare type must be
    // its own singleton group, not swallowed by the empty entry.
    let module = decode("(module (type (struct)) (rec) (type (func)))");
    assert_eq!(module.types().len(), 2);
    assert!(!equivalent(&module, TypeIdx(0), TypeIdx(1)));
    assert!(equivalent(&module, TypeIdx(1), TypeIdx(1)));
}

fn spec_files() -> Vec<std::path::PathBuf> {
    baedeker_testdata::spec_wast_cases("gc-validation")
}

#[test]
fn canon_agrees_with_reference_closed_forms() {
    for path in spec_files() {
        let text = baedeker_testdata::spec_case_text(&path);
        let buf = ParseBuffer::new(&text).unwrap();
        let wast = parse::<Wast<'_>>(&buf).unwrap();
        for directive in wast.directives {
            let (WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat)) = directive
            else {
                continue;
            };
            let bytes = wat_bytes(wat);
            let module = Module::decode(&bytes)
                .unwrap_or_else(|error| panic!("{}: decode failed: {error}", path.display()));
            module
                .validate()
                .unwrap_or_else(|error| panic!("{}: validation failed: {error}", path.display()));
            let spans = group_spans(&module);
            let n = module.types().len();
            for i in 0..n {
                for j in 0..n {
                    let canon = equivalent(&module, TypeIdx(i as u32), TypeIdx(j as u32));
                    let reference = reference_equal(&module, &spans, i as u32, j as u32);
                    assert_eq!(
                        canon,
                        reference,
                        "{}: canon mismatch for types {i} and {j}",
                        path.display()
                    );
                }
            }
            // The canonical representative is the lowest index in its class.
            for i in 0..n {
                let canonical = module
                    .type_canon
                    .canonical_id(TypeIdx(i as u32))
                    .expect("in-range type has a canonical id");
                assert!(canonical as usize <= i);
                assert_eq!(
                    module.type_canon.canonical_id(TypeIdx(canonical)),
                    Some(canonical)
                );
            }
        }
    }
}

#[test]
fn type_canon_file_class_counts() {
    // The two type-canon modules have hand-derived class counts of 3 and 5:
    // every member of each rec group has a distinct rolled form.
    let module = decode(
        "(module
           (rec
             (type $t1 (func (param i32 (ref $t3))))
             (type $t2 (func (param i32 (ref $t1))))
             (type $t3 (func (param i32 (ref $t2))))))",
    );
    assert_eq!(classes(&module), 3);

    let module = decode(
        "(module
           (rec
             (type $t0 (func (param i32 (ref $t2) (ref $t3))))
             (type $t1 (func (param i32 (ref $t0) i32 (ref $t4))))
             (type $t2 (func (param i32 (ref $t2) (ref $t1))))
             (type $t3 (func (param i32 (ref $t2) i32 (ref $t4))))
             (type $t4 (func (param (ref $t0) (ref $t2))))))",
    );
    assert_eq!(classes(&module), 5);
}

fn classes(module: &Module<'_>) -> usize {
    let mut ids = BTreeSet::new();
    for i in 0..module.types().len() {
        ids.insert(
            module
                .type_canon
                .canonical_id(TypeIdx(i as u32))
                .expect("in-range type has a canonical id"),
        );
    }
    ids.len()
}
