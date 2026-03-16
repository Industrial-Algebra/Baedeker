//! WASM section parsing.
//!
//! Sections are the top-level organizational unit of a WASM binary.
//! See [Spec §5.5](https://webassembly.github.io/spec/core/binary/modules.html#sections).

use crate::binary::leb128::{self, Cursor};
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};

/// WASM section identifiers.
/// See [Spec §5.5](https://webassembly.github.io/spec/core/binary/modules.html#sections).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SectionId {
    Custom = 0,
    Type = 1,
    Import = 2,
    Function = 3,
    Table = 4,
    Memory = 5,
    Global = 6,
    Export = 7,
    Start = 8,
    Element = 9,
    Code = 10,
    Data = 11,
    DataCount = 12,
}

impl SectionId {
    /// Try to construct a `SectionId` from a raw byte value.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(SectionId::Custom),
            1 => Some(SectionId::Type),
            2 => Some(SectionId::Import),
            3 => Some(SectionId::Function),
            4 => Some(SectionId::Table),
            5 => Some(SectionId::Memory),
            6 => Some(SectionId::Global),
            7 => Some(SectionId::Export),
            8 => Some(SectionId::Start),
            9 => Some(SectionId::Element),
            10 => Some(SectionId::Code),
            11 => Some(SectionId::Data),
            12 => Some(SectionId::DataCount),
            _ => None,
        }
    }

    /// Human-readable name for this section.
    pub fn name(self) -> &'static str {
        match self {
            SectionId::Custom => "custom",
            SectionId::Type => "type",
            SectionId::Import => "import",
            SectionId::Function => "function",
            SectionId::Table => "table",
            SectionId::Memory => "memory",
            SectionId::Global => "global",
            SectionId::Export => "export",
            SectionId::Start => "start",
            SectionId::Element => "element",
            SectionId::Code => "code",
            SectionId::Data => "data",
            SectionId::DataCount => "datacount",
        }
    }
}

/// A parsed but not yet interpreted section — just the labeled byte span.
#[derive(Debug, Clone)]
pub struct RawSection<'a> {
    /// The section ID.
    pub id: SectionId,
    /// Byte offset of the section contents within the original binary.
    pub offset: usize,
    /// The raw section contents (after the section header).
    pub data: &'a [u8],
}

/// The WASM binary magic number: `\0asm`.
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

