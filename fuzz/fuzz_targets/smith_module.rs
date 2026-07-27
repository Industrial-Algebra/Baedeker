#![no_main]

//! Fuzz target: structured generation via wasm-smith.
//!
//! wasm-smith turns arbitrary bytes into *valid* WebAssembly modules
//! (within the configured proposal set), giving the decoder, validator,
//! and lowering pipeline deep structural coverage that byte-level mutation
//! rarely reaches. Invariant: no panics. Lowering failures are expected
//! for valid-but-unsupported shapes and are not errors.

use baedeker_core::binary::module::Module;
use libfuzzer_sys::fuzz_target;
use wasm_smith::Config;

fn config() -> Config {
    let mut config = Config::default();
    // Restrict to the proposal surface Baedeker supports.
    config.bulk_memory_enabled = true;
    config.multi_value_enabled = true;
    config.simd_enabled = true;
    config.reference_types_enabled = false;
    config.tail_call_enabled = false;
    config.threads_enabled = false;
    config.memory64_enabled = false;
    config.max_memories = 1;
    config.relaxed_simd_enabled = false;
    config.exceptions_enabled = false;
    config.gc_enabled = false;
    config.shared_everything_threads_enabled = false;
    config.custom_descriptors_enabled = false;
    config.custom_page_sizes_enabled = false;
    config.wide_arithmetic_enabled = false;
    config
}

fuzz_target!(|data: &[u8]| {
    let mut unstructured = arbitrary::Unstructured::new(data);
    let Ok(module) = wasm_smith::Module::new(config(), &mut unstructured) else {
        return;
    };
    let bytes = module.to_bytes();

    let Ok(module) = Module::decode(&bytes) else {
        return;
    };
    if module.validate().is_ok() {
        let _ = module.lower();
    }
});
