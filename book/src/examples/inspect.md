# Example: Inspect a Module

Decode a `.wasm` binary and summarise its structure — the first pipeline stage,
without validating or executing. This is
`crates/baedeker-cli/examples/inspect.rs`, runnable with
`cargo run -p baedeker-cli --example inspect`.

```rust,no_run
use baedeker_core::binary::module::Module;

fn main() {
    let bytes: &[u8] = include_bytes!("../../../crates/baedeker-cli/examples/add.wasm");
    let module = Module::decode(bytes).expect("decode failed");

    println!("Baedeker decoded add.wasm:");
    println!("  types:     {}", module.types.len());
    println!("  functions: {}", module.functions.len());
    println!("  exports:   {}", module.exports.len());
    println!("  memories:  {}", module.memories.len());
    println!("  globals:   {}", module.globals.len());

    for export in &module.exports {
        println!("  export:    {}", export.name);
    }
}
```

`Module` exposes its parsed sections as public fields (`types`, `imports`,
`exports`, `functions`, `tables`, `memories`, `globals`, `elements`, `data`,
`codes`, `start`), so tooling can inspect a module without running it.

## Expected output

```
Baedeker decoded add.wasm:
  types:     1
  functions: 1
  exports:   1
  memories:  0
  globals:   0
  export:    add
```
