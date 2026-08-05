# GPU Offload

Baedeker can dispatch bulk SIMD work to GPU compute via
[Borsalino](https://crates.io/crates/borsalino), a thin GPU abstraction over
Vulkan (Linux/Windows) and Metal (macOS). There are two distinct surfaces.

## Level-1 SIMD offload (engine-internal)

`baedeker-core`'s runtime optionally holds a `GpuBackend` slot. Large elementwise
operations — currently `Store::f32_add_region` — dispatch to the GPU when the
element count exceeds an offload threshold, falling back to CPU otherwise.
Below the threshold, dispatch overhead dominates, so the register IR executes
element-wise on CPU. The threshold is tunable per host.

This is transparent to guest modules: a WASM program that adds two large f32
regions simply runs faster when a GPU is attached.

## The GPU host module (`baedeker-gpu`)

For guest modules that want explicit GPU compute, `baedeker-gpu` exposes a
versioned import ABI under the module name `baedeker:gpu`. A guest imports
`buffer_create`, `buffer_upload`, `kernel_create`, `dispatch`, `buffer_read`,
and friends; the host module owns a `GpuBackend` and per-instance handle tables,
and captures the guest's linear memory so kernels read from and write back into
it.

```wat
(import "baedeker:gpu" "buffer_upload" (func $upload (param i32 i32) (result i32)))
(import "baedeker:gpu" "dispatch" (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
```

All operands are `i32`; handles are non-negative indices; every function returns
`-1` on failure with a stashed diagnostic for `gpu_last_error`. Bounds are
checked against guest memory and buffer sizes.

## Layered verification

GPU dispatch is verified in two layers:

1. **`dispatch_verified`** (structural, uniform) — every dispatch carries an
   explicit `threads_per_group`; a workgroup-divisibility proof confirms the
   config is sound. This catches mis-dispatch of non-default `@workgroup_size`
   kernels.
2. **Exact-match numerical** (opt-in, on demand) — for known-linear kernels,
   binary `{0,1}` inputs are run through both the GPU kernel and an FP32 CPU
   reference and compared with bit-exact equality below the FP16 exact-integer
   ceiling (2048).

See [Verification & Hardening](../design/verification.md).
