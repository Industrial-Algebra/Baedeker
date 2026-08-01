// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: MIT

//! Numerical correctness verification for GPU offload kernels (Layer 2).
//!
//! This is the **pure, no-std core** of the DeepReinforce exact-match
//! correctness protocol — the protocol described in [_Towards a Reliable
//! Kernel Correctness Check in Matrix
//! Multiplication_](https://deep-reinforce.com/correctness_check.html).
//! The GPU-dependent driver lives in the `baedeker-borsalino` adapter (it
//! needs `rand` + a concrete [`GpuBackend`](crate::runtime::gpu::GpuBackend)).
//!
//! # Why exact match, not tolerance
//!
//! Tolerance-based checks (`abs(gpu - ref) < eps`) are unreliable for GPU
//! kernels because floating-point associativity does not hold:
//! `(a + b) + c ≠ a + (b + c)` in reduced precision, and different GPU thread
//! orderings produce different accumulation sequences. Two correct kernels can
//! therefore emit different outputs, and no universal tolerance works across
//! matrix sizes or precisions.
//!
//! The exact-match protocol sidesteps this by restricting kernel inputs to
//! **binary `{0, 1}`** values with a zero-biased distribution. This guarantees
//! every partial sum is a small non-negative integer. Within the exact-integer
//! range of the target precision — `[0, 2048]` for FP16, and far wider for
//! FP32 — floating-point associativity holds **exactly**. The kernel output is
//! then compared against an FP32 CPU reference with **bit-exact equality** at
//! every position whose reference value is at or below the threshold;
//! positions above the threshold are ignored (they have lost exactness).
//!
//! # Applicability
//!
//! The protocol applies to **linear** kernels (elementwise add/scale, saxpy,
//! matmul) and bilinear kernels with binary operands (geometric product). It
//! does **not** apply to non-linear operations (`log`, `exp`, `tanh`): those
//! produce irrational outputs that cannot be checked with exact match.
//!
//! Baedeker's first verified kernel is the [`F32_ADD_WGSL`] elementwise add —
//! see [`f32_add_reference`] for its CPU reference. Adding another offload
//! kernel means adding its reference function here and a driver entry in the
//! adapter.
//!
//! # Layering
//!
//! This module is Layer 2 of the "both, layered" GPU verification strategy.
//! Layer 1 ([`GpuBackend::dispatch_verified`](crate::runtime::gpu::GpuBackend::dispatch_verified))
//! is a uniform structural check (workgroup-divisibility proof) that runs on
//! every dispatch. Layer 2 (this module) is opt-in numerical correctness for
//! known-linear kernels, run on demand to prove a kernel's math is right.

use alloc::vec::Vec;

/// FP16 exact-integer ceiling: the largest integer exactly representable in
/// half precision. Positions whose FP32 CPU reference exceeds this are
/// ignored by [`compare_outputs`] because they may have lost exactness under
/// reduced-precision accumulation.
///
/// FP32's own exact-integer ceiling is ~16 million, so this threshold is the
/// binding constraint whenever a kernel might run (or be compared) at half
/// precision.
pub const FP_BINARY_THRESHOLD: f32 = 2048.0;

/// Production WGSL for the `f32_add` SIMD offload kernel: `out[i] = a[i] + b[i]`.
///
/// Three storage bindings (`a`, `b` read; `out` read-write), `@workgroup_size(256)`,
/// bounds-checked against `arrayLength(&out)`. The runtime caches a compiled
/// copy per store; the verification driver compiles this same source so it
/// exercises the production kernel, not a parallel verification kernel.
pub const F32_ADD_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(256)
fn vadd(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i < arrayLength(&out)) {
        out[i] = a[i] + b[i];
    }
}
"#;

/// Configuration for the exact-match numerical correctness protocol.
///
/// # Defaults
///
/// | Field | Default | Rationale |
/// |---|---|---|
/// | `threshold` | 2048.0 | [`FP_BINARY_THRESHOLD`] — FP16 exact-integer ceiling |
/// | `trials` | 16 | Enough random binary trials to catch systematic bugs |
/// | `p_zero` | 0.7 | 70% zeros keeps accumulated sums below the threshold |
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerifyConfig {
    /// Bit-exact ceiling. Positions where the FP32 CPU reference exceeds this
    /// value are ignored (they may have lost floating-point exactness).
    pub threshold: f32,

    /// Number of random binary-input trials to run.
    pub trials: u32,

    /// Probability of sampling `0.0` vs `1.0` for each input element. Higher
    /// zero bias keeps accumulated sums below the threshold for larger
    /// problem sizes.
    pub p_zero: f32,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            threshold: FP_BINARY_THRESHOLD,
            trials: 16,
            p_zero: 0.7,
        }
    }
}

