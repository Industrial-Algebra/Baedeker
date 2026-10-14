// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! EH unit 2: `try_table` / `throw` / `throw_ref` validation fixtures.
//!
//! Runs the `eh-validation` spec-text corpus through decode + validation.
//! Modules free of tag imports additionally run the full lower + instantiate
//! pipeline (EH instructions lower and execute as of unit 3); modules that
//! import tags stop after validation because tag linking lands in unit 4.

use std::path::Path;

use baedeker_core::binary::module::Module;
use baedeker_core::runtime::Store;
use baedeker_core::types::ImportDesc;
use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastDirective};

#[derive(Debug, Default)]
struct EhValidationStats {
    files: usize,
    directives: usize,
    modules: usize,
    assertions: usize,
    validated_only: usize,
}

fn encode_wat(mut wat: QuoteWat<'_>) -> Result<Vec<u8>, String> {
    wat.encode().map_err(|error| error.to_string())
}

/// Whether a module imports a tag (tag imports cannot link until unit 4).
fn module_imports_tag(module: &Module<'_>) -> bool {
    module
        .imports()
        .iter()
        .any(|import| matches!(import.desc, ImportDesc::Tag(_)))
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

fn run_eh_validation_case(path: &Path, stats: &mut EhValidationStats) {
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
                if module_imports_tag(&module) {
                    stats.validated_only += 1;
                } else {
                    let lowered = module.lower().unwrap_or_else(|error| {
                        panic!("{}: expected module to lower: {error:?}", path.display())
                    });
                    Store::instantiate(&lowered).unwrap_or_else(|error| {
                        panic!(
                            "{}: expected module to instantiate: {error:?}",
                            path.display()
                        )
                    });
                }
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

fn run_eh_validation(subdir: &str) -> EhValidationStats {
    let mut total = EhValidationStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        total.files += 1;
        run_eh_validation_case(&path, &mut total);
    }

    total
}

#[test]
fn eh_validation_cases() {
    let stats = run_eh_validation("eh-validation");
    println!(
        "eh validation summary: files={} directives={} modules={} assertions={} validated_only={}",
        stats.files, stats.directives, stats.modules, stats.assertions, stats.validated_only
    );

    assert_eq!(stats.files, 1, "expected exactly one EH validation fixture");
    assert_eq!(
        stats.directives, 18,
        "unexpected EH validation directive count"
    );
    assert_eq!(stats.modules, 3, "unexpected EH validation module count");
    assert_eq!(
        stats.assertions, 15,
        "unexpected EH validation assertion count"
    );
    assert_eq!(
        stats.validated_only, 1,
        "unexpected EH validated-only module count"
    );
}
