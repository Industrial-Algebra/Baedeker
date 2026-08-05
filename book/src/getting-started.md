# Getting Started

Baedeker runs a WebAssembly module through decode → validate → lower →
instantiate → execute. This page walks through the smallest end-to-end example.

## Install

Baedeker is a Rust workspace published to crates.io:

```toml
[dependencies]
baedeker-core = "0.1"
```

For a command-line harness, install the CLI:

```sh
cargo install baedeker-cli
```

## Your first module

This is the `execute` example (`crates/baedeker-cli/examples/execute.rs`),
running a module that exports `add(i32, i32) -> i32`:

```rust,no_run
use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::{Store, Value, execute_export};

// The four-stage pipeline every Baedeker run goes through.
let module = Module::decode(WASM_BYTES).expect("decode");
module.validate().expect("validate");
let reg = lower_module(&module).expect("lower");
let store = Store::instantiate(&reg).expect("instantiate");

let results = execute_export(&reg, &store, "add", &[Value::I32(20), Value::I32(22)])
    .expect("execute");
assert_eq!(results, vec![Value::I32(42)]);
```

`WASM_BYTES` is a `&[u8]` containing a `.wasm` binary. In the example it is
embedded with `include_bytes!("add.wasm")`; in your code it comes from
wherever modules originate (the filesystem, the network, a build step).

## Run the bundled examples

```sh
# Full pipeline ending in executing an exported function.
cargo run -p baedeker-cli --example execute
# add(20, 22) = [I32(42)]

# Decode + structural summary (no validation or execution).
cargo run -p baedeker-cli --example inspect
```

## Decode without executing

To inspect a `.wasm` binary's structure:

```sh
baedeker path/to/module.wasm
```

To validate and lower it to an ahead-of-time artifact:

```sh
baedeker compile path/to/module.wasm -o module.bdkaot
```

## Next steps

- [Architecture](./concepts/architecture.md) — the four-stage pipeline in depth.
- [Executing a Module](./guide/execution.md) — the runtime API.
- [Embedding via FFI](./guide/ffi.md) — the C ABI for host embedding.
