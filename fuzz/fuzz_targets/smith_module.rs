#![no_main]

//! Fuzz target: structured generation via wasm-smith.
//!
//! wasm-smith turns arbitrary bytes into *valid* WebAssembly modules
//! (within the configured proposal set), giving the decoder, validator,
//! and lowering pipeline deep structural coverage that byte-level mutation
//! rarely reaches. Invariant: no panics. Lowering failures are expected
//! for valid-but-unsupported shapes and are not errors.
//!
//! With the `gc-smith` feature the generator additionally emits GC type
//! sections (`struct`/`array`/rec groups) and a conservative instruction
//! subset — numeric, parametric, variable, control, reference, aggregate
//! (struct/array/i31) and memory-integer instructions, no vector kinds. That is
//! the R1 rung's robustness surface: decode + validate + lower acceptance with
//! no panic (there is no execution differential in this slice).
//!
//! The threads slice (unit 3) enables `threads_enabled` and admits
//! `InstructionKind::MemoryInt`, the bucket every atomic instruction sits in
//! (`wasm-smith src/core/code_builder.rs:597-664`), so generated modules carry
//! `0xFE` atomics; plain loads/stores come along with it. Nothing is executed
//! here, so no generated `atomic.wait` can block.

use baedeker_core::binary::module::Module;
use libfuzzer_sys::fuzz_target;
use wasm_smith::Config;

#[cfg(feature = "gc-smith")]
fn config() -> Config {
    use wasm_smith::{InstructionKind, InstructionKinds};

    let mut config = Config::default();
    // Conservative instruction subset: everything the GC and threads surfaces
    // touch, nothing that drags in vector pricing. Atomics are MemoryInt-bucketed
    // (`code_builder.rs:597-664`); MemoryInt also admits plain loads/stores.
    config.allowed_instructions = InstructionKinds::new(&[
        InstructionKind::NumericInt,
        InstructionKind::Parametric,
        InstructionKind::Variable,
        InstructionKind::Control,
        InstructionKind::Reference,
        InstructionKind::Aggregate,
        InstructionKind::MemoryInt,
    ]);
    config.bulk_memory_enabled = true;
    config.multi_value_enabled = true;
    config.simd_enabled = false;
    config.reference_types_enabled = true;
    config.tail_call_enabled = false;
    config.threads_enabled = true;
    config.memory64_enabled = false;
    config.max_memories = 1;
    config.relaxed_simd_enabled = false;
    config.exceptions_enabled = false;
    config.gc_enabled = true;
    config.shared_everything_threads_enabled = false;
    config.custom_descriptors_enabled = false;
    config.custom_page_sizes_enabled = false;
    config.wide_arithmetic_enabled = false;
    config.max_funcs = 8;
    config.max_types = 16;
    config.max_globals = 4;
    config.max_element_segments = 4;
    config.max_instructions = 32;
    config
}

#[cfg(not(feature = "gc-smith"))]
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
