//! Allocates linear memory and exports a function that writes to it.
//! Exercises the memory section.
#![no_std]
#![no_main]

static mut BUFFER: [u8; 256] = [0u8; 256];

/// Write a byte at the given offset in the buffer. Returns the previous value.
#[no_mangle]
pub unsafe extern "C" fn write_byte(offset: u32, value: u8) -> u8 {
    let idx = offset as usize;
    if idx < BUFFER.len() {
        let prev = BUFFER[idx];
        BUFFER[idx] = value;
        prev
    } else {
        0
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
