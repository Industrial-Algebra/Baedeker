# Embedding Model

Baedeker's central design constraint is that the engine core is `no_std` +
`alloc`. Everything that needs an operating system lives in a std-bearing
wrapper crate.

## The no_std core

`baedeker-core` uses no filesystem, no threads, and no OS services — only
`alloc` for heap data structures. This means the same engine that runs in a
server process also runs:

- inside an iOS app (linked as a static library, called from Swift),
- on bare metal (no allocator surprises, explicit resource limits),
- inside another WebAssembly runtime (Baedeker compiling to WASM, a future
  target).

All public APIs use owned types or explicit lifetimes. Error types carry byte
offsets into the original binary and structured context, so diagnostics survive
the no_std boundary.

## The wrapper crates

| Crate | Role | std? |
|---|---|---|
| `baedeker-ffi` | C ABI (`staticlib`/`cdylib`), opaque handles, cbindgen header | yes |
| `baedeker-cli` | command-line harness and runnable examples | yes |
| `baedeker-borsalino` | GPU compute backend adapter (Vulkan/Metal) | yes |
| `baedeker-gpu` | GPU compute host module (importable `baedeker:gpu` ABI) | yes |
| `baedeker-testdata` | spec fixtures and test WASM (dev-only, not published) | yes |

The host decides which wrappers to pull in. A server embedder uses the CLI or
FFI; an Apple embedder uses the FFI behind a Swift package; a GPU workload
adds the Borsalino adapter and the GPU host module.

## Resource limits

Because the core is untrusted-input-facing, it carries explicit limits:
interpreter fuel (`Store::set_fuel`), a maximum call depth, and bounds-checked
memory/table access that traps rather than escapes. These are the levers an
embedder uses to run untrusted modules safely. See
[Security Considerations](../design/security.md).
