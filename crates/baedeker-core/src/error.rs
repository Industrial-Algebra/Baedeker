//! Error types for binary decoding.
//!
//! All errors carry byte offsets into the original binary and structured context,
//! enabling precise diagnostic messages.

use core::fmt;

/// The byte offset into the WASM binary where an error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteOffset(pub usize);

/// Contextual information about what was being decoded when the error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeContext {
    /// Decoding the WASM magic number.
    Magic,
    /// Decoding the WASM version number.
    Version,
    /// Decoding a section header.
    SectionHeader,
    /// Decoding section contents.
    SectionBody { id: u8 },
    /// Decoding a LEB128 value.
    Leb128,
    /// Decoding a type section entry.
    TypeSection,
    /// Decoding an import section entry.
    ImportSection,
    /// Decoding a function section entry.
    FunctionSection,
    /// Decoding a global section entry.
    GlobalSection,
    /// Decoding a memory section entry.
    MemorySection,
    /// Decoding a code section entry.
    CodeSection,
}

impl fmt::Display for DecodeContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeContext::Magic => write!(f, "WASM magic number"),
            DecodeContext::Version => write!(f, "WASM version"),
            DecodeContext::SectionHeader => write!(f, "section header"),
            DecodeContext::SectionBody { id } => write!(f, "section body (id={id})"),
            DecodeContext::Leb128 => write!(f, "LEB128 value"),
            DecodeContext::TypeSection => write!(f, "type section"),
            DecodeContext::ImportSection => write!(f, "import section"),
            DecodeContext::FunctionSection => write!(f, "function section"),
            DecodeContext::GlobalSection => write!(f, "global section"),
            DecodeContext::MemorySection => write!(f, "memory section"),
            DecodeContext::CodeSection => write!(f, "code section"),
        }
    }
}

/// Errors that can occur during binary decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// Byte offset into the binary where the error was detected.
    pub offset: ByteOffset,
    /// What was being decoded.
    pub context: DecodeContext,
    /// The specific error kind.
    pub kind: DecodeErrorKind,
}

/// Specific categories of decode errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeErrorKind {
    /// Unexpected end of input.
    UnexpectedEof,
    /// Invalid magic number (expected `\0asm`).
    InvalidMagic,
    /// Unsupported WASM version.
    UnsupportedVersion { found: u32 },
    /// LEB128 encoding exceeds the maximum number of bytes for the target type.
    Leb128TooLong,
    /// LEB128 encoding has unused bits set in the final byte (overlong/overflow).
    Leb128Overflow,
    /// Unknown section ID.
    UnknownSectionId { id: u8 },
    /// Section extends beyond the end of the binary.
    SectionOverflow,
    /// Sections are out of order (non-custom sections must be ordered by ID).
    SectionOutOfOrder { prev: u8, current: u8 },
    /// Duplicate non-custom section.
    DuplicateSection { id: u8 },
    /// Unknown value type encoding byte.
    UnknownValType { byte: u8 },
    /// Unknown reference type encoding byte.
    UnknownRefType { byte: u8 },
    /// Unknown import descriptor tag.
    UnknownImportDesc { byte: u8 },
    /// Invalid global mutability encoding.
    InvalidMutability { byte: u8 },
    /// Invalid UTF-8 in a name string.
    InvalidUtf8,
    /// Function and code section counts disagree.
    FunctionCodeLengthMismatch { functions: u32, codes: u32 },
    /// Unknown instruction opcode.
    UnknownOpcode { byte: u8 },
    /// Unknown SIMD-prefixed instruction opcode.
    UnknownSimdOpcode { opcode: u32 },
    /// Unexpected byte value.
    UnexpectedByte { expected: u8, found: u8 },
    /// Section body was not fully consumed.
    SectionSizeMismatch { expected: u32, consumed: u32 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "decode error at byte {}: {}: {}",
            self.offset.0, self.context, self.kind
        )
    }
}

impl fmt::Display for DecodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeErrorKind::UnexpectedEof => write!(f, "unexpected end of input"),
            DecodeErrorKind::InvalidMagic => write!(f, "invalid magic number (expected \\0asm)"),
            DecodeErrorKind::UnsupportedVersion { found } => {
                write!(f, "unsupported WASM version {found} (expected 1)")
            }
            DecodeErrorKind::Leb128TooLong => write!(f, "LEB128 encoding too long"),
            DecodeErrorKind::Leb128Overflow => write!(f, "LEB128 overflow (unused bits set)"),
            DecodeErrorKind::UnknownSectionId { id } => {
                write!(f, "unknown section ID {id:#04x}")
            }
            DecodeErrorKind::SectionOverflow => {
                write!(f, "section extends beyond end of binary")
            }
            DecodeErrorKind::SectionOutOfOrder { prev, current } => {
                write!(
                    f,
                    "section {current} appears after section {prev} (out of order)"
                )
            }
            DecodeErrorKind::DuplicateSection { id } => {
                write!(f, "duplicate section (id={id})")
            }
            DecodeErrorKind::UnknownValType { byte } => {
                write!(f, "unknown value type {byte:#04x}")
            }
            DecodeErrorKind::UnknownRefType { byte } => {
                write!(f, "unknown reference type {byte:#04x}")
            }
            DecodeErrorKind::UnknownImportDesc { byte } => {
                write!(f, "unknown import descriptor {byte:#04x}")
            }
            DecodeErrorKind::InvalidMutability { byte } => {
                write!(f, "invalid mutability {byte:#04x}")
            }
            DecodeErrorKind::InvalidUtf8 => write!(f, "invalid UTF-8 string"),
            DecodeErrorKind::FunctionCodeLengthMismatch { functions, codes } => {
                write!(
                    f,
                    "function/code section length mismatch: {functions} declarations, {codes} bodies"
                )
            }
            DecodeErrorKind::UnknownOpcode { byte } => {
                write!(f, "unknown opcode {byte:#04x}")
            }
            DecodeErrorKind::UnknownSimdOpcode { opcode } => {
                write!(f, "unknown SIMD opcode {opcode:#04x}")
            }
            DecodeErrorKind::UnexpectedByte { expected, found } => {
                write!(f, "expected {expected:#04x}, found {found:#04x}")
            }
            DecodeErrorKind::SectionSizeMismatch { expected, consumed } => {
                write!(
                    f,
                    "section size mismatch: declared {expected} bytes, consumed {consumed}"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

// For no_std with core::error::Error (stabilized in Rust 1.81+)
#[cfg(not(feature = "std"))]
impl core::error::Error for DecodeError {}
