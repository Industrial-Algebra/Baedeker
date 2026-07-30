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
///
/// Field order matters: Rust drops struct fields in declaration order, so
/// the pipeline and buffer registries MUST be dropped before `inner` (the
/// backend device) — destroying a buffer or pipeline requires a live device.
pub struct BorsalinoGpu<B: borsalino::GpuBackend> {
    pipelines: HashMap<GpuKernelId, borsalino::ComputePipeline>,
    buffers: HashMap<GpuBufferId, borsalino::GpuBuffer>,
    inner: B,
    next_id: u64,
}

impl<B: borsalino::GpuBackend> BorsalinoGpu<B> {
    /// Wrap an initialized Borsalino backend.
    pub fn new(inner: B) -> Self {
        Self {
            pipelines: HashMap::new(),
            buffers: HashMap::new(),
            inner,
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Resolve a kernel handle and bound-buffer handles into the borrowed
    /// Borsalino objects, in dispatch order. Shared by `dispatch` and
    /// `dispatch_verified`.
    fn resolve_dispatch(
        &self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
    ) -> Result<(&borsalino::ComputePipeline, Vec<&borsalino::GpuBuffer>), GpuError> {
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
        Ok((pipeline, refs))
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
        let (pipeline, refs) = self.resolve_dispatch(kernel, buffers)?;
        self.inner
            .dispatch(
                pipeline,
                &refs,
                (workgroups[0], workgroups[1], workgroups[2]),
            )
            .map_err(map_error)
    }

    /// Dispatch through Borsalino's proof-gated `dispatch_verified`.
    ///
    /// The divisibility proof is built from the x-dimension product
    /// `workgroups[0] * threads_per_group[0]`, which is divisible by
    /// `threads_per_group[0]` by construction; the proof is structural
    /// confirmation. The substantive guarantee is that the explicit
    /// `threads_per_group` is honoured (via `dispatch_ex`) instead of the
    /// backend's 256-thread default, so non-default `@workgroup_size`
    /// kernels dispatch correctly.
    fn dispatch_verified(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
        threads_per_group: [u32; 3],
    ) -> Result<(), GpuError> {
        let (pipeline, refs) = self.resolve_dispatch(kernel, buffers)?;
        let proof = proof_for(workgroups, threads_per_group)?;
        self.inner
            .dispatch_verified(
                pipeline,
                &refs,
                (workgroups[0], workgroups[1], workgroups[2]),
                (
                    threads_per_group[0],
                    threads_per_group[1],
                    threads_per_group[2],
                ),
                &proof,
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

/// Construct Borsalino's workgroup-divisibility proof for a dispatch.
///
/// The proof checks the x-dimension product
/// `total_threads = workgroups[0] * threads_per_group[0]` is divisible by
/// `threads_per_group[0]`, matching Borsalino's 1-D proof scope. With the
/// product form this holds structurally for well-formed configs; the
/// observable failure mode here is x-dimension overflow. Extracted as a
/// pure function so the overflow path is unit-testable without a GPU.
///
/// The substantive reason [`BorsalinoGpu`] routes through `dispatch_verified`
/// is that the explicit `threads_per_group` is honoured (via `dispatch_ex`)
/// rather than the backend's 256-thread default, so non-default
/// `@workgroup_size` kernels dispatch correctly.
fn proof_for(
    workgroups: [u32; 3],
    threads_per_group: [u32; 3],
) -> Result<borsalino::WorkgroupProof, GpuError> {
    let total_threads = workgroups[0]
        .checked_mul(threads_per_group[0])
        .ok_or(GpuError {
            kind: GpuErrorKind::DispatchFailed,
            message: "workgroups[0] * threads_per_group[0] overflows u32".into(),
        })?;
    borsalino::DispatchConfig {
        total_threads,
        threads_per_group: threads_per_group[0],
    }
    .verify()
    .map_err(|error| GpuError {
        kind: GpuErrorKind::DispatchFailed,
        message: format!("indivisible dispatch config: {error:?}"),
    })
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

    /// Hardware integration test: run explicitly with `--ignored`. Verified
    /// working end-to-end on lavapipe (Mesa software Vulkan, VM) — the
    /// v0.5.0-era SIGSEGV was this adapter's field drop order (registries
    /// outliving the device), fixed by dropping pipelines/buffers before
    /// the backend. Remains ignored by default so GPU-less CI skips
    /// cleanly (`init()` returns `Err`).
    #[test]
    #[ignore = "requires a working GPU driver; verified on lavapipe — run with --ignored"]
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
        // tpg=[1,1,1] matches the kernel's @workgroup_size(1).
        gpu.dispatch_verified(kernel, &[buf], [1, 1, 1], [1, 1, 1])
            .unwrap();
        let result = gpu.read_buffer(buf).unwrap();
        assert_eq!(u32::from_le_bytes(result[..4].try_into().unwrap()), 42);
    }

    /// The divisibility proof accepts a well-formed x-dimension product.
    #[test]
    fn proof_for_accepts_well_formed_dispatch() {
        // workgroups=4 × tpg=128 → total 512, divisible by 128.
        assert!(proof_for([4, 1, 1], [128, 1, 1]).is_ok());
        // Default 256-thread workgroup, single group.
        assert!(proof_for([1, 1, 1], [256, 1, 1]).is_ok());
    }

    /// x-dimension overflow surfaces as a categorized dispatch error rather
    /// than panicking or silently wrapping.
    #[test]
    fn proof_for_rejects_x_dimension_overflow() {
        let err = proof_for([u32::MAX, 1, 1], [2, 1, 1]).err().unwrap();
        assert_eq!(err.kind, GpuErrorKind::DispatchFailed);
        assert!(err.message.contains("overflows"));
    }
}
