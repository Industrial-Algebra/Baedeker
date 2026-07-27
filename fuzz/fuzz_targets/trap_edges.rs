#![no_main]

//! Fuzz target: trap-edge semantics with a self-oracle (issue #21).
//!
//! Builds tiny single-function modules around trap-prone operations
//! (division by zero, INT_MIN ÷ -1, float-to-int truncation out of range,
//! OOB loads at exact boundaries, call_indirect past the table) with
//! fuzzed operands, executes them, and compares the outcome against an
//! oracle computed in Rust: the right VALUE when no trap applies, the
//! right TRAP when it does. This is stronger than the no-panic invariant
//! of the other targets — it pins trap behavior, not just robustness.

use baedeker_core::binary::module::Module;
use baedeker_core::runtime::{RuntimeErrorKind, RuntimeTrap, Store, Value, execute_export};
use libfuzzer_sys::fuzz_target;

/// Minimal wasm encoder for the fixed module shapes below.
fn module(body: &[u8], params: u8, results: &[u8], with_mem: bool) -> Vec<u8> {
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    // type section: one func type
    let mut ty = vec![0x60, params];
    ty.extend(std::iter::repeat_n(0x7f, params as usize)); // i32 params
    if params == 1 {
        // caller patches the actual param type below via body prefix
    }
    ty.push(results.len() as u8);
    ty.extend_from_slice(results);
    section(&mut out, 1, &{
        let mut v = vec![1];
        v.extend_from_slice(&ty);
        v
    });
    // function section
    section(&mut out, 3, &[1, 0]);
    if with_mem {
        section(&mut out, 5, &[1, 0, 1]); // memory min 1 page
    }
    // export "f" func 0
    section(&mut out, 7, &[1, 1, b'f', 0, 0]);
    // code section
    let mut code = vec![1];
    code.push(body.len() as u8 + 1); // body size = locals(0) + body
    code.push(0); // no locals
    code.extend_from_slice(body);
    section(&mut out, 10, &code);
    out
}

fn section(out: &mut Vec<u8>, id: u8, data: &[u8]) {
    out.push(id);
    out.extend_from_slice(&leb(data.len() as u32));
    out.extend_from_slice(data);
}

fn leb(mut v: u32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
    out
}

