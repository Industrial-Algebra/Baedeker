#![no_main]

//! Fuzz target: tail-call stack safety on the trampoline.
//!
//! Builds a self-recursive `return_call` module at a fuzzed depth (1 to
//! 100_000 — far beyond MAX_CALL_DEPTH) and asserts it completes with the
//! base-case value instead of trapping `CallStackExhausted`. Invariant:
//! tail calls must not grow the call stack. Where a call mirrors existing
//! API usage, copy the exact form from `validate_lower.rs` / the core
//! integration tests.

use baedeker_core::binary::module::Module;
use baedeker_core::runtime::{Store, Value, execute_export};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|depth: u32| {
    let depth = (depth % 100_000) + 1;
    let wat = format!(
        "(module (func $run (export \"run\") (param $n i32) (result i32) \
           (if (result i32) (i32.eqz (local.get $n)) \
             (then (i32.const 42)) \
             (else (return_call $run (i32.sub (local.get $n) (i32.const 1)))))))"
    );
    let buffer = wast::parser::ParseBuffer::new(&wat).expect("wat parses");
    let mut parsed = wast::parser::parse::<wast::Wat>(&buffer).expect("wat parses");
    let bytes = parsed.encode().expect("wat encodes");
    let module = Module::decode(&bytes).expect("module decodes");
    module.validate().expect("module validates");
    let reg = module.lower().expect("module lowers");
    let store = Store::instantiate(&reg).expect("module instantiates");
    let result = execute_export(&reg, &store, "run", &[Value::I32(depth as i32)])
        .expect("tail recursion completes without stack exhaustion");
    assert_eq!(result, vec![Value::I32(42)]);
});
