// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! memory64 unit 2: index-type-aware validation fixtures.
//!
//! Runs the `mem64-validation` spec-text corpus through decode + validation.
//! No module is lowered or instantiated: runtime/IR semantics for 64-bit
//! memories and tables land in unit 3, and some valid fixtures use enormous
//! limits that are only meaningful to the validator.

use std::path::Path;

use baedeker_core::binary::module::Module;
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastDirective};

#[derive(Debug, Default)]
struct Mem64ValidationStats {
    files: usize,
    directives: usize,
    modules: usize,
    assertions: usize,
}

fn encode_wat(mut wat: QuoteWat<'_>) -> Result<Vec<u8>, String> {
    wat.encode().map_err(|error| error.to_string())
}

fn directive_name(directive: &WastDirective<'_>) -> &'static str {
    match directive {
        WastDirective::Module(_) => "module",
        WastDirective::ModuleDefinition(_) => "module_definition",
        WastDirective::AssertMalformed { .. } => "assert_malformed",
        WastDirective::AssertInvalid { .. } => "assert_invalid",
        WastDirective::AssertTrap { .. } => "assert_trap",
        WastDirective::AssertReturn { .. } => "assert_return",
        WastDirective::AssertExhaustion { .. } => "assert_exhaustion",
        WastDirective::AssertUnlinkable { .. } => "assert_unlinkable",
        WastDirective::AssertException { .. } => "assert_exception",
        WastDirective::Register { .. } => "register",
        WastDirective::Invoke(_) => "invoke",
        WastDirective::Thread(_) => "thread",
        WastDirective::Wait { .. } => "wait",
        WastDirective::ModuleInstance { .. } => "module_instance",
        WastDirective::AssertSuspension { .. } => "assert_suspension",
    }
}

fn run_mem64_validation_case(path: &Path, stats: &mut Mem64ValidationStats) {
    let text = baedeker_testdata::spec_case_text(path);
    let buf = ParseBuffer::new(&text)
        .unwrap_or_else(|error| panic!("{}: failed to parse wast buffer: {error}", path.display()));
    let wast = parse::<Wast<'_>>(&buf).unwrap_or_else(|error| {
        panic!(
            "{}: failed to parse wast directives: {error}",
            path.display()
        )
    });

    for directive in wast.directives {
        stats.directives += 1;
        match directive {
            WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat) => {
                stats.modules += 1;
                let bytes = encode_wat(wat).unwrap_or_else(|error| {
                    panic!("{}: failed to encode module: {error}", path.display())
                });
                let module = Module::decode(&bytes).unwrap_or_else(|error| {
                    panic!("{}: expected module to decode: {error}", path.display())
                });
                module.validate().unwrap_or_else(|error| {
                    panic!("{}: expected module to validate: {error}", path.display())
                });
            }
            WastDirective::AssertInvalid {
                module, message, ..
            } => {
                stats.assertions += 1;
                let bytes = encode_wat(module).unwrap_or_else(|error| {
                    panic!(
                        "{}: failed to encode invalid module ({message}): {error}",
                        path.display()
                    )
                });
                let module = Module::decode(&bytes).unwrap_or_else(|error| {
                    panic!(
                        "{}: expected invalid module to decode before validation ({message}): {error}",
                        path.display()
                    )
                });
                if module.validate().is_ok() {
                    panic!(
                        "{}: expected invalid module to fail validation ({message})",
                        path.display()
                    );
                }
            }
            WastDirective::AssertMalformed {
                module, message, ..
            } => {
                stats.assertions += 1;
                if let Ok(bytes) = encode_wat(module)
                    && Module::decode(&bytes).is_ok()
                {
                    panic!(
                        "{}: expected malformed module to fail decode ({message})",
                        path.display()
                    );
                }
            }
            other => panic!(
                "{}: unsupported directive {}",
                path.display(),
                directive_name(&other)
            ),
        }
    }
}

fn run_mem64_validation(subdir: &str) -> Mem64ValidationStats {
    let mut total = Mem64ValidationStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        total.files += 1;
        run_mem64_validation_case(&path, &mut total);
    }

    total
}

#[test]
fn mem64_validation_cases() {
    let stats = run_mem64_validation("mem64-validation");
    println!(
        "mem64 validation summary: files={} directives={} modules={} assertions={}",
        stats.files, stats.directives, stats.modules, stats.assertions
    );

    assert_eq!(
        stats.files, 1,
        "expected exactly one mem64 validation fixture"
    );
    assert_eq!(
        stats.directives, 38,
        "unexpected mem64 validation directive count"
    );
    assert_eq!(
        stats.modules, 14,
        "unexpected mem64 validation module count"
    );
    assert_eq!(
        stats.assertions, 24,
        "unexpected mem64 validation assertion count"
    );
}

/// Imported tables must be limit-checked too: min > max on an i64 table
/// import is invalid even though no defined table exists (U2 disclosure 8).
#[test]
fn imported_table_min_exceeds_max_is_invalid() {
    let source = r#"(module (import "a" "t" (table i64 5 2 funcref)))"#;
    let buf = wast::parser::ParseBuffer::new(source).unwrap();
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buf).unwrap();
    let bytes = wat.encode().unwrap();
    let module = baedeker_core::binary::module::Module::decode(&bytes).unwrap();
    let error = module
        .validate()
        .expect_err("imported table with min > max must be invalid");
    assert!(
        format!("{error:?}").contains("TableMinExceedsMax"),
        "expected TableMinExceedsMax, got {error:?}"
    );
}
