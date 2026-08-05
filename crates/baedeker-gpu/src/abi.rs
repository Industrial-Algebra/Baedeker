// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! The v1 `baedeker:gpu` import ABI: one host-function builder per import.
//!
//! Each builder wraps a closure sharing per-instance state through interior
//! mutability (`Rc<RefCell<GpuState>>`) and the captured guest-memory handle.
//! Operand type is `i32` throughout; handles are non-negative indices and
//! every function returns `-1` (or a non-positive length for `gpu_last_error`)
//! on failure, stashing a diagnostic for [`make_last_error`].

use std::cell::RefCell;
use std::rc::Rc;

use baedeker_core::runtime::{HostFunction, Value};
use baedeker_core::types::{FuncType, NumType, ValType};

use crate::GpuState;

/// `i32` value type, the only operand type in the v1 ABI.
const I32: ValType = ValType::Num(NumType::I32);

/// `gpu_probe() -> i32`: reports the GPU backend is attached. Always `1`
/// once registered, letting guests feature-detect at runtime.
pub(crate) fn make_probe(
    _state: Rc<RefCell<GpuState>>,
    _memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(ft(&[], &[I32]), move |_args| Ok(vec![Value::I32(1)]))
}

/// `buffer_create(size: i32) -> i32`: allocate an uninitialised output buffer
/// of `size` bytes. Returns a non-negative handle or `-1`.
pub(crate) fn make_buffer_create(
    state: Rc<RefCell<GpuState>>,
    _memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(ft(&[I32], &[I32]), move |args| {
        let size = arg(args, 0).unwrap_or(-1);
        if size < 0 {
            set_err(&state, "buffer_create: negative size");
            return Ok(vec![Value::I32(-1)]);
        }
        let mut s = state.borrow_mut();
        match s.backend.create_buffer_uninit(size as usize) {
            Ok(id) => Ok(vec![Value::I32(s.alloc_buffer(id, size as usize) as i32)]),
            Err(error) => {
                s.last_error = format!("{error:?}");
                Ok(vec![Value::I32(-1)])
            }
        }
    })
}

/// `buffer_upload(mem_offset: i32, len: i32) -> i32`: allocate a buffer and
/// upload `len` bytes from guest memory at `mem_offset`. Returns a handle.
pub(crate) fn make_buffer_upload(
    state: Rc<RefCell<GpuState>>,
    memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(ft(&[I32, I32], &[I32]), move |args| {
        let mem_offset = arg(args, 0).unwrap_or(-1);
        let len = arg(args, 1).unwrap_or(-1);
        if mem_offset < 0 || len < 0 {
            set_err(&state, "buffer_upload: negative offset or length");
            return Ok(vec![Value::I32(-1)]);
        }
        let data = match read_mem(&memory, mem_offset as usize, len as usize) {
            Some(data) => data,
            None => {
                set_err(&state, "buffer_upload: guest memory out of bounds");
                return Ok(vec![Value::I32(-1)]);
            }
        };
        let mut s = state.borrow_mut();
        match s.backend.create_buffer(&data) {
            Ok(id) => Ok(vec![Value::I32(s.alloc_buffer(id, len as usize) as i32)]),
            Err(error) => {
                s.last_error = format!("{error:?}");
                Ok(vec![Value::I32(-1)])
            }
        }
    })
}

/// `buffer_read(handle, buf_offset, mem_offset, len) -> i32`: read `len` bytes
/// starting at `buf_offset` out of a buffer and write them into guest memory at
/// `mem_offset`. Returns `0` on success.
pub(crate) fn make_buffer_read(
    state: Rc<RefCell<GpuState>>,
    memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(ft(&[I32, I32, I32, I32], &[I32]), move |args| {
        let handle = arg(args, 0).unwrap_or(-1);
        let buf_offset = arg(args, 1).unwrap_or(-1);
        let mem_offset = arg(args, 2).unwrap_or(-1);
        let len = arg(args, 3).unwrap_or(-1);
        if handle < 0 || buf_offset < 0 || mem_offset < 0 || len < 0 {
            set_err(&state, "buffer_read: negative argument");
            return Ok(vec![Value::I32(-1)]);
        }
        let mut s = state.borrow_mut();
        let (id, size) = match s.buffer(handle) {
            Some(entry) => entry,
            None => {
                s.last_error = "buffer_read: unknown buffer handle".into();
                return Ok(vec![Value::I32(-1)]);
            }
        };
        let end = match (buf_offset as usize).checked_add(len as usize) {
            Some(end) if end <= size => end,
            _ => {
                s.last_error = "buffer_read: read range exceeds buffer size".into();
                return Ok(vec![Value::I32(-1)]);
            }
        };
        let full = match s.backend.read_buffer(id) {
            Ok(data) => data,
            Err(error) => {
                s.last_error = format!("{error:?}");
                return Ok(vec![Value::I32(-1)]);
            }
        };
        let start = buf_offset as usize;
        match write_mem(&memory, mem_offset as usize, &full[start..end]) {
            Some(()) => Ok(vec![Value::I32(0)]),
            None => {
                s.last_error = "buffer_read: guest memory out of bounds".into();
                Ok(vec![Value::I32(-1)])
            }
        }
    })
}

