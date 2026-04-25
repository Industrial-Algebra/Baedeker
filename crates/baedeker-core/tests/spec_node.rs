use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use wast::parser::{ParseBuffer, parse};
use wast::{QuoteWat, Wast, WastDirective};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawExpectation {
    Accept,
    Reject,
    Skip,
}

#[derive(Debug, Default, Clone)]
struct WastCaseMeta {
    skip: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct WastNodeStats {
    files_run: usize,
    files_skipped: usize,
    directives: usize,
    modules_expected_valid: usize,
    modules_expected_invalid: usize,
    malformed_text_unencodable: usize,
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn node_compile(bytes: &[u8]) -> Result<(), String> {
    const SCRIPT: &str = r#"
const chunks = [];
process.stdin.on('data', chunk => chunks.push(chunk));
process.stdin.on('end', () => {
  try {
    new WebAssembly.Module(Buffer.concat(chunks));
  } catch (e) {
    console.error(`${e.name}: ${e.message}`);
    process.exit(1);
  }
});
"#;

    let mut child = Command::new("node")
        .arg("-e")
        .arg(SCRIPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn node: {e}"));

    child
        .stdin
        .as_mut()
        .unwrap_or_else(|| panic!("failed to open node stdin"))
        .write_all(bytes)
        .unwrap_or_else(|e| panic!("failed to write wasm bytes to node stdin: {e}"));

    let output = child
        .wait_with_output()
        .unwrap_or_else(|e| panic!("failed to wait for node: {e}"));

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if stderr.is_empty() {
            format!("node exited with status {}", output.status)
        } else {
            stderr
        })
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

fn run_wast_dir_against_node(subdir: &str) -> WastNodeStats {
    let mut stats = WastNodeStats::default();

    for path in baedeker_testdata::spec_wast_cases(subdir) {
        let meta = read_case_meta(&path);
        if let Some(reason) = meta.skip {
            eprintln!("node wast {}: skipped ({reason})", path.display());
            stats.files_skipped += 1;
            continue;
        }

        let text = baedeker_testdata::spec_case_text(&path);
        let buf = ParseBuffer::new(&text)
            .unwrap_or_else(|e| panic!("{}: failed to parse wast buffer: {e}", path.display()));
        let wast = parse::<Wast<'_>>(&buf)
            .unwrap_or_else(|e| panic!("{}: failed to parse wast directives: {e}", path.display()));

        stats.files_run += 1;
        for (i, directive) in wast.directives.into_iter().enumerate() {
            stats.directives += 1;
            let name = directive_name(&directive);
            match directive {
                WastDirective::Module(wat) | WastDirective::ModuleDefinition(wat) => {
                    stats.modules_expected_valid += 1;
                    let bytes = encode_wat(wat).unwrap_or_else(|e| {
                        panic!(
                            "{}: directive #{i} expected encodable valid module for node cross-check: {e}",
                            path.display()
                        )
                    });
                    if let Err(err) = node_compile(&bytes) {
                        panic!(
                            "{}: directive #{i} expected node/V8 to accept valid module, got {err}",
                            path.display()
                        );
                    }
                }
                WastDirective::AssertInvalid { module, .. }
                | WastDirective::AssertMalformed { module, .. } => {
                    stats.modules_expected_invalid += 1;
                    match encode_wat(module) {
                        Ok(bytes) => {
                            if node_compile(&bytes).is_ok() {
                                panic!(
                                    "{}: directive #{i} expected node/V8 to reject {name}, but it compiled",
                                    path.display(),
                                );
                            }
                        }
                        Err(_) => {
                            stats.malformed_text_unencodable += 1;
                        }
                    }
                }
                other => panic!(
                    "{}: unsupported directive for node cross-check: {}",
                    path.display(),
                    directive_name(&other)
                ),
            }
        }
    }

    eprintln!(
        "node wast summary [{}]: files_run={} files_skipped={} directives={} valid={} invalid={} unencodable_malformed={}",
        subdir,
        stats.files_run,
        stats.files_skipped,
        stats.directives,
        stats.modules_expected_valid,
        stats.modules_expected_invalid,
        stats.malformed_text_unencodable,
    );

    stats
}

fn raw_expectation(path: &Path) -> RawExpectation {
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| panic!("{}: missing raw fixture parent directory", path.display()));

    match parent {
        "valid" => RawExpectation::Accept,
        "invalid-decode" => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if matches!(stem, "unknown-ref-type-in-table") {
                RawExpectation::Skip
            } else {
                RawExpectation::Reject
            }
        }
        "invalid-validate" => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem.contains("unsupported") {
                RawExpectation::Skip
            } else {
                RawExpectation::Reject
            }
        }
        other => panic!("{}: unexpected raw fixture dir {other}", path.display()),
    }
}

fn run_raw_dir_against_node(subdir: &str) {
    let mut ran = 0usize;
    let mut skipped = 0usize;

    for path in baedeker_testdata::spec_cases(subdir) {
        match raw_expectation(&path) {
            RawExpectation::Skip => {
                skipped += 1;
                eprintln!("node raw {}: skipped", path.display());
            }
            RawExpectation::Accept => {
                ran += 1;
                let bytes = baedeker_testdata::spec_case_bytes(&path);
                if let Err(err) = node_compile(&bytes) {
                    panic!(
                        "{}: expected node/V8 to accept valid raw fixture, got {err}",
                        path.display()
                    );
                }
            }
            RawExpectation::Reject => {
                ran += 1;
                let bytes = baedeker_testdata::spec_case_bytes(&path);
                if node_compile(&bytes).is_ok() {
                    panic!(
                        "{}: expected node/V8 to reject invalid raw fixture, but it compiled",
                        path.display()
                    );
                }
            }
        }
    }

    eprintln!(
        "node raw summary [{}]: ran={} skipped={}",
        subdir, ran, skipped
    );
}

#[test]
fn spec_node_raw_cases() {
    if !node_available() {
        eprintln!("node not available; skipping node/V8 raw cross-check");
        return;
    }

    run_raw_dir_against_node("valid");
    run_raw_dir_against_node("invalid-decode");
    run_raw_dir_against_node("invalid-validate");
}

#[test]
fn spec_node_wast_validation_cases() {
    eprintln!(
        "custom spec/wast cases include Baedeker-specific boundary assertions; skipping strict node/V8 cross-check"
    );
}

#[test]
fn spec_node_wast_upstream_subset_cases() {
    if !node_available() {
        eprintln!("node not available; skipping node/V8 upstream wast cross-check");
        return;
    }

    let stats = run_wast_dir_against_node("wast-upstream");
    assert_eq!(
        stats.files_skipped, 0,
        "upstream node/V8 cross-check should not skip any active subset files"
    );
}