#[derive(Debug, PartialEq)]
enum Expected {
    Value(Value),
    Trap(&'static str),
}

fn execute(bytes: &[u8], args: &[Value]) -> Result<Vec<Value>, RuntimeTrap> {
    let module = Module::decode(bytes).map_err(|_| RuntimeTrap::Unreachable)?;
    let lowered = module.lower().map_err(|_| RuntimeTrap::Unreachable)?;
    let store = Store::instantiate(&lowered).map_err(|_| RuntimeTrap::Unreachable)?;
    store.set_fuel(Some(10_000));
    execute_export(&lowered, &store, "f", args).map_err(|e| match e.kind {
        RuntimeErrorKind::Trap(trap) => trap,
        _ => RuntimeTrap::Unreachable,
    })
}

fn check(expected: Expected, bytes: &[u8], args: &[Value]) {
    match (execute(bytes, args), &expected) {
        (Ok(values), Expected::Value(expected)) => assert_eq!(values, [*expected]),
        (Err(trap), Expected::Trap(message)) => assert_eq!(trap.wast_message(), *message),
        (got, expected) => panic!("oracle {expected:?} but execution gave {got:?}"),
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 17 {
        return;
    }
    let op = data[0] % 6;
    let a32 = i32::from_le_bytes(data[1..5].try_into().unwrap());
    let b32 = i32::from_le_bytes(data[5..9].try_into().unwrap());
    let a64 = i64::from_le_bytes(data[9..17].try_into().unwrap());

    match op {
        // i32.div_s / i32.rem_s with oracle semantics.
        0 | 1 => {
            let (opcode, name, oracle): (u8, _, fn(i32, i32) -> Option<i32>) = if op == 0 {
                (0x6D, "div", |a, b| a.checked_div(b))
            } else {
                (0x6F, "rem", |a, b| a.checked_rem(b).or(Some(0)))
            };
            let body = [0x20, 0x00, 0x20, 0x01, opcode, 0x0B];
            let bytes = module(&body, 2, &[0x7f], false);
            let expected = match oracle(a32, b32) {
                _ if op == 0 && b32 == 0 => Expected::Trap("integer divide by zero"),
                _ if op == 0 && a32 == i32::MIN && b32 == -1 => Expected::Trap("integer overflow"),
                _ if b32 == 0 => Expected::Trap("integer divide by zero"),
                Some(v) => Expected::Value(Value::I32(v)),
                None => Expected::Trap("integer divide by zero"),
            };
            let _ = name;
            check(expected, &bytes, &[Value::I32(a32), Value::I32(b32)]);
        }
        // i64.div_s.
        2 => {
            let b64 = a64.wrapping_mul(0x2545_F491_4F6C_DD1D);
            // i64.div_s = 0x7F.
            let body = [0x20, 0x00, 0x20, 0x01, 0x7F, 0x0B];
            let mut bytes = module(&body, 2, &[0x7e], false);
            // Patch both param types (i32 -> i64): bytes 13 and 14.
            bytes[13] = 0x7e;
            bytes[14] = 0x7e;
            let expected = if b64 == 0 {
                Expected::Trap("integer divide by zero")
            } else if a64 == i64::MIN && b64 == -1 {
                Expected::Trap("integer overflow")
            } else {
                Expected::Value(Value::I64(a64 / b64))
            };
            check(expected, &bytes, &[Value::I64(a64), Value::I64(b64)]);
        }
        // i32.trunc_f32_s: NaN and out-of-range trap.
        3 => {
            let f = f32::from_bits(a32 as u32);
            let body = [0x20, 0x00, 0xA8, 0x0B];
            let mut bytes = module(&body, 1, &[0x7f], false);
            // Patch the param type (i32 -> f32) in the type section:
            // header(8) + section id/size(2) + count(1) + 0x60(1) +
            // params count(1) = byte 13.
            bytes[13] = 0x7d;
            let expected = if f.is_nan() {
                Expected::Trap("invalid conversion to integer")
            } else if f >= 2147483648.0 || f < -2147483648.0 {
                Expected::Trap("integer overflow")
            } else {
                Expected::Value(Value::I32(f as i32))
            };
            check(expected, &bytes, &[Value::F32(f)]);
        }
        // i32.load at exact page boundaries (1-page memory, data pattern).
        4 => {
            let addr = (a32 as u32) % 65540;
            let body = [0x20, 0x00, 0x28, 0x02, 0x00, 0x0B];
            let mut bytes = module(&body, 1, &[0x7f], true);
            // data section: active at 0, bytes 01 02 03 04
            section(&mut bytes, 11, &[1, 0, 0x41, 0, 0x0B, 4, 1, 2, 3, 4]);
            let expected = if addr + 4 <= 65536 {
                // Reads the data pattern for addr < 4, zeros elsewhere.
                let mut buf = [0u8; 4];
                for (i, slot) in buf.iter_mut().enumerate() {
                    let p = addr as usize + i;
                    *slot = if p < 4 { [1, 2, 3, 4][p] } else { 0 };
                }
                Expected::Value(Value::I32(i32::from_le_bytes(buf)))
            } else {
                Expected::Trap("out of bounds memory access")
            };
            check(expected, &bytes, &[Value::I32(addr as i32)]);
        }
        // call_indirect past a 2-entry table.
        _ => {
            let idx = (a32 as u32) % 4;
            // (type $t (func)) (type $u (func (param i32)))
            // (table 2 funcref) (elem (i32.const 0) $g)
            // (func $g (type $t)) (func (export "f") (type $u) local.get 0 call_indirect $t)
            let mut out = b"\0asm\x01\0\0\0".to_vec();
            section(&mut out, 1, &[2, 0x60, 0, 0, 0x60, 1, 0x7f, 0]);
            section(&mut out, 3, &[2, 0, 1]);
            section(&mut out, 4, &[1, 0x70, 0, 2]);
            section(&mut out, 7, &[1, 1, b'f', 0, 1]);
            section(&mut out, 9, &[1, 0, 0x41, 0, 0x0B, 1, 0]);
            let body_g = [0x0B];
            let body_f = [0x20, 0x00, 0x11, 0x00, 0x00, 0x0B];
            let mut code = vec![2];
            for body in [&body_g[..], &body_f[..]] {
                code.push(body.len() as u8 + 1);
                code.push(0);
                code.extend_from_slice(body);
            }
            section(&mut out, 10, &code);
            let expected = if idx >= 2 {
                Expected::Trap("undefined element")
            } else if idx == 1 {
                Expected::Trap("uninitialized element")
            } else {
                Expected::Value(Value::I32(0)) // hmm: no results; see below
            };
            if idx == 0 {
                // The callee returns nothing; success is an empty vec.
                match execute(&out, &[Value::I32(idx as i32)]) {
                    Ok(v) => assert!(v.is_empty()),
                    Err(trap) => panic!("expected success, trapped {trap:?}"),
                }
            } else {
                check(expected, &out, &[Value::I32(idx as i32)]);
            }
        }
    }
});
