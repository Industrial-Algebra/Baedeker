# Critique & Future Work

> **Snapshot: 0.1.0 (unreleased).** This is an honest self-assessment at the
> first public release, not a marketing document. It records what is solid, what
> is deliberately narrow, and what is known to be missing.

## What is solid

- **Spec conformance.** The official WebAssembly 2.0 suite runs at 19,204
  assertions / 0 failures. This is the strongest available evidence that the
  engine is correct, and it is green, not aspirational.
- **Differential agreement.** Zero divergences against Wasmtime across a
  wasm-smith sweep. A second, independent oracle agrees.
- **Embeddability.** The `no_std` core genuinely has no OS dependencies, and the
  FFI + Swift + AOT surface is exercised by tests, not just sketched.
- **GPU verification is layered and honest.** Structural proof runs on every
  dispatch; numerical proof is opt-in for known-linear kernels with a clearly
  stated applicability bound (not non-linear ops).

## What is deliberately narrow

- **The interpreter is a register-IR interpreter, not a JIT.** This is a
  deliberate 0.1.0 choice for portability and embeddability (no codegen, no
  platform-specific backend). Throughput is adequate for many embedders but not
  competitive with Cranelift/Wasmtime on hot loops. A register-block JIT is a
  future integration-level option (see the Borsalino integration notes).
- **GPU offload is one kernel deep.** `f32_add` is the offloaded operation; the
  host-module ABI is upload-on-create + read-back. This proves the pipeline end
  to end; expanding the kernel set and adding partial-write/destroy to the
  backend trait are follow-ups.
- **Numerical verification covers `f32_add`.** The exact-match protocol applies
  to linear/bilinear kernels; the reference for each additional kernel must be
  written. Non-linear kernels are out of scope for this protocol by design.

## Known gaps

- **WebAssembly 3.0 is absent.** Tail calls, exception handling, memory64, GC,
  and threads are rejected at validation. These are the roadmap, not a defect of
  the 2.0 milestone.
- **No hosted documentation site yet at release.** This book is the first cut;
  the API reference points at rustdoc and is intentionally terse.
- **MSRV is asserted (1.85) but CI tests on stable only.** A dedicated MSRV CI
  job would harden the claim.
- **Threading/shared-everything threads** are a post-3.0 consideration; the
  engine is single-threaded by design today.

## What this release is not

- Not a production-hardated server runtime (no observability hooks, no
  component-model support).
- Not a JIT (interpreter only).
- Not a drop-in Wasmtime replacement (smaller surface, different goals).

It is a correct, portable, embeddable WebAssembly 2.0 engine with a real
platform-integration story — a credible foundation for the 3.0 work and for
running the IA ecosystem on any platform.
