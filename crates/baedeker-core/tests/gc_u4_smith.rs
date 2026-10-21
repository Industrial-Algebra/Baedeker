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
//!
//! Review r2 P1: the stack-compatibility helper behind both `end`/`else` result
//! checks (`pop_control_result_types`) and branch checks (`pop_branch_types`)
//! read the ENTIRE operand vector. An enclosing frame's operand therefore
//! matched a result type the current frame had not produced, so partially
//! satisfied polymorphic results in an unreachable block were rejected:
//!
//! ```wat
//! (module (func
//!   i32.const 1
//!   block (result i64 f32)
//!     unreachable
//!     f32.const 0
//!   end
//!   drop drop drop))
//! ```
//!
//! is valid (the missing `i64` is polymorphic) but reported
//! `ControlResultTypeMismatch { expected: [I64, F32], found: [I32, F32] }` — the
//! enclosing `i32` consumed as the block's missing result. `ensure_stack_types`
//! is now frame-local (operands at or above the current frame's `outer_height`),
//! matching `ensure_frame_end_types`, and the fresh seeds below are pinned in
//! the campaign's fixed seed set.

use arbitrary::Unstructured;
use baedeker_core::binary::module::Module;
use baedeker_core::types::{CompType, ConcreteKind};
use baedeker_core::validate::error::ValidationErrorKind;
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

    // The fixed seed set: the original 0..400 campaign plus the three fresh
    // wasm-smith seeds review r2 surfaced. All three are independently VALID
    // per wasmparser at this pin and used to be rejected by the whole-stack
    // polymorphic result check (`ControlResultTypeMismatch` with an enclosing
    // frame's operand in `found`).
    for seed in (0..400u64).chain([4097, 10158, 10334]) {
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
    // Decode acceptance is TOTAL since unit 5: the data-section offset
    // expression decoder used to scan for the first `0x0b` byte instead of
    // decoding instructions, truncating any expression whose operands contain
    // `0x0b` (seeds 36/89/275 all failed this way with `data section:
    // unexpected end of input`). The data and element sections now share
    // `instr::decode_const_expr_bytes`, so a decode rejection is a defect.
    assert!(
        decode_failures.is_empty(),
        "every generated module must decode: {decode_failures:?}"
    );
    assert_eq!(
        decoded, generated,
        "every generated module must decode (no decoder rejections)"
    );
    println!(
        "gc-smith smoke: generated={generated} decoded={decoded} with_gc_types={with_gc_types} \
         validated={validated} lowered={lowered} decode_failures={} validation_failures={}",
        decode_failures.len(),
        validation_failures.len()
    );
    println!("decode failures: {decode_failures:?}");
}

/// The three seeds the U4 campaign recorded as decode failures
/// (`data section: unexpected end of input`) now decode: each one's data
/// section carries an offset/init expression whose operand bytes include
/// `0x0b`, which the old first-`0x0b` scan truncated.
#[test]
fn u4_decode_failure_seeds_now_decode() {
    for seed in [36u64, 89, 275] {
        let bytes = seed_bytes(seed, 2048);
        let mut unstructured = Unstructured::new(&bytes);
        let Ok(smith_module) = wasm_smith::Module::new(config(), &mut unstructured) else {
            panic!("seed {seed} no longer generates a module");
        };
        let encoded = smith_module.to_bytes();
        let module = Module::decode(&encoded)
            .unwrap_or_else(|error| panic!("seed {seed} must decode: {error}"));
        assert!(
            !module.data.is_empty(),
            "seed {seed} was a DATA-section decode failure; the module must still carry data"
        );
    }
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

/// Review r2 P1 root cause, pinned as hand-written regressions: the
/// stack-compatibility check for a frame's results/branch types is
/// FRAME-LOCAL. Operands below the current frame's `outer_height` belong to
/// enclosing frames and are never candidates for this frame's results;
/// operands the current frame has not produced stay polymorphic while the
/// frame is unreachable.
#[test]
fn polymorphic_result_checks_are_frame_local() {
    fn validate(source: &str) -> Result<(), String> {
        let buf = wast::parser::ParseBuffer::new(source).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        let module = Module::decode(&bytes).unwrap();
        module
            .validate()
            .map_err(|error| format!("{:?}", error.kind))
    }

    fn validate_and_lower(source: &str) {
        let buf = wast::parser::ParseBuffer::new(source).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        let bytes = wat.encode().unwrap();
        let module = Module::decode(&bytes).unwrap();
        module
            .validate()
            .unwrap_or_else(|error| panic!("{:?} must validate: {error:?}", error.kind));
        module.lower().expect("a valid module must lower");
    }

    // The reviewer's minimized case: the block's missing `i64` is polymorphic
    // (its body is unreachable) and the enclosing `i32` must not be consumed
    // as it. Valid in wasmparser at this pin.
    validate_and_lower(
        r#"(module (func
          i32.const 1
          block (result i64 f32)
            unreachable
            f32.const 0
          end
          drop drop drop))"#,
    );

    // The same shape on the BRANCH path (`pop_branch_types` shares the helper):
    // `br 0` needs `[i64 f32]`, the frame supplied only `f32`, and the missing
    // `i64` is polymorphic.
    validate_and_lower(
        r#"(module (func
          i32.const 1
          block (result i64 f32)
            unreachable
            f32.const 0
            br 0
          end
          drop drop drop))"#,
    );

    // Frame locality is not a licence to accept anything: a concrete
    // frame-local operand of the wrong type is still a mismatch.
    validate(
        r#"(module (func
          i32.const 1
          block (result i64 f32)
            unreachable
            i64.const 0
          end
          drop drop drop))"#,
    )
    .expect_err("the frame-local operand must match the expected result type");
    // ...and a still-reachable frame owes every result concretely.
    validate(
        r#"(module (func
          i32.const 1
          block (result i64 f32)
            i64.const 0
          end
          drop drop drop))"#,
    )
    .expect_err("a reachable frame must supply all of its results");
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

