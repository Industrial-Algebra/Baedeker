use std::path::Path;
use std::process::Command;
use std::{env, fs};

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let fixtures_dir = Path::new(&manifest_dir).join("fixtures");
    let spec_dir = Path::new(&manifest_dir).join("spec");
    let verbose = env::var_os("BAEDEKER_TESTDATA_VERBOSE").is_some();

    println!("cargo::rerun-if-changed=fixtures/");
    println!("cargo::rerun-if-changed=spec/");

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
                "--edition",
                "2024",
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

        if verbose {
            println!(
                "cargo:warning=compiled fixture: {stem}.wasm ({} bytes)",
                fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0)
            );
        }
    }

    if spec_dir.exists() {
        let out_spec_dir = Path::new(&out_dir).join("spec");
        if out_spec_dir.exists() {
            fs::remove_dir_all(&out_spec_dir).unwrap_or_else(|e| {
                panic!(
                    "failed to clear stale spec output directory {}: {e}",
                    out_spec_dir.display()
                )
            });
        }
        copy_spec_fixtures(&spec_dir, out_spec_dir.as_path(), verbose);
    }
}

fn copy_spec_fixtures(src_root: &Path, dst_root: &Path, verbose: bool) {
    fs::create_dir_all(dst_root).unwrap_or_else(|e| {
        panic!(
            "failed to create spec output directory {}: {e}",
            dst_root.display()
        )
    });

    for entry in fs::read_dir(src_root)
        .unwrap_or_else(|e| panic!("failed to read spec directory {}: {e}", src_root.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("failed to read spec directory entry: {e}"));
        let path = entry.path();
        let dst = dst_root.join(entry.file_name());
        if path.is_dir() {
            copy_spec_fixtures(&path, &dst, verbose);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "wasm" || ext == "meta" || ext == "wast")
        {
            fs::create_dir_all(dst.parent().expect("spec file must have parent")).unwrap_or_else(
                |e| panic!("failed to create directory for {}: {e}", dst.display()),
            );
            fs::copy(&path, &dst).unwrap_or_else(|e| {
                panic!(
                    "failed to copy spec fixture {} to {}: {e}",
                    path.display(),
                    dst.display()
                )
            });
            if verbose {
                println!("cargo:warning=copied spec fixture: {}", dst.display());
            }
        }
    }
}