/// The WASM binary version we support: 1.
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Validate the WASM preamble (magic number + version), returning the cursor
/// positioned after the 8-byte header.
pub fn parse_preamble<'a>(cursor: &mut Cursor<'a>) -> Result<(), DecodeError> {
    let magic = cursor.read_bytes(4).map_err(|_| DecodeError {
        offset: ByteOffset(0),
        context: DecodeContext::Magic,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    if magic != WASM_MAGIC {
        return Err(DecodeError {
            offset: ByteOffset(0),
            context: DecodeContext::Magic,
            kind: DecodeErrorKind::InvalidMagic,
        });
    }

    let version = cursor.read_bytes(4).map_err(|_| DecodeError {
        offset: ByteOffset(4),
        context: DecodeContext::Version,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    if version != WASM_VERSION {
        let found = u32::from_le_bytes([version[0], version[1], version[2], version[3]]);
        return Err(DecodeError {
            offset: ByteOffset(4),
            context: DecodeContext::Version,
            kind: DecodeErrorKind::UnsupportedVersion { found },
        });
    }

    Ok(())
}

/// Parse all sections from a cursor positioned after the preamble.
///
/// Returns the sections as raw byte spans. Non-custom sections must appear
/// in order of their section IDs (custom sections may appear anywhere).
pub fn parse_sections<'a>(
    cursor: &mut Cursor<'a>,
) -> Result<alloc::vec::Vec<RawSection<'a>>, DecodeError> {
    let mut sections = alloc::vec::Vec::new();
    let mut last_non_custom_id: Option<u8> = None;

    while !cursor.is_empty() {
        let id_offset = cursor.position();
        let id_byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(id_offset),
            context: DecodeContext::SectionHeader,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;

        let id = SectionId::from_byte(id_byte).ok_or(DecodeError {
            offset: ByteOffset(id_offset),
            context: DecodeContext::SectionHeader,
            kind: DecodeErrorKind::UnknownSectionId { id: id_byte },
        })?;

        let size = leb128::decode_u32(cursor).map_err(|mut e| {
            e.context = DecodeContext::SectionHeader;
            e
        })?;

        let content_offset = cursor.position();

        if content_offset + size as usize > cursor.position() + cursor.remaining().len() {
            return Err(DecodeError {
                offset: ByteOffset(id_offset),
                context: DecodeContext::SectionHeader,
                kind: DecodeErrorKind::SectionOverflow,
            });
        }

        // Ordering check: non-custom sections must appear in ascending ID order,
        // and no duplicates.
        if id != SectionId::Custom {
            if let Some(prev) = last_non_custom_id
                && id_byte <= prev
            {
                return Err(if id_byte == prev {
                    DecodeError {
                        offset: ByteOffset(id_offset),
                        context: DecodeContext::SectionHeader,
                        kind: DecodeErrorKind::DuplicateSection { id: id_byte },
                    }
                } else {
                    DecodeError {
                        offset: ByteOffset(id_offset),
                        context: DecodeContext::SectionHeader,
                        kind: DecodeErrorKind::SectionOutOfOrder {
                            prev,
                            current: id_byte,
                        },
                    }
                });
            }
            last_non_custom_id = Some(id_byte);
        }

        let data = cursor.read_bytes(size as usize).map_err(|_| DecodeError {
            offset: ByteOffset(content_offset),
            context: DecodeContext::SectionBody { id: id_byte },
            kind: DecodeErrorKind::SectionOverflow,
        })?;

        sections.push(RawSection {
            id,
            offset: content_offset,
            data,
        });
    }

    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid WASM module: just the 8-byte header.
    const MINIMAL_MODULE: [u8; 8] = [
        0x00, 0x61, 0x73, 0x6D, // \0asm
        0x01, 0x00, 0x00, 0x00, // version 1
    ];

    #[test]
    fn parse_minimal_module_preamble() {
        let mut cursor = Cursor::new(&MINIMAL_MODULE);
        parse_preamble(&mut cursor).unwrap();
        assert!(cursor.is_empty());
    }

    #[test]
    fn reject_bad_magic() {
        let data = [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let err = parse_preamble(&mut cursor).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::InvalidMagic);
    }

    #[test]
    fn reject_bad_version() {
        let data = [0x00, 0x61, 0x73, 0x6D, 0x02, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let err = parse_preamble(&mut cursor).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnsupportedVersion { found: 2 }
        ));
    }

    #[test]
    fn parse_empty_sections() {
        let mut cursor = Cursor::new(&MINIMAL_MODULE);
        parse_preamble(&mut cursor).unwrap();
        let sections = parse_sections(&mut cursor).unwrap();
        assert!(sections.is_empty());
    }

    #[test]
    fn parse_single_type_section() {
        // Header + type section (id=1) with 2 bytes of content
        let data = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, // section id: type
            0x02, // section size: 2 bytes
            0xAA, 0xBB, // section content
        ];
        let mut cursor = Cursor::new(&data);
        parse_preamble(&mut cursor).unwrap();
        let sections = parse_sections(&mut cursor).unwrap();

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, SectionId::Type);
        assert_eq!(sections[0].data, &[0xAA, 0xBB]);
    }

    #[test]
    fn parse_multiple_sections_in_order() {
        let data = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x01, 0xFF, // type section (1 byte)
            0x03, 0x01, 0xEE, // function section (1 byte)
            0x07, 0x01, 0xDD, // export section (1 byte)
        ];
        let mut cursor = Cursor::new(&data);
        parse_preamble(&mut cursor).unwrap();
        let sections = parse_sections(&mut cursor).unwrap();

        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].id, SectionId::Type);
        assert_eq!(sections[1].id, SectionId::Function);
        assert_eq!(sections[2].id, SectionId::Export);
    }

    #[test]
    fn reject_duplicate_section() {
        let data = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0x01, 0xFF, // type section
            0x01, 0x01, 0xEE, // duplicate type section
        ];
        let mut cursor = Cursor::new(&data);
        parse_preamble(&mut cursor).unwrap();
        let err = parse_sections(&mut cursor).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::DuplicateSection { id: 1 }
        ));
    }

    #[test]
    fn reject_out_of_order_sections() {
        let data = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x03, 0x01, 0xFF, // function section (id=3)
            0x01, 0x01, 0xEE, // type section (id=1) — out of order
        ];
        let mut cursor = Cursor::new(&data);
        parse_preamble(&mut cursor).unwrap();
        let err = parse_sections(&mut cursor).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::SectionOutOfOrder {
                prev: 3,
                current: 1
            }
        ));
    }

    #[test]
    fn custom_sections_allowed_anywhere() {
        let data = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x00, 0x01, 0xFF, // custom section
            0x01, 0x01, 0xAA, // type section
            0x00, 0x01, 0xBB, // another custom section
            0x03, 0x01, 0xCC, // function section
            0x00, 0x01, 0xDD, // yet another custom section
        ];
        let mut cursor = Cursor::new(&data);
        parse_preamble(&mut cursor).unwrap();
        let sections = parse_sections(&mut cursor).unwrap();

        assert_eq!(sections.len(), 5);
        assert_eq!(sections[0].id, SectionId::Custom);
        assert_eq!(sections[1].id, SectionId::Type);
        assert_eq!(sections[2].id, SectionId::Custom);
        assert_eq!(sections[3].id, SectionId::Function);
        assert_eq!(sections[4].id, SectionId::Custom);
    }

    #[test]
    fn reject_section_overflow() {
        let data = [
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version
            0x01, 0xFF, 0x01, // type section claiming 255 bytes, but none follow
        ];
        let mut cursor = Cursor::new(&data);
        parse_preamble(&mut cursor).unwrap();
        let err = parse_sections(&mut cursor).unwrap_err();
        assert!(matches!(err.kind, DecodeErrorKind::SectionOverflow));
    }
}
