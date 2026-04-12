use std::path::Path;

use baedeker_core::binary::module::Module;
use wast::parser::{parse, ParseBuffer};
use wast::{QuoteWat, Wast, WastDirective};

#[derive(Debug, Default, Clone)]
struct WastCaseStats {
    total: usize,
    modules: usize,
    malformed: usize,
    invalid: usize,
    unsupported: Vec<&'static str>,
}

fn encode_wat(mut wat: QuoteWat<'_>) -> Result<Vec<u8>, String> {
    wat.encode().map_err(|e| e.to_string())
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

fn run_wast_case(path: &Path) -> WastCaseStats {
    let text = baedeker_testdata::spec_case_text(path);
    let buf = ParseBuffer::new(&text)
        .unwrap_or_else(|e| panic!("{}: failed to parse wast buffer: {e}", path.display()));
    let wast = parse::<Wast<'_>>(&buf)
        .unwrap_or_else(|e| panic!("{}: failed to parse wast directives: {e}", path.display()));

    let mut stats = WastCaseStats::default();

    for directive in wast.directives {
        stats.total += 1;
        match directive {
            WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat) => {
                stats.modules += 1;
                let bytes = encode_wat(wat).unwrap_or_else(|e| {
                    panic!("{}: failed to encode valid module directive: {e}", path.display())
                });
                let module = Module::decode(&bytes).unwrap_or_else(|e| {
                    panic!("{}: expected valid module directive, got decode error: {e}", path.display())
                });
                module.validate().unwrap_or_else(|e| {
                    panic!("{}: expected valid module directive, got validation error: {e}", path.display())
                });
            }
            WastDirective::AssertMalformed { module, .. } => {
                stats.malformed += 1;
                if let Ok(bytes) = encode_wat(module)
                    && Module::decode(&bytes).is_ok()
                {
                    panic!("{}: expected malformed module to fail decode", path.display());
                }
            }
            WastDirective::AssertInvalid { module, .. } => {
                stats.invalid += 1;
                let bytes = encode_wat(module).unwrap_or_else(|e| {
                    panic!("{}: failed to encode invalid module directive: {e}", path.display())
                });
                let module = Module::decode(&bytes).unwrap_or_else(|e| {
                    panic!("{}: expected invalid module to decode before validation failure: {e}", path.display())
                });
                if module.validate().is_ok() {
                    panic!("{}: expected invalid module to fail validation", path.display());
                }
            }
            other => stats.unsupported.push(directive_name(&other)),
        }
    }

    stats
}

fn run_wast_dir(subdir: &str) {
    let mut total = WastCaseStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        let stats = run_wast_case(&path);
        eprintln!(
            "wast {}: total={} modules={} invalid={} malformed={} unsupported={}",
            path.display(),
            stats.total,
            stats.modules,
            stats.invalid,
            stats.malformed,
            stats.unsupported.len()
        );
        if !stats.unsupported.is_empty() {
            panic!(
                "{}: unsupported directives encountered: {}",
                path.display(),
                stats.unsupported.join(", ")
            );
        }

        total.total += stats.total;
        total.modules += stats.modules;
        total.invalid += stats.invalid;
        total.malformed += stats.malformed;
    }

    eprintln!(
        "wast summary [{}]: total={} modules={} invalid={} malformed={}",
        subdir, total.total, total.modules, total.invalid, total.malformed
    );
}

#[test]
fn spec_wast_validation_cases() {
    run_wast_dir("wast");
}

#[test]
fn spec_wast_upstream_subset_cases() {
    run_wast_dir("wast-upstream");
}