/// Result of an exact-match numerical correctness check.
///
/// Aggregated across trials by the driver: `positions_checked` and
/// `positions_exact` sum over every trial, while `max_diff` is the maximum
/// observed at any checked position. `passed` is true only if **every**
/// position at or below the threshold matched exactly across **every** trial
/// (and at least one position was checked).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerifyResult {
    /// Whether the kernel passed (every checked position exact, ≥1 checked).
    pub passed: bool,

    /// Number of trials run.
    pub trials: u32,

    /// Total output positions compared across all trials (reference ≤ threshold).
    pub positions_checked: usize,

    /// Positions that matched the reference exactly.
    pub positions_exact: usize,

    /// Maximum absolute difference at checked positions. `0.0` for a correct
    /// kernel.
    pub max_diff: f32,
}

impl VerifyResult {
    /// An empty aggregator to fold per-trial [`compare_outputs`] results into.
    pub fn empty_aggregator() -> Self {
        Self {
            passed: true,
            trials: 0,
            positions_checked: 0,
            positions_exact: 0,
            max_diff: 0.0,
        }
    }

    /// Fold a single-trial result into an aggregator.
    pub fn fold_trial(&mut self, trial: VerifyResult) {
        self.trials += trial.trials;
        self.positions_checked += trial.positions_checked;
        self.positions_exact += trial.positions_exact;
        if trial.max_diff > self.max_diff {
            self.max_diff = trial.max_diff;
        }
        if !trial.passed {
            self.passed = false;
        }
    }
}

/// Compare GPU output against an FP32 reference with threshold gating.
///
/// For each position where `reference[i] <= threshold`, the GPU value must
/// match exactly. Positions above the threshold are ignored. This is the pure,
/// GPU-free heart of the protocol — fully unit-testable.
///
/// Returns a single-trial result (`trials == 1`). Pass is `true` only if at
/// least one position was checked and every checked position matched exactly.
///
/// ```
/// use baedeker_core::runtime::verify::compare_outputs;
///
/// let gpu = [1.0_f32, 2.0, 3.0];
/// let r#ref = [1.0_f32, 2.0, 3.0];
/// assert!(compare_outputs(&gpu, &r#ref, 2048.0).passed);
/// ```
pub fn compare_outputs(gpu_output: &[f32], reference: &[f32], threshold: f32) -> VerifyResult {
    let mut positions_checked = 0usize;
    let mut positions_exact = 0usize;
    let mut max_diff = 0.0f32;

    for (gpu_val, ref_val) in gpu_output.iter().zip(reference.iter()) {
        if *ref_val <= threshold {
            positions_checked += 1;
            let diff = (*gpu_val - ref_val).abs();
            if diff == 0.0 {
                positions_exact += 1;
            }
            if diff > max_diff {
                max_diff = diff;
            }
        }
    }

    let passed = positions_checked > 0 && positions_exact == positions_checked;
    VerifyResult {
        passed,
        trials: 1,
        positions_checked,
        positions_exact,
        max_diff,
    }
}

