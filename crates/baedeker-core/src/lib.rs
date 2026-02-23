//! Baedeker WebAssembly runtime core.
//!
//! A `no_std` WebAssembly 2.0 engine providing binary decoding, validation,
//! and execution. Designed to embed cleanly into iOS, bare metal, or even
//! another WASM runtime.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod binary;
pub mod error;
pub mod types;
