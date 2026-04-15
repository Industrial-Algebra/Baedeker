use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use baedeker_core::binary::module::Module;
use wast::parser::{parse, ParseBuffer};
use wast::{QuoteWat, Wast, WastDirective};

#[derive(Debug, Default, Clone)]
struct WastCaseStats {
    total: usize,
    modules: usize,
    malformed: usize,
    invalid: usize,
    unsupported: BTreeMap<&'static str, usize>,
}

#[derive(Debug, Default, Clone)]
struct WastCaseMeta {
    skip: Option<String>,
}

#[derive(Debug)]
enum WastCaseOutcome {
    Ran(WastCaseStats),
    Skipped(String),
}

#[derive(Debug, Default)]
struct WastDirStats {
    files_run: usize,
    files_skipped: usize,
    total: usize,
    modules: usize,
    malformed: usize,
    invalid: usize,
    unsupported: BTreeMap<&'static str, usize>,
    skipped_files: Vec<(PathBuf, String)>,
}

impl WastCaseStats {
    fn record_unsupported(&mut self, name: &'static str) {
        *self.unsupported.entry(name).or_default() += 1;
    }

    fn unsupported_summary(&self) -> String {
        if self.unsupported.is_empty() {
            return "none".to_owned();
        }

        self.unsupported
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl WastDirStats {
    fn add_case(&mut self, stats: WastCaseStats) {
        self.files_run += 1;
        self.total += stats.total;
        self.modules += stats.modules;
        self.invalid += stats.invalid;
        self.malformed += stats.malformed;
        for (name, count) in stats.unsupported {
            *self.unsupported.entry(name).or_default() += count;
        }
    }

    fn add_skipped(&mut self, path: PathBuf, reason: String) {
        self.files_skipped += 1;
        self.skipped_files.push((path, reason));
    }

    fn unsupported_summary(&self) -> String {
        if self.unsupported.is_empty() {
            return "none".to_owned();
        }

        self.unsupported
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn encode_wat(mut wat: QuoteWat<'_>) -> Result<Vec<u8>, String> {
    wat.encode().map_err(|e| e.to_string())
}

fn read_case_meta(path: &Path) -> WastCaseMeta {
    let meta_path = path.with_extension("meta");
    if !meta_path.exists() {
        return WastCaseMeta::default();
    }

    let text = std::fs::read_to_string(&meta_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", meta_path.display()));
    let mut meta = WastCaseMeta::default();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').unwrap_or_else(|| {
            panic!(
                "{}: expected key=value metadata line, found {line:?}",
                meta_path.display()
            )
        });
        match key.trim() {
            "skip" => meta.skip = Some(value.trim().to_owned()),
            other => panic!("{}: unknown metadata key {other:?}", meta_path.display()),
        }
    }
    meta
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

fn run_wast_case(path: &Path) -> WastCaseOutcome {
    let meta = read_case_meta(path);
    if let Some(reason) = meta.skip {
        return WastCaseOutcome::Skipped(reason);
    }

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
            other => stats.record_unsupported(directive_name(&other)),
        }
    }

    WastCaseOutcome::Ran(stats)
}

fn run_wast_dir(subdir: &str) {
    let mut total = WastDirStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        match run_wast_case(&path) {
            WastCaseOutcome::Ran(stats) => {
                eprintln!(
                    "wast {}: total={} modules={} invalid={} malformed={} unsupported={}",
                    path.display(),
                    stats.total,
                    stats.modules,
                    stats.invalid,
                    stats.malformed,
                    stats.unsupported_summary()
                );
                if !stats.unsupported.is_empty() {
                    panic!(
                        "{}: unsupported directives encountered: {}",
                        path.display(),
                        stats.unsupported_summary()
                    );
                }
                total.add_case(stats);
            }
            WastCaseOutcome::Skipped(reason) => {
                eprintln!("wast {}: skipped ({reason})", path.display());
                total.add_skipped(path, reason);
            }
        }
    }

    eprintln!(
        "wast summary [{}]: files_run={} files_skipped={} total={} modules={} invalid={} malformed={} unsupported={}",
        subdir,
        total.files_run,
        total.files_skipped,
        total.total,
        total.modules,
        total.invalid,
        total.malformed,
        total.unsupported_summary()
    );
    if !total.skipped_files.is_empty() {
        eprintln!("wast skipped [{}]:", subdir);
        for (path, reason) in &total.skipped_files {
            eprintln!("  {} => {}", path.display(), reason);
        }
    }
}

#[test]
fn spec_wast_validation_cases() {
    run_wast_dir("wast");
}

#[test]
fn spec_wast_upstream_subset_cases() {
    run_wast_dir("wast-upstream");
}
