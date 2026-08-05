// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for the GPU host module: a WAT guest imports the
//! `baedeker:gpu` ABI and drives it against a recording fake backend.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::RegModule;
use baedeker_core::lower::lower_module;
use baedeker_core::runtime::execute_export;
use baedeker_core::runtime::gpu::{GpuBackend, GpuBufferId, GpuError, GpuKernelId};
use baedeker_core::runtime::{Store, Value};
use baedeker_gpu::GpuHostModule;

/// One recorded `dispatch_verified` call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeDispatch {
    kernel: GpuKernelId,
    buffers: Vec<GpuBufferId>,
    workgroups: [u32; 3],
    threads_per_group: [u32; 3],
}

#[derive(Debug, Default)]
struct FakeStats {
    dispatches: Vec<FakeDispatch>,
}

/// A [`GpuBackend`] that hands out incrementing ids, sizes buffers, fills
/// readback with `0xAA`, and records every dispatch into a shared stats cell.
#[derive(Debug)]
struct FakeBackend {
    next_id: u64,
    sizes: HashMap<GpuBufferId, usize>,
    stats: Rc<RefCell<FakeStats>>,
}

impl FakeBackend {
    fn new(stats: Rc<RefCell<FakeStats>>) -> Self {
        Self {
            next_id: 1,
            sizes: HashMap::new(),
            stats,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl GpuBackend for FakeBackend {
    fn name(&self) -> &str {
        "fake"
    }

    fn compile(&mut self, _name: &str, _wgsl: &str) -> Result<GpuKernelId, GpuError> {
        Ok(self.alloc())
    }

    fn create_buffer(&mut self, data: &[u8]) -> Result<GpuBufferId, GpuError> {
        let id = self.alloc();
        self.sizes.insert(id, data.len());
        Ok(id)
    }

    fn create_buffer_uninit(&mut self, byte_len: usize) -> Result<GpuBufferId, GpuError> {
        let id = self.alloc();
        self.sizes.insert(id, byte_len);
        Ok(id)
    }

    fn dispatch(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
    ) -> Result<(), GpuError> {
        self.dispatch_verified(kernel, buffers, workgroups, [256, 1, 1])
    }

    fn dispatch_verified(
        &mut self,
        kernel: GpuKernelId,
        buffers: &[GpuBufferId],
        workgroups: [u32; 3],
        threads_per_group: [u32; 3],
    ) -> Result<(), GpuError> {
        self.stats.borrow_mut().dispatches.push(FakeDispatch {
            kernel,
            buffers: buffers.to_vec(),
            workgroups,
            threads_per_group,
        });
        Ok(())
    }

    fn read_buffer(&mut self, buffer: GpuBufferId) -> Result<Vec<u8>, GpuError> {
        let size = self.sizes.get(&buffer).copied().unwrap_or(0);
        Ok(vec![0xAA; size])
    }
}

/// Guest module importing the full v1 ABI. WGSL source lives at offset 0;
/// `run` exercises the happy path, `run_err` an error + `gpu_last_error`.
const WAT: &str = r#"
(module
  (import "baedeker:gpu" "gpu_probe" (func $probe (result i32)))
  (import "baedeker:gpu" "buffer_create" (func $buffer_create (param i32) (result i32)))
  (import "baedeker:gpu" "buffer_upload" (func $buffer_upload (param i32 i32) (result i32)))
  (import "baedeker:gpu" "buffer_read" (func $buffer_read (param i32 i32 i32 i32) (result i32)))
  (import "baedeker:gpu" "kernel_create" (func $kernel_create (param i32 i32) (result i32)))
  (import "baedeker:gpu" "dispatch"
    (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "baedeker:gpu" "gpu_last_error" (func $gpu_last_error (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  ;; WGSL source, 17 bytes: "@compute fn k(){}"
  (data (i32.const 0) "@compute fn k(){}")

  (func (export "run") (result i32)
    (local $probe i32) (local $inbuf i32) (local $outbuf i32)
    (local $kernel i32) (local $disp i32)
    (local.set $probe (call $probe))
    (if (i32.eq (local.get $probe) (i32.const 0))
      (then (return (i32.const -1))))
    ;; input buffer from memory offset 512, 16 bytes
    (local.set $inbuf (call $buffer_upload (i32.const 512) (i32.const 16)))
    (if (i32.lt_s (local.get $inbuf) (i32.const 0))
      (then (return (i32.const -2))))
    ;; output buffer, 16 bytes
    (local.set $outbuf (call $buffer_create (i32.const 16)))
    (if (i32.lt_s (local.get $outbuf) (i32.const 0))
      (then (return (i32.const -3))))
    ;; bindings array at offset 256: [inbuf, outbuf]
    (i32.store (i32.const 256) (local.get $inbuf))
    (i32.store (i32.const 260) (local.get $outbuf))
    ;; kernel from WGSL at offset 0, length 17
    (local.set $kernel (call $kernel_create (i32.const 0) (i32.const 17)))
    (if (i32.lt_s (local.get $kernel) (i32.const 0))
      (then (return (i32.const -4))))
    ;; dispatch(kernel, 1,1,1, 1,1,1, bindings=256, count=2)
    (local.set $disp
      (call $dispatch (local.get $kernel)
                     (i32.const 1) (i32.const 1) (i32.const 1)
                     (i32.const 1) (i32.const 1) (i32.const 1)
                     (i32.const 256) (i32.const 2)))
    (if (i32.ne (local.get $disp) (i32.const 0))
      (then (return (i32.const -5))))
    ;; read 16 bytes of outbuf into memory offset 768
    (local.set $disp
      (call $buffer_read (local.get $outbuf) (i32.const 0) (i32.const 768) (i32.const 16)))
    (if (i32.ne (local.get $disp) (i32.const 0))
      (then (return (i32.const -6))))
    (i32.const 0)
  )

  (func (export "run_err") (result i32)
    ;; bad handle → -1, then surface the diagnostic via gpu_last_error.
    (drop (call $buffer_read (i32.const 999) (i32.const 0) (i32.const 0) (i32.const 0)))
    (call $gpu_last_error (i32.const 256) (i32.const 64))
  )
)
"#;

fn build() -> (RegModule, Store) {
    let wasm = wat::parse_str(WAT).expect("wat parses");
    let module = Module::decode(&wasm).expect("module decodes");
    module.validate().expect("module validates");
    let reg = lower_module(&module).expect("module lowers");
    let store = Store::instantiate(&reg).expect("store instantiates");
    (reg, store)
}

#[test]
fn probe_reports_attachment() {
    let (reg, mut store) = build();
    let stats = Rc::new(RefCell::new(FakeStats::default()));
    GpuHostModule::for_store(Box::new(FakeBackend::new(stats)), &store)
        .expect("memory 0 present")
        .register(&mut store, &reg)
        .expect("registers");
    let result = execute_export(&reg, &store, "run", &[]).expect("run completes");
    // `run` returns 0 only after `gpu_probe` returned 1.
    assert_eq!(result, vec![Value::I32(0)]);
}

#[test]
fn happy_path_dispatches_and_reads_back() {
    let (reg, mut store) = build();
    let stats = Rc::new(RefCell::new(FakeStats::default()));
    GpuHostModule::for_store(Box::new(FakeBackend::new(stats.clone())), &store)
        .expect("memory 0 present")
        .register(&mut store, &reg)
        .expect("registers");

    let result = execute_export(&reg, &store, "run", &[]).expect("run completes");
    assert_eq!(result, vec![Value::I32(0)], "happy path returns 0");

    // One dispatch, with the explicit workgroup + thread config passed through.
    let dispatches = stats.borrow().dispatches.clone();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].workgroups, [1, 1, 1]);
    assert_eq!(dispatches[0].threads_per_group, [1, 1, 1]);
    // Fake alloc order: inbuf=1 (upload), outbuf=2 (create); kernel=3.
    assert_eq!(dispatches[0].buffers, vec![1, 2]);
    assert_eq!(dispatches[0].kernel, 3);

    // Output buffer read back into guest memory at offset 768.
    let mem = store.shared_memory(0).expect("memory 0");
    assert_eq!(&mem.borrow()[768..784], &[0xAA; 16]);
}

#[test]
fn bad_handle_records_diagnostic() {
    let (reg, mut store) = build();
    let stats = Rc::new(RefCell::new(FakeStats::default()));
    GpuHostModule::for_store(Box::new(FakeBackend::new(stats)), &store)
        .expect("memory 0 present")
        .register(&mut store, &reg)
        .expect("registers");

    // run_err returns the gpu_last_error byte count (>0 once a diagnostic is set).
    let result = execute_export(&reg, &store, "run_err", &[]).expect("run_err completes");
    let written = match result.as_slice() {
        [Value::I32(n)] => *n,
        _ => panic!("unexpected result: {result:?}"),
    };
    assert!(written > 0, "a diagnostic should have been recorded");

    let mem = store.shared_memory(0).expect("memory 0");
    let binding = mem.borrow();
    let message = String::from_utf8_lossy(&binding[256..256 + written as usize]);
    assert!(
        message.contains("unknown buffer handle"),
        "diagnostic should mention the handle, got: {message:?}"
    );
}

#[test]
fn register_skips_unimported_functions() {
    // A module that imports nothing from baedeker:gpu: register is a no-op
    // and instantiation still succeeds (no unresolved imports).
    let wasm = wat::parse_str("(module (memory (export \"memory\") 1))").unwrap();
    let module = Module::decode(&wasm).unwrap();
    module.validate().unwrap();
    let reg = lower_module(&module).unwrap();
    let mut store = Store::instantiate(&reg).unwrap();
    let stats = Rc::new(RefCell::new(FakeStats::default()));
    GpuHostModule::for_store(Box::new(FakeBackend::new(stats)), &store)
        .expect("memory 0 present")
        .register(&mut store, &reg)
        .expect("register is a no-op");
}