/// `kernel_create(code_ptr: i32, code_len: i32) -> i32`: compile a WGSL kernel
/// supplied as UTF-8 bytes in guest memory. Returns a handle or `-1`.
pub(crate) fn make_kernel_create(
    state: Rc<RefCell<GpuState>>,
    memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(ft(&[I32, I32], &[I32]), move |args| {
        let code_ptr = arg(args, 0).unwrap_or(-1);
        let code_len = arg(args, 1).unwrap_or(-1);
        if code_ptr < 0 || code_len < 0 {
            set_err(&state, "kernel_create: negative offset or length");
            return Ok(vec![Value::I32(-1)]);
        }
        let bytes = match read_mem(&memory, code_ptr as usize, code_len as usize) {
            Some(bytes) => bytes,
            None => {
                set_err(&state, "kernel_create: guest memory out of bounds");
                return Ok(vec![Value::I32(-1)]);
            }
        };
        let wgsl = match std::str::from_utf8(&bytes) {
            Ok(wgsl) => wgsl,
            Err(_) => {
                set_err(&state, "kernel_create: WGSL source is not valid UTF-8");
                return Ok(vec![Value::I32(-1)]);
            }
        };
        let mut s = state.borrow_mut();
        match s.backend.compile("baedeker_gpu_guest", wgsl) {
            Ok(id) => Ok(vec![Value::I32(s.alloc_kernel(id) as i32)]),
            Err(error) => {
                s.last_error = format!("{error:?}");
                Ok(vec![Value::I32(-1)])
            }
        }
    })
}

/// `dispatch(kernel, wg_x, wg_y, wg_z, tpg_x, tpg_y, tpg_z, bindings_ptr,
/// bindings_len) -> i32`: run `kernel` over the given workgroup grid with the
/// given per-workgroup thread count, binding `bindings_len` buffers whose u32
/// handles are read little-endian from guest memory at `bindings_ptr`. Routes
/// through [`GpuBackend::dispatch_verified`](baedeker_core::runtime::gpu::GpuBackend::dispatch_verified)
/// so the explicit thread count is honoured. Returns `0` on success.
pub(crate) fn make_dispatch(
    state: Rc<RefCell<GpuState>>,
    memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(
        ft(&[I32, I32, I32, I32, I32, I32, I32, I32, I32], &[I32]),
        move |args| {
            let kernel = arg(args, 0).unwrap_or(-1);
            let workgroups = [
                arg(args, 1).unwrap_or(0) as u32,
                arg(args, 2).unwrap_or(0) as u32,
                arg(args, 3).unwrap_or(0) as u32,
            ];
            let threads_per_group = [
                arg(args, 4).unwrap_or(0) as u32,
                arg(args, 5).unwrap_or(0) as u32,
                arg(args, 6).unwrap_or(0) as u32,
            ];
            let bindings_ptr = arg(args, 7).unwrap_or(-1);
            let bindings_len = arg(args, 8).unwrap_or(-1);
            if kernel < 0 || bindings_ptr < 0 || bindings_len < 0 {
                set_err(&state, "dispatch: negative argument");
                return Ok(vec![Value::I32(-1)]);
            }
            if threads_per_group[0] == 0 {
                set_err(&state, "dispatch: threads_per_group_x is zero");
                return Ok(vec![Value::I32(-1)]);
            }
            let count = bindings_len as usize;
            let byte_len = match count.checked_mul(4) {
                Some(byte_len) => byte_len,
                None => {
                    set_err(&state, "dispatch: bindings length overflows");
                    return Ok(vec![Value::I32(-1)]);
                }
            };
            let bytes = match read_mem(&memory, bindings_ptr as usize, byte_len) {
                Some(bytes) => bytes,
                None => {
                    set_err(&state, "dispatch: bindings exceed guest memory");
                    return Ok(vec![Value::I32(-1)]);
                }
            };
            let mut handles = Vec::with_capacity(count);
            for i in 0..count {
                let chunk: [u8; 4] = bytes[i * 4..i * 4 + 4].try_into().expect("4-byte slice");
                handles.push(u32::from_le_bytes(chunk) as i32);
            }
            let mut s = state.borrow_mut();
            let kernel_id = match s.kernel(kernel) {
                Some(id) => id,
                None => {
                    s.last_error = "dispatch: unknown kernel handle".into();
                    return Ok(vec![Value::I32(-1)]);
                }
            };
            let mut buffer_ids = Vec::with_capacity(count);
            for handle in handles {
                match s.buffer(handle) {
                    Some((id, _size)) => buffer_ids.push(id),
                    None => {
                        s.last_error = "dispatch: unknown buffer handle in bindings".into();
                        return Ok(vec![Value::I32(-1)]);
                    }
                }
            }
            match s
                .backend
                .dispatch_verified(kernel_id, &buffer_ids, workgroups, threads_per_group)
            {
                Ok(()) => Ok(vec![Value::I32(0)]),
                Err(error) => {
                    s.last_error = format!("{error:?}");
                    Ok(vec![Value::I32(-1)])
                }
            }
        },
    )
}

