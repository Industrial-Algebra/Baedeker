# API Reference Overview

Baedeker's API is split across the workspace crates. This book documents the
shapes and usage patterns; for exhaustive signatures, the generated rustdoc
(`cargo doc --no-deps` or <https://docs.rs/baedeker-core>) is authoritative.

| Crate | Primary surface |
|---|---|
| [`baedeker-core`](./core.md) | `Module`, `validate`, `lower_module`, `Store`, `execute_export`, `HostFunction`, `Value` |
| [`baedeker-gpu`](./gpu.md) | `GpuHostModule`, the `baedeker:gpu` import ABI |
| `baedeker-ffi` | the C ABI (`baedeker.h`) |
| `baedeker-borsalino` | `BorsalinoGpu`, `verify_f32_add` |

## Conventions

- **Newtype indices** — `TypeIdx(u32)`, `FuncIdx(u32)`, `LabelIdx(u32)`, etc.
  rather than bare `u32`, so you cannot pass a function index where a type index
  is expected.
- **Exhaustive enums** — `Value`, `ValType`, trap kinds, and section kinds are
  enums, not boolean flags.
- **Owned or lifetime'd** — public APIs use owned types or explicit lifetime
  parameters; `Module<'a>` borrows the input bytes.
- **Structured errors** — decode/validate errors carry byte offsets; runtime
  errors are categorised traps.

The naming follows the WebAssembly spec: `FuncType`, `ValType`, `BlockType`,
`MemArg`, and so on. Internal IR types are prefixed (`RegInstr`, `RegBlock`,
`RegFunc`).
