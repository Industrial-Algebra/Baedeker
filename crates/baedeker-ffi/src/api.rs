//! The `extern "C"` entry points. Every function runs inside a panic guard
//! and reports through [`BaedekerStatus`] + the thread-local last error.

use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use baedeker_core::binary::module::Module;
use baedeker_core::lower::{RegModule, lower_module};
use baedeker_core::runtime::Store;

use crate::error::{BaedekerStatus, set_last_error};
use crate::value::BaedekerValue;

/// Opaque handle to a compiled module (decoded, validated, lowered).
pub struct BaedekerModule {
    _private: [u8; 0],
}

/// Opaque handle to an instantiated module with its own store.
pub struct BaedekerInstance {
    _private: [u8; 0],
}

struct ModuleHandle {
    module: Arc<RegModule>,
}

pub(crate) struct InstanceHandle {
    pub(crate) module: Arc<RegModule>,
    pub(crate) store: Store,
}

/// Host function callback (nullable). `args`/`results` carry exactly the
/// declared signature arity. Return `BaedekerStatusOk` on success; any other
/// status traps the calling WASM function, with `baedeker_set_last_error`
/// providing the message when set.
pub type BaedekerHostFn = Option<
    unsafe extern "C" fn(
        args: *const BaedekerValue,
        n_args: usize,
        results: *mut BaedekerValue,
        n_results: usize,
        user_data: *mut c_void,
    ) -> BaedekerStatus,
>;

pub(crate) fn guard(f: impl FnOnce() -> BaedekerStatus) -> BaedekerStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(status) => status,
        Err(_) => {
            set_last_error("panic caught at the FFI boundary (this is a bug)");
            BaedekerStatus::Panic
        }
    }
}

unsafe fn module_ref<'a>(ptr: *const BaedekerModule) -> Option<&'a ModuleHandle> {
    unsafe { ptr.cast::<ModuleHandle>().as_ref() }
}

pub(crate) unsafe fn instance_ref<'a>(ptr: *const BaedekerInstance) -> Option<&'a InstanceHandle> {
    unsafe { ptr.cast::<InstanceHandle>().as_ref() }
}

pub(crate) unsafe fn instance_mut<'a>(
    ptr: *mut BaedekerInstance,
) -> Option<&'a mut InstanceHandle> {
    unsafe { ptr.cast::<InstanceHandle>().as_mut() }
}

pub(crate) fn c_name<'a>(ptr: *const c_char, what: &str) -> Result<&'a str, BaedekerStatus> {
    if ptr.is_null() {
        set_last_error(format!("null {what} pointer"));
        return Err(BaedekerStatus::Usage);
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().map_err(|_| {
        set_last_error(format!("{what} is not valid UTF-8"));
        BaedekerStatus::Usage
    })
}

/// The Baedeker version string (static storage, do not free).
#[unsafe(no_mangle)]
pub extern "C" fn baedeker_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Decode, validate, and lower a WASM binary. `bytes` may be freed after this
/// returns. On success `*out` receives an owned module handle.
///
/// # Safety
/// `bytes` must be valid for `len` bytes; `out` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_module_compile(
    bytes: *const u8,
    len: usize,
    out: *mut *mut BaedekerModule,
) -> BaedekerStatus {
    guard(|| {
        if bytes.is_null() || out.is_null() {
            set_last_error("null bytes or out pointer");
            return BaedekerStatus::Usage;
        }
        let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
        let module = match Module::decode(bytes) {
            Ok(module) => module,
            Err(e) => {
                set_last_error(format!("decode failed: {e:?}"));
                return BaedekerStatus::Decode;
            }
        };
        if let Err(e) = module.validate() {
            set_last_error(format!("validation failed: {e:?}"));
            return BaedekerStatus::Validation;
        }
        match lower_module(&module) {
            Ok(lowered) => {
                let handle = Box::new(ModuleHandle {
                    module: Arc::new(lowered),
                });
                unsafe { *out = Box::into_raw(handle).cast::<BaedekerModule>() };
                BaedekerStatus::Ok
            }
            Err(e) => {
                set_last_error(format!("lowering failed: {e:?}"));
                BaedekerStatus::Lowering
            }
        }
    })
}

/// Free a module handle (null is allowed).
///
/// # Safety
/// `module` must be a handle from `baedeker_module_compile`, freed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_module_free(module: *mut BaedekerModule) {
    if !module.is_null() {
        drop(unsafe { Box::from_raw(module.cast::<ModuleHandle>()) });
    }
}

/// Instantiate a compiled module. The instance owns its store; the module
/// handle may be freed afterwards (the instance keeps its own reference).
///
/// # Safety
/// `module` must be a valid module handle; `out` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_new(
    module: *const BaedekerModule,
    out: *mut *mut BaedekerInstance,
) -> BaedekerStatus {
    guard(|| {
        if out.is_null() {
            set_last_error("null out pointer");
            return BaedekerStatus::Usage;
        }
        let Some(handle) = (unsafe { module_ref(module) }) else {
            set_last_error("null module handle");
            return BaedekerStatus::Usage;
        };
        match Store::instantiate(&handle.module) {
            Ok(store) => {
                let instance = Box::new(InstanceHandle {
                    module: Arc::clone(&handle.module),
                    store,
                });
                unsafe { *out = Box::into_raw(instance).cast::<BaedekerInstance>() };
                BaedekerStatus::Ok
            }
            Err(e) => {
                set_last_error(format!("instantiation failed: {e:?}"));
                BaedekerStatus::Instantiation
            }
        }
    })
}

/// Free an instance handle (null is allowed). The instance must not be freed
/// while one of its host callbacks is executing.
///
/// # Safety
/// `instance` must be a handle from `baedeker_instance_new`, freed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_free(instance: *mut BaedekerInstance) {
    if !instance.is_null() {
        drop(unsafe { Box::from_raw(instance.cast::<InstanceHandle>()) });
    }
}

/// Set the instance's instruction fuel budget. A negative value means
/// unlimited (the default).
///
/// # Safety
/// `instance` must be a valid instance handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_instance_set_fuel(
    instance: *mut BaedekerInstance,
    fuel: i64,
) -> BaedekerStatus {
    guard(|| {
        let Some(handle) = (unsafe { instance_mut(instance) }) else {
            set_last_error("null instance handle");
            return BaedekerStatus::Usage;
        };
        handle
            .store
            .set_fuel(if fuel < 0 { None } else { Some(fuel as u64) });
        BaedekerStatus::Ok
    })
}
