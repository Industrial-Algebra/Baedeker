// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Shared article harness: module building, execution, accounting helpers and
//! the capped-child protocol.

use std::path::PathBuf;
use std::process::Command;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::runtime::{
    GcStats, RuntimeError, RuntimeErrorKind, Store, Value, execute_export,
};

/// Environment knob: divide every article magnitude by this value. CI sets it
/// to keep the suite wall-time bounded; the formulas are unaffected because
/// they are expressed in run counts, not fixed magnitudes.
pub const SCALE_DIVISOR_ENV: &str = "BAEDEKER_GC_ARTICLE_DIVISOR";

/// Environment variable that puts this binary into child mode.
pub const CHILD_ENV: &str = "GC_ARTICLE_CHILD";

/// The full-magnitude count divided by the CI divisor (at least 1).
pub fn article_count(full: u64) -> u64 {
    let divisor = std::env::var(SCALE_DIVISOR_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    (full / divisor).max(1)
}

/// Whether this process is the child for `article`.
pub fn is_child(article: &str) -> bool {
    std::env::var(CHILD_ENV).is_ok_and(|value| value == article)
}

/// Lower a WAT source string through the full front end.
pub fn wat_module(source: &str) -> RegModule {
    let buffer = wast::parser::ParseBuffer::new(source).expect("parse buffer");
    let mut wat = wast::parser::parse::<wast::Wat<'_>>(&buffer).expect("parse wat");
    let bytes = wat.encode().expect("encode wat");
    let module = Module::decode(&bytes).expect("decode module");
    module.lower().expect("lower module")
}

/// Instantiate a module in a standalone store.
pub fn instantiate(source: &str) -> (RegModule, Store) {
    let module = wat_module(source);
    let store = Store::instantiate(&module).expect("instantiate");
    (module, store)
}

/// Call an export, panicking with the error on failure.
pub fn call(module: &RegModule, store: &Store, name: &str, args: &[Value]) -> Vec<Value> {
    execute_export(module, store, name, args)
        .unwrap_or_else(|error| panic!("export {name} with {args:?} failed: {}", error.kind))
}

/// Call an export expecting exactly one `i32` result.
pub fn call_i32(module: &RegModule, store: &Store, name: &str, args: &[Value]) -> i32 {
    match call(module, store, name, args).as_slice() {
        [Value::I32(value)] => *value,
        other => panic!("export {name} returned {other:?}, expected one i32"),
    }
}

/// Largest work unit fed to ONE export call.
///
/// `execute_func_in` has a coarse no-fuel backstop: a single call may dispatch
/// at most 10 000 000 blocks and then reports `MissingReturn`. That guard is
/// KEPT ACTIVE for the articles (a fuel budget would disable it); workloads
/// larger than the backstop are split into chunks across calls, which is
/// observationally identical for a schedule-independent result.
pub const CHUNK: u64 = 1_000_000;

/// Run `total` units in chunks, accumulating the (wrapping) `i32` results.
/// `run` receives the chunk's `(base, count)` pair, so the INPUT RANGE
/// advances with `base` between chunks — a campaign split into several calls
/// computes the same total-range result as one whole call.
/// Returns the first error unchanged, so a structured refusal is preserved.
pub fn sum_chunks<F>(total: u64, chunk: u64, mut run: F) -> Result<i64, RuntimeError>
where
    F: FnMut(u64, u64) -> Result<i32, RuntimeError>,
{
    let mut remaining = total;
    let mut base = 0u64;
    let mut sum: i32 = 0;
    while remaining > 0 {
        let this = chunk.min(remaining);
        sum = sum.wrapping_add(run(base, this)?);
        base += this;
        remaining -= this;
    }
    Ok(i64::from(sum))
}

/// The sum of `0..n` — the schedule-independent churn/loop oracle.
pub fn triangular(n: u64) -> u64 {
    n.saturating_mul(n.saturating_sub(1)) / 2
}

/// A formatted stats line for diagnostics and child markers.
pub fn format_stats(stats: GcStats) -> String {
    format!(
        "live_objects={} live_bytes={} reserved_slots={} reserved_bytes={}",
        stats.live_objects, stats.live_bytes, stats.reserved_slots, stats.reserved_bytes
    )
}

/// Linear-memory size in 64 KiB pages.
pub fn memory_pages(store: &Store, index: u32) -> u64 {
    store
        .shared_memory(index)
        .map(|memory| (memory.borrow().len() / 65536) as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Capped-child protocol
// ---------------------------------------------------------------------------

/// The cap applied to the child process (address space, KiB). Documented at
/// each capped article: the design's 48/64 MiB smoke, with 64 MiB used as the
/// executable point (a Rust test binary's own address space needs headroom).
/// Engine-level GC heap byte cap for capped children (logical bytes; the
/// A2b-style structured-refusal grading). Replaces the old `ulimit -v`
/// approach: macOS does not enforce RLIMIT_AS usefully, which let children
/// believe themselves capped and still get OOM-killed by the hosted
/// MacCloud monitor (three CI incidents). The engine cap is identical on
/// every platform.
pub const CHILD_CAP_BYTES: u64 = 32 * 1024 * 1024;
pub const CHILD_CAP_ENV: &str = "GC_ARTICLE_CHILD_CAP_BYTES";

/// Print a child marker line.
pub fn child_marker(line: &str) {
    println!("GC-ARTICLE-CHILD {line}");
}

/// Print a child stats marker.
pub fn child_stats(stage: &str, stats: GcStats) {
    println!(
        "GC-ARTICLE-CHILD-STATS stage={stage} {}",
        format_stats(stats)
    );
}

/// `yes`/`no` for child markers.
pub fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Whether a clean structured refusal is the expected outcome.
pub fn is_refusal(error: &RuntimeError) -> bool {
    matches!(error.kind, RuntimeErrorKind::ResourceLimitExceeded { .. })
}

/// Apply the engine-level cap the parent requested (if any) to `store`.
/// Returns whether a cap is in force.
pub fn apply_engine_cap(store: &baedeker_core::runtime::Store) -> bool {
    match std::env::var(CHILD_CAP_ENV) {
        Ok(value) => {
            let bytes: u64 = value.parse().expect("cap bytes parse");
            store.set_gc_heap_byte_limit(Some(bytes));
            true
        }
        Err(_) => false,
    }
}

/// What a capped child reported.
#[derive(Debug)]
pub struct CappedOutcome {
    /// The child's stdout (markers included).
    pub stdout: String,
    /// The child's stderr (test-framework noise included).
    pub stderr: String,
    /// Exit status code; a signal death has no code.
    pub code: Option<i32>,
    /// Full process status (carries the signal number on a signal death).
    pub status: std::process::ExitStatus,
    /// Whether the child process was actually memory-capped.
    pub capped: bool,
    /// Whether the pressure phase itself was refused (only meaningful capped).
    pub pressure_refused: bool,
    /// Whether the guaranteed-too-large allocation was refused.
    pub huge_refused: bool,
}

impl CappedOutcome {
    /// Grade the child against the capped-article contract: exit 0 (never a
    /// panic/abort), the pressure phase refused when the cap was real, the
    /// final over-large allocation refused with identical stats, and the
    /// refusal surfaced as a structured `ResourceLimitExceeded`.
    pub fn require_structured_refusal(&self, article: &str) {
        eprintln!("{article} child markers:\n{}", self.stdout.trim_end());
        assert_eq!(
            self.code,
            Some(0),
            "{article}: child must exit 0 (a panic/abort/signal is a failure; \
             status = {:?}, so the child reached start-marker={} and no further); stderr:\n{}",
            self.status,
            self.stdout.contains("GC-ARTICLE-CHILD article="),
            self.stderr
        );
        assert!(
            self.stdout
                .contains(&format!("GC-ARTICLE-CHILD article={article}")),
            "{article}: child did not report; stdout:\n{}",
            self.stdout
        );
        assert!(
            self.huge_refused,
            "{article}: the over-large allocation must be refused"
        );
        assert!(
            self.stdout.contains("stats_untouched=yes"),
            "{article}: the refused allocation must leave the full GcStats snapshot identical; stdout:\n{}",
            self.stdout
        );
        if self.capped {
            assert!(
                self.pressure_refused,
                "{article}: under the {CHILD_CAP_BYTES}-byte engine cap the pressure workload \
                 must be refused, not completed; stdout:\n{}",
                self.stdout
            );
        } else {
            // No enforceable RLIMIT_AS here (e.g. macOS): the pressure phase is
            // allowed to complete, but the refusal discipline is still graded
            // through the over-large allocation above. Report it honestly.
            eprintln!(
                "{article}: no enforceable address-space cap on this host; graded the structured \
                 refusal through the over-large allocation only"
            );
        }
    }
}

/// Re-run this test binary as a child process for `test_name`, under the
/// engine-level GC heap cap ([`CHILD_CAP_ENV`] — no platform rlimits).
pub fn spawn_capped_child(article: &str, test_name: &str) -> CappedOutcome {
    let exe: PathBuf = std::env::current_exe().expect("current test binary");
    // `sh -c '<cap>; exec "$0" "$@"' <exe> --exact <test> …` — no shell
    // interpolation of the test name.
    let script = "exec \"$0\" --exact \"$1\" --nocapture --test-threads=1";
    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg(&exe)
        .arg(test_name)
        .env(CHILD_ENV, article)
        .env(CHILD_CAP_ENV, CHILD_CAP_BYTES.to_string())
        .output()
        .expect("spawn article child");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let capped = marker_bool(&stdout, "capped");
    let pressure_refused = marker_bool(&stdout, "pressure_refused");
    let huge_refused = marker_bool(&stdout, "huge_refused");
    CappedOutcome {
        stdout,
        stderr,
        code: output.status.code(),
        status: output.status,
        capped,
        pressure_refused,
        huge_refused,
    }
}

/// Read `name=yes|no` from the child's marker block.
fn marker_bool(stdout: &str, name: &str) -> bool {
    let needle = format!(" {name}=");
    stdout
        .lines()
        .filter(|line| line.starts_with("GC-ARTICLE-CHILD "))
        .find_map(|line| {
            let index = line.find(&needle)?;
            let value = &line[index + needle.len()..];
            Some(value.starts_with("yes"))
        })
        .unwrap_or(false)
}
