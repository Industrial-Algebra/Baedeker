use std::path::Path;
use std::process::Command;
use std::{env, fs};

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let fixtures_dir = Path::new(&manifest_dir).join("fixtures");

    println!("cargo::rerun-if-changed=fixtures/");

    // Verify the wasm target is installed
    let rustup_output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .expect("failed to run rustup");
    let installed = String::from_utf8_lossy(&rustup_output.stdout);
    assert!(
        installed.contains("wasm32-unknown-unknown"),
        "wasm32-unknown-unknown target not installed. Run: rustup target add wasm32-unknown-unknown"
    );

    let entries: Vec<_> = fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| {
            panic!(
                "failed to read fixtures directory {}: {e}",
                fixtures_dir.display()
            )
        })
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    for src_path in &entries {
        let stem = src_path.file_stem().unwrap().to_str().unwrap();
        let wasm_path = Path::new(&out_dir).join(format!("{stem}.wasm"));

        println!("cargo::rerun-if-changed={}", src_path.display());

        let status = Command::new("rustc")
            .args([
                "--target",
                "wasm32-unknown-unknown",
                "--crate-type",
                "cdylib",
                "-O",
                "-o",
            ])
            .arg(&wasm_path)
            .arg(src_path)
            .status()
            .unwrap_or_else(|e| panic!("failed to invoke rustc for {stem}.rs: {e}"));

        assert!(
            status.success(),
            "rustc failed to compile {} to WASM (exit code: {:?})",
            src_path.display(),
            status.code()
        );

        println!(
            "cargo:warning=compiled fixture: {stem}.wasm ({} bytes)",
            fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0)
        );
    }
}
