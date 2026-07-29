//! End-to-end exercise of the C ABI from Rust: compile → instantiate → call,
//! memory access, host functions, fuel, and error/status mapping.

use std::ffi::{CStr, c_void};

use baedeker_ffi::api::*;
use baedeker_ffi::api2::*;
use baedeker_ffi::error::*;
use baedeker_ffi::value::*;

const WAT: &str = r#"
(module
  (import "env" "host_add" (func $host_add (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "add") (param i32 i32) (result i32)
    local.get 0 local.get 1 i32.add)
  (func (export "via_host") (param i32 i32) (result i32)
    local.get 0 local.get 1 call $host_add)
  (func (export "boom") (param i32 i32) unreachable)
  (func (export "store42") (param i32 i32)
    local.get 0 i32.const 42 i32.store)
  (func (export "spin") (param i32 i32) (loop br 0))
)"#;

fn wasm_bytes() -> Vec<u8> {
    let buf = wast::parser::ParseBuffer::new(WAT).unwrap();
    wast::parser::parse::<wast::Wat>(&buf)
        .unwrap()
        .encode()
        .unwrap()
}

unsafe extern "C" fn host_add_ok(
    args: *const BaedekerValue,
    n_args: usize,
    results: *mut BaedekerValue,
    n_results: usize,
    _user: *mut c_void,
) -> BaedekerStatus {
    assert_eq!(n_args, 2);
    assert_eq!(n_results, 1);
    let args = unsafe { std::slice::from_raw_parts(args, n_args) };
    let (a, b) = unsafe { (args[0].data.i32_, args[1].data.i32_) };
    unsafe { (*results).tag = BaedekerValueTag::I32 };
    unsafe { (*results).data.i32_ = a + b };
    BaedekerStatus::Ok
}

unsafe extern "C" fn host_fail(
    _args: *const BaedekerValue,
    _n: usize,
    _r: *mut BaedekerValue,
    _n_r: usize,
    _u: *mut c_void,
) -> BaedekerStatus {
    let msg = c"host says no";
    unsafe { baedeker_set_last_error(msg.as_ptr()) };
    BaedekerStatus::HostError
}

struct Instance(*mut BaedekerInstance);

impl Instance {
    fn new() -> Self {
        let bytes = wasm_bytes();
        let mut module: *mut BaedekerModule = std::ptr::null_mut();
        let status = unsafe { baedeker_module_compile(bytes.as_ptr(), bytes.len(), &mut module) };
        assert_eq!(status, BaedekerStatus::Ok);
        let mut instance: *mut BaedekerInstance = std::ptr::null_mut();
        let status = unsafe { baedeker_instance_new(module, &mut instance) };
        assert_eq!(status, BaedekerStatus::Ok);
        // The module handle can be freed immediately; the instance owns a ref.
        unsafe { baedeker_module_free(module) };
        Self(instance)
    }

    fn call_i32s(&self, name: &CStr, args: [i32; 2]) -> (BaedekerStatus, i32) {
        let c_args = [
            BaedekerValue {
                tag: BaedekerValueTag::I32,
                data: BaedekerValueData { i32_: args[0] },
            },
            BaedekerValue {
                tag: BaedekerValueTag::I32,
                data: BaedekerValueData { i32_: args[1] },
            },
        ];
        let mut out = [BaedekerValue {
            tag: BaedekerValueTag::I32,
            data: BaedekerValueData { i32_: 0 },
        }];
        let mut n_out = 0usize;
        let status = unsafe {
            baedeker_instance_call(
                self.0,
                name.as_ptr(),
                c_args.as_ptr(),
                c_args.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut n_out,
            )
        };
        (status, unsafe { out[0].data.i32_ })
    }

    fn last_error() -> String {
        let mut buf = [0i8; 256];
        let n = unsafe { baedeker_last_error(buf.as_mut_ptr(), buf.len()) };
        assert!(n > 0, "expected a last-error message");
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { baedeker_instance_free(self.0) };
    }
}

#[test]
fn version_is_a_static_c_string() {
    let version = unsafe { CStr::from_ptr(baedeker_version()) }.to_string_lossy();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn compile_rejects_garbage_with_decode_status() {
    let mut module: *mut BaedekerModule = std::ptr::null_mut();
    let status = unsafe { baedeker_module_compile(b"not wasm".as_ptr(), 8, &mut module) };
    assert_eq!(status, BaedekerStatus::Decode);
    assert!(module.is_null());
    assert!(Instance::last_error().contains("decode failed"));
}

#[test]
fn call_export_and_memory_roundtrip() {
    let instance = Instance::new();

    let (status, sum) = instance.call_i32s(c"add", [40, 2]);
    assert_eq!(status, BaedekerStatus::Ok);
    assert_eq!(sum, 42);

    // Store through WASM, read back through the FFI memory API.
    let (status, _) = instance.call_i32s(c"store42", [16, 0]);
    assert_eq!(status, BaedekerStatus::Ok);
    let mut buf = [0u8; 4];
    let status = unsafe { baedeker_instance_memory_read(instance.0, 16, buf.as_mut_ptr(), 4) };
    assert_eq!(status, BaedekerStatus::Ok);
    assert_eq!(i32::from_le_bytes(buf), 42);

    // Write through the FFI, read through the data pointer.
    let status =
        unsafe { baedeker_instance_memory_write(instance.0, 20, 77i32.to_le_bytes().as_ptr(), 4) };
    assert_eq!(status, BaedekerStatus::Ok);
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len: u64 = 0;
    let status = unsafe { baedeker_instance_memory_data(instance.0, &mut data, &mut len) };
    assert_eq!(status, BaedekerStatus::Ok);
    assert_eq!(len, 65536);
    let value = unsafe { std::ptr::read_unaligned(data.add(20).cast::<i32>()) };
    assert_eq!(i32::from_le(value), 77);

    // Out-of-bounds read is a usage error, not a crash.
    let status = unsafe { baedeker_instance_memory_read(instance.0, 65533, buf.as_mut_ptr(), 4) };
    assert_eq!(status, BaedekerStatus::Usage);
}

#[test]
fn host_function_success_and_failure_paths() {
    let instance = Instance::new();
    let params = [BaedekerValueTag::I32 as u8, BaedekerValueTag::I32 as u8];
    let results = [BaedekerValueTag::I32 as u8];

    let status = unsafe {
        baedeker_instance_register_host_func(
            instance.0,
            c"env".as_ptr(),
            c"host_add".as_ptr(),
            params.as_ptr(),
            params.len(),
            results.as_ptr(),
            results.len(),
            Some(host_add_ok),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, BaedekerStatus::Ok);
    let (status, sum) = instance.call_i32s(c"via_host", [20, 22]);
    assert_eq!(status, BaedekerStatus::Ok);
    assert_eq!(sum, 42);

    // Re-register with a failing callback; the host message surfaces.
    let status = unsafe {
        baedeker_instance_register_host_func(
            instance.0,
            c"env".as_ptr(),
            c"host_add".as_ptr(),
            params.as_ptr(),
            params.len(),
            results.as_ptr(),
            results.len(),
            Some(host_fail),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, BaedekerStatus::Ok);
    let (status, _) = instance.call_i32s(c"via_host", [1, 1]);
    assert_eq!(status, BaedekerStatus::HostError);
    assert!(Instance::last_error().contains("host says no"));
}

#[test]
fn traps_and_fuel_map_to_distinct_statuses() {
    let instance = Instance::new();

    let (status, _) = instance.call_i32s(c"boom", [0, 0]);
    assert_eq!(status, BaedekerStatus::Trap);
    assert!(Instance::last_error().contains("Unreachable"));

    // Unlimited fuel spins forever — bound it instead.
    let status = unsafe { baedeker_instance_set_fuel(instance.0, 100_000) };
    assert_eq!(status, BaedekerStatus::Ok);
    let (status, _) = instance.call_i32s(c"spin", [0, 0]);
    assert_eq!(status, BaedekerStatus::FuelExhausted);
}

#[test]
fn null_and_misuse_paths_are_usage_errors() {
    let mut module: *mut BaedekerModule = std::ptr::null_mut();
    let status = unsafe { baedeker_module_compile(std::ptr::null(), 8, &mut module) };
    assert_eq!(status, BaedekerStatus::Usage);

    let instance = Instance::new();
    let (status, _) = instance.call_i32s(c"no_such_export", [0, 0]);
    assert_eq!(status, BaedekerStatus::Runtime);
    assert!(Instance::last_error().contains("UnknownExport"));

    // Freeing null is explicitly allowed.
    unsafe {
        baedeker_module_free(std::ptr::null_mut());
        baedeker_instance_free(std::ptr::null_mut());
    }
}
