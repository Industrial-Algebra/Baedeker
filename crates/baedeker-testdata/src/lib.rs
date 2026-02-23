//! WASM test fixtures compiled from Rust sources at build time.
//!
//! Add `.rs` files to `fixtures/` and they will be compiled to `.wasm`
//! by the build script. Use [`fixture_path`] or [`fixture_bytes`] to
//! load them in tests.

use std::path::PathBuf;

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
