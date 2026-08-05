// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

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
    ///
    /// Each workgroup runs the backend's default thread count. Prefer
    /// [`dispatch_verified`](GpuBackend::dispatch_verified) when the workgroup
    /// size is known, so non-default thread counts dispatch correctly.
    fn dispatch(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
    ) -> Result<(), GpuError>;

    /// Dispatch a kernel with an explicit per-workgroup thread count.
    ///
    /// Like [`dispatch`](GpuBackend::dispatch), but each workgroup runs
    /// `threads_per_group` threads rather than a backend-implied default.
    /// This matters for kernels whose WGSL declares a non-default
    /// `@workgroup_size`: `dispatch` silently uses the backend's default
    /// (256 for Borsalino), which mis-dispatches such kernels.
    ///
    /// Backends that verify workgroup divisibility (Borsalino's
    /// `dispatch_verified`) construct their proof from
    /// `(workgroups, threads_per_group)` here; the divisibility check runs
    /// on the x-dimension product `workgroups[0] * threads_per_group[0]`,
    /// matching Borsalino's 1-D proof scope.
    ///
    /// The default implementation forwards to [`dispatch`](GpuBackend::dispatch),
    /// ignoring `threads_per_group`. Concrete backends that honour explicit
    /// workgroup sizes override this.
    fn dispatch_verified(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
        threads_per_group: [u32; 3],
    ) -> Result<(), GpuError> {
        let _ = threads_per_group;
        self.dispatch(kernel, buffers, workgroups)
    }

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
    use alloc::boxed::Box;

    use super::*;
    use crate::lower::RegModule;

    /// Shared counters for observing a fake backend's activity.
    #[derive(Debug, Default)]
    struct FakeStats {
        compiled: usize,
        dispatches: usize,
    }

    /// A recording fake backend for slot tests.
    #[derive(Debug)]
    struct FakeBackend {
        stats: alloc::rc::Rc<core::cell::RefCell<FakeStats>>,
        buffers: Vec<(GpuBufferId, usize)>,
    }

    impl FakeBackend {
        fn new(stats: alloc::rc::Rc<core::cell::RefCell<FakeStats>>) -> Self {
            Self {
                stats,
                buffers: Vec::new(),
            }
        }
    }

    impl GpuBackend for FakeBackend {
        fn name(&self) -> &str {
            "fake"
        }

        fn compile(&mut self, _name: &str, _wgsl: &str) -> Result<GpuKernelId, GpuError> {
            let mut stats = self.stats.borrow_mut();
            stats.compiled += 1;
            Ok(stats.compiled as u64)
        }

        fn create_buffer(&mut self, data: &[u8]) -> Result<GpuBufferId, GpuError> {
            let id = self.buffers.len() as u64 + 1;
            self.buffers.push((id, data.len()));
            Ok(id)
        }

        fn create_buffer_uninit(&mut self, byte_len: usize) -> Result<GpuBufferId, GpuError> {
            let id = self.buffers.len() as u64 + 1;
            self.buffers.push((id, byte_len));
            Ok(id)
        }

        fn dispatch(
            &mut self,
            _kernel: GpuKernelId,
            _buffers: &[GpuBufferId],
            _workgroups: [u32; 3],
        ) -> Result<(), GpuError> {
            self.stats.borrow_mut().dispatches += 1;
            Ok(())
        }

        fn read_buffer(&mut self, buffer: GpuBufferId) -> Result<Vec<u8>, GpuError> {
            let (_, size) = self
                .buffers
                .iter()
                .find(|(id, _)| *id == buffer)
                .expect("buffer exists");
            Ok(alloc::vec![0xAA; *size])
        }
    }

    fn empty_module() -> RegModule {
        RegModule {
            funcs: Vec::new(),
            exports: Vec::new(),
            imported_func_count: 0,
            imported_funcs: Vec::new(),
            imported_memories: Vec::new(),
            imported_globals: Vec::new(),
            imported_tables: Vec::new(),
            start: None,
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
        assert!(!store.has_gpu());
    }

    #[test]
    fn gpu_slot_accepts_and_exercises_a_backend() {
        let module = empty_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        let stats = alloc::rc::Rc::new(core::cell::RefCell::new(FakeStats::default()));
        store.set_gpu(Box::new(FakeBackend::new(stats.clone())));
        assert!(
            store
                .with_gpu_mut(|gpu| gpu.name() == "fake")
                .expect("backend installed")
        );

        let (kernel, buf_a, buf_out) = store
            .with_gpu_mut(|gpu| {
                let kernel = gpu.compile("vadd_f32x4", "@compute fn vadd() {}")?;
                let buf_a = gpu.create_buffer(&[1, 2, 3, 4])?;
                let buf_out = gpu.create_buffer_uninit(16)?;
                Ok::<_, GpuError>((kernel, buf_a, buf_out))
            })
            .expect("backend installed")
            .unwrap();
        store
            .with_gpu_mut(|gpu| gpu.dispatch(kernel, &[buf_a, buf_out], [1, 1, 1]))
            .expect("backend installed")
            .unwrap();
        let result = store
            .with_gpu_mut(|gpu| gpu.read_buffer(buf_out))
            .expect("backend installed")
            .unwrap();
        assert_eq!(result, alloc::vec![0xAA; 16]);

        store.clear_gpu();
        assert!(!store.has_gpu());
    }

    #[test]
    fn dispatch_verified_default_delegates_to_dispatch() {
        // A backend that only implements `dispatch` (the required method)
        // inherits the trait default for `dispatch_verified`, which must
        // forward to `dispatch` and accept an arbitrary `threads_per_group`.
        let module = empty_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        let stats = alloc::rc::Rc::new(core::cell::RefCell::new(FakeStats::default()));
        store.set_gpu(Box::new(FakeBackend::new(stats.clone())));
        let (kernel, buf) = store
            .with_gpu_mut(|gpu| {
                let kernel = gpu.compile("k", "@compute fn k() {}")?;
                let buf = gpu.create_buffer(&[0; 4])?;
                Ok::<_, GpuError>((kernel, buf))
            })
            .expect("backend installed")
            .unwrap();
        // Non-default threads_per_group must be accepted by the default impl.
        store
            .with_gpu_mut(|gpu| gpu.dispatch_verified(kernel, &[buf], [4, 1, 1], [128, 1, 1]))
            .expect("backend installed")
            .unwrap();
        assert_eq!(
            stats.borrow().dispatches,
            1,
            "default forwarded to dispatch"
        );
    }

    fn memory_module() -> RegModule {
        let mut module = empty_module();
        module.memories.push(crate::types::MemType {
            limits: crate::types::Limits { min: 1, max: None },
        });
        module
    }

    /// Write an f32 into the store's memory 0.
    fn store_f32(store: &mut crate::runtime::Store, addr: u32, value: f32) {
        store
            .with_memory_mut(0, |mem| {
                mem[addr as usize..addr as usize + 4].copy_from_slice(&value.to_le_bytes());
            })
            .expect("memory 0");
    }

    fn read_f32(store: &crate::runtime::Store, addr: u32) -> f32 {
        store
            .with_memory(0, |mem| {
                f32::from_le_bytes(mem[addr as usize..addr as usize + 4].try_into().unwrap())
            })
            .expect("memory 0")
    }

    #[test]
    fn f32_add_region_below_threshold_uses_cpu() {
        let module = memory_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        store_f32(&mut store, 0, 1.5);
        store_f32(&mut store, 64, 2.25);
        let stats = alloc::rc::Rc::new(core::cell::RefCell::new(FakeStats::default()));
        store.set_gpu(Box::new(FakeBackend::new(stats.clone())));

        store.f32_add_region(0, 64, 128, 1).unwrap();

        assert_eq!(read_f32(&store, 128), 3.75);
        assert_eq!(
            stats.borrow().dispatches,
            0,
            "below threshold must not dispatch"
        );
    }

    #[test]
    fn f32_add_region_above_threshold_dispatches() {
        let module = memory_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        store.set_offload_threshold(4);
        let stats = alloc::rc::Rc::new(core::cell::RefCell::new(FakeStats::default()));
        store.set_gpu(Box::new(FakeBackend::new(stats.clone())));

        // 8 elements = 32 bytes per region, all within one page.
        store.f32_add_region(0, 256, 512, 8).unwrap();

        // The fake's readback fills the output region with 0xAA.
        let all_filled = store
            .with_memory(0, |mem| mem[512..512 + 32].iter().all(|byte| *byte == 0xAA))
            .expect("memory 0");
        assert!(all_filled);
        assert_eq!(stats.borrow().dispatches, 1);
        assert_eq!(stats.borrow().compiled, 1, "kernel compiled once");

        // A second call reuses the cached kernel.
        store.f32_add_region(0, 256, 512, 8).unwrap();
        assert_eq!(stats.borrow().compiled, 1, "kernel not recompiled");
        assert_eq!(stats.borrow().dispatches, 2);
    }

    #[test]
    fn f32_add_region_without_gpu_uses_cpu() {
        let module = memory_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        store_f32(&mut store, 0, 1.0);
        store_f32(&mut store, 64, 2.0);
        store.set_offload_threshold(0);

        store.f32_add_region(0, 64, 128, 1).unwrap();
        assert_eq!(read_f32(&store, 128), 3.0);
    }

    #[test]
    fn f32_add_region_out_of_bounds_traps() {
        let module = memory_module();
        let mut store = crate::runtime::Store::instantiate(&module).unwrap();
        let error = store.f32_add_region(0, 65533, 128, 1).unwrap_err();
        assert_eq!(
            error.kind,
            crate::runtime::RuntimeErrorKind::Trap(
                crate::runtime::RuntimeTrap::OutOfBoundsMemoryAccess
            )
        );
    }
}
