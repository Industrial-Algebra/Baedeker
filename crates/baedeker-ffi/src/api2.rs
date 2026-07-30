//! Calls, host functions, and memory access — the second half of the API.

use std::ffi::{c_char, c_void};

use baedeker_core::runtime::host::HostFunction;
use baedeker_core::runtime::{RuntimeError, RuntimeErrorKind, Value, execute_export};
use baedeker_core::types::FuncType;

use crate::api::{BaedekerHostFn, BaedekerInstance, c_name, guard, instance_mut, instance_ref};
use crate::error::{BaedekerStatus, set_last_error, take_last_error};
use crate::value::{BaedekerValue, BaedekerValueTag};

/// Register a host function for one of the instance's imports. Must be called
/// after instantiation and before the import is called. `params`/`results`
/// are arrays of `BaedekerValueTag` bytes describing the WASM signature.
///
/// # Safety
/// All pointers must be valid; `callback` must remain callable for the
/// instance's lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_register_host_func(
    instance: *mut BaedekerInstance,
    module: *const c_char,
    name: *const c_char,
    params: *const u8,
    n_params: usize,
    results: *const u8,
    n_results: usize,
    callback: BaedekerHostFn,
    user_data: *mut c_void,
) -> BaedekerStatus {
    guard(|| {
        let Some(handle) = (unsafe { instance_mut(instance) }) else {
            set_last_error("null instance handle");
            return BaedekerStatus::Usage;
        };
        let module = match c_name(module, "module name") {
            Ok(m) => m,
            Err(s) => return s,
        };
        let name = match c_name(name, "function name") {
            Ok(n) => n,
            Err(s) => return s,
        };
        let Some(callback) = callback else {
            set_last_error("null host callback");
            return BaedekerStatus::Usage;
        };
        let parse_tags = |ptr: *const u8, n: usize, what: &str| -> Option<Vec<_>> {
            if n > 0 && ptr.is_null() {
                set_last_error(format!("null {what} array"));
                return None;
            }
            let raw = unsafe { std::slice::from_raw_parts(ptr, n) };
            let mut tags = Vec::with_capacity(n);
            for &byte in raw {
                match BaedekerValueTag::from_u8(byte) {
                    Some(tag) => tags.push(tag.val_type()),
                    None => {
                        set_last_error(format!("unknown value tag {byte} in {what}"));
                        return None;
                    }
                }
            }
            Some(tags)
        };
        let (Some(params), Some(results_tys)) = (
            parse_tags(params, n_params, "params"),
            parse_tags(results, n_results, "results"),
        ) else {
            return BaedekerStatus::Usage;
        };

        let ty = FuncType {
            params,
            results: results_tys,
        };
        let user_data = user_data as usize; // capture without Send/Sync bounds
        let func = HostFunction::new(ty, move |args: &[Value]| {
            let c_args: Vec<BaedekerValue> = args
                .iter()
                .map(|&v| {
                    BaedekerValue::from_core(v).ok_or_else(|| RuntimeError {
                        kind: RuntimeErrorKind::HostError {
                            message: "reference argument is not supported by this FFI".into(),
                        },
                    })
                })
                .collect::<Result<_, _>>()?;
            let mut c_results = vec![
                BaedekerValue {
                    tag: BaedekerValueTag::I32 as u8,
                    data: crate::value::BaedekerValueData { i32_: 0 },
                };
                n_results
            ];
            let status = unsafe {
                callback(
                    c_args.as_ptr(),
                    c_args.len(),
                    c_results.as_mut_ptr(),
                    c_results.len(),
                    user_data as *mut c_void,
                )
            };
            if status != BaedekerStatus::Ok {
                let message = take_last_error()
                    .unwrap_or_else(|| format!("host function returned status {status:?}"));
                return Err(RuntimeError {
                    kind: RuntimeErrorKind::HostError { message },
                });
            }
            Ok(c_results.iter().map(|&v| v.to_core()).collect())
        });
        match handle.store.register_host_func(module, name, func) {
            Ok(()) => BaedekerStatus::Ok,
            Err(e) => {
                set_last_error(format!("host registration failed: {e:?}"));
                BaedekerStatus::Usage
            }
        }
    })
}

