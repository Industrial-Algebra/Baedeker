# GPU Host Module API

`baedeker_gpu` exposes the `baedeker:gpu` import ABI as a host module a guest
WASM module can import for explicit GPU compute.

## Building and registering

```rust,ignore
use baedeker_core::runtime::Store;
use baedeker_gpu::GpuHostModule;
# fn example(store: &mut Store, reg: &baedeker_core::lower::RegModule, backend: ()) {
#   // let backend: Box<dyn baedeker_core::runtime::gpu::GpuBackend> = /* ... */;
#   let backend: Box<dyn baedeker_core::runtime::gpu::GpuBackend> = unimplemented!();
let gpu = GpuHostModule::for_store(backend, store).unwrap();
gpu.register(store, reg).unwrap();
# }
```

`for_store` captures the instance's linear memory 0 (it fails if the module
declares none). `register` wires up every `baedeker:gpu` import the module
declares and silently skips the rest.

## The v1 ABI

All imports take `i32` operands; handles are non-negative indices. Every
function returns `-1` (or a non-positive length for `gpu_last_error`) on
failure, stashing a diagnostic.

| Import | Signature |
|---|---|
| `gpu_probe` | `() -> i32` |
| `buffer_create` | `(size) -> handle` |
| `buffer_upload` | `(mem_offset, len) -> handle` |
| `buffer_read` | `(handle, buf_offset, mem_offset, len) -> i32` |
| `kernel_create` | `(code_ptr, code_len) -> handle` |
| `dispatch` | `(kernel, wg_x, wg_y, wg_z, tpg_x, tpg_y, tpg_z, bindings_ptr, bindings_len) -> i32` |
| `gpu_last_error` | `(mem_offset, max_len) -> bytes_written` |

`dispatch` routes through `GpuBackend::dispatch_verified` so the explicit
per-workgroup thread count is honoured.

## v1 constraints

The backing `GpuBackend` trait offers whole-buffer upload (on create) and full
readback, but no partial writes or explicit destruction. v1 is therefore
upload-on-create + read-back: upload inputs, allocate an uninitialised output,
dispatch, read the result. Re-uploading mid-computation leaks the prior buffer
until the instance is dropped — a v2 concern once the trait gains a
partial-write method.
