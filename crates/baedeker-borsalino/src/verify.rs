// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: MIT

//! Numerical verification driver for baedeker's GPU offload kernels.
//!
//! This is Layer 2 of the "both, layered" GPU verification strategy. Layer 1
//! ([`GpuBackend::dispatch_verified`]) runs a uniform structural check on every
//! dispatch; this module runs the opt-in DeepReinforce exact-match numerical
//! protocol on demand, for known-linear kernels where the math can be proven.
//!
//! The pure protocol core — [`compare_outputs`], [`f32_add_reference`],
//! [`VerifyResult`], [`VerifyConfig`] — lives in [`baedeker_core::runtime::verify`]
//! (no_std, fully unit-tested, dependency-free). This module is the
//! **GPU-dependent driver**: it generates binary `{0,1}` inputs, compiles the
//! production WGSL kernel, dispatches it through a [`GpuBackend`], reads the
//! result back, and compares with bit-exact equality below the threshold.
//!
//! The driver is generic over [`GpuBackend`], so it works with any backend —
//! the real [`BorsalinoGpu`], a future wgpu adapter, or a recording fake in
//! tests. That makes the pass/fail logic testable without a GPU (the
//! hardware-dependent path is a single `#[ignore]` lavapipe test).
//!
//! [`compare_outputs`]: baedeker_core::runtime::verify::compare_outputs
//! [`f32_add_reference`]: baedeker_core::runtime::verify::f32_add_reference
//! [`VerifyResult`]: baedeker_core::runtime::verify::VerifyResult
//! [`VerifyConfig`]: baedeker_core::runtime::verify::VerifyConfig
//! [`dispatch_verified`]: baedeker_core::runtime::gpu::GpuBackend::dispatch_verified

use baedeker_core::runtime::gpu::{GpuBackend, GpuError};
use baedeker_core::runtime::verify::{
    F32_ADD_WGSL, VerifyConfig, VerifyResult, compare_outputs, f32_add_reference,
};
use rand::Rng;

/// Verify the `f32_add` SIMD offload kernel (`out[i] = a[i] + b[i]`) over
/// `len` f32 elements.
///
/// For each of `cfg.trials` trials: generates binary `{0, 1}` f32 inputs with
/// zero bias `cfg.p_zero`, computes the FP32 CPU reference, uploads `a`/`b`,
/// allocates an output buffer, dispatches the production [`F32_ADD_WGSL`]
/// kernel via [`dispatch_verified`](GpuBackend::dispatch_verified) (workgroups
/// sized exactly as the runtime does: `ceil(len/256) × 1 × 1`, 256 threads per
/// group), reads the output back, and compares with bit-exact equality at
/// positions where the reference is at or below `cfg.threshold`.
///
/// With binary inputs every output lies in `{0, 1, 2}`, all far below the
/// default [`FP_BINARY_THRESHOLD`](baedeker_core::runtime::verify::FP_BINARY_THRESHOLD),
/// so every position is checked.
///
/// Returns an aggregated result across all trials. `passed` is true only if
/// every checked position matched exactly across every trial.
pub fn verify_f32_add(
    gpu: &mut impl GpuBackend,
    len: usize,
    cfg: &VerifyConfig,
) -> Result<VerifyResult, GpuError> {
    // Compile once (the runtime caches similarly); verify the production source.
    let kernel = gpu.compile("vadd", F32_ADD_WGSL)?;
    let mut rng = rand::thread_rng();
    let mut agg = VerifyResult::empty_aggregator();

    for _ in 0..cfg.trials {
        let a = sample_binary(len, cfg.p_zero, &mut rng);
        let b = sample_binary(len, cfg.p_zero, &mut rng);
        let reference = f32_add_reference(&a, &b);

        let buf_a = gpu.create_buffer(&f32_slice_to_bytes(&a))?;
        let buf_b = gpu.create_buffer(&f32_slice_to_bytes(&b))?;
        let buf_out = gpu.create_buffer_uninit(len * 4)?;

        // Mirror the production dispatch sizing in `Store::f32_add_region`.
        let workgroups = [len.div_ceil(256) as u32, 1, 1];
        gpu.dispatch_verified(kernel, &[buf_a, buf_b, buf_out], workgroups, [256, 1, 1])?;

        let out_bytes = gpu.read_buffer(buf_out)?;
        let gpu_out = bytes_to_f32_slice(&out_bytes, len);
        agg.fold_trial(compare_outputs(&gpu_out, &reference, cfg.threshold));
    }

    Ok(agg)
}