/// Call an exported function. `args` must match the export's parameter types.
/// Up to `results_cap` results are written to `results`; `*n_results_out`
/// always receives the export's true result count (pass `results = null`
/// with `results_cap = 0` to query arity without buffers).
///
/// # Safety
/// All pointers must be valid for their respective counts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_call(
    instance: *mut BaedekerInstance,
    name: *const c_char,
    args: *const BaedekerValue,
    n_args: usize,
    results: *mut BaedekerValue,
    results_cap: usize,
    n_results_out: *mut usize,
) -> BaedekerStatus {
    guard(|| {
        let Some(handle) = (unsafe { instance_ref(instance) }) else {
            set_last_error("null instance handle");
            return BaedekerStatus::Usage;
        };
        let name = match c_name(name, "export name") {
            Ok(n) => n,
            Err(s) => return s,
        };
        if n_args > 0 && args.is_null() {
            set_last_error("null args array");
            return BaedekerStatus::Usage;
        }
        if results_cap > 0 && results.is_null() {
            set_last_error("null results array");
            return BaedekerStatus::Usage;
        }
        let empty: &[BaedekerValue] = &[];
        let arg_slice = if n_args == 0 {
            empty
        } else {
            unsafe { std::slice::from_raw_parts(args, n_args) }
        };
        let args: Vec<Value> = arg_slice.iter().map(|&v| v.to_core()).collect();
        match execute_export(&handle.module, &handle.store, name, &args) {
            Ok(values) => {
                if !n_results_out.is_null() {
                    unsafe { *n_results_out = values.len() };
                }
                let out = if results_cap == 0 {
                    &mut []
                } else {
                    unsafe { std::slice::from_raw_parts_mut(results, results_cap) }
                };
                for (slot, &value) in out.iter_mut().zip(values.iter()) {
                    let Some(converted) = BaedekerValue::from_core(value) else {
                        set_last_error("result is a reference value, not supported by this FFI");
                        return BaedekerStatus::Unsupported;
                    };
                    *slot = converted;
                }
                BaedekerStatus::Ok
            }
            Err(e) => {
                set_last_error(format!("{e:?}"));
                match e.kind {
                    RuntimeErrorKind::Trap(_) => BaedekerStatus::Trap,
                    RuntimeErrorKind::FuelExhausted => BaedekerStatus::FuelExhausted,
                    RuntimeErrorKind::HostError { .. } => BaedekerStatus::HostError,
                    RuntimeErrorKind::ArityMismatch { .. }
                    | RuntimeErrorKind::TypeMismatch { .. } => BaedekerStatus::Usage,
                    _ => BaedekerStatus::Runtime,
                }
            }
        }
    })
}

/// Borrow the instance's linear memory (index 0) as a raw pointer and length
/// in bytes. The pointer is invalidated by ANY subsequent call into the
/// instance (memory may grow and reallocate) and by freeing the instance.
///
/// # Safety
/// `data` and `len` must be valid pointers; the returned pointer must not be
/// used after any further call on this instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_memory_data(
    instance: *mut BaedekerInstance,
    data: *mut *mut u8,
    len: *mut u64,
) -> BaedekerStatus {
    guard(|| {
        let Some(handle) = (unsafe { instance_mut(instance) }) else {
            set_last_error("null instance handle");
            return BaedekerStatus::Usage;
        };
        if data.is_null() || len.is_null() {
            set_last_error("null data or len pointer");
            return BaedekerStatus::Usage;
        }
        match handle
            .store
            .with_memory_mut(0, |m| (m.as_mut_ptr(), m.len() as u64))
        {
            Some((ptr, n)) => {
                unsafe {
                    *data = ptr;
                    *len = n;
                }
                BaedekerStatus::Ok
            }
            None => {
                set_last_error("instance has no memory 0");
                BaedekerStatus::Usage
            }
        }
    })
}

/// Copy `len` bytes out of linear memory starting at `offset`.
///
/// # Safety
/// `dst` must be writable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_memory_read(
    instance: *mut BaedekerInstance,
    offset: u64,
    dst: *mut u8,
    len: u64,
) -> BaedekerStatus {
    guard(|| {
        let Some(handle) = (unsafe { instance_ref(instance) }) else {
            set_last_error("null instance handle");
            return BaedekerStatus::Usage;
        };
        if dst.is_null() && len > 0 {
            set_last_error("null destination");
            return BaedekerStatus::Usage;
        }
        let Some(result) = handle.store.with_memory(0, |m| {
            let end = offset.saturating_add(len);
            if end > m.len() as u64 {
                set_last_error("read range out of bounds");
                return Err(BaedekerStatus::Usage);
            }
            let src = &m[offset as usize..end as usize];
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) };
            Ok(())
        }) else {
            set_last_error("instance has no memory 0");
            return BaedekerStatus::Usage;
        };
        match result {
            Ok(()) => BaedekerStatus::Ok,
            Err(s) => s,
        }
    })
}

/// Copy `len` bytes into linear memory starting at `offset`.
///
/// # Safety
/// `src` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_memory_write(
    instance: *mut BaedekerInstance,
    offset: u64,
    src: *const u8,
    len: u64,
) -> BaedekerStatus {
    guard(|| {
        let Some(handle) = (unsafe { instance_mut(instance) }) else {
            set_last_error("null instance handle");
            return BaedekerStatus::Usage;
        };
        if src.is_null() && len > 0 {
            set_last_error("null source");
            return BaedekerStatus::Usage;
        }
        let Some(result) = handle.store.with_memory_mut(0, |m| {
            let end = offset.saturating_add(len);
            if end > m.len() as u64 {
                set_last_error("write range out of bounds");
                return Err(BaedekerStatus::Usage);
            }
            let dst = &mut m[offset as usize..end as usize];
            unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len()) };
            Ok(())
        }) else {
            set_last_error("instance has no memory 0");
            return BaedekerStatus::Usage;
        };
        match result {
            Ok(()) => BaedekerStatus::Ok,
            Err(s) => s,
        }
    })
}
