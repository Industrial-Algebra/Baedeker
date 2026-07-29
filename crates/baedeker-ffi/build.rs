//! Regenerates `include/baedeker.h` from the crate's `extern "C"` surface.
//!
//! The header is committed to the repo; CI fails on drift, so any change to
//! the FFI surface shows up as a build-time header rewrite that must be
//! committed alongside the code change.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = crate_dir.join("include").join("baedeker.h");

    let bindings = cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_file(crate_dir.join("cbindgen.toml")).unwrap())
        .generate()
        .expect("cbindgen generation failed");

    let mut bytes = Vec::new();
    bindings.write(&mut bytes);
    // Avoid touching the file (and dirtying the working tree / triggering
    // rebuild loops) when the output is unchanged.
    let unchanged = std::fs::read(&out)
        .map(|existing| existing == bytes)
        .unwrap_or(false);
    if !unchanged {
        std::fs::write(&out, bytes).expect("failed to write include/baedeker.h");
    }
}
