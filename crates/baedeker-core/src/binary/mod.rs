//! WebAssembly binary format decoding.
//!
//! See [Spec §5](https://webassembly.github.io/spec/core/binary/index.html).

pub mod leb128;
pub mod module;
pub mod section;
