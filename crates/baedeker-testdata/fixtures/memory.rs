//! Allocates linear memory and exports a function that writes to it.
//! Exercises the memory section.
#![no_std]
#![no_main]

const BUFFER_LEN: usize = 256;
static mut BUFFER: [u8; BUFFER_LEN] = [0u8; BUFFER_LEN];

/// Write a byte at the given offset in the buffer. Returns the previous value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn write_byte(offset: u32, value: u8) -> u8 {
    let idx = offset as usize;
    if idx < BUFFER_LEN {
        unsafe {
            let ptr = core::ptr::addr_of_mut!(BUFFER) as *mut u8;
            let cell = ptr.add(idx);
            let prev = cell.read();
            cell.write(value);
            prev
        }
    } else {
        0
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
