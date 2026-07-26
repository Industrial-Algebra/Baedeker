#![no_main]

//! Fuzz target: decode → validate → lower on arbitrary bytes.
//!
//! Same pipeline as `decode` but the corpus focus is structurally-plausible
//! modules (seeded from valid fixtures), driving validator and lowering
//! coverage. Invariant: no panics, no matter how malformed the input.

use baedeker_core::binary::module::Module;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(module) = Module::decode(data) else {
        return;
    };
    if module.validate().is_ok() {
        let _ = module.lower();
    }
});
