//! Baedeker CLI — decode a `.wasm` binary and print its section layout.

use std::process;

use baedeker_core::binary::module::Module;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() != 2 {
        eprintln!("usage: baedeker-cli <file.wasm>");
        process::exit(1);
    }

    let path = &args[1];
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: failed to read {path}: {e}");
            process::exit(1);
        }
    };

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
