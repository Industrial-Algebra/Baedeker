# Executing a Module

This page covers the runtime API in detail. For the quick version, see
[Getting Started](../getting-started.md).

## The pipeline

```rust,no_run
use baedeker_core::binary::module::Module;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::{Store, Value, execute_export};

let module = Module::decode(bytes)?;
module.validate()?;
let reg = lower_module(&module)?;
let store = Store::instantiate(&reg)?;
let results = execute_export(&reg, &store, "add", &[Value::I32(20), Value::I32(22)])?;
```

`Module::decode` borrows the input bytes (`Module<'a>`), so it allocates no copy
of the binary. `lower_module` consumes the module into an owned `RegModule`.
`Store::instantiate` resolves the module's imports (or fails if a required
import is unmet) and builds runtime state.

## Values

Arguments and results are `Value`:

```rust,ignore
pub enum Value {
    I32(i32), I64(i64), F32(f32), F64(f64),
    FuncRef(Option<(u32, u32)>),
    ExternRef(Option<u32>),
    V128([u8; 16]),
}
```

The `V128` variant carries raw little-endian bytes; lane interpretation happens
per operation (fixed-width SIMD).

## Fuel and traps

Cap execution with fuel before running untrusted code:

```rust,no_run
use baedeker_core::runtime::Store;
# // let store: Store = /* ... */;
// store.set_fuel(Some(1_000_000));
```

When fuel is exhausted, execution stops with `FuelExhausted` rather than running
unbounded. Other traps are categorised: out-of-bounds memory/table access,
integer overflow on float→int truncation, indirect-call type mismatch,
unreachable, call exhaustion (`MAX_CALL_DEPTH = 512`). Each carries the context
needed for a precise diagnostic.

## Host functions

Register host functions before instantiation so the module's imports resolve:

```rust,no_run
# use baedeker_core::lower::RegModule;
# use baedeker_core::runtime::{HostFunction, Store};
# fn example(reg: &RegModule, store: &mut Store) {
#   use baedeker_core::types::{FuncType, NumType, ValType};
#   let ft = FuncType { params: vec![ValType::Num(NumType::I32)], results: vec![] };
#   let log = HostFunction::new(ft, Box::new(|_args| Ok(vec![])));
#   store.register_host_func("env", "log", log).unwrap();
# }
```

`register_host_func` errors if the module does not declare the import (catching
typos) or if the signature mismatches.