/// `gpu_last_error(mem_offset: i32, max_len: i32) -> i32`: write the most
/// recent diagnostic (UTF-8) into guest memory, truncated to `max_len`. Returns
/// the number of bytes written, or `-1` on a bounds failure.
pub(crate) fn make_last_error(
    state: Rc<RefCell<GpuState>>,
    memory: Rc<RefCell<Vec<u8>>>,
) -> HostFunction {
    HostFunction::new(ft(&[I32, I32], &[I32]), move |args| {
        let mem_offset = arg(args, 0).unwrap_or(-1);
        let max_len = arg(args, 1).unwrap_or(-1);
        if mem_offset < 0 || max_len < 0 {
            set_err(&state, "gpu_last_error: negative offset or length");
            return Ok(vec![Value::I32(-1)]);
        }
        let message = state.borrow().last_error.clone();
        let bytes = message.as_bytes();
        let n = bytes.len().min(max_len as usize);
        if n == 0 {
            return Ok(vec![Value::I32(0)]);
        }
        match write_mem(&memory, mem_offset as usize, &bytes[..n]) {
            Some(()) => Ok(vec![Value::I32(n as i32)]),
            None => {
                set_err(&state, "gpu_last_error: guest memory out of bounds");
                Ok(vec![Value::I32(-1)])
            }
        }
    })
}

// ── helpers ────────────────────────────────────────────────────────

fn ft(params: &[ValType], results: &[ValType]) -> FuncType {
    FuncType {
        params: params.to_vec(),
        results: results.to_vec(),
    }
}

/// Read the `i32` at argument position `i`, if present.
fn arg(args: &[Value], i: usize) -> Option<i32> {
    match args.get(i)? {
        Value::I32(v) => Some(*v),
        _ => None,
    }
}

fn read_mem(memory: &Rc<RefCell<Vec<u8>>>, offset: usize, len: usize) -> Option<Vec<u8>> {
    let mem = memory.borrow();
    let end = offset.checked_add(len)?;
    mem.get(offset..end).map(|slice| slice.to_vec())
}

fn write_mem(memory: &Rc<RefCell<Vec<u8>>>, offset: usize, data: &[u8]) -> Option<()> {
    let mut mem = memory.borrow_mut();
    let end = offset.checked_add(data.len())?;
    let dst = mem.get_mut(offset..end)?;
    dst.copy_from_slice(data);
    Some(())
}

fn set_err(state: &Rc<RefCell<GpuState>>, message: &str) {
    state.borrow_mut().last_error = message.to_string();
}
