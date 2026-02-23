//! Exports an `add(i32, i32) -> i32` function.
//! Produces type, function, export, and code sections.
#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
