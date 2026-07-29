//! Error reporting for the C ABI: status codes plus a thread-local
//! last-error message string.

use std::cell::RefCell;
use std::ffi::CString;

/// Status codes returned by every `baedeker_*` function.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaedekerStatus {
    /// Success.
    Ok = 0,
    /// The binary failed to decode.
    Decode = 1,
    /// The module failed validation.
    Validation = 2,
    /// The validated module failed to lower to register IR.
    Lowering = 3,
    /// Instantiation failed (imports, segments, start function).
    Instantiation = 4,
    /// Execution trapped (use `baedeker_last_error` for the trap kind).
    Trap = 5,
    /// The instance's fuel budget was exhausted.
    FuelExhausted = 6,
    /// Bad argument or handle usage by the caller.
    Usage = 7,
    /// The requested operation is not supported by this FFI version.
    Unsupported = 8,
    /// A host function callback returned a failure.
    HostError = 9,
    /// Another runtime failure (see `baedeker_last_error`).
    Runtime = 10,
    /// A panic was caught at the FFI boundary (a bug — please report).
    Panic = 255,
}

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Record an error message on the calling thread.
pub(crate) fn set_last_error(message: impl Into<String>) {
    let message = message.into();
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(message).ok();
    });
}

/// Clear the calling thread's error message.
pub(crate) fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Copy the calling thread's error message into `buf`, returning the message
/// length excluding the NUL terminator (0 when there is no message). When the
/// message exceeds `buf_cap - 1` it is truncated; the buffer is always
/// NUL-terminated when `buf_cap > 0`.
///
/// # Safety
/// `buf` must point to at least `buf_cap` writable bytes, or be null (in
/// which case the required length is still returned).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_last_error(buf: *mut std::ffi::c_char, buf_cap: usize) -> usize {
    LAST_ERROR.with(|slot| {
        let borrow = slot.borrow();
        let Some(message) = borrow.as_ref() else {
            if !buf.is_null() && buf_cap > 0 {
                unsafe { *buf = 0 };
            }
            return 0;
        };
        let bytes = message.as_bytes();
        if !buf.is_null() && buf_cap > 0 {
            let n = bytes.len().min(buf_cap - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), n);
                *buf.add(n) = 0;
            }
        }
        bytes.len()
    })
}

/// Set the calling thread's error message from a C string. Intended for host
/// function callbacks: when a callback returns a non-`Ok` status, the FFI
/// reads this message to build the runtime error.
///
/// # Safety
/// `message` must be a valid NUL-terminated string, or null to clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn baedeker_set_last_error(message: *const std::ffi::c_char) {
    if message.is_null() {
        clear_last_error();
        return;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy();
    set_last_error(text.into_owned());
}

/// Fetch and clear the calling thread's message (used by the host-callback
/// wrapper).
pub(crate) fn take_last_error() -> Option<String> {
    LAST_ERROR.with(|slot| {
        slot.borrow_mut()
            .take()
            .map(|m| m.to_string_lossy().into_owned())
    })
}