/// Sample `n` binary `{0.0, 1.0}` f32 values: `0.0` with probability `p_zero`.
fn sample_binary(n: usize, p_zero: f32, rng: &mut impl Rng) -> Vec<f32> {
    (0..n)
        .map(|_| {
            if rng.gen_bool(p_zero as f64) {
                0.0
            } else {
                1.0
            }
        })
        .collect()
}

/// Encode an f32 slice as little-endian bytes.
fn f32_slice_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Decode `len` little-endian f32 values from a byte buffer.
fn bytes_to_f32_slice(bytes: &[u8], len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let start = i * 4;
            f32::from_le_bytes(bytes[start..start + 4].try_into().unwrap_or([0; 4]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use baedeker_core::runtime::gpu::{GpuBufferId, GpuKernelId};
    use std::collections::HashMap;

    /// A fake backend that faithfully computes `out[i] = a[i] + b[i]` on CPU,
    /// so `verify_f32_add` should report a pass. Purpose-built for the
    /// `f32_add` kernel: dispatch always performs an elementwise add of the
    /// first two bound buffers into the third.
    #[derive(Debug, Default)]
    struct CpuAddBackend {
        buffers: HashMap<GpuBufferId, Vec<u8>>,
        next: GpuBufferId,
    }

    impl CpuAddBackend {
        fn new() -> Self {
            Self {
                buffers: HashMap::new(),
                next: 1,
            }
        }
    }

    impl GpuBackend for CpuAddBackend {
        fn name(&self) -> &str {
            "cpu-add"
        }

        fn compile(&mut self, _name: &str, _wgsl: &str) -> Result<GpuKernelId, GpuError> {
            Ok(1)
        }

        fn create_buffer(&mut self, data: &[u8]) -> Result<GpuBufferId, GpuError> {
            let id = self.next;
            self.next += 1;
            self.buffers.insert(id, data.to_vec());
            Ok(id)
        }

        fn create_buffer_uninit(&mut self, byte_len: usize) -> Result<GpuBufferId, GpuError> {
            let id = self.next;
            self.next += 1;
            self.buffers.insert(id, vec![0; byte_len]);
            Ok(id)
        }

        fn dispatch(
            &mut self,
            _kernel: GpuKernelId,
            buffers: &[GpuBufferId],
            _workgroups: [u32; 3],
        ) -> Result<(), GpuError> {
            add_into_third(self, buffers)
        }

        fn read_buffer(&mut self, buffer: GpuBufferId) -> Result<Vec<u8>, GpuError> {
            Ok(self.buffers.get(&buffer).cloned().unwrap_or_default())
        }
    }

    /// Read buffers 0 and 1 as f32 slices, add elementwise into buffer 2.
    fn add_into_third(
        backend: &mut CpuAddBackend,
        buffers: &[GpuBufferId],
    ) -> Result<(), GpuError> {
        let a = backend
            .buffers
            .get(&buffers[0])
            .ok_or(GpuError {
                kind: baedeker_core::runtime::gpu::GpuErrorKind::DispatchFailed,
                message: "missing buffer a".into(),
            })?
            .clone();
        let b = backend
            .buffers
            .get(&buffers[1])
            .ok_or(GpuError {
                kind: baedeker_core::runtime::gpu::GpuErrorKind::DispatchFailed,
                message: "missing buffer b".into(),
            })?
            .clone();
        let out = backend.buffers.get_mut(&buffers[2]).ok_or(GpuError {
            kind: baedeker_core::runtime::gpu::GpuErrorKind::DispatchFailed,
            message: "missing output buffer".into(),
        })?;
        let count = out.len() / 4;
        for i in 0..count {
            let x = f32::from_le_bytes(a[i * 4..i * 4 + 4].try_into().unwrap());
            let y = f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
            out[i * 4..i * 4 + 4].copy_from_slice(&(x + y).to_le_bytes());
        }
        Ok(())
    }

    /// A backend that computes correctly but reads back all zeros — a
    /// systematically wrong kernel. `verify_f32_add` must detect it.
    #[derive(Debug, Default)]
    struct GarbageReadBackend {
        inner: CpuAddBackend,
    }

    impl GpuBackend for GarbageReadBackend {
        fn name(&self) -> &str {
            "garbage-read"
        }
        fn compile(&mut self, name: &str, wgsl: &str) -> Result<GpuKernelId, GpuError> {
            self.inner.compile(name, wgsl)
        }
        fn create_buffer(&mut self, data: &[u8]) -> Result<GpuBufferId, GpuError> {
            self.inner.create_buffer(data)
        }
        fn create_buffer_uninit(&mut self, byte_len: usize) -> Result<GpuBufferId, GpuError> {
            self.inner.create_buffer_uninit(byte_len)
        }
        fn dispatch(
            &mut self,
            kernel: GpuKernelId,
            buffers: &[GpuBufferId],
            workgroups: [u32; 3],
        ) -> Result<(), GpuError> {
            self.inner.dispatch(kernel, buffers, workgroups)
        }
        fn dispatch_verified(
            &mut self,
            kernel: GpuKernelId,
            buffers: &[GpuBufferId],
            workgroups: [u32; 3],
            threads_per_group: [u32; 3],
        ) -> Result<(), GpuError> {
            self.inner
                .dispatch_verified(kernel, buffers, workgroups, threads_per_group)
        }
        fn read_buffer(&mut self, buffer: GpuBufferId) -> Result<Vec<u8>, GpuError> {
            // Return zeros of the right length — wrong vs the {0,1,2} reference.
            let len = self.inner.buffers.get(&buffer).map(Vec::len).unwrap_or(0);
            Ok(vec![0u8; len])
        }
    }

    #[test]
    fn verify_f32_add_passes_with_correct_backend() {
        let mut gpu = CpuAddBackend::new();
        let result = verify_f32_add(&mut gpu, 1024, &VerifyConfig::default()).unwrap();
        assert!(result.passed, "correct backend must pass: {result:?}");
        assert_eq!(result.trials, VerifyConfig::default().trials);
        assert_eq!(result.positions_exact, result.positions_checked);
        assert_eq!(result.max_diff, 0.0);
        assert!(result.positions_checked > 0);
    }

    #[test]
    fn verify_f32_add_detects_garbage_backend() {
        let mut gpu = GarbageReadBackend::default();
        let result = verify_f32_add(&mut gpu, 1024, &VerifyConfig::default()).unwrap();
        assert!(
            !result.passed,
            "garbage backend must be detected: {result:?}"
        );
        // All-zeros vs a {0,1,2} reference: positions where ref != 0 mismatch.
        assert!(result.positions_checked > 0, "no evidence gathered");
        assert!(
            result.positions_exact < result.positions_checked,
            "expected some mismatches against an all-zero readback"
        );
    }

    #[test]
    fn verify_f32_add_works_for_non_workgroup_multiple() {
        // len not divisible by 256 still verifies cleanly (ceil workgroups).
        let mut gpu = CpuAddBackend::new();
        let result = verify_f32_add(&mut gpu, 300, &VerifyConfig::default()).unwrap();
        assert!(result.passed, "non-aligned length must pass: {result:?}");
    }

    /// Hardware integration test: run with `--ignored`. Verified on lavapipe.
    /// Skips cleanly when no GPU is available (`borsalino::init()` returns `Err`).
    #[test]
    #[ignore = "requires a working GPU driver; verified on lavapipe — run with --ignored"]
    fn verify_f32_add_on_real_backend() {
        let Ok(backend) = borsalino::init() else {
            eprintln!("no GPU backend available; skipping hardware test");
            return;
        };
        let mut gpu = crate::BorsalinoGpu::new(backend);
        let result = verify_f32_add(&mut gpu, 1024, &VerifyConfig::default())
            .expect("verification dispatch must succeed on a real backend");
        assert!(
            result.passed,
            "Borsalino's vadd must be numerically exact on binary inputs: {result:?}"
        );
    }
}
