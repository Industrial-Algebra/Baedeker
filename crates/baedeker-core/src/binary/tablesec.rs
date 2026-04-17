//! Table section parsing.
//!
//! Decodes defined tables from the table section.
//! See [Spec §5.5.6](https://webassembly.github.io/spec/core/binary/modules.html#table-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{Limits, RefType, TableType};

pub fn parse_table_section(section: &RawSection<'_>) -> Result<Vec<TableType>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut tables = Vec::with_capacity(count as usize);
    for _ in 0..count {
        tables.push(parse_table_type(&mut cursor, section.offset)?);
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::TableSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(tables)
}

fn parse_table_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TableType, DecodeError> {
    let elem = parse_ref_type(cursor, base_offset)?;
    let limits = parse_limits(cursor, base_offset)?;
    Ok(TableType { elem, limits })
}

fn parse_ref_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<RefType, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::TableSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match byte {
        0x70 => Ok(RefType::FuncRef),
        0x6F => Ok(RefType::ExternRef),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::TableSection,
            kind: DecodeErrorKind::UnknownRefType { byte },
        }),
    }
}

fn parse_limits(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Limits, DecodeError> {
    let tag_offset = cursor.position();
    let tag = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + tag_offset),
        context: DecodeContext::TableSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match tag {
        0x00 => {
            let min = decode_u32_in_section(cursor, base_offset)?;
            Ok(Limits { min, max: None })
        }
        0x01 => {
            let min = decode_u32_in_section(cursor, base_offset)?;
            let max = decode_u32_in_section(cursor, base_offset)?;
            Ok(Limits {
                min,
                max: Some(max),
            })
        }
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + tag_offset),
            context: DecodeContext::TableSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: tag,
            },
        }),
    }
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::TableSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;

    fn raw_table_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Table,
            offset: 22,
            data,
        }
    }

    #[test]
    fn parse_empty_table_section() {
        let section = raw_table_section(&[0x00]);
        let tables = parse_table_section(&section).unwrap();
        assert!(tables.is_empty());
    }

    #[test]
    fn parse_single_funcref_table() {
        let section = raw_table_section(&[0x01, 0x70, 0x00, 0x02]);
        let tables = parse_table_section(&section).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].elem, RefType::FuncRef);
        assert_eq!(tables[0].limits.min, 2);
        assert_eq!(tables[0].limits.max, None);
    }

    #[test]
    fn parse_single_externref_bounded_table() {
        let section = raw_table_section(&[0x01, 0x6F, 0x01, 0x01, 0x03]);
        let tables = parse_table_section(&section).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].elem, RefType::ExternRef);
        assert_eq!(tables[0].limits.min, 1);
        assert_eq!(tables[0].limits.max, Some(3));
    }

    #[test]
    fn reject_unknown_ref_type() {
        let section = raw_table_section(&[0x01, 0x6E, 0x00, 0x01]);
        let err = parse_table_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnknownRefType { byte: 0x6E }
        ));
    }

    #[test]
    fn reject_invalid_limits_tag() {
        let section = raw_table_section(&[0x01, 0x70, 0x02, 0x01]);
        let err = parse_table_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: 0x02,
            }
        ));
    }
}
