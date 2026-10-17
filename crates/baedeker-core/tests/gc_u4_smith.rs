// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 4: the R1 wasm-smith smoke shape.
//!
//! The fuzz target (`fuzz/fuzz_targets/smith_module.rs`, feature `gc-smith`)
//! generates GC type sections and a conservative struct/array/i31 instruction
//! subset. Its invariant is decode + validate + lower acceptance with no
//! panic — no execution differential in this slice. This test runs the same
//! generator shape for a fixed seed set so the property is enforced in CI,
//! where the cargo-fuzz toolchain is not run.
//!
//! Review r1 P2-7: the campaign used to skip every VALIDATION rejection
//! silently — `validated` and `lowered` only counted the accepts, so 54 of the
//! 400 seeds (fresh campaigns: 56/55/58/46/56) could be false rejections of
//! valid modules and the "acceptance" gate would still be green. The
//! classification showed they were NOT generator configurations we decline:
//! every rejected module is VALID per an independent validator (wasmparser at
//! this pin), and all 54 shared one cause — the validator reset its
//! stack-polymorphic (unreachable) state to reachable at every `end`, so an
//! enclosing unreachable frame's polymorphic operand stack disappeared after a
//! nested block closed. The error classes were `ControlResultTypeMismatch`
//! (12 seeds; 8 of them with empty `expected`/`found`, the mangled stack
//! dropping below the frame's outer height), `StackUnderflow` (21 seeds; a pop
//! that must be `Bot` reported as a concrete underflow) and `TypeMismatch`
//! (21 seeds; a concrete leftover type checked where the frame was
//! polymorphic).
//!
//! The fix makes unreachability per frame (`push_ctrl` marks the new block
//! reachable, `end` restores the enclosing frame's state), which is the
//! pinned algorithm's behaviour, and the campaign now asserts FULL acceptance:
//! every generated module that decodes validates and lowers. No rejection
//! ledger is needed because there are no rejections left.

use arbitrary::Unstructured;
use baedeker_core::binary::module::Module;
use baedeker_core::types::{CompType, ConcreteKind};
use wasm_smith::{Config, InstructionKind, InstructionKinds};

