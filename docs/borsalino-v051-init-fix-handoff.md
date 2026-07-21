# Borsalino v0.5.1 Vulkan Init Fix — Handoff to Baedeker

**Date:** 2026-07-20
**From:** Borsalino session (issue #34, PR #35)
**To:** Baedeker session (PR #33, `baedeker-borsalino` crate)
**Status:** Fix merged to Borsalino `develop`; awaiting v0.5.1 release

---

## What Changed

Borsalino v0.5.1 fixes the Vulkan `init()` SIGSEGV that blocked the Baedeker
hardware test (`real_backend_dispatch_when_gpu_available` in
`crates/baedeker-borsalino/src/lib.rs`).

**Root cause:** `init()` hard-coded `vk::API_VERSION_1_3` in
`VkApplicationInfo`. In environments where the available ICD only supports an
older API version, the Mesa loader dispatches to the driver and it **crashes
(SIGSEGV)** inside its internal validation instead of returning
`VK_ERROR_INCOMPATIBLE_DRIVER`. A SIGSEGV cannot be caught in Rust
(`catch_unwind` catches panics, not signals), so the fix had to *prevent* the
crash.

**Fix:** `init()` now queries `vkEnumerateInstanceVersion` *before* requesting
a version, then requests only the available version (capped at 1.3). This
avoids triggering the buggy driver code path entirely.

---

## VM vs. Physical Hardware — The Key Clarification

Issue #34 was filed from a **headless VM** environment. The crash and the fix
are environment-dependent:

### Where the crash occurs (VM environments)

| Condition | Why it crashes |
|---|---|
| No GPU passthrough | Only software ICDs (lavapipe) available |
| Older Mesa (<23.x) | lavapipe only supports Vulkan 1.2; requesting 1.3 triggers the driver bug |
| Multiple broken ICDs | gfxstream/virtio ICDs crash during `vkEnumeratePhysicalDevices` dispatch |

This is the Baedeker CI/test VM scenario. The `#[ignore]` on the hardware test
exists because of this.

### Where the crash does NOT occur (physical hardware)

| Machine | Vulkan version | Behavior |
|---|---|---|
| **norma-wall** (AMD Phoenix1) | 1.4.341 | `init()` succeeds on both v0.5.0 and v0.5.1 — old and new |

On norma-wall, the AMD radeon ICD and lavapipe both fully support 1.3+, so the
old hardcoded request never hit the buggy code path. **The bug cannot be
reproduced on the physical hardware**, only in the VM.

### What this means for testing

- **The fix is verified by logic** (5 unit tests for `negotiate_api_version`)
- **The fix is NOT verified by crash reproduction** on norma-wall — the
  preconditions (older Mesa, no GPU) don't exist on the physical machine
- A true before/after confirmation requires the Baedeker VM environment (older
  Mesa lavapipe, or a headless VM without GPU passthrough)

---

## Action Items for Baedeker

### 1. Bump the Borsalino dependency to 0.5.1

```toml
# crates/baedeker-borsalino/Cargo.toml
# Linux:
borsalino = { version = "0.5.1", features = ["vulkan"] }
# macOS:
borsalino = { version = "0.5.1", features = ["metal"] }
```

### 2. Update the `#[ignore]` note on the hardware test

The current note says:
```rust
#[ignore = "requires a working GPU driver; see Borsalino init segfault note"]
```

Update to reflect that the SIGSEGV is fixed:
```rust
#[ignore = "requires a working GPU driver; run with --ignored on hardware"]
```

The test can now be un-ignored on physical hardware (norma-wall). It should
remain `#[ignore]`d in CI if CI runs in a headless VM — the version negotiation
fix prevents the crash, but a VM without GPU passthrough will return a clean
`Err(NoBackend)` or `Err(InitFailed)` rather than dispatching a real kernel.

### 3. Verify on norma-wall

The hardware test can now be run directly on the physical machine:

```sh
cd ../Baedeker
cargo test -p baedeker-borsalino -- --ignored real_backend_dispatch_when_gpu_available
```

This should pass — norma-wall's AMD Phoenix1 provides a working Vulkan 1.4
driver via the radeon ICD.

### 4. VM testing (optional, separate session)

If the Baedeker VM environment is still available, verify the clean-error path:

```sh
# In the headless VM (older Mesa, lavapipe only):
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p baedeker-borsalino -- --ignored
```

With v0.5.1, this should either:
- Succeed (if lavapipe supports the negotiated API version), or
- Return a clean `Err(InitFailed)` (if no compatible ICD found)

It should **not** SIGSEGV.

---

## Reference

- Borsalino issue: [#34 — Vulkan backend init() SIGSEGVs under Mesa ICD setups](https://github.com/Industrial-Algebra/Borsalino/issues/34)
- Borsalino fix: [PR #35](https://github.com/Industrial-Algebra/Borsalino/pull/35)
- Baedeker integration design: `docs/borsalino-integration.md`
- Baedeker hardware test: `crates/baedeker-borsalino/src/lib.rs` line 145
