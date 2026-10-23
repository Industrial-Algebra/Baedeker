// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Threads U2: interpreter execution of the 0xFE atomic family.
//!
//! Contract: `docs/plans/threads-unit2-contract.md`. Semantics pinned from
//! `WebAssembly/threads@main-legacy` `33b2dcd6`
//! (`interpreter/exec/eval.ml:271-360`, `interpreter/runtime/memory.ml:23-28`)
//! plus the rule-7 resolution: alignment is checked against the
//! **effective** address (`base + memarg.offset`), not the base address the
//! pin's interpreter happens to pass (`eval.ml:275`).
//!
//! Modules are hand-built bytes: the `wast` 240 text grammar can express the
//! atomic instructions but not classic `(memory … shared)`, and every shared
//! memory here is a real shared memory. Byte tables are cross-checked against
//! the pin's `binary/instructions.rst` and wast-240 `core/expr.rs:786-855`.

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    RuntimeError, RuntimeErrorKind, RuntimeTrap, Store, Value, execute_export,
};
use baedeker_core::validate::validate_module;

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

fn uleb(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn sleb(mut v: i64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let sign = byte & 0x40 != 0;
        if (v == 0 && !sign) || (v == -1 && sign) {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn section(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![id];
    uleb(payload.len() as u64, &mut s);
    s.extend_from_slice(payload);
    s
}

const I32: u8 = 0x7f;
const I64: u8 = 0x7e;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    I32,
    I64,
}

impl Ty {
    fn val_type(self) -> u8 {
        match self {
            Ty::I32 => I32,
            Ty::I64 => I64,
        }
    }
}

fn i32_const(v: i32, out: &mut Vec<u8>) {
    out.push(0x41);
    sleb(v as i64, out);
}

fn i64_const(v: i64, out: &mut Vec<u8>) {
    out.push(0x42);
    sleb(v, out);
}

fn const_of(ty: Ty, v: i64, out: &mut Vec<u8>) {
    match ty {
        Ty::I32 => i32_const(v as i32, out),
        Ty::I64 => i64_const(v, out),
    }
}

fn local_get(idx: u32, out: &mut Vec<u8>) {
    out.push(0x20);
    uleb(idx as u64, out);
}

/// A module under construction: shared/unshared memory, no locals, no
/// imports, named function exports.
#[derive(Default)]
struct ModuleBuilder {
    types: Vec<(Vec<u8>, Vec<u8>)>,
    funcs: Vec<u32>,
    exports: Vec<(String, u32)>,
    memory: Option<(bool, u32, Option<u32>)>,
    bodies: Vec<Vec<u8>>,
}

impl ModuleBuilder {
    fn new() -> Self {
        Self::default()
    }

    /// A classic shared memory `(memory min max shared)` (flags 0x03 —
    /// a shared memory always carries its maximum, pin `binary/types.rst`
    /// `_binary-memtype_`).
    fn shared_memory(&mut self, min: u32, max: u32) -> &mut Self {
        self.memory = Some((true, min, Some(max)));
        self
    }

    fn unshared_memory(&mut self, min: u32) -> &mut Self {
        self.memory = Some((false, min, None));
        self
    }

    fn func(&mut self, params: &[u8], results: &[u8], body: Vec<u8>) -> u32 {
        let ty_idx = match self
            .types
            .iter()
            .position(|(p, r)| p == params && r == results)
        {
            Some(idx) => idx as u32,
            None => {
                self.types.push((params.to_vec(), results.to_vec()));
                (self.types.len() - 1) as u32
            }
        };
        self.funcs.push(ty_idx);
        self.bodies.push(body);
        (self.funcs.len() - 1) as u32
    }

    fn export(&mut self, name: &str, func: u32) -> &mut Self {
        self.exports.push((name.to_owned(), func));
        self
    }

    fn build(&self) -> Vec<u8> {
        let mut out = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];

        let mut types = Vec::new();
        uleb(self.types.len() as u64, &mut types);
        for (params, results) in &self.types {
            types.push(0x60);
            uleb(params.len() as u64, &mut types);
            types.extend_from_slice(params);
            uleb(results.len() as u64, &mut types);
            types.extend_from_slice(results);
        }
        if !self.types.is_empty() {
            out.extend_from_slice(&section(1, &types));
        }

        if !self.funcs.is_empty() {
            let mut funcs = Vec::new();
            uleb(self.funcs.len() as u64, &mut funcs);
            for ty in &self.funcs {
                uleb(*ty as u64, &mut funcs);
            }
            out.extend_from_slice(&section(3, &funcs));
        }

        if let Some((shared, min, max)) = self.memory {
            let mut mem = Vec::new();
            uleb(1, &mut mem);
            mem.push(match (shared, max.is_some()) {
                (true, _) => 0x03,
                (false, true) => 0x01,
                (false, false) => 0x00,
            });
            uleb(min as u64, &mut mem);
            if let Some(max) = max {
                uleb(max as u64, &mut mem);
            }
            out.extend_from_slice(&section(5, &mem));
        }

        if !self.exports.is_empty() {
            let mut exports = Vec::new();
            uleb(self.exports.len() as u64, &mut exports);
            for (name, func) in &self.exports {
                uleb(name.len() as u64, &mut exports);
                exports.extend_from_slice(name.as_bytes());
                exports.push(0x00); // function
                uleb(*func as u64, &mut exports);
            }
            out.extend_from_slice(&section(7, &exports));
        }

        if !self.bodies.is_empty() {
            let mut code = Vec::new();
            uleb(self.bodies.len() as u64, &mut code);
            for body in &self.bodies {
                let mut full = vec![0x00]; // no locals
                full.extend_from_slice(body);
                full.push(0x0B);
                uleb(full.len() as u64, &mut code);
                code.extend_from_slice(&full);
            }
            out.extend_from_slice(&section(10, &code));
        }

        out
    }
}

/// A built module plus the store its exports run against.
struct Harness {
    module: RegModule,
    store: Store,
}

impl Harness {
    fn new(builder: &ModuleBuilder) -> Self {
        let bytes = builder.build();
        let module = Module::decode(&bytes).unwrap_or_else(|e| panic!("decode failed: {e}"));
        validate_module(&module).unwrap_or_else(|e| panic!("validation failed: {e}"));
        let module = module
            .lower()
            .unwrap_or_else(|e| panic!("lowering failed: {e:?}"));
        let store =
            Store::instantiate(&module).unwrap_or_else(|e| panic!("instantiation failed: {e:?}"));
        Self { module, store }
    }

    fn call(&self, name: &str, args: &[Value]) -> Result<Vec<Value>, RuntimeError> {
        execute_export(&self.module, &self.store, name, args)
    }
}

fn trap_of(result: Result<Vec<Value>, RuntimeError>) -> RuntimeTrap {
    match result.expect_err("expected a trap") {
        RuntimeError {
            kind: RuntimeErrorKind::Trap(trap),
        } => trap,
        other => panic!("expected a trap, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Opcode-by-width tables (sub-opcode, natural alignment exponent, byte width)
// ---------------------------------------------------------------------------
//
// width index — the layout `validate::width_index` and the pin's
// `binary/instructions.rst` both use:
//   0 i32, 1 i64, 2 i32.8_u, 3 i32.16_u, 4 i64.8_u, 5 i64.16_u, 6 i64.32_u

fn width_ty(width: u8) -> Ty {
    match width {
        0 | 2 | 3 => Ty::I32,
        _ => Ty::I64,
    }
}

fn width_bytes(width: u8) -> u32 {
    match width {
        0 => 4,
        1 => 8,
        2 | 4 => 1,
        3 | 5 => 2,
        6 => 4,
        _ => unreachable!("width index {width}"),
    }
}

fn width_align(width: u8) -> u32 {
    match width_bytes(width) {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        _ => unreachable!(),
    }
}

/// `i32.atomic.load` … `i64.atomic.load32_u` (0x10..=0x16).
fn load_bytes(width: u8) -> Vec<u8> {
    vec![0xFE, 0x10 + width, width_align(width) as u8, 0x00]
}

/// `i32.atomic.store` … `i64.atomic.store32` (0x17..=0x1d).
fn store_bytes(width: u8) -> Vec<u8> {
    vec![0xFE, 0x17 + width, width_align(width) as u8, 0x00]
}

/// rmw kind index: add=0, sub=1, and=2, or=3, xor=4, xchg=5.
/// sub-opcode = 0x1e + kind * 7 + width (0x1e..=0x47).
fn rmw_bytes(kind: u8, width: u8) -> Vec<u8> {
    vec![
        0xFE,
        0x1e + kind * 7 + width,
        width_align(width) as u8,
        0x00,
    ]
}

/// `i32.atomic.rmw.cmpxchg` … `i64.atomic.rmw32.cmpxchg_u` (0x48..=0x4e).
fn cmpxchg_bytes(width: u8) -> Vec<u8> {
    vec![0xFE, 0x48 + width, width_align(width) as u8, 0x00]
}

const ATOMIC_FENCE: &[u8] = &[0xFE, 0x03, 0x00];
const NOTIFY: &[u8] = &[0xFE, 0x00, 0x02, 0x00];
const WAIT32: &[u8] = &[0xFE, 0x01, 0x02, 0x00];
const WAIT64: &[u8] = &[0xFE, 0x02, 0x03, 0x00];

// ---------------------------------------------------------------------------
// 1. Loads and stores: full width, narrow zero-extension, narrow truncation
// ---------------------------------------------------------------------------

#[test]
fn full_width_load_store_roundtrip() {
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    let mut body = Vec::new();
    i32_const(0, &mut body);
    i32_const(0x12345678, &mut body);
    body.extend_from_slice(&store_bytes(0)); // i32.atomic.store
    i32_const(0, &mut body);
    body.extend_from_slice(&load_bytes(0)); // i32.atomic.load
    let f = b.func(&[], &[I32], body);
    b.export("rt32", f);

    let mut body = Vec::new();
    i32_const(0, &mut body);
    i64_const(0x1122_3344_5566_7788, &mut body);
    body.extend_from_slice(&store_bytes(1)); // i64.atomic.store
    i32_const(0, &mut body);
    body.extend_from_slice(&load_bytes(1)); // i64.atomic.load
    let f = b.func(&[], &[I64], body);
    b.export("rt64", f);

    let h = Harness::new(&b);
    assert_eq!(h.call("rt32", &[]), Ok(vec![Value::I32(0x12345678)]));
    assert_eq!(
        h.call("rt64", &[]),
        Ok(vec![Value::I64(0x1122_3344_5566_7788)])
    );
}

// A narrow store truncates; a narrow load zero-extends (`eval.ml:271-294`,
// `ZX` for loads, `store_packed` for stores).

struct NarrowCase {
    label: &'static str,
    ty: Ty,
    width: u8,
    operand: Value,
    want: Value,
}

fn narrow_cases() -> Vec<NarrowCase> {
    vec![
        NarrowCase {
            label: "i32.atomic.store8 truncates, load8_u zero-extends",
            ty: Ty::I32,
            width: 2,
            operand: Value::I32(0x1234),
            want: Value::I32(0x34),
        },
        NarrowCase {
            label: "i32.atomic.load8_u of 0xFF is 255, never -1",
            ty: Ty::I32,
            width: 2,
            operand: Value::I32(0xFF),
            want: Value::I32(255),
        },
        NarrowCase {
            label: "i32.atomic.store16 truncates, load16_u zero-extends",
            ty: Ty::I32,
            width: 3,
            operand: Value::I32(0x123456),
            want: Value::I32(0x3456),
        },
        NarrowCase {
            label: "i32.atomic.load16_u of 0xFFFF is 65535, never -1",
            ty: Ty::I32,
            width: 3,
            operand: Value::I32(0xFFFF),
            want: Value::I32(0xFFFF),
        },
        NarrowCase {
            label: "i64.atomic.store8 truncates, load8_u zero-extends",
            ty: Ty::I64,
            width: 4,
            operand: Value::I64(0xFF),
            want: Value::I64(255),
        },
        NarrowCase {
            label: "i64.atomic.load16_u of 0xFFFF is 65535",
            ty: Ty::I64,
            width: 5,
            operand: Value::I64(0xFFFF),
            want: Value::I64(0xFFFF),
        },
        NarrowCase {
            label: "i64.atomic.store32 truncates, load32_u zero-extends",
            ty: Ty::I64,
            width: 6,
            operand: Value::I64(0x1122_3344_5566_7788),
            want: Value::I64(0x5566_7788),
        },
        NarrowCase {
            label: "i64.atomic.load32_u of 0xFFFFFFFF is 4294967295, never -1",
            ty: Ty::I64,
            width: 6,
            operand: Value::I64(0xFFFF_FFFF),
            want: Value::I64(0xFFFF_FFFF),
        },
    ]
}

#[test]
fn narrow_store_truncates_and_load_zero_extends() {
    for case in narrow_cases() {
        let ty = case.ty.val_type();
        let mut b = ModuleBuilder::new();
        b.shared_memory(1, 2);
        // (param ty) -> ty: store the operand narrow, load narrow back.
        let mut body = Vec::new();
        i32_const(0, &mut body);
        local_get(0, &mut body);
        body.extend_from_slice(&store_bytes(case.width));
        i32_const(0, &mut body);
        body.extend_from_slice(&load_bytes(case.width));
        let f = b.func(&[ty], &[ty], body);
        b.export("probe", f);

        let h = Harness::new(&b);
        assert_eq!(
            h.call("probe", &[case.operand]),
            Ok(vec![case.want]),
            "{}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Read-modify-write: OLD value returned, full-width computation, narrow
//    ZX-on-load / truncate-on-store (`eval.ml:295-311`)
// ---------------------------------------------------------------------------

struct RmwCase {
    label: &'static str,
    ty: Ty,
    width: u8,
    kind: u8,
    initial: Value,
    operand: Value,
    want_old: Value,
    want_after: Value,
}

const ADD: u8 = 0;
const SUB: u8 = 1;
const AND: u8 = 2;
const OR: u8 = 3;
const XOR: u8 = 4;
const XCHG: u8 = 5;

fn rmw_cases() -> Vec<RmwCase> {
    vec![
        RmwCase {
            label: "i32.atomic.rmw.add",
            ty: Ty::I32,
            width: 0,
            kind: ADD,
            initial: Value::I32(10),
            operand: Value::I32(5),
            want_old: Value::I32(10),
            want_after: Value::I32(15),
        },
        RmwCase {
            label: "i32.atomic.rmw.sub",
            ty: Ty::I32,
            width: 0,
            kind: SUB,
            initial: Value::I32(5),
            operand: Value::I32(3),
            want_old: Value::I32(5),
            want_after: Value::I32(2),
        },
        RmwCase {
            label: "i32.atomic.rmw.and",
            ty: Ty::I32,
            width: 0,
            kind: AND,
            initial: Value::I32(0xF0F0),
            operand: Value::I32(0x0FF0),
            want_old: Value::I32(0xF0F0),
            want_after: Value::I32(0x00F0),
        },
        RmwCase {
            label: "i32.atomic.rmw.or",
            ty: Ty::I32,
            width: 0,
            kind: OR,
            initial: Value::I32(0xF000),
            operand: Value::I32(0x0F00),
            want_old: Value::I32(0xF000),
            want_after: Value::I32(0xFF00),
        },
        RmwCase {
            label: "i32.atomic.rmw.xor",
            ty: Ty::I32,
            width: 0,
            kind: XOR,
            initial: Value::I32(0xFF00),
            operand: Value::I32(0x0FF0),
            want_old: Value::I32(0xFF00),
            want_after: Value::I32(0xF0F0),
        },
        RmwCase {
            label: "i32.atomic.rmw.xchg",
            ty: Ty::I32,
            width: 0,
            kind: XCHG,
            initial: Value::I32(7),
            operand: Value::I32(9),
            want_old: Value::I32(7),
            want_after: Value::I32(9),
        },
        RmwCase {
            label: "i32.atomic.rmw.add wraps at full width",
            ty: Ty::I32,
            width: 0,
            kind: ADD,
            initial: Value::I32(i32::MAX),
            operand: Value::I32(1),
            want_old: Value::I32(i32::MAX),
            want_after: Value::I32(i32::MIN),
        },
        RmwCase {
            label: "i64.atomic.rmw.add",
            ty: Ty::I64,
            width: 1,
            kind: ADD,
            initial: Value::I64(10),
            operand: Value::I64(5),
            want_old: Value::I64(10),
            want_after: Value::I64(15),
        },
        RmwCase {
            label: "i64.atomic.rmw.sub wraps at full width",
            ty: Ty::I64,
            width: 1,
            kind: SUB,
            initial: Value::I64(i64::MIN),
            operand: Value::I64(1),
            want_old: Value::I64(i64::MIN),
            want_after: Value::I64(i64::MAX),
        },
        RmwCase {
            label: "i64.atomic.rmw.and",
            ty: Ty::I64,
            width: 1,
            kind: AND,
            initial: Value::I64(0xF0F0),
            operand: Value::I64(0x0FF0),
            want_old: Value::I64(0xF0F0),
            want_after: Value::I64(0x00F0),
        },
        RmwCase {
            label: "i64.atomic.rmw.or",
            ty: Ty::I64,
            width: 1,
            kind: OR,
            initial: Value::I64(0xF000),
            operand: Value::I64(0x0F00),
            want_old: Value::I64(0xF000),
            want_after: Value::I64(0xFF00),
        },
        RmwCase {
            label: "i64.atomic.rmw.xor",
            ty: Ty::I64,
            width: 1,
            kind: XOR,
            initial: Value::I64(0xFF00),
            operand: Value::I64(0x0FF0),
            want_old: Value::I64(0xFF00),
            want_after: Value::I64(0xF0F0),
        },
        RmwCase {
            label: "i64.atomic.rmw.xchg",
            ty: Ty::I64,
            width: 1,
            kind: XCHG,
            initial: Value::I64(0x1111),
            operand: Value::I64(0x2222),
            want_old: Value::I64(0x1111),
            want_after: Value::I64(0x2222),
        },
        // Narrow forms: load zero-extends to full width, the arithmetic runs
        // at full width and wraps there, the store truncates, and the OLD
        // (zero-extended) value is returned.
        RmwCase {
            label: "i32.atomic.rmw8.add_u: 0xFF + 1 wraps at 32 bits, stores 0x00, returns 255",
            ty: Ty::I32,
            width: 2,
            kind: ADD,
            initial: Value::I32(0xFF),
            operand: Value::I32(1),
            want_old: Value::I32(255),
            want_after: Value::I32(0),
        },
        RmwCase {
            label: "i32.atomic.rmw16.sub_u: 0 - 1 wraps at 32 bits, stores 0xFFFF, returns 0",
            ty: Ty::I32,
            width: 3,
            kind: SUB,
            initial: Value::I32(0),
            operand: Value::I32(1),
            want_old: Value::I32(0),
            want_after: Value::I32(0xFFFF),
        },
        RmwCase {
            label: "i32.atomic.rmw8.and_u",
            ty: Ty::I32,
            width: 2,
            kind: AND,
            initial: Value::I32(0xFF),
            operand: Value::I32(0x0F),
            want_old: Value::I32(255),
            want_after: Value::I32(0x0F),
        },
        RmwCase {
            label: "i32.atomic.rmw16.xor_u",
            ty: Ty::I32,
            width: 3,
            kind: XOR,
            initial: Value::I32(0xF0F0),
            operand: Value::I32(0x0FF0),
            want_old: Value::I32(0xF0F0),
            want_after: Value::I32(0xFF00),
        },
        RmwCase {
            label: "i32.atomic.rmw8.xchg_u truncates the stored operand",
            ty: Ty::I32,
            width: 2,
            kind: XCHG,
            initial: Value::I32(0x12),
            operand: Value::I32(0x34),
            want_old: Value::I32(0x12),
            want_after: Value::I32(0x34),
        },
        RmwCase {
            label: "i64.atomic.rmw8.add_u: 255 + 1 wraps at 64 bits, stores 0x00",
            ty: Ty::I64,
            width: 4,
            kind: ADD,
            initial: Value::I64(0xFF),
            operand: Value::I64(1),
            want_old: Value::I64(255),
            want_after: Value::I64(0),
        },
        RmwCase {
            label: "i64.atomic.rmw16.or_u",
            ty: Ty::I64,
            width: 5,
            kind: OR,
            initial: Value::I64(0xF000),
            operand: Value::I64(0x0F00),
            want_old: Value::I64(0xF000),
            want_after: Value::I64(0xFF00),
        },
        RmwCase {
            label: "i64.atomic.rmw32.add_u: full-width compute, truncated store, ZX old",
            ty: Ty::I64,
            width: 6,
            kind: ADD,
            initial: Value::I64(0xFFFF_FFFF),
            operand: Value::I64(1),
            want_old: Value::I64(0xFFFF_FFFF),
            want_after: Value::I64(0),
        },
        RmwCase {
            label: "i64.atomic.rmw32.xchg_u",
            ty: Ty::I64,
            width: 6,
            kind: XCHG,
            initial: Value::I64(0x1234_5678),
            operand: Value::I64(0xAABB_CCDD),
            want_old: Value::I64(0x1234_5678),
            want_after: Value::I64(0xAABB_CCDD),
        },
    ]
}

#[test]
fn rmw_returns_old_value_and_stores_the_result() {
    for case in rmw_cases() {
        let ty = case.ty.val_type();
        let mut b = ModuleBuilder::new();
        b.shared_memory(1, 2);
        // (param initial, param operand) -> old
        let mut body = Vec::new();
        i32_const(0, &mut body);
        local_get(0, &mut body);
        body.extend_from_slice(&store_bytes(case.width));
        i32_const(0, &mut body);
        local_get(1, &mut body);
        body.extend_from_slice(&rmw_bytes(case.kind, case.width));
        let f = b.func(&[ty, ty], &[ty], body);
        b.export("rmw", f);

        let mut body = Vec::new();
        i32_const(0, &mut body);
        body.extend_from_slice(&load_bytes(case.width));
        let f = b.func(&[], &[ty], body);
        b.export("load", f);

        let h = Harness::new(&b);
        let old = h.call("rmw", &[case.initial, case.operand]);
        assert_eq!(old, Ok(vec![case.want_old]), "{}: old value", case.label);
        let after = h.call("load", &[]);
        assert_eq!(
            after,
            Ok(vec![case.want_after]),
            "{}: stored value",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// 3. cmpxchg: compare the zero-extended OLD against `expected`; store the
//    replacement only on equality; return OLD in every case
//    (`eval.ml:312-328`)
// ---------------------------------------------------------------------------

struct CmpCase {
    label: &'static str,
    ty: Ty,
    width: u8,
    initial: Value,
    expected: Value,
    replacement: Value,
    want_old: Value,
    want_after: Value,
}

fn cmpxchg_cases() -> Vec<CmpCase> {
    vec![
        CmpCase {
            label: "i32.atomic.rmw.cmpxchg equal stores the replacement",
            ty: Ty::I32,
            width: 0,
            initial: Value::I32(0x1234),
            expected: Value::I32(0x1234),
            replacement: Value::I32(0x5678),
            want_old: Value::I32(0x1234),
            want_after: Value::I32(0x5678),
        },
        CmpCase {
            label: "i32.atomic.rmw.cmpxchg unequal leaves memory untouched",
            ty: Ty::I32,
            width: 0,
            initial: Value::I32(0x1234),
            expected: Value::I32(0x9999),
            replacement: Value::I32(0x5678),
            want_old: Value::I32(0x1234),
            want_after: Value::I32(0x1234),
        },
        CmpCase {
            label: "i64.atomic.rmw.cmpxchg equal",
            ty: Ty::I64,
            width: 1,
            initial: Value::I64(0x1122_3344_5566_7788),
            expected: Value::I64(0x1122_3344_5566_7788),
            replacement: Value::I64(0x8877_6655_4433_2211u64 as i64),
            want_old: Value::I64(0x1122_3344_5566_7788),
            want_after: Value::I64(0x8877_6655_4433_2211u64 as i64),
        },
        CmpCase {
            label: "i64.atomic.rmw.cmpxchg unequal",
            ty: Ty::I64,
            width: 1,
            initial: Value::I64(7),
            expected: Value::I64(8),
            replacement: Value::I64(9),
            want_old: Value::I64(7),
            want_after: Value::I64(7),
        },
        // Narrow: the comparison is against the ZERO-EXTENDED old value, so
        // expected = 0x100 can never match an i32.atomic.rmw8 load of 0xFF.
        CmpCase {
            label: "i32.atomic.rmw8.cmpxchg_u compares the zero-extended old",
            ty: Ty::I32,
            width: 2,
            initial: Value::I32(0xFF),
            expected: Value::I32(0xFF),
            replacement: Value::I32(0x1FF),
            want_old: Value::I32(255),
            want_after: Value::I32(0xFF),
        },
        CmpCase {
            label: "i32.atomic.rmw8.cmpxchg_u: expected 0x100 does not match old 0xFF",
            ty: Ty::I32,
            width: 2,
            initial: Value::I32(0xFF),
            expected: Value::I32(0x100),
            replacement: Value::I32(0x12),
            want_old: Value::I32(255),
            want_after: Value::I32(0xFF),
        },
        CmpCase {
            label: "i32.atomic.rmw16.cmpxchg_u unequal",
            ty: Ty::I32,
            width: 3,
            initial: Value::I32(0xFFFF),
            expected: Value::I32(0x1_0000),
            replacement: Value::I32(0),
            want_old: Value::I32(0xFFFF),
            want_after: Value::I32(0xFFFF),
        },
        CmpCase {
            label: "i64.atomic.rmw32.cmpxchg_u compares 32 bits zero-extended",
            ty: Ty::I64,
            width: 6,
            initial: Value::I64(0xFFFF_FFFF),
            expected: Value::I64(0xFFFF_FFFF),
            replacement: Value::I64(0x1_0000_0000),
            want_old: Value::I64(0xFFFF_FFFF),
            want_after: Value::I64(0),
        },
    ]
}

#[test]
fn cmpxchg_success_and_failure_paths() {
    for case in cmpxchg_cases() {
        let ty = case.ty.val_type();
        let mut b = ModuleBuilder::new();
        b.shared_memory(1, 2);
        // (param initial, param expected, param replacement) -> old
        let mut body = Vec::new();
        i32_const(0, &mut body);
        local_get(0, &mut body);
        body.extend_from_slice(&store_bytes(case.width));
        i32_const(0, &mut body);
        local_get(1, &mut body);
        local_get(2, &mut body);
        body.extend_from_slice(&cmpxchg_bytes(case.width));
        let f = b.func(&[ty, ty, ty], &[ty], body);
        b.export("cmp", f);

        let mut body = Vec::new();
        i32_const(0, &mut body);
        body.extend_from_slice(&load_bytes(case.width));
        let f = b.func(&[], &[ty], body);
        b.export("load", f);

        let h = Harness::new(&b);
        let old = h.call("cmp", &[case.initial, case.expected, case.replacement]);
        assert_eq!(old, Ok(vec![case.want_old]), "{}: old value", case.label);
        let after = h.call("load", &[]);
        assert_eq!(
            after,
            Ok(vec![case.want_after]),
            "{}: memory after",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Unaligned traps, parameterized across the whole 67-op family
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Layout {
    Notify,
    Wait(Ty),
    Load,
    Store(Ty),
    Rmw(Ty),
    Cmpxchg(Ty),
}

/// Body fragment for one 0xFE sub-opcode at `addr`, with `align` and
/// `offset` memarg fields. Operands follow validation's pop order (address
/// deepest).
fn atomic_body(sub: u8, addr: i32, align: u32, offset: u32, layout: Layout) -> Vec<u8> {
    let mut body = Vec::new();
    i32_const(addr, &mut body);
    match layout {
        Layout::Notify => i32_const(0, &mut body),
        Layout::Wait(ty) => {
            const_of(ty, 0, &mut body);
            i64_const(0, &mut body);
        }
        Layout::Load => {}
        Layout::Store(ty) | Layout::Rmw(ty) => const_of(ty, 0, &mut body),
        Layout::Cmpxchg(ty) => {
            const_of(ty, 0, &mut body);
            const_of(ty, 0, &mut body);
        }
    }
    body.push(0xFE);
    body.push(sub);
    uleb(align as u64, &mut body);
    uleb(offset as u64, &mut body);
    if !matches!(layout, Layout::Store(_)) {
        body.push(0x1A); // drop the result
    }
    body
}

/// The 67-row family as (sub-opcode, byte width, layout).
fn family_rows() -> Vec<(u8, u32, Layout)> {
    let mut rows = vec![
        (0x00, 4, Layout::Notify),
        (0x01, 4, Layout::Wait(Ty::I32)),
        (0x02, 8, Layout::Wait(Ty::I64)),
    ];
    for width in 0..7u8 {
        rows.push((0x10 + width, width_bytes(width), Layout::Load));
        rows.push((
            0x17 + width,
            width_bytes(width),
            Layout::Store(width_ty(width)),
        ));
    }
    for kind in 0..6u8 {
        for width in 0..7u8 {
            rows.push((
                0x1e + kind * 7 + width,
                width_bytes(width),
                Layout::Rmw(width_ty(width)),
            ));
        }
    }
    for width in 0..7u8 {
        rows.push((
            0x48 + width,
            width_bytes(width),
            Layout::Cmpxchg(width_ty(width)),
        ));
    }
    assert_eq!(
        rows.len(),
        66,
        "3 sync + 7 load + 7 store + 42 rmw + 7 cmp = the 66 memory ops of\
         the 67-member family (atomic.fence is the 67th, exercised separately)"
    );
    rows
}

#[test]
fn unaligned_traps_across_the_whole_family() {
    for (sub, byte_width, layout) in family_rows() {
        let align = match byte_width {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            other => panic!("bad width {other}"),
        };
        let mut b = ModuleBuilder::new();
        b.shared_memory(1, 2);
        // Address 1 is in bounds; it is unaligned for every natural width
        // above one byte. A one-byte access is aligned at every address —
        // there is no unaligned case to trap on (pin `memory.ml:23-28`:
        // 8-bit accesses have alignment 1).
        let f = b.func(&[], &[], atomic_body(sub, 1, align, 0, layout));
        b.export("op", f);
        let h = Harness::new(&b);
        let result = h.call("op", &[]);
        if byte_width == 1 {
            assert_eq!(
                result,
                Ok(vec![]),
                "0xFE {sub:#04x}: one-byte access is always aligned"
            );
        } else {
            assert_eq!(
                trap_of(result),
                RuntimeTrap::UnalignedAtomicMemoryAccess,
                "0xFE {sub:#04x} (width {byte_width})"
            );
        }
    }
}

#[test]
fn unaligned_trap_beats_out_of_bounds() {
    // 0xFFFF is unaligned at every width >= 2 and its access runs past a
    // one-page memory. `check_align` precedes the access (`eval.ml:271-357`),
    // so the unaligned trap wins.
    let cases: Vec<(u8, u32, Layout)> = vec![
        (0x10, 4, Layout::Load),
        (0x17, 4, Layout::Store(Ty::I32)),
        (0x1e, 4, Layout::Rmw(Ty::I32)),
        (0x48, 4, Layout::Cmpxchg(Ty::I32)),
        (0x00, 4, Layout::Notify),
        (0x01, 4, Layout::Wait(Ty::I32)),
        (0x02, 8, Layout::Wait(Ty::I64)),
    ];
    for (sub, byte_width, layout) in cases {
        let align = match byte_width {
            4 => 2,
            8 => 3,
            other => panic!("bad width {other}"),
        };
        let mut b = ModuleBuilder::new();
        b.shared_memory(1, 2);
        let f = b.func(&[], &[], atomic_body(sub, 0xFFFF, align, 0, layout));
        b.export("op", f);
        let h = Harness::new(&b);
        assert_eq!(
            trap_of(h.call("op", &[])),
            RuntimeTrap::UnalignedAtomicMemoryAccess,
            "0xFE {sub:#04x}: unaligned must beat out-of-bounds"
        );
    }
}

#[test]
fn alignment_uses_the_effective_address() {
    // Rule-7 resolution: `ea = i + memarg.OFFSET`, then "If ea modulo N/8 is
    // not equal to 0, then: trap" (document/core/exec/instructions.rst). The
    // pin's interpreter checks the base address only (`eval.ml:275` passes
    // `addr` before `offset` is applied) — a bug invisible to the corpus,
    // which never uses `offset=` on atomics. Base 0 is aligned; the offset
    // makes the effective address unaligned.
    for (sub, byte_width, layout, offset) in [
        (0x10u8, 4u32, Layout::Load, 5u32),     // i32.atomic.load
        (0x13, 2, Layout::Load, 1),             // i32.atomic.load16_u
        (0x11, 8, Layout::Load, 3),             // i64.atomic.load
        (0x17, 4, Layout::Store(Ty::I32), 2),   // i32.atomic.store
        (0x1e, 4, Layout::Rmw(Ty::I32), 6),     // i32.atomic.rmw.add
        (0x48, 4, Layout::Cmpxchg(Ty::I32), 1), // cmpxchg
        (0x00, 4, Layout::Notify, 2),           // notify
        (0x02, 8, Layout::Wait(Ty::I64), 4),    // wait64
    ] {
        let align = match byte_width {
            2 => 1,
            4 => 2,
            8 => 3,
            other => panic!("bad width {other}"),
        };
        let mut b = ModuleBuilder::new();
        b.shared_memory(1, 2);
        let f = b.func(&[], &[], atomic_body(sub, 0, align, offset, layout));
        b.export("op", f);
        let h = Harness::new(&b);
        assert_eq!(
            trap_of(h.call("op", &[])),
            RuntimeTrap::UnalignedAtomicMemoryAccess,
            "0xFE {sub:#04x}: base 0 + offset {offset} is unaligned"
        );
    }

    // Control: the same op with an aligned offset executes in bounds.
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    let mut body = Vec::new();
    i32_const(0, &mut body);
    body.extend_from_slice(&[0xFE, 0x10, 0x02, 0x04]); // i32.atomic.load offset=4
    body.push(0x1A);
    let f = b.func(&[], &[], body);
    b.export("op", f);
    let h = Harness::new(&b);
    assert_eq!(h.call("op", &[]), Ok(vec![]), "aligned offset 4 executes");
}

// ---------------------------------------------------------------------------
// 5. wait / notify (`eval.ml:329-357`)
// ---------------------------------------------------------------------------

/// Shared memory holding `initial` at address 0, plus functions that wait on
/// it. Timeouts are parameters so one harness covers the epsilon band, the
/// "large" band, and negative (infinite) timeouts.
fn wait_harness(initial: i64, ty: Ty) -> Harness {
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    // init(): store `initial` at address 0.
    let mut body = Vec::new();
    i32_const(0, &mut body);
    const_of(ty, initial, &mut body);
    body.extend_from_slice(&store_bytes(if ty == Ty::I32 { 0 } else { 1 }));
    let f = b.func(&[], &[], body);
    b.export("init", f);

    // wait32/wait64 (param $expected, param $timeout) -> i32
    let wait = match ty {
        Ty::I32 => WAIT32,
        Ty::I64 => WAIT64,
    };
    let mut body = Vec::new();
    i32_const(0, &mut body);
    local_get(0, &mut body);
    local_get(1, &mut body);
    body.extend_from_slice(wait);
    let f = b.func(&[ty.val_type(), I64], &[I32], body);
    b.export("wait", f);

    let h = Harness::new(&b);
    h.call("init", &[]).expect("init");
    h
}

#[test]
fn wait_returns_one_on_value_mismatch() {
    let h = wait_harness(5, Ty::I32);
    assert_eq!(
        h.call("wait", &[Value::I32(7), Value::I64(0)]),
        Ok(vec![Value::I32(1)])
    );
    // A mismatching wait returns 1 regardless of the timeout.
    assert_eq!(
        h.call("wait", &[Value::I32(7), Value::I64(-1)]),
        Ok(vec![Value::I32(1)])
    );

    let h = wait_harness(0x1122_3344_5566_7788, Ty::I64);
    assert_eq!(
        h.call("wait", &[Value::I64(0), Value::I64(0)]),
        Ok(vec![Value::I32(1)])
    );
}

#[test]
fn wait_returns_two_when_the_value_matches() {
    // Single-agent determination (documented): with one agent there is no
    // wake source, so every matching wait ends in a timeout. We return 2
    // without sleeping for ALL timeouts — including the epsilon band
    // (`timeout_epsilon = 1_000_000` ns, `eval.ml:38`), larger timeouts the
    // pin would suspend on, and negative (infinite) timeouts.
    let h = wait_harness(5, Ty::I32);
    for timeout in [0i64, 1, 999_999, 1_000_000, 5_000_000_000, i64::MAX, -1] {
        assert_eq!(
            h.call("wait", &[Value::I32(5), Value::I64(timeout)]),
            Ok(vec![Value::I32(2)]),
            "i32 wait match with timeout {timeout}"
        );
    }

    let h = wait_harness(0x1122_3344_5566_7788, Ty::I64);
    for timeout in [0i64, 999_999, -1] {
        assert_eq!(
            h.call(
                "wait",
                &[Value::I64(0x1122_3344_5566_7788), Value::I64(timeout)]
            ),
            Ok(vec![Value::I32(2)]),
            "i64 wait match with timeout {timeout}"
        );
    }
}

#[test]
fn unaligned_wait_traps_before_the_shared_check() {
    // `check_align` → `check_shared` → load (`eval.ml:329-335`). An unshared
    // memory and an out-of-bounds address both lose to the alignment trap.
    let mut b = ModuleBuilder::new();
    b.unshared_memory(0);
    let f = b.func(&[], &[], atomic_body(0x01, 2, 2, 0, Layout::Wait(Ty::I32)));
    b.export("op", f);
    let h = Harness::new(&b);
    assert_eq!(
        trap_of(h.call("op", &[])),
        RuntimeTrap::UnalignedAtomicMemoryAccess
    );
}

#[test]
fn shared_check_traps_before_the_bounds_check() {
    // An aligned wait on an UNSHARED, out-of-bounds address traps
    // "expected shared memory", never out-of-bounds (`eval.ml:329-335`:
    // `check_align`, then `check_shared`, then `Memory.load_value`). A
    // zero-page memory makes the address out of bounds.
    for (wait, ty) in [(WAIT32, Ty::I32), (WAIT64, Ty::I64)] {
        let mut b = ModuleBuilder::new();
        b.unshared_memory(0);
        let mut body = Vec::new();
        i32_const(0, &mut body);
        const_of(ty, 0, &mut body);
        i64_const(0, &mut body);
        body.extend_from_slice(wait);
        let f = b.func(&[], &[I32], body);
        b.export("op", f);
        let h = Harness::new(&b);
        assert_eq!(
            trap_of(h.call("op", &[])),
            RuntimeTrap::ExpectedSharedMemory,
            "{ty:?}"
        );
    }
}

#[test]
fn wait_on_unshared_memory_traps_even_when_it_would_not_block() {
    // atomic_wait_notify.wast:58-59: "traps on unshared memory even if it
    // wouldn't block". Memory holds 5, the wait's expected value is 7, so a
    // plain wait would return 1 — the shared check still traps first.
    let mut b = ModuleBuilder::new();
    b.unshared_memory(1);
    let mut body = Vec::new();
    i32_const(0, &mut body);
    i32_const(5, &mut body);
    body.extend_from_slice(&store_bytes(0));
    let f = b.func(&[], &[], body);
    b.export("init", f);

    let mut body = Vec::new();
    i32_const(0, &mut body);
    i32_const(7, &mut body);
    i64_const(0, &mut body);
    body.extend_from_slice(WAIT32);
    let f = b.func(&[], &[I32], body);
    b.export("wait", f);

    let h = Harness::new(&b);
    h.call("init", &[]).expect("init");
    assert_eq!(
        trap_of(h.call("wait", &[])),
        RuntimeTrap::ExpectedSharedMemory
    );
}

#[test]
fn wait_on_shared_out_of_bounds_memory_traps() {
    // Shared, aligned, out of bounds → the load traps.
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    let mut body = Vec::new();
    i32_const(0x10000, &mut body); // one past a one-page memory
    i32_const(0, &mut body);
    i64_const(0, &mut body);
    body.extend_from_slice(WAIT32);
    let f = b.func(&[], &[I32], body);
    b.export("wait", f);
    let h = Harness::new(&b);
    assert_eq!(
        trap_of(h.call("wait", &[])),
        RuntimeTrap::OutOfBoundsMemoryAccess
    );

    let mut body = Vec::new();
    i32_const(0xFFFC, &mut body); // aligned, but offset 4 crosses the end
    i64_const(0, &mut body);
    i64_const(0, &mut body);
    body.extend_from_slice(&[0xFE, 0x02, 0x03, 0x04]); // wait64 offset=4
    let f = b.func(&[], &[I32], body);
    b.export("wait64", f);
    let h = Harness::new(&b);
    assert_eq!(
        trap_of(h.call("wait64", &[])),
        RuntimeTrap::OutOfBoundsMemoryAccess
    );
}

#[test]
fn notify_returns_zero_for_every_count() {
    // Single-agent determination: no waiters can exist, so notify returns 0
    // for every count (`eval.ml:347-357`; the count-0 short-circuit and the
    // NotifyAction case collapse to the same result with one agent).
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    let mut body = Vec::new();
    i32_const(0, &mut body);
    i32_const(5, &mut body);
    body.extend_from_slice(&store_bytes(0));
    let f = b.func(&[], &[], body);
    b.export("init", f);
    for (name, count) in [("notify0", 0usize), ("notify5", 5)] {
        let mut body = Vec::new();
        i32_const(0, &mut body);
        i32_const(count as i32, &mut body);
        body.extend_from_slice(NOTIFY);
        let f = b.func(&[], &[I32], body);
        b.export(name, f);
    }
    let h = Harness::new(&b);
    h.call("init", &[]).expect("init");
    assert_eq!(h.call("notify0", &[]), Ok(vec![Value::I32(0)]));
    assert_eq!(h.call("notify5", &[]), Ok(vec![Value::I32(0)]));
}

#[test]
fn notify_has_no_shared_requirement() {
    // "unshared memory is OK" (atomic.wast:440): only wait demands a shared
    // memory.
    let mut b = ModuleBuilder::new();
    b.unshared_memory(1);
    let mut body = Vec::new();
    i32_const(4, &mut body);
    i32_const(3, &mut body);
    body.extend_from_slice(NOTIFY);
    let f = b.func(&[], &[I32], body);
    b.export("notify", f);
    let h = Harness::new(&b);
    assert_eq!(h.call("notify", &[]), Ok(vec![Value::I32(0)]));
}

#[test]
fn notify_on_out_of_bounds_memory_traps() {
    // The notify load exists only for its trap effect (`eval.ml:349`:
    // `let _ = Memory.load_value …`).
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    let mut body = Vec::new();
    i32_const(0x10000, &mut body);
    i32_const(1, &mut body);
    body.extend_from_slice(NOTIFY);
    let f = b.func(&[], &[I32], body);
    b.export("notify", f);
    let h = Harness::new(&b);
    assert_eq!(
        trap_of(h.call("notify", &[])),
        RuntimeTrap::OutOfBoundsMemoryAccess
    );
}

// ---------------------------------------------------------------------------
// 6. fence — a pure no-op (`eval.ml:359-360`), needing no memory
// ---------------------------------------------------------------------------

#[test]
fn fence_executes_as_a_no_op() {
    let mut b = ModuleBuilder::new();
    // No memory section at all: atomic.fence requires none (valid.ml:311).
    let mut body = Vec::new();
    body.extend_from_slice(ATOMIC_FENCE);
    i32_const(42, &mut body);
    let f = b.func(&[], &[I32], body);
    b.export("fenced", f);

    let mut body = Vec::new();
    body.extend_from_slice(ATOMIC_FENCE);
    body.extend_from_slice(ATOMIC_FENCE);
    i32_const(7, &mut body);
    let f = b.func(&[], &[I32], body);
    b.export("double", f);

    let h = Harness::new(&b);
    assert_eq!(h.call("fenced", &[]), Ok(vec![Value::I32(42)]));
    assert_eq!(h.call("double", &[]), Ok(vec![Value::I32(7)]));
}

#[test]
fn fence_between_atomic_accesses_is_transparent() {
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);
    let mut body = Vec::new();
    i32_const(0, &mut body);
    i32_const(0x1234, &mut body);
    body.extend_from_slice(&store_bytes(0));
    body.extend_from_slice(ATOMIC_FENCE);
    i32_const(0, &mut body);
    body.extend_from_slice(&load_bytes(0));
    let f = b.func(&[], &[I32], body);
    b.export("rt", f);
    let h = Harness::new(&b);
    assert_eq!(h.call("rt", &[]), Ok(vec![Value::I32(0x1234)]));
}

// ---------------------------------------------------------------------------
// 7. memory.grow on a shared memory grows normally
// ---------------------------------------------------------------------------

#[test]
fn shared_memory_grow_grows_normally() {
    let mut b = ModuleBuilder::new();
    b.shared_memory(1, 2);

    // grow(): memory.grow 1 — returns the previous size in pages.
    let mut body = Vec::new();
    i32_const(1, &mut body);
    body.push(0x40); // memory.grow
    uleb(0, &mut body);
    let f = b.func(&[], &[I32], body);
    b.export("grow", f);

    // size(): memory.size
    let mut body = Vec::new();
    body.push(0x3F); // memory.size
    uleb(0, &mut body);
    let f = b.func(&[], &[I32], body);
    b.export("size", f);

    // high(): store/load atomically in the newly grown page.
    let mut body = Vec::new();
    i32_const(0x10000, &mut body);
    i32_const(7, &mut body);
    body.extend_from_slice(&store_bytes(0));
    i32_const(0x10000, &mut body);
    body.extend_from_slice(&load_bytes(0));
    let f = b.func(&[], &[I32], body);
    b.export("high", f);

    let h = Harness::new(&b);
    assert_eq!(h.call("size", &[]), Ok(vec![Value::I32(1)]));
    assert_eq!(h.call("grow", &[]), Ok(vec![Value::I32(1)]));
    assert_eq!(h.call("size", &[]), Ok(vec![Value::I32(2)]));
    assert_eq!(h.call("high", &[]), Ok(vec![Value::I32(7)]));
    // The shared maximum is enforced like any other maximum.
    assert_eq!(h.call("grow", &[]), Ok(vec![Value::I32(-1)]));
}
