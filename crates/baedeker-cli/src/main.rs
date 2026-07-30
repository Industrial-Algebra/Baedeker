//! Baedeker CLI — decode a `.wasm` binary and print its section layout, or
//! compile one to an AOT artifact (`.bdkaot`) for bundling into host apps.

use std::process;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("compile") => compile(&args[2..]),
        Some(path) => inspect(path),
        None => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage:");
    eprintln!("  baedeker-cli <file.wasm>                  print section layout");
    eprintln!("  baedeker-cli compile <in.wasm> -o <out.bdkaot>");
    eprintln!("                                            validate + lower to an AOT artifact");
    process::exit(1);
}

fn read(path: &str) -> Vec<u8> {
    match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("error: failed to read {path}: {e}");
            process::exit(1);
        }
    }
}

fn inspect(path: &str) {
    let bytes = read(path);
    let module = match Module::decode(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    println!("Decoded: {path}");
    println!("Sections: {}", module.sections.len());
    println!();

    for (id, offset, size) in module.section_summary() {
        println!(
            "  {:>12}  offset={:<8}  size={} bytes",
            id.name(),
            offset,
            size
        );
    }
}

fn compile(args: &[String]) {
    let (input, output) = match args {
        [input, flag, output] if flag == "-o" => (input, output),
        _ => usage(),
    };

    let bytes = read(input);
    let module = Module::decode(&bytes).unwrap_or_else(|e| {
        eprintln!("error: decode failed: {e}");
        process::exit(1);
    });
    module.validate().unwrap_or_else(|e| {
        eprintln!("error: validation failed: {e:?}");
        process::exit(1);
    });
    let lowered = lower_module(&module).unwrap_or_else(|e| {
        eprintln!("error: lowering failed: {e:?}");
        process::exit(1);
    });

    let artifact = baedeker_core::aot::serialize(&lowered);
    if let Err(e) = std::fs::write(output, &artifact) {
        eprintln!("error: failed to write {output}: {e}");
        process::exit(1);
    }
    println!(
        "compiled {input} -> {output} ({} bytes, format v{})",
        artifact.len(),
        baedeker_core::aot::AOT_FORMAT_VERSION
    );
}
