//! WebAssembly binary format decoding.
//!
//! See [Spec §5](https://webassembly.github.io/spec/core/binary/index.html).

pub mod codesec;
pub mod datasec;
pub mod elemsec;
pub mod exportsec;
pub mod functionsec;
pub mod globalsec;
pub mod importsec;
pub mod instr;
pub mod leb128;
pub mod memorysec;
pub mod module;
pub mod section;
pub mod startsec;
pub mod tablesec;
pub mod typeparser;
pub mod typesec;
