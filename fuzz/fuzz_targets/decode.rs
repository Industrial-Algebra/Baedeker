#![no_main]

//! Fuzz target: raw bytes → `Module::decode`.
//!
//! Invariant: decoding arbitrary bytes must never panic. On successful
//! decode, the pipeline continues through validate + lower, so byte-level
//! mutations that slip past the decoder are exercised deeper too.

use baedeker_core::binary::module::Module;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(module) = Module::decode(data) {
        let _ = module.validate();
        let _ = module.lower();
    }
});
