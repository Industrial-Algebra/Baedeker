// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GPU compute host module for the Baedeker WebAssembly runtime.
//!
//! Exposes a small, versioned import ABI under the module name `baedeker:gpu`
//! that WASM guest modules can import to dispatch GPU compute kernels via a
//! [`GpuBackend`] (Borsalino over Vulkan/Metal in production). The host module
//! owns its own backend and handle tables, and captures the guest's linear
//! memory so kernels can be fed from and read back into guest memory.
//!
//! # v1 ABI constraints
//!
//! The backing [`GpuBackend`] trait offers whole-buffer upload (on create) and
//! full readback, but no partial writes or explicit buffer/kernel destruction.
//! Accordingly the v1 ABI is upload-on-create + read-back: an input buffer is
//! uploaded from guest memory in one shot, an output buffer is allocated
//! uninitialised, dispatches mutate GPU-resident state in place, and results
//! are read back at the end. Re-uploading mid-computation leaks the prior
//! buffer until the instance (and thus the backend) is dropped — a v2 concern
//! once the trait gains a partial-write method.
//!
//! All imports use `i32` operands; handles are non-negative indices, and every
//! function returns `-1` (or a non-positive length for `gpu_last_error`) on
//! failure with a diagnostic stashed for `gpu_last_error`. Bounds are checked
//! against guest memory and buffer sizes so an untrusted guest cannot escape.
//!
//! # Example
//!
//! ```no_run
//! use baedeker_core::runtime::Store;
//! use baedeker_gpu::GpuHostModule;
//! # fn make_backend() -> Box<dyn baedeker_core::runtime::gpu::GpuBackend> { unimplemented!() }
//! # fn example(reg: &baedeker_core::lower::RegModule, store: &mut Store) {
//! let gpu = GpuHostModule::for_store(make_backend(), store).unwrap();
//! gpu.register(store, reg).unwrap();
//! # }
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::lower::RegModule;
use baedeker_core::runtime::gpu::{GpuBackend, GpuBufferId, GpuError, GpuErrorKind, GpuKernelId};
use baedeker_core::runtime::{HostFunction, RuntimeError, Store};

mod abi;

/// Import module name all v1 GPU functions live under.
pub const MODULE: &str = "baedeker:gpu";

/// A GPU compute host module attached to a single instance.
///
/// Owns a backend and per-instance handle tables. Built once and registered
/// against a [`Store`] via [`register`](Self::register); each imported
/// function becomes an independent host closure sharing the tables through
/// interior mutability.
pub struct GpuHostModule {
    state: Rc<RefCell<GpuState>>,
    memory: Rc<RefCell<Vec<u8>>>,
}

/// Interior-mutable state shared across every registered host function.
pub(crate) struct GpuState {
    backend: Box<dyn GpuBackend>,
    buffers: Vec<Option<(GpuBufferId, usize)>>,
    kernels: Vec<Option<GpuKernelId>>,
    last_error: String,
}

impl GpuHostModule {
    /// Build a host module from an owned backend and a captured memory handle.
    pub fn new(backend: Box<dyn GpuBackend>, memory: Rc<RefCell<Vec<u8>>>) -> Self {
        Self {
            state: Rc::new(RefCell::new(GpuState::new(backend))),
            memory,
        }
    }

    /// Build a host module capturing the instance's linear memory 0.
    ///
    /// Fails with [`GpuErrorKind::Unavailable`] if the module declares no
    /// linear memory at index 0.
    pub fn for_store(backend: Box<dyn GpuBackend>, store: &Store) -> Result<Self, GpuError> {
        let memory = store.shared_memory(0).ok_or_else(|| GpuError {
            kind: GpuErrorKind::Unavailable,
            message: "module declares no linear memory at index 0".into(),
        })?;
        Ok(Self::new(backend, memory))
    }

    /// Register every `baedeker:gpu` import the module declares.
    ///
    /// Functions the module does not import are skipped silently; the rest are
    /// registered with signatures checked against the import declarations.
    pub fn register(&self, store: &mut Store, module: &RegModule) -> Result<(), RuntimeError> {
        self.register_if(store, module, "gpu_probe", abi::make_probe)?;
        self.register_if(store, module, "buffer_create", abi::make_buffer_create)?;
        self.register_if(store, module, "buffer_upload", abi::make_buffer_upload)?;
        self.register_if(store, module, "buffer_read", abi::make_buffer_read)?;
        self.register_if(store, module, "kernel_create", abi::make_kernel_create)?;
        self.register_if(store, module, "dispatch", abi::make_dispatch)?;
        self.register_if(store, module, "gpu_last_error", abi::make_last_error)?;
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn register_if(
        &self,
        store: &mut Store,
        module: &RegModule,
        name: &str,
        build: fn(Rc<RefCell<GpuState>>, Rc<RefCell<Vec<u8>>>) -> HostFunction,
    ) -> Result<(), RuntimeError> {
        let imported = module
            .imported_funcs
            .iter()
            .any(|import| import.module == MODULE && import.name == name);
        if !imported {
            return Ok(());
        }
        store.register_host_func(MODULE, name, build(self.state.clone(), self.memory.clone()))
    }
}

impl GpuState {
    fn new(backend: Box<dyn GpuBackend>) -> Self {
        Self {
            backend,
            buffers: Vec::new(),
            kernels: Vec::new(),
            last_error: String::new(),
        }
    }

    fn alloc_buffer(&mut self, id: GpuBufferId, size: usize) -> u32 {
        let handle = self.buffers.len() as u32;
        self.buffers.push(Some((id, size)));
        handle
    }

    fn alloc_kernel(&mut self, id: GpuKernelId) -> u32 {
        let handle = self.kernels.len() as u32;
        self.kernels.push(Some(id));
        handle
    }

    fn buffer(&self, handle: i32) -> Option<(GpuBufferId, usize)> {
        if handle < 0 {
            return None;
        }
        self.buffers.get(handle as usize).and_then(|entry| *entry)
    }

    fn kernel(&self, handle: i32) -> Option<GpuKernelId> {
        if handle < 0 {
            return None;
        }
        self.kernels.get(handle as usize).copied().flatten()
    }
}
