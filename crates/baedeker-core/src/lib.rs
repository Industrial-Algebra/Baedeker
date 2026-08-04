// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Baedeker WebAssembly runtime core.
//!
//! A `no_std` WebAssembly 2.0 engine providing binary decoding, validation,
//! lowering, and execution. Designed to embed cleanly into iOS, bare metal,
//! or even another WASM runtime.
//!
//! This crate is language-runtime infrastructure: its binary parsing,
//! malformed-input handling, validation, and future robustness testing exist
//! to improve standards compliance, portability, diagnostic quality, and safe
//! execution behavior. It is not intended for offensive security workflows.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Unit tests may use std (threads for deep-recursion stack headroom, etc.)
// even when the crate itself is built no_std.
#[cfg(all(test, not(feature = "std")))]
extern crate std;

#[cfg(feature = "aot")]
pub mod aot;
pub mod binary;
pub mod error;
pub mod lower;
pub mod runtime;
pub mod types;
pub mod validate;
