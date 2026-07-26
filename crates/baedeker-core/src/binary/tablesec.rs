//! Table section parsing.
//!
//! Decodes defined tables from the table section.
//! See [Spec §5.5.6](https://webassembly.github.io/spec/core/binary/modules.html#table-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::{
    parse_ref_type as parse_binary_ref_type, parse_ref_type_with_first_byte,
};
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
    // The 0x40 marker denotes a table with an initializer expression.
    let first = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + cursor.position()),
        context: DecodeContext::TableSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    if first == 0x40 {
        // The 0x40 0x00 marker pair denotes a table with an initializer
        // expression: reftype, limits, then a const expr.
        let second = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + cursor.position()),
            context: DecodeContext::TableSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if second != 0x00 {
            return Err(DecodeError {
                offset: ByteOffset(base_offset + cursor.position() - 1),
                context: DecodeContext::TableSection,
                kind: DecodeErrorKind::UnknownRefType { byte: second },
            });
        }
        let elem = parse_ref_type(cursor, base_offset)?;
        let limits = parse_limits(cursor, base_offset)?;
        // The initializer is a const expr terminated by `end` (0x0B),
        // decoded instruction-wise so immediate payloads can't confuse the
        // terminator scan.
        let expr_start = cursor.position();
        let mut block_depth = 0usize;
        loop {
            let instr = crate::binary::instr::decode_instr(cursor, base_offset)?;
            match instr {
                crate::binary::instr::Instr::Block(_)
                | crate::binary::instr::Instr::Loop(_)
                | crate::binary::instr::Instr::If(_) => block_depth += 1,
                crate::binary::instr::Instr::End => {
                    if block_depth == 0 {
                        break;
                    }
                    block_depth -= 1;
                }
                _ => {}
            }
        }
        let init = cursor.original()[expr_start..cursor.position()].to_vec();
        return Ok(TableType {
            elem,
            limits,
            init: Some(init),
        });
    }
    let elem = parse_ref_type_with_first_byte(
        cursor,
        first,
        base_offset + cursor.position() - 1,
        DecodeContext::TableSection,
    )?;
    let limits = parse_limits(cursor, base_offset)?;
    Ok(TableType {
        elem,
        limits,
        init: None,
    })
}

fn parse_ref_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<RefType, DecodeError> {
    parse_binary_ref_type(cursor, base_offset, DecodeContext::TableSection)
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
    fn parse_typed_function_reference_table() {
        let section = raw_table_section(&[
            0x01, // one table
            0x63, 0x00, // (ref null type 0)
            0x00, 0x01, // min 1
        ]);
        let tables = parse_table_section(&section).unwrap();

        assert_eq!(
            tables[0].elem,
            RefType::concrete(true, crate::types::TypeIdx(0))
        );
        assert_eq!(tables[0].limits.min, 1);
        assert_eq!(tables[0].limits.max, None);
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
