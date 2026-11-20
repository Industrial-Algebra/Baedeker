// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC unit 1: validation-mode runner for the type-grammar corpus.
//!
//! Runs the vendored `gc-validation` spec-text corpus (`type`, `type-rec`,
//! `type-canon`, `type-equivalence` from the reference at pin `dfa3f32`)
//! through decode + validation only. Runnable modules are validated but not
//! lowered or instantiated: the corpus exercises the type grammar and its
//! canonicalization, not execution, and GC execution lands in U2/U3.
//!
//! `assert_malformed` / `assert_invalid` are processed as rejections.
//! Directives that need linking or execution (`assert_unlinkable`,
//! `assert_return`, `assert_trap`, `register`, …) are counted and reported
//! as skips rather than silently ignored.

use std::path::Path;

use baedeker_core::binary::module::Module;
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastDirective};

#[derive(Debug, Default)]
struct GcValidationStats {
    files: usize,
    directives: usize,
    modules: usize,
    assertions: usize,
    skipped: usize,
    validated_only: usize,
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

fn decode_and_validate<'a>(path: &Path, bytes: &'a [u8]) -> Module<'a> {
    let module = Module::decode(bytes)
        .unwrap_or_else(|error| panic!("{}: expected module to decode: {error}", path.display()));
    module
        .validate()
        .unwrap_or_else(|error| panic!("{}: expected module to validate: {error}", path.display()));
    module
}

fn run_gc_validation_case(path: &Path, stats: &mut GcValidationStats) {
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
                decode_and_validate(path, &bytes);
                stats.validated_only += 1;
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
            WastDirective::AssertUnlinkable {
                module, message, ..
            } => {
                // The module is validation-valid; only link-time matching is
                // out of scope here.
                let mut module = module;
                let bytes = module.encode().unwrap_or_else(|error| {
                    panic!(
                        "{}: failed to encode unlinkable module ({message}): {error}",
                        path.display()
                    )
                });
                decode_and_validate(path, &bytes);
                stats.skipped += 1;
            }
            other => {
                eprintln!(
                    "gc validation {}: skipping {} directive",
                    path.display(),
                    directive_name(&other)
                );
                stats.skipped += 1;
            }
        }
    }
}

fn run_gc_validation(subdir: &str) -> GcValidationStats {
    let mut total = GcValidationStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        total.files += 1;
        run_gc_validation_case(&path, &mut total);
    }

    total
}

#[test]
fn gc_validation_cases() {
    let stats = run_gc_validation("gc-validation");
    println!(
        "gc validation summary: files={} directives={} modules={} assertions={} skipped={} validated_only={}",
        stats.files,
        stats.directives,
        stats.modules,
        stats.assertions,
        stats.skipped,
        stats.validated_only,
    );

    assert_eq!(stats.files, 4, "expected exactly four GC type fixtures");
    assert_eq!(stats.modules, 35, "unexpected GC validation module count");
    assert_eq!(
        stats.assertions, 13,
        "unexpected GC validation assertion count"
    );
    assert_eq!(stats.skipped, 16, "unexpected GC validation skip count");
    assert_eq!(
        stats.validated_only, 35,
        "invalid modules must not validate"
    );
}
