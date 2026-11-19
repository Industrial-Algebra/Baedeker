// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Compiles `tests/smoke.c` against the generated `include/baedeker.h`, links
//! it against the built `libbaedeker_ffi` static library, and runs it — a
//! true C-consumer end-to-end test of the ABI and header.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The profile directory containing `libbaedeker_ffi.a`, derived from the
/// workspace target directory — NOT from the test binary's location.
///
/// The nested `cargo build -p baedeker-ffi` (no `--release`) always emits the
/// staticlib at `<target>/debug/libbaedeker_ffi.a`. Deriving the path by
/// walking up from the test binary is fragile: cargo can place the test
/// binary in a build-script out-dir
/// (`target/<profile>/build/<pkg>/<hash>/out/`), where the
/// two-levels-up assumption points into the out-dir and the staticlib is
/// never found (observed on CI: the FFI `c_smoke_end_to_end` test failing
/// with "staticlib not found at …/build/baedeker-ffi/<hash>/"). Anchor on
/// `CARGO_TARGET_DIR` when set, else the workspace root's `target/`.
fn profile_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("debug");
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2) // crates/baedeker-ffi → crates → workspace root
        .expect("workspace root above crates/baedeker-ffi")
        .join("target")
        .join("debug")
}

fn run(cmd: &mut Command, what: &str) {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn c_smoke_end_to_end() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = profile_dir();
    let staticlib = target.join("libbaedeker_ffi.a");
    // Integration tests build only the rlib; always refresh the staticlib so
    // the C side links current code (cargo no-ops when fresh).
    run(
        Command::new("cargo").args(["build", "-p", "baedeker-ffi"]),
        "cargo build baedeker-ffi",
    );
    assert!(
        staticlib.exists(),
        "staticlib not found at {}",
        staticlib.display()
    );

    let work = std::env::temp_dir().join(format!("baedeker-c-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let object = work.join("smoke.o");
    let binary = work.join("smoke");

    run(
        Command::new("cc")
            .arg("-c")
            .arg(manifest.join("tests/smoke.c"))
            .arg("-I")
            .arg(manifest.join("include"))
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-std=c11")
            .arg("-o")
            .arg(&object),
        "compile smoke.c",
    );

    let mut link = Command::new("cc");
    link.arg(&object)
        .arg(&staticlib)
        .arg("-o")
        .arg(&binary)
        .arg("-lpthread")
        .arg("-lm");
    if cfg!(target_os = "linux") {
        link.arg("-ldl");
    }
    run(&mut link, "link smoke");

    let output = Command::new(&binary).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}{stderr}");
    assert!(output.status.success(), "smoke binary failed: {stderr}");
    assert!(stdout.contains("mul6x7() = 42"));
    assert!(stdout.contains("memory roundtrip ok"));
}
