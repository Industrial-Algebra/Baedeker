# Borsalino Integration for Baedeker

**Date:** 2026-06-03 (revised 2026-06 for cross-platform Vulkan)
**Status:** Design exploration — Borsalino v0.1.0 API is complete for all levels

## Overview

Baedeker targets every platform Rust compiles to, with GPU acceleration as a first-class
goal. Borsalino provides the GPU compute layer — a thin `GpuBackend` trait with WGSL
compilation, synchronous dispatch, and batched execution — over Vulkan, which runs
transparently across Metal, Nvidia, and AMD hardware. The integration is purely on
Baedeker's side: Borsalino's API surface is complete for WASM GPU acceleration with zero
extensions needed.

## Why Vulkan, and what portability costs

Borsalino's Vulkan backend is the portability layer: one WGSL shader source and one
dispatch API work on Apple silicon (through Metal), discrete Nvidia/AMD GPUs, and mobile
parts, with no per-vendor code in Baedeker.

The performance picture differs by memory architecture, and it matters for the offload
thresholds in Level 1-2:

- **Unified memory (Apple silicon; Nvidia Grace Blackwell such as DGX Spark GB10):**
  CPU and GPU share physical RAM. Borsalino's `HOST_VISIBLE | HOST_COHERENT` memory
  strategy (debugged on M3/M3 Pro and tuned further on GB10, where dispatch shows
  considerable compute gains) means zero-copy between Baedeker's WASM linear memory
  and GPU buffers. These are the first-class offload targets.
- **Discrete GPUs (PCIe-attached):** buffer uploads/downloads cross the PCIe bus. The
  same API works, but the CPU↔GPU crossover point moves to larger N — the per-platform
  benchmarks in the Implementation Strategy are how we set thresholds.

```rust
// Baedeker holds WASM linear memory as a Rust Vec<u8>
let wasm_memory: Vec<u8> = ...;

// Borsalino can reference it directly via create_buffer (zero-copy on unified memory)
let gpu_buf = gpu.create_buffer(wasm_memory.as_slice())?;
// After dispatch, read back:
let result = gpu.read_buffer::<f32>(&gpu_buf)?;
```

## Integration Levels

### Level 1: Bulk SIMD Offload (~200 lines in Baedeker)

Baedeker implements WASM 2.0 SIMD (`v128`). For bulk operations on large vectors,
dispatch a pre-compiled WGSL kernel via Borsalino instead of executing element-by-element
in the register IR.

**Borsalino API used:**
- `GpuBackend::compile()` — compile WGSL SIMD kernels once at startup
- `GpuBackend::create_buffer()` — map WASM linear memory to GPU buffers
- `GpuBackend::dispatch()` — execute kernel
- `GpuBackend::read_buffer()` — read results back to WASM memory

**Example: WASM `f32x4.add` dispatched on GPU for N elements**

```rust
// Compiled once at init:
let vadd_kernel = gpu.compile("vadd_f32x4", r#"
    @group(0) @binding(0) var<storage, read> a: array<f32>;
    @group(0) @binding(1) var<storage, read> b: array<f32>;
    @group(0) @binding(2) var<storage, read_write> out: array<f32>;
    @compute @workgroup_size(256)
    fn vadd(@builtin(global_invocation_id) gid: vec3<u32>) {
        out[gid.x] = a[gid.x] + b[gid.x];
    }
"#)?;

// Per SIMD operation dispatch:
fn exec_f32x4_add(&self, a_ptr: u32, b_ptr: u32, out_ptr: u32, count: u32) {
    let buf_a = self.gpu.create_buffer(&self.memory[a_ptr..][..count*4])?;
    let buf_b = self.gpu.create_buffer(&self.memory[b_ptr..][..count*4])?;
    let buf_out = self.gpu.create_buffer_uninit::<f32>(count)?;
    self.gpu.dispatch(&self.vadd_kernel, &[&buf_a, &buf_b, &buf_out],
                      (count.div_ceil(256), 1, 1))?;
    let result = self.gpu.read_buffer::<f32>(&buf_out)?;
    self.memory[out_ptr..][..count*4].copy_from_slice(&result);
}
```

**When to dispatch on GPU vs CPU:** Threshold decision. For N < 256, CPU execution
(in the register IR) is faster (avoids dispatch overhead). For N >= 1024, GPU wins.
Benchmark the crossover point per platform (unified-memory and discrete GPUs differ).