/// Review r3 P2 root cause, pinned as hand-written regressions: operand pops
/// are bounded by the CURRENT frame's floor.
///
/// The r2 fix made the frame-END and BRANCH type checks frame-local, but the
/// individual instruction pops still popped the shared operand vector
/// directly. A REACHABLE frame could therefore consume an enclosing frame's
/// operand whenever a later push restored the stack length before the frame
/// closed:
///
/// ```wat
/// (module (func i32.const 7 i32.const 8 block drop i32.const 9 end drop drop))
/// ```
///
/// The block's floor is 2, its `drop` popped `vec[1]` (an operand of the
/// enclosing function frame), `i32.const 9` restored the length to 2, and
/// `end`'s frame-local result check (empty frame-local slice, empty results)
/// passed — so an invalid module was accepted. wasmparser at this pin rejects
/// it: "type mismatch: expected a type but nothing on stack" at the inner
/// `drop`.
///
/// The spec's validation algorithm (appendix) compares the operand stack
/// height against the current control frame's height:
///
/// ```text
/// func pop_val() : val_type =
///   if (vals.size() = ctrls[0].val_height && ctrls[0].unreachable) return Bot
///   error_if(vals.size() = ctrls[0].val_height)
///   return vals.pop()
/// ```
///
/// (verbatim from `document/core/appendix/algorithm.rst:181-185` at this
/// pin — the interpreter's `valid.ml` implements the same rule inline
/// with frame-local checking but defines no `pop_opd`; the earlier
/// attribution here was wrong, caught in review r4)
///
/// — a reachable frame at its floor has nothing to pop (an error naming that
/// floor's emptiness), while an unreachable (polymorphic) frame at its floor
/// yields `Bot` and may pop any number of operands. `pop_operand` now applies
/// exactly that rule; every pop path (`pop_any`, `pop_expect`, `pop_exact`,
/// `pop_operand_type`, `pop_ref_type`, the indexed/ref pops) funnels through
/// it, and it is the only site that pops the operand vector.
#[test]
fn operand_pops_respect_the_current_frame_floor() {
    fn encode(source: &str) -> Vec<u8> {
        let buf = wast::parser::ParseBuffer::new(source).unwrap();
        let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
        wat.encode().unwrap()
    }

    /// Our verdict (lowers accepted modules so the whole pipeline is covered).
    fn ours(source: &str) -> Result<(), baedeker_core::validate::error::ValidationError> {
        let bytes = encode(source);
        let module = Module::decode(&bytes).unwrap();
        module.validate()?;
        module.lower().unwrap_or_else(|error| {
            panic!("a validated module must lower: {error:?} for {source}")
        });
        Ok(())
    }

    /// The independent oracle at this pin: wasmparser via Wasmtime's
    /// validator. Rejections here are the authority for "this module is
    /// invalid", so every rejection case below is double-checked.
    fn pin_rejects(source: &str) -> bool {
        let bytes = encode(source);
        let engine = wasmtime::Engine::default();
        wasmtime::Module::validate(&engine, &bytes).is_err()
    }

    /// A reachable frame at its floor: rejected, naming the floor's emptiness
    /// (the enclosing frames' operands are not "available" to this frame).
    fn expect_floor_rejection(source: &str, op: &'static str) {
        assert!(
            pin_rejects(source),
            "the pin must reject this module: {source}"
        );
        let error = ours(source).err().unwrap_or_else(|| {
            panic!("a pop below a reachable frame floor must be rejected: {source}")
        });
        match error.kind {
            ValidationErrorKind::StackUnderflow {
                op: found_op,
                ref available,
                ..
            } => {
                assert_eq!(
                    found_op, op,
                    "the underflow must name the offending op: {source}"
                );
                assert!(
                    available.is_empty(),
                    "the frame floor is empty: enclosing operands must not be reported as \
                     available ({available:?}) for {source}"
                );
            }
            other => panic!("expected a frame-floor StackUnderflow, found {other:?} for {source}"),
        }
    }

    /// A module the pin also accepts: accepted (and lowered) by ours.
    fn expect_accept(source: &str) {
        assert!(
            !pin_rejects(source),
            "the pin must accept this module: {source}"
        );
        ours(source).unwrap_or_else(|error| {
            panic!("a valid module must be accepted: {error:?} for {source}")
        });
    }

    // (a) The reviewer's escaping module: the inner `drop` sits exactly on the
    // block's floor while reachable, and `i32.const 9` restores the length so
    // every frame-end check still passes.
    expect_floor_rejection(
        "(module (func i32.const 7 i32.const 8 block drop i32.const 9 end drop drop))",
        "drop",
    );

    // (a2) The same below-floor pop through a TYPED pop: `i32.add`'s first
    // operand would be the enclosing `i64`. Before the floor check that leaked
    // operand surfaced as a downstream `TypeMismatch` (the enclosing `i64` read
    // as an `i32` operand); it is a frame-floor underflow at the offending pop.
    expect_floor_rejection(
        "(module (func i64.const 1 block i32.add i32.const 0 end drop))",
        "i32.binary",
    );

    // (b) Unreachable (polymorphic) frames may pop below their floor — the
    // appendix algorithm's `pop_val` yields `Bot` at the floor and never reaches into an
    // enclosing frame. Both engines accept these.
    expect_accept("(module (func block unreachable drop drop drop end))");
    expect_accept("(module (func i32.const 1 block unreachable drop drop end drop))");
    expect_accept("(module (func block unreachable drop drop drop i32.const 1 drop end))");

    // (c) Positive controls at floor boundaries: pops of frame-local operands
    // stay legal, in both arms of an `if` and across `br` targets.
    expect_accept("(module (func i32.const 1 block i32.const 2 drop end drop))");
    expect_accept(
        "(module (func i32.const 1 i32.const 2 if i32.const 3 drop else i32.const 4 drop end drop))",
    );
    expect_accept(
        "(module (func (result i32) i32.const 1 if (result i32) i32.const 2 else i32.const 3 end))",
    );
    expect_accept("(module (func (result i32) block (result i32) i32.const 1 br 0 end))");
    expect_accept("(module (func i32.const 1 i32.const 2 block i32.const 3 br 0 end drop drop))");
    expect_accept("(module (func i32.const 1 loop i32.const 2 drop br 0 end drop))");

    // (d) Negative controls at the same boundaries: the then-arm, the else-arm
    // and a `br` target each leak one enclosing operand and restore the length
    // before the frame closes, which is exactly what the floor check must
    // catch.
    expect_floor_rejection(
        "(module (func i32.const 1 i32.const 2 if drop i32.const 5 else end drop))",
        "drop",
    );
    expect_floor_rejection(
        "(module (func i32.const 1 i32.const 2 if else drop i32.const 6 end drop))",
        "drop",
    );
    expect_floor_rejection(
        "(module (func i32.const 1 i32.const 2 block drop i32.const 9 br 0 end drop drop))",
        "drop",
    );
}
