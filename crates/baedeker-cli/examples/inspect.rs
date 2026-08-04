// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Decode a `.wasm` binary and print a summary of its structure — the first
//! stage of the Baedeker pipeline (`decode`), without validating or executing.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p baedeker-cli --example inspect
//! ```

use baedeker_core::binary::module::Module;

fn main() {
    let bytes: &[u8] = include_bytes!("add.wasm");
    let module = Module::decode(bytes).expect("decode failed");

    println!("Baedeker decoded add.wasm:");
    println!("  types:     {}", module.types.len());
    println!("  functions: {}", module.functions.len());
    println!("  exports:   {}", module.exports.len());
    println!("  memories:  {}", module.memories.len());
    println!("  globals:   {}", module.globals.len());

    for export in &module.exports {
        println!("  export:    {}", export.name);
    }
}
