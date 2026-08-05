# Core Engine API

The engine lives in `baedeker_core`. The pipeline types are re-exported at the
crate root and in `runtime`.

## Decode + validate + lower

```rust,ignore
pub struct Module<'a> { /* types, imports, exports, functions, ... */ }

impl Module<'_> {
    pub fn decode(bytes: &[u8]) -> Result<Module<'_>, DecodeError>;
    pub fn validate(&self) -> Result<(), ValidationError>;
}

pub fn lower_module(module: Module<'_>) -> Result<RegModule, LowerError>;
```

`Module` exposes its parsed sections as public fields (`types`, `imports`,
`exports`, `functions`, `memories`, `globals`, `tables`, `data`, `codes`, …) so
tooling can inspect a module without executing it.

## The runtime

```rust,ignore
pub struct Store { /* memories, tables, globals, funcrefs, fuel, gpu slot */ }

impl Store {
    pub fn instantiate(module: &RegModule) -> Result<Store, RuntimeError>;
    pub fn register_host_func(&mut self, module: &str, name: &str, f: HostFunction)
        -> Result<(), RuntimeError>;
    pub fn set_fuel(&mut self, fuel: Option<u64>);
    pub fn with_memory(&self, idx: usize, f: impl FnOnce(&[u8])) -> Option<()>;
    pub fn with_memory_mut(&mut self, idx: usize, f: impl FnOnce(&mut [u8])) -> Option<()>;
}

pub fn execute_export(
    module: &RegModule,
    store: &Store,
    name: &str,
    args: &[Value],
) -> Result<Vec<Value>, RuntimeError>;
```

## Values and types

```rust,ignore
pub enum Value {
    I32(i32), I64(i64), F32(f32), F64(f64),
    FuncRef(Option<(u32, u32)>), ExternRef(Option<u32>),
    V128([u8; 16]),
}
```

`ValType` and `FuncType` mirror the spec; `HostFunction::new(func_type, closure)`
wraps a host callable. Host closures are `Box<dyn FnMut(&[Value]) -> Result<Vec<Value>, RuntimeError>>`.

## Errors

Decode and validation errors carry byte offsets into the original binary.
Runtime errors are categorised (`RuntimeErrorKind`): unknown export/memory,
traps (`RuntimeTrap`), fuel exhaustion, and GPU errors. See the rustdoc for the
full taxonomy.
