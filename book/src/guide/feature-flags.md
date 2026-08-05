# Installing and Feature Flags

## Workspace crates

Baedeker is published to crates.io as a set of crates. Pull in the one that
matches your embedding target:

```toml
# The engine itself (no_std + alloc).
baedeker-core = "0.1"

# C ABI for host embedding (staticlib/cdylib).
baedeker-ffi = "0.1"

# Optional GPU offload.
baedeker-borsalino = "0.1"
baedeker-gpu = "0.1"
```

Install the command-line harness with `cargo install baedeker-cli`.

## `baedeker-core` features

| Feature | Enables | Default |
|---|---|---|
| *(none)* | the `no_std` + `alloc` engine only | yes |
| `std` | links the standard library (for std-bearing hosts) | off |
| `serde` | serde derives on the register IR (used by tooling) | off |
| `aot` | ahead-of-time artifact envelope (`serde` + postcard) | off |

Features are additive and independent. A no_std embedder uses the defaults; a
host that wants ahead-of-time loading enables `aot`; a std embedder enables
`std`.

## `baedeker-borsalino` features

| Feature | Enables |
|---|---|
| `vulkan` (default off-Linux/macOS) | the Vulkan backend |
| `metal` (default on macOS) | the Metal backend |

The adapter selects a backend at compile time by target OS; both pull
[Borsalino](https://crates.io/crates/borsalino) from crates.io.

## Rust toolchain

Baedeker targets the stable toolchain with the `wasm32-unknown-unknown` target
available. `rust-toolchain.toml` pins the channel; edition 2024 requires Rust
1.85 or later.
