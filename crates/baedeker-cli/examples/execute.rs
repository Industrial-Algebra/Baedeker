// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! End-to-end: decode a `.wasm` binary, validate it, lower it to the
//! register IR, instantiate it, and execute an exported function.
//!
//! The embedded `add.wasm` exports a single function `add(i32, i32) -> i32`
//! that returns the sum of its arguments.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p baedeker-cli --example execute
//! ```

use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::{Store, Value, execute_export};

fn main() {
    // The four-stage pipeline every Baedeker run goes through:
    // decode -> validate -> lower (to register IR) -> instantiate.
    let bytes: &[u8] = include_bytes!("add.wasm");

    let module = Module::decode(bytes).expect("decode failed");
    module.validate().expect("validation failed");
    let reg = lower_module(&module).expect("lowering failed");
    let store = Store::instantiate(&reg).expect("instantiation failed");

    // Execute the exported `add` with two i32 arguments.
    let results = execute_export(&reg, &store, "add", &[Value::I32(20), Value::I32(22)])
        .expect("execution trapped");

    println!("add(20, 22) = {results:?}");
    assert_eq!(results, vec![Value::I32(42)]);
}
