# Architecture

Baedeker is a pipeline runtime: a WebAssembly binary flows through four stages,
each producing a richer representation, until it reaches an executable register
IR.

```
 .wasm bytes
     │
     ▼
┌─────────┐     ┌───────────┐     ┌────────┐     ┌───────────┐
│ Decode  │────▶│ Validate  │────▶│ Lower  │────▶│ Execute   │
└─────────┘     └───────────┘     └────────┘     └───────────┘
  binary           spec rules     stack→reg       register IR
  parsing          + types        IR lowering     interpreter
```

## Decode

[`Module::decode`](https://docs.rs/baedeker-core) parses the WebAssembly binary
format into a typed `Module`: types, imports, functions, tables, memories,
globals, elements, data, exports, code bodies. Malformed input produces a
structured decode error carrying the byte offset.

## Validate

`Module::validate` checks the module against the WebAssembly specification:
type checking, reference subtyping, block result types, init-expression
correctness, and structural constraints. This is where the bulk of spec
conformance lives — and where the official test suite exercises the engine.

## Lower

`lower_module` translates the stack-machine bytecode into a **register IR**: a
representation better suited to direct execution than the operand stack.
Control flow becomes explicit blocks with phi-copy joins; values flow through
registers rather than an implicit stack. See [The Register IR](./register-ir.md).

## Execute

`Store::instantiate` resolves imports and builds the runtime state (memories,
tables, globals, funcref identity). `execute_export` then runs the register IR
on a self-contained interpreter. Execution is fuel-bound and produces
categorised traps (out-of-bounds memory, integer overflow, call exhaustion,
unreachable, …).

## Workspace layout

```
crates/
├── baedeker-core/       no_std engine — decode, validate, lower, execute
├── baedeker-ffi/        C ABI (staticlib/cdylib) for host embedding
├── baedeker-cli/        command-line harness + examples
├── baedeker-borsalino/  optional GPU offload (via Borsalino)
├── baedeker-gpu/        GPU compute host module (importable baedeker:gpu ABI)
└── baedeker-testdata/   spec fixtures, incl. the vendored official suite
```

The `no_std` core holds no GPU, FFI, or OS code; those live in the std-bearing
crates that wrap it. See [Embedding Model](./embedding.md).
