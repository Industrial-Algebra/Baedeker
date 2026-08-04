// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! WASM test fixtures compiled from Rust sources at build time.
//!
//! Add `.rs` files to `fixtures/` and they will be compiled to `.wasm`
//! by the build script. Use [`fixture_path`] or [`fixture_bytes`] to
//! load them in tests.

use std::path::{Path, PathBuf};

/// Directory containing compiled `.wasm` fixtures.
pub const OUT_DIR: &str = env!("OUT_DIR");

/// Get the path to a named fixture (e.g. `"add"` → `"{OUT_DIR}/add.wasm"`).
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(OUT_DIR).join(format!("{name}.wasm"))
}

/// Load a named fixture's bytes.
pub fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name))
        .unwrap_or_else(|e| panic!("failed to load fixture '{name}.wasm': {e}"))
}

/// Get the root path of copied spec-test fixtures.
pub fn spec_root() -> PathBuf {
    PathBuf::from(OUT_DIR).join("spec")
}

fn cases_with_extension(subdir: &str, ext: &str) -> Vec<PathBuf> {
    let root = spec_root().join(subdir);
    let mut cases: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| {
            panic!(
                "failed to read spec fixture directory {}: {e}",
                root.display()
            )
        })
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.extension()
                .is_some_and(|found| found == ext)
                .then_some(path)
        })
        .collect();
    cases.sort();
    cases
}

/// List `.wasm` files under a spec-test subdirectory such as `"valid"` or `"invalid"`.
pub fn spec_cases(subdir: &str) -> Vec<PathBuf> {
    cases_with_extension(subdir, "wasm")
}

/// List `.wast` files under a spec-test subdirectory.
pub fn spec_wast_cases(subdir: &str) -> Vec<PathBuf> {
    cases_with_extension(subdir, "wast")
}

/// Load a copied spec-test fixture by path.
pub fn spec_case_bytes(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path)
        .unwrap_or_else(|e| panic!("failed to load spec fixture {}: {e}", path.display()))
}

/// Load a copied spec-test text fixture by path.
pub fn spec_case_text(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to load spec text fixture {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fixture_exists() {
        let bytes = fixture_bytes("empty");
        // Minimal WASM: at least the 8-byte preamble
        assert!(
            bytes.len() >= 8,
            "empty.wasm too small: {} bytes",
            bytes.len()
        );
        assert_eq!(&bytes[..4], b"\0asm", "not a valid WASM binary");
    }

    #[test]
    fn add_fixture_exists() {
        let bytes = fixture_bytes("add");
        assert!(bytes.len() >= 8);
        assert_eq!(&bytes[..4], b"\0asm");
    }

    #[test]
    fn memory_fixture_exists() {
        let bytes = fixture_bytes("memory");
        assert!(bytes.len() >= 8);
        assert_eq!(&bytes[..4], b"\0asm");
    }
}
