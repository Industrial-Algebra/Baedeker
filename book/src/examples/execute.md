# Example: Execute a Module

The full decode → validate → lower → instantiate → execute pipeline. This is
`crates/baedeker-cli/examples/execute.rs`, runnable with
`cargo run -p baedeker-cli --example execute`.

```rust,no_run
use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::{Store, Value, execute_export};

fn main() {
    let bytes: &[u8] = include_bytes!("../../../crates/baedeker-cli/examples/add.wasm");

    let module = Module::decode(bytes).expect("decode failed");
    module.validate().expect("validation failed");
    let reg = lower_module(&module).expect("lowering failed");
    let store = Store::instantiate(&reg).expect("instantiation failed");

    let results = execute_export(&reg, &store, "add", &[Value::I32(20), Value::I32(22)])
        .expect("execution trapped");

    println!("add(20, 22) = {results:?}");
    assert_eq!(results, vec![Value::I32(42)]);
}
```

The `add.wasm` module exports a single function `add(i32, i32) -> i32` that
returns the sum of its arguments. In your own code, replace `include_bytes!`
with whatever supplies the `.wasm` bytes.

## Expected output

```
add(20, 22) = [I32(42)]
```
