// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Threads U3: shared memories on the NATIVE encoding path.
//!
//! Contract: `docs/plans/threads-unit3-contract.md` (r1). Pin:
//! `WebAssembly/threads@main-legacy` `33b2dcd6`.
//!
//! The threads corpus writes classic shared memories as `(memory 1 2 shared)`
//! and wast-240 encodes them natively: `MemoryType { shared }`
//! (`wast-240 src/core/types.rs:649+`) is parsed from the trailing `shared`
//! keyword and forwarded to the limits tag by the encoder
//! (`wast-240 src/core/binary.rs:515-524`: shared+max ⇒ tag `0x03`,
//! shared-without-max ⇒ tag `0x02`). No syntax transform is applied anywhere
//! in this harness; these regressions pin the resulting decode behaviour.

use baedeker_core::binary::module::Module;
use baedeker_core::types::ImportDesc;

fn encode(wat_source: &str) -> Vec<u8> {
    let buf = wast::parser::ParseBuffer::new(wat_source).expect("parse buffer");
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).expect("parse wat");
    wat.encode().expect("encode wat")
}

/// A defined shared memory decodes with `limits.shared == true` and keeps its
/// bounds — through encoding, decoding, validation and lowering.
#[test]
fn defined_shared_memory_decodes_shared() {
    let bytes = encode("(module (memory 1 2 shared))");
    let module = Module::decode(&bytes).expect("a defined shared memory decodes");
    assert_eq!(module.memories.len(), 1);
    let limits = module.memories[0].limits;
    assert!(limits.shared, "the `shared` keyword must reach the decoder");
    assert_eq!(limits.min, 1);
    assert_eq!(limits.max, Some(2));
    assert!(!limits.is64);
    module.validate().expect("a shared memory validates");
    module.lower().expect("a shared memory lowers");
}

/// The control: the SAME limits without the keyword stay unshared, so the
/// assertions above measure the keyword and not a default.
#[test]
fn defined_unshared_memory_decodes_unshared() {
    let bytes = encode("(module (memory 1 2))");
    let module = Module::decode(&bytes).expect("an unshared memory decodes");
    assert!(!module.memories[0].limits.shared);
}

/// An imported shared memory decodes with `limits.shared == true` (the import
/// section carries its own memtype production).
#[test]
fn imported_shared_memory_decodes_shared() {
    let bytes = encode(r#"(module (import "spectest" "memory" (memory 1 2 shared)))"#);
    let module = Module::decode(&bytes).expect("an imported shared memory decodes");
    assert_eq!(module.imports.len(), 1);
    let ImportDesc::Mem(mem) = &module.imports[0].desc else {
        panic!("the import must be a memory: {:?}", module.imports[0].desc);
    };
    assert!(mem.limits.shared, "the imported memtype must stay shared");
    assert_eq!(mem.limits.min, 1);
    assert_eq!(mem.limits.max, Some(2));
    module
        .validate()
        .expect("an imported shared memory validates");
    module.lower().expect("an imported shared memory lowers");
}

/// `memory.wast:12` at the pin:
///
/// ```text
/// (assert_invalid (module (memory 1 shared)) "shared memory must have maximum")
/// ```
///
/// The native encoder writes the shared-without-max limits tag `0x02`, and the
/// decoder refuses it with EXACTLY the corpus string (`memtype` requires a
/// maximum when shared), so the directive is satisfied by a decode refusal —
/// the mapping the official runner already applies to `assert_invalid`.
#[test]
fn shared_memory_without_maximum_is_refused_with_the_corpus_string() {
    let bytes = encode("(module (memory 1 shared))");
    // The memory section is the module's only section: id 0x05, size 0x03,
    // payload `count=1, limits tag=0x02 (shared, no max), min=1`.
    assert_eq!(
        &bytes[8..],
        &[0x05, 0x03, 0x01, 0x02, 0x01],
        "wast-240 must encode `shared`-without-max as limits tag 0x02"
    );

    let error = Module::decode(&bytes).expect_err("shared without max must not decode");
    assert_eq!(
        error.kind.to_string(),
        "shared memory must have maximum",
        "the refusal must be the corpus' exact assertion string"
    );

    // Same shape through the IMPORT section, which has its own memtype rule.
    let imported = encode(r#"(module (import "spectest" "memory" (memory 1 shared)))"#);
    let error = Module::decode(&imported)
        .expect_err("an imported shared memory without max is refused too");
    assert_eq!(error.kind.to_string(), "shared memory must have maximum");
}
