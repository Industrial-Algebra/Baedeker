// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
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

    // cbindgen's dual-mode enum output typedefs the enum for C23/C++ but
    // plain `uint8_t` for older C — which gives Swift's Clang importer two
    // same-named candidates. Typedef the enum in both branches instead:
    // valid C89+ everywhere and unambiguous for Swift. (The enum's fixed
    // `uint8_t` underlying type still applies on C23/C++; older C uses the
    // compiler-chosen enum representation, which is ABI-compatible with
    // the FFI's u8 statuses/tags on every targeted platform.)
    let mut text = String::from_utf8(bytes).expect("cbindgen emits UTF-8");
    for name in ["BaedekerStatus", "BaedekerValueTag"] {
        text = text.replace(
            &format!("typedef uint8_t {name};"),
            &format!("typedef enum {name} {name};"),
        );
    }
    let bytes = text.into_bytes();
    // Avoid touching the file (and dirtying the working tree / triggering
    // rebuild loops) when the output is unchanged.
    let unchanged = std::fs::read(&out)
        .map(|existing| existing == bytes)
        .unwrap_or(false);
    if !unchanged {
        std::fs::write(&out, bytes).expect("failed to write include/baedeker.h");
    }
}
