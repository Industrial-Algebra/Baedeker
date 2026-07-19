//! Optional GPU backend slot for bulk SIMD offload.
//!
//! The trait defines the contract a backend (Borsalino/Metal on iOS, a
//! wgpu-based desktop backend, or a test fake) implements so the runtime
//! can dispatch bulk SIMD work to GPU compute. Backends are identified by
//! opaque handles, keeping the trait object-safe and FFI-friendly.
//!
//! See `docs/borsalino-integration.md` for the integration design.

use alloc::string::String;
use alloc::vec::Vec;

/// Opaque handle to a compiled compute kernel, owned by the backend.
pub type GpuKernelId = u64;

/// Opaque handle to a GPU buffer, owned by the backend.
pub type GpuBufferId = u64;

/// A GPU compute backend for bulk SIMD offload (Borsalino Level 1).
///
/// The runtime uses this to offload large vector operations: upload WASM
/// linear-memory regions to buffers, dispatch a pre-compiled WGSL kernel,
/// and read results back. Below the offload threshold the register IR
/// executes element-wise on CPU.
pub trait GpuBackend: core::fmt::Debug {
    /// Human-readable backend name for diagnostics.
    fn name(&self) -> &str;

    /// Compile a WGSL compute kernel (once, cached by the backend).
    fn compile(&mut self, name: &str, wgsl: &str) -> Result<GpuKernelId, GpuError>;

    /// Create a GPU buffer initialized with `data`.
    fn create_buffer(&mut self, data: &[u8]) -> Result<GpuBufferId, GpuError>;

    /// Create an uninitialized GPU buffer of `byte_len` bytes.
    fn create_buffer_uninit(&mut self, byte_len: usize) -> Result<GpuBufferId, GpuError>;

    /// Dispatch a kernel over `workgroups` (x, y, z) with `buffers` bound
    /// in order.
    fn dispatch(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
    ) -> Result<(), GpuError>;

    /// Read a buffer's full contents back to host memory.
    fn read_buffer(&mut self, buffer: GpuBufferId) -> Result<Vec<u8>, GpuError>;
}

/// GPU backend failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuError {
    pub kind: GpuErrorKind,
    pub message: String,
}

/// The category of a GPU backend failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuErrorKind {
    /// No GPU or driver available on this host.
    Unavailable,
    /// Kernel compilation failed.
    CompileFailed,
    /// Buffer allocation failed.
    OutOfMemory,
    /// Kernel dispatch failed.
    DispatchFailed,
    /// Buffer readback failed.
    ReadbackFailed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::RegModule;

    /// A recording fake backend for slot tests.
    #[derive(Debug, Default)]
    struct FakeBackend {
        compiled: Vec<String>,
        buffers: usize,
        dispatches: usize,
    }

    impl GpuBackend for FakeBackend {
        fn name(&self) -> &str {
            "fake"
        }

        fn compile(&mut self, name: &str, _wgsl: &str) -> Result<GpuKernelId, GpuError> {
            self.compiled.push(name.into());
            Ok(self.compiled.len() as u64)
        }

        fn create_buffer(&mut self, _data: &[u8]) -> Result<GpuBufferId, GpuError> {
            self.buffers += 1;
            Ok(self.buffers as u64)
        }

        fn create_buffer_uninit(&mut self, _byte_len: usize) -> Result<GpuBufferId, GpuError> {
            self.buffers += 1;
            Ok(self.buffers as u64)
        }

        fn dispatch(
            &mut self,
            _kernel: GpuKernelId,
            _buffers: &[GpuBufferId],
            _workgroups: [u32; 3],
        ) -> Result<(), GpuError> {
            self.dispatches += 1;
            Ok(())
        }

        fn read_buffer(&mut self, _buffer: GpuBufferId) -> Result<Vec<u8>, GpuError> {
            Ok(alloc::vec![0xAA; 16])
        }
    }

    fn empty_module() -> RegModule {
        RegModule {
            funcs: Vec::new(),
            exports: Vec::new(),
            imported_func_count: 0,
            memories: Vec::new(),
            globals: Vec::new(),
            tables: Vec::new(),
            elements: Vec::new(),
            types: Vec::new(),
            data: Vec::new(),
            imported_memory_count: 0,
            imported_global_count: 0,
            imported_table_count: 0,
        }
    }

    #[test]
    fn store_has_no_gpu_by_default() {
        let module = empty_module();
        let store = crate::runtime::Store::instantiate(&module).unwrap();
        assert!(store.gpu().is_none());
    }

    #[test]
    fn gpu_slot_accepts_and_exercises_a_backend() {
        let module = empty_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        store.set_gpu(Box::new(FakeBackend::default()));
        assert_eq!(store.gpu().map(GpuBackend::name), Some("fake"));

        let gpu = store.gpu_mut().expect("backend installed");
        let kernel = gpu.compile("vadd_f32x4", "@compute fn vadd() {}").unwrap();
        let buf_a = gpu.create_buffer(&[1, 2, 3, 4]).unwrap();
        let buf_out = gpu.create_buffer_uninit(16).unwrap();
        gpu.dispatch(kernel, &[buf_a, buf_out], [1, 1, 1]).unwrap();
        assert_eq!(gpu.read_buffer(buf_out).unwrap(), alloc::vec![0xAA; 16]);

        store.clear_gpu();
        assert!(store.gpu().is_none());
    }
}
