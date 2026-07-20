// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: MIT

//! Borsalino GPU backend adapter for Baedeker.
//!
//! Bridges the handle-based [`GpuBackend`] trait Baedeker's engine state
//! expects onto Borsalino's object-based Vulkan/Metal backends. Pipelines
//! and buffers live in a registry keyed by opaque u64 handles.
//!
//! ```ignore
//! let gpu = baedeker_borsalino::BorsalinoGpu::new(borsalino::init()?);
//! store.set_gpu(Box::new(gpu));
//! ```

use std::collections::HashMap;

use baedeker_core::runtime::gpu::{GpuBackend, GpuBufferId, GpuError, GpuErrorKind, GpuKernelId};

/// A [`GpuBackend`] implementation backed by Borsalino.
pub struct BorsalinoGpu<B: borsalino::GpuBackend> {
    inner: B,
    next_id: u64,
    pipelines: HashMap<GpuKernelId, borsalino::ComputePipeline>,
    buffers: HashMap<GpuBufferId, borsalino::GpuBuffer>,
}

impl<B: borsalino::GpuBackend> BorsalinoGpu<B> {
    /// Wrap an initialized Borsalino backend.
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            next_id: 1,
            pipelines: HashMap::new(),
            buffers: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl<B: borsalino::GpuBackend> core::fmt::Debug for BorsalinoGpu<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BorsalinoGpu")
            .field("pipelines", &self.pipelines.len())
            .field("buffers", &self.buffers.len())
            .finish()
    }
}

impl<B: borsalino::GpuBackend> GpuBackend for BorsalinoGpu<B> {
    fn name(&self) -> &str {
        "borsalino"
    }

    fn compile(&mut self, name: &str, wgsl: &str) -> Result<GpuKernelId, GpuError> {
        let pipeline = self.inner.compile(name, wgsl).map_err(map_error)?;
        let id = self.alloc_id();
        self.pipelines.insert(id, pipeline);
        Ok(id)
    }

    fn create_buffer(&mut self, data: &[u8]) -> Result<GpuBufferId, GpuError> {
        let buffer = self.inner.create_buffer(data).map_err(map_error)?;
        let id = self.alloc_id();
        self.buffers.insert(id, buffer);
        Ok(id)
    }

    fn create_buffer_uninit(&mut self, byte_len: usize) -> Result<GpuBufferId, GpuError> {
        let buffer = self
            .inner
            .create_buffer_uninit::<u8>(byte_len)
            .map_err(map_error)?;
        let id = self.alloc_id();
        self.buffers.insert(id, buffer);
        Ok(id)
    }

    fn dispatch(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
    ) -> Result<(), GpuError> {
        let pipeline = self.pipelines.get(&kernel).ok_or(GpuError {
            kind: GpuErrorKind::DispatchFailed,
            message: format!("unknown kernel handle {kernel}"),
        })?;
        let mut refs = Vec::with_capacity(buffers.len());
        for id in buffers {
            let buffer = self.buffers.get(id).ok_or(GpuError {
                kind: GpuErrorKind::DispatchFailed,
                message: format!("unknown buffer handle {id}"),
            })?;
            refs.push(buffer);
        }
        self.inner
            .dispatch(
                pipeline,
                &refs,
                (workgroups[0], workgroups[1], workgroups[2]),
            )
            .map_err(map_error)
    }

    fn read_buffer(&mut self, buffer: GpuBufferId) -> Result<Vec<u8>, GpuError> {
        let buffer = self.buffers.get(&buffer).ok_or(GpuError {
            kind: GpuErrorKind::ReadbackFailed,
            message: format!("unknown buffer handle {buffer}"),
        })?;
        self.inner.read_buffer::<u8>(buffer).map_err(map_error)
    }
}

/// Map a Borsalino error into Baedeker's categorized GPU error.
fn map_error(error: borsalino::GpuError) -> GpuError {
    let kind = match &error {
        borsalino::GpuError::NoBackend | borsalino::GpuError::InitFailed(_) => {
            GpuErrorKind::Unavailable
        }
        borsalino::GpuError::CompileFailed { .. } | borsalino::GpuError::PipelineFailed { .. } => {
            GpuErrorKind::CompileFailed
        }
        borsalino::GpuError::BufferCreationFailed { .. } => GpuErrorKind::OutOfMemory,
        borsalino::GpuError::BufferReadFailed { .. } => GpuErrorKind::ReadbackFailed,
        borsalino::GpuError::DispatchFailed { .. }
        | borsalino::GpuError::InvalidBinding { .. }
        | borsalino::GpuError::Internal(_)
        | borsalino::GpuError::Io(_) => GpuErrorKind::DispatchFailed,
    };
    GpuError {
        kind,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hardware integration test: run explicitly with `--ignored` on a
    /// machine with a working Vulkan/Metal driver. NOTE: borsalino's
    /// Vulkan init currently SIGSEGVs under Mesa ICD setups (tracked in
    /// the Borsalino repo); ignored by default so CI stays green.
    #[test]
    #[ignore = "requires a working GPU driver; see Borsalino init segfault note"]
    fn real_backend_dispatch_when_gpu_available() {
        let Ok(backend) = borsalino::init() else {
            eprintln!("no GPU backend available; skipping hardware test");
            return;
        };
        let mut gpu = BorsalinoGpu::new(backend);

        let wgsl = r#"
            @group(0) @binding(0) var<storage, read_write> data: array<u32>;
            @compute @workgroup_size(1)
            fn bump() {
                data[0] = data[0] + 1u;
            }
        "#;
        let kernel = gpu.compile("bump", wgsl).unwrap();
        let buf = gpu.create_buffer(&41u32.to_le_bytes()).unwrap();
        gpu.dispatch(kernel, &[buf], [1, 1, 1]).unwrap();
        let result = gpu.read_buffer(buf).unwrap();
        assert_eq!(u32::from_le_bytes(result[..4].try_into().unwrap()), 42);
    }
}