/// The GC-smith configuration: mirror of the `gc-smith` fuzz-target config.
#[allow(clippy::field_reassign_with_default)]
fn config() -> Config {
    let mut config = Config::default();
    config.allowed_instructions = InstructionKinds::new(&[
        InstructionKind::NumericInt,
        InstructionKind::Parametric,
        InstructionKind::Variable,
        InstructionKind::Control,
        InstructionKind::Reference,
        InstructionKind::Aggregate,
    ]);
    config.bulk_memory_enabled = true;
    config.multi_value_enabled = true;
    config.simd_enabled = false;
    config.reference_types_enabled = true;
    config.tail_call_enabled = false;
    config.threads_enabled = false;
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

/// Deterministic byte stream for a seed (no external RNG dependency).
fn seed_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

/// One seeded campaign: every generated module decodes, and every decoded
/// module validates and lowers without panicking. GC type sections must
/// actually appear. Validation acceptance is TOTAL — a rejection is a hard
/// failure naming the seed and error kind, never a silent skip.
#[test]
fn seeded_gc_smith_campaign_decodes_validates_and_lowers() {
    let mut generated = 0usize;
    let mut decoded = 0usize;
    let mut validated = 0usize;
    let mut lowered = 0usize;
    let mut with_gc_types = 0usize;
    let mut decode_failures: Vec<(u64, String)> = Vec::new();
    let mut validation_failures: Vec<(u64, String)> = Vec::new();

    for seed in 0..400u64 {
        let bytes = seed_bytes(seed, 2048);
        let mut unstructured = Unstructured::new(&bytes);
        let Ok(smith_module) = wasm_smith::Module::new(config(), &mut unstructured) else {
            continue;
        };
        generated += 1;
        let encoded = smith_module.to_bytes();

        let module = match Module::decode(&encoded) {
            Ok(module) => module,
            // A decode rejection is not a panic; wasm-smith output should
            // decode, so every occurrence is recorded (and asserted to stay a
            // small, known-issue set) rather than hidden.
            Err(error) => {
                decode_failures.push((seed, error.to_string()));
                continue;
            }
        };
        decoded += 1;
        if module
            .types
            .iter()
            .any(|sub| matches!(sub.comp, CompType::Struct(_) | CompType::Array(_)))
        {
            with_gc_types += 1;
        }
        match module.validate() {
            Ok(()) => {}
            Err(error) => {
                validation_failures.push((seed, format!("{:?}", error.kind)));
                continue;
            }
        }
        validated += 1;
        module.lower().unwrap_or_else(|error| {
            panic!("seed {seed}: validated module failed to lower: {error:?}")
        });
        lowered += 1;
    }

    assert!(
        generated > 100,
        "generator produced only {generated} modules"
    );
    assert!(
        with_gc_types > 0,
        "no generated module carried a GC type section"
    );
    assert!(
        validation_failures.is_empty(),
        "validation acceptance is total: every decoded module must validate, \
         these did not (false rejections are validator defects, not \
         unsupported generator configurations): {validation_failures:?}"
    );
    assert_eq!(
        validated, lowered,
        "every validated module must lower (no panics, no false rejections)"
    );
    assert_eq!(
        validated, decoded,
        "every decoded module must validate (full acceptance)"
    );
    // Known decoder gap (recorded for unit 5): the data-section offset
    // expression parser scans for the first `0x0b` byte instead of decoding
    // instructions, so an `i32.const` whose LEB128 payload contains `0x0b`
    // truncates the expression. Only a few seeds hit it; the campaign must
    // not regress beyond that.
    assert!(
        decode_failures.len() <= 8,
        "decode rejections grew beyond the known offset-expression gap: {decode_failures:?}"
    );
    println!(
        "gc-smith smoke: generated={generated} decoded={decoded} with_gc_types={with_gc_types} \
         validated={validated} lowered={lowered} decode_failures={} validation_failures={}",
        decode_failures.len(),
        validation_failures.len()
    );
}

/// The decoder/validator/lowerer accept GC aggregate types end to end on a
/// hand-written module (the smoke campaign's kernel, kept explicit).
#[test]
fn hand_written_gc_surface_lowers() {
    let wat = r#"
(module
  (rec (type $node (sub (struct (field i32) (field (ref null $node)))))
       (type $pair (sub (array (ref null $node)))))
  (func (export "build") (result (ref null $node))
    i32.const 1
    ref.null $node
    struct.new $node)
  (func (export "i31") (result i32)
    i32.const 3
    ref.i31
    i31.get_s))"#;
    let buf = wast::parser::ParseBuffer::new(wat).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::decode(&bytes).unwrap();

    let kinds: Vec<ConcreteKind> = module
        .types
        .iter()
        .map(|sub| match &sub.comp {
            CompType::Struct(_) => ConcreteKind::Struct,
            CompType::Array(_) => ConcreteKind::Array,
            CompType::Func(_) => ConcreteKind::Func,
        })
        .collect();
    assert!(kinds.contains(&ConcreteKind::Struct));
    assert!(kinds.contains(&ConcreteKind::Array));

    module.validate().expect("GC module validates");
    module.lower().expect("GC module lowers");
}

/// Review r1 P2-7 root cause, pinned as a hand-written regression: stack
/// polymorphism is PER FRAME (`push_ctrl` marks the new block reachable, the
/// enclosing unreachable state is restored at `end`).
///
/// `(unreachable) (block) (drop)` is valid — the enclosing function frame is
/// still unreachable after the empty block closes, so `drop`'s operand is
/// `Bot`. Before the fix, `end` reset the validator to reachable and the
/// campaign's rejections appeared as `StackUnderflow { op: "drop",
/// available: [] }` (18 of 54 seeds).
#[test]
fn unreachable_state_is_restored_after_a_nested_block() {
    fn validate(source: &str) -> Result<(), String> {
        let buf = wast::parser::ParseBuffer::new(source).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        let module = Module::decode(&bytes).unwrap();
        module
            .validate()
            .map_err(|error| format!("{:?}", error.kind))
    }

    // Valid: the polymorphic enclosing frame resumes after the block.
    validate("(module (func (result i32) unreachable (block) drop i32.const 7))")
        .expect("an enclosing unreachable frame keeps its polymorphic stack");
    validate("(module (func unreachable (block) i64.const 5 drop i32.const 7 drop))")
        .expect("concrete values pushed after the block pop concretely");
    // Valid: `unreachable` itself is stack-polymorphic, so an `if` condition
    // is supplied by its polymorphic output.
    validate("(module (func unreachable if end))").expect("unreachable supplies the if condition");

    // Invalid: a block entered in unreachable code is validated as a
    // REACHABLE block, so its own operand floor is not polymorphic.
    validate("(module (func unreachable (block i32.add drop)))")
        .expect_err("a block body's pops must not inherit the enclosing polymorphism");
    // Invalid: a block body must leave exactly its declared results.
    validate("(module (func unreachable (block i32.const 1 end)))")
        .expect_err("a block body may not leave extra operands");
}
