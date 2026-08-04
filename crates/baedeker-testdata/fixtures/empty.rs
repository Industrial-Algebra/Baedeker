// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Minimal valid WASM module — no exports, no functions.
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
