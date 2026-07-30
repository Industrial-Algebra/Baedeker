# GPU host module (`baedeker:gpu`)

`baedeker-gpu` exposes a versioned import ABI under the module name
`baedeker:gpu` that WASM guest modules import to dispatch GPU compute kernels
through a `GpuBackend` (Borsalino over Vulkan/Metal in production). The host
module owns its own backend and per-instance handle tables, and captures the
guest's linear memory so kernels can be fed from and read back into guest
memory.

## Embedding

```rust,ignore
use baedeker_core::runtime::Store;
use baedeker_gpu::GpuHostModule;

let gpu = GpuHostModule::for_store(make_backend(), &store)?;
gpu.register(&mut store, &reg)?;
```

`register` wires up every `baedeker:gpu` import the module declares; functions
the module does not import are skipped. The backend is owned by the host module
(separate from the store's Level-1 SIMD-offload slot).

## v1 ABI

All imports use `i32` operands. Handles are non-negative indices. Every
function returns `-1` (or a non-positive length for `gpu_last_error`) on
failure, stashing a diagnostic for `gpu_last_error`. Bounds are checked against
guest memory and buffer sizes.

| Import | Signature | Returns |
| --- | --- | --- |
| `gpu_probe` | `() -> i32` | `1` (backend attached) |
| `buffer_create` | `(size: i32) -> i32` | uninitialised output-buffer handle |
| `buffer_upload` | `(mem_offset: i32, len: i32) -> i32` | buffer handle, uploaded from guest memory |
| `buffer_read` | `(handle, buf_offset, mem_offset, len: i32) -> i32` | `0` on success |
| `kernel_create` | `(code_ptr, code_len: i32) -> i32` | kernel handle (WGSL is UTF-8 in guest memory) |
| `dispatch` | `(kernel, wg_x, wg_y, wg_z, tpg_x, tpg_y, tpg_z, bindings_ptr, bindings_len: i32) -> i32` | `0` on success |
| `gpu_last_error` | `(mem_offset, max_len: i32) -> i32` | bytes written |

`dispatch` binds `bindings_len` buffers whose `u32` handles are read
little-endian from guest memory at `bindings_ptr`, and routes through
`GpuBackend::dispatch_verified` so the explicit per-workgroup thread count is
honoured (instead of the backend's 256-thread default).

### v1 constraints

The backing `GpuBackend` trait offers whole-buffer upload (on create) and full
readback, but **no partial writes or explicit buffer/kernel destruction**.
Accordingly v1 is upload-on-create + read-back: upload inputs in one shot,
allocate uninitialised outputs, let dispatches mutate GPU-resident state in
place, and read results back at the end. Re-uploading mid-computation leaks the
prior buffer until the instance (and thus the backend) is dropped — a v2
concern once the trait gains a partial-write method.

## Guest example

```wat
(module
  (import "baedeker:gpu" "gpu_probe" (func $probe (result i32)))
  (import "baedeker:gpu" "buffer_upload" (func $upload (param i32 i32) (result i32)))
  (import "baedeker:gpu" "buffer_create" (func $create (param i32) (result i32)))
  (import "baedeker:gpu" "kernel_create" (func $kcreate (param i32 i32) (result i32)))
  (import "baedeker:gpu" "dispatch"
    (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "baedeker:gpu" "buffer_read" (func $bread (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "@compute @workgroup_size(1) fn k(){}")
  (func (export "run") (result i32)
    ;; ...upload inputs, create outputs, build bindings, dispatch, read back...
    (i32.const 0)
  )
)
```

## Verification roadmap

This is Layer 1 (uniform dispatch with the workgroup-divisibility proof). Layer
2 — numerical correctness via Borsalino's `verify_numerical` (DeepReinforce
exact-match protocol) — applies opt-in to known-linear kernels and is a
follow-up slice.