/// FP32 CPU reference for the [`F32_ADD_WGSL`] kernel: `out[i] = a[i] + b[i]`.
///
/// With binary `{0, 1}` inputs every output lies in `{0, 1, 2}`, all far below
/// [`FP_BINARY_THRESHOLD`], so every position is checked.
///
/// ```
/// use baedeker_core::runtime::verify::f32_add_reference;
///
/// let out = f32_add_reference(&[0.0, 1.0, 1.0], &[1.0, 0.0, 1.0]);
/// assert_eq!(out, [1.0, 1.0, 2.0]);
/// ```
pub fn f32_add_reference(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b.iter()).map(|(&x, &y)| x + y).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── VerifyConfig ──────────────────────────────────────────────

    #[test]
    fn default_config_uses_fp16_threshold() {
        let cfg = VerifyConfig::default();
        assert_eq!(cfg.threshold, FP_BINARY_THRESHOLD);
        assert_eq!(cfg.threshold, 2048.0);
        assert!(cfg.trials >= 1);
        assert!(cfg.p_zero > 0.0 && cfg.p_zero < 1.0);
    }

    // ── compare_outputs: correct kernel ───────────────────────────

    #[test]
    fn compare_outputs_exact_match_passes() {
        let gpu = [1.0_f32, 2.0, 3.0, 4.0];
        let r#ref = [1.0_f32, 2.0, 3.0, 4.0];
        let result = compare_outputs(&gpu, &r#ref, 2048.0);
        assert!(result.passed);
        assert_eq!(result.positions_checked, 4);
        assert_eq!(result.positions_exact, 4);
        assert_eq!(result.max_diff, 0.0);
    }

    // ── compare_outputs: incorrect kernel detected ────────────────

    #[test]
    fn compare_outputs_mismatch_detected() {
        let gpu = [1.0_f32, 2.0, 3.0, 5.0]; // last element wrong
        let r#ref = [1.0_f32, 2.0, 3.0, 4.0];
        let result = compare_outputs(&gpu, &r#ref, 2048.0);
        assert!(!result.passed);
        assert_eq!(result.positions_checked, 4);
        assert_eq!(result.positions_exact, 3);
        assert_eq!(result.max_diff, 1.0);
    }

    // ── compare_outputs: threshold gating ─────────────────────────

    #[test]
    fn compare_outputs_ignores_positions_above_threshold() {
        // 3000 > 2048 threshold → that position is ignored, not a failure.
        let gpu = [1.0_f32, 2.0, 3000.0];
        let r#ref = [1.0_f32, 2.0, 2049.0];
        let result = compare_outputs(&gpu, &r#ref, 2048.0);
        assert!(result.passed);
        assert_eq!(result.positions_checked, 2);
        assert_eq!(result.positions_exact, 2);
    }

    #[test]
    fn compare_outputs_threshold_boundary_inclusive() {
        // Reference exactly at threshold IS checked.
        let gpu = [2048.0_f32];
        let r#ref = [2048.0_f32];
        let result = compare_outputs(&gpu, &r#ref, 2048.0);
        assert!(result.passed);
        assert_eq!(result.positions_checked, 1);
    }

    // ── compare_outputs: vacuous check is not a pass ──────────────

    #[test]
    fn compare_outputs_all_above_threshold_is_not_a_pass() {
        // No checked positions means no evidence — must not pass vacuously,
        // otherwise a totally-out-of-range kernel would look correct.
        let gpu = [5000.0_f32, 6000.0];
        let r#ref = [5000.0_f32, 6000.0];
        let result = compare_outputs(&gpu, &r#ref, 2048.0);
        assert!(!result.passed, "vacuous pass hides lack of evidence");
        assert_eq!(result.positions_checked, 0);
    }

    #[test]
    fn compare_outputs_unequal_lengths_compare_only_overlap() {
        // zip stops at the shorter slice; extra GPU elements are never read.
        let gpu = [1.0_f32, 2.0, 3.0];
        let r#ref = [1.0_f32, 2.0];
        let result = compare_outputs(&gpu, &r#ref, 2048.0);
        assert!(result.passed);
        assert_eq!(result.positions_checked, 2);
    }

    // ── f32_add_reference ─────────────────────────────────────────

    #[test]
    fn f32_add_reference_sums_elementwise() {
        let out = f32_add_reference(&[0.0, 1.0, 1.0, 0.0], &[1.0, 0.0, 1.0, 0.0]);
        assert_eq!(out, [1.0, 1.0, 2.0, 0.0]);
    }

    #[test]
    fn f32_add_reference_binary_outputs_stay_below_threshold() {
        // Every binary-input sum is in {0,1,2}, all ≤ 2048 → all checked.
        let a = [0.0_f32, 1.0, 1.0];
        let b = [1.0_f32, 1.0, 0.0];
        let out = f32_add_reference(&a, &b);
        assert!(out.iter().all(|&v| v <= FP_BINARY_THRESHOLD));
    }

    // ── VerifyResult aggregation ──────────────────────────────────

    #[test]
    fn aggregator_folds_passing_trials_into_pass() {
        let mut agg = VerifyResult::empty_aggregator();
        agg.fold_trial(compare_outputs(&[1.0], &[1.0], 2048.0));
        agg.fold_trial(compare_outputs(&[2.0, 3.0], &[2.0, 3.0], 2048.0));
        assert!(agg.passed);
        assert_eq!(agg.trials, 2);
        assert_eq!(agg.positions_checked, 3);
        assert_eq!(agg.positions_exact, 3);
    }

    #[test]
    fn aggregator_folds_any_failing_trial_into_fail() {
        let mut agg = VerifyResult::empty_aggregator();
        agg.fold_trial(compare_outputs(&[1.0], &[1.0], 2048.0));
        agg.fold_trial(compare_outputs(&[9.0], &[1.0], 2048.0)); // mismatch
        assert!(!agg.passed);
        assert_eq!(agg.trials, 2);
        assert_eq!(agg.positions_checked, 2);
        assert_eq!(agg.positions_exact, 1, "only the matching trial counted");
        assert_eq!(agg.max_diff, 8.0);
    }
}