**Borsalino changes required:** None.

---

### Level 2: Register-Block JIT (~2000 lines in Baedeker)

Baedeker lowers WASM to a register-based IR with basic blocks. For arithmetic-only
blocks (no control flow, no memory load/store beyond the block's input registers),
compile the block to WGSL and dispatch all blocks in a batch.

**Borsalino API used:**
- `GpuBackend::compile()` — JIT-compile each unique basic block to WGSL
- `GpuBackend::dispatch_many()` — dispatch all blocks in one Vulkan command buffer
  (amortises command-buffer overhead, critical for many small blocks)

**Architecture:**

```
WASM function
    ↓
Register IR (Baedeker)
    ↓
Basic-block extractor → identifies arithmetic-only blocks
    ↓
Block → WGSL compiler (Baedeker) → naga validates
    ↓
GpuBackend::compile() → Vulkan compute pipeline
    ↓
dispatch_many([block1, block2, ...]) → single command buffer
    ↓
read_buffer → register values back to Baedeker's VM state
```

**Key design decision:** Mapping WASM linear memory to GPU buffers. On unified
memory (Apple silicon), Baedeker's `Vec<u8>` linear memory can be bound directly as a
buffer. GPU blocks that need memory access bind the full linear memory at `[[buffer(0)]]`
and use an offset parameter for bounds.

**Performance:** RTX 5080 benchmarks show `dispatch_many()` reduces per-dispatch
latency from 37 µs to 0.5 µs at 256 dispatches. On M3 (through Metal), 59× faster
per-dispatch was measured. For register blocks averaging 10-50 instructions, this overhead
is critical — without batching, GPU dispatch would be slower than CPU execution.

**Borsalino changes required:** None.

---

### Level 3: Full WASM→WGSL Compiler (~20,000 lines, research project)

Compile entire WASM functions to WGSL compute shaders. The GPU becomes a WASM
coprocessor. This involves:

- Register allocation across GPU threadgroups
- WASM control flow (`br`, `br_if`, `loop`) → WGSL control flow
- WASM memory model → GPU buffer bindings with bounds checking
- WASM table/call_indirect → GPU-side dispatch or fallback
- Stack frame management on GPU (shared memory per threadgroup)

**Borsalino API used:** Same as Level 1-2.

**Borsalino changes required:** None — but may want:
- GPU performance counters (`gpu.timestamp()`) for profiling WASM execution
- `dispatch_async()` for non-blocking WASM coprocessor model (Phase 3+)

## Implementation Strategy

### Phase 1: SDK Setup (Baedeker)

```toml
[dependencies]
borsalino = { version = "0.1", features = ["vulkan"] }
```

Borsalino's Vulkan backend builds anywhere a Vulkan driver exists: desktop Linux/Windows,
macOS/iOS (through Metal compatibility), and Android. Platform packaging notes live with
the platform integration work (Baedeker Phase 5); the runtime itself needs no per-vendor
code.

### Phase 2: Level 1 SIMD Offload

1. Install a Borsalino Vulkan backend in Baedeker's `GpuBackend` slot
2. Compile v128 SIMD kernels at init time
3. Implement SIMD dispatch with a size threshold (benchmark per platform)
4. Fall back to register IR execution for small N

### Phase 3: Benchmark and Tune

Use `examples/dispatch_profile.rs` and `examples/bench.rs` from Borsalino to establish
baselines across the memory-architecture spectrum: Apple silicon (M-class), Nvidia Grace
Blackwell (DGX Spark GB10), and PCIe-discrete GPUs (RTX class). Key metrics:
- Compile latency for SIMD kernels
- Dispatch overhead per platform
- Crossover point where GPU beats CPU for f32x4 operations (differs by memory architecture)
- Memory bandwidth for WASM linear memory → GPU buffer transfers

### Phase 4: Level 2 Register-Block JIT (Optional)

If SIMD offload shows strong results, extend to register-block compilation.

## Borsalino Roadmap Items That Would Help

None required for Levels 1-2, but these would benefit Level 3:

| Feature | Benefit for Baedeker | Priority |
|---|---|---|
| `gpu.timestamp()` | Profile WASM execution on GPU | Low |
| `dispatch_async()` | Non-blocking WASM coprocessor | Medium |
| GPU performance counters | Occupancy, bandwidth metrics | Low |

All are Baedeker-side work items; Borsalino's API is feature-complete for the
integration.
