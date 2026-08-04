# Ahead-of-Time Compilation

Baedeker can serialize a lowered module to an ahead-of-time (AOT) artifact,
skipping decode + validate + lower at load time. This is useful for embedded
targets where startup cost matters or where carrying a validator is unnecessary.

## The artifact format

An AOT artifact is a postcard-serialized `RegModule` wrapped in a small
envelope:

```
BDKAOT1  magic (7 bytes)
< u32 >  version
< postcard-encoded RegModule >
```

The `aot` feature (`baedeker-core`'s `aot` = `serde` + `postcard`) enables
serialization and deserialization.

## Compile

```sh
baedeker compile path/to/module.wasm -o module.bdkaot
```

The CLI decodes, validates, lowers, and writes the envelope. Or, in code:

```rust,no_run
# use baedeker_core::binary::module::Module;
# use baedeker_core::lower::lower_module;
# use baedeker_core::aot;
# fn go(bytes: &[u8]) {
let reg = lower_module(&Module::decode(bytes).unwrap()).unwrap();
let artifact = aot::serialize(&reg);
# }
```

## Load

```rust,no_run
# use baedeker_core::runtime::{Store, execute_export, Value};
# use baedeker_core::aot;
# fn go(artifact: &[u8]) {
let reg = aot::deserialize(artifact).expect("valid artifact");
let store = Store::instantiate(&reg).unwrap();
let _ = execute_export(&reg, &store, "add", &[Value::I32(1), Value::I32(2)]);
# }
```

Deserialization **skips validation** — the artifact is trusted. The FFI mirrors
this with `baedeker_module_from_aot`.

## Version-locking

AOT artifacts are version-locked: the magic and version prefix let a loader
reject artifacts from an incompatible Baedeker version rather than
misinterpreting them. Regenerate artifacts when bumping the IR.
