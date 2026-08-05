// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Global section parsing.
//!
//! Decodes defined globals and their initializer expressions from the global section.
//! See [Spec §5.5.11](https://webassembly.github.io/spec/core/binary/modules.html#global-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::parse_val_type as parse_binary_val_type;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{Global, GlobalType, Mutability, ValType};

pub fn parse_global_section<'a>(section: &RawSection<'a>) -> Result<Vec<Global<'a>>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut globals = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let global_type = parse_global_type(&mut cursor, section.offset)?;
        let init_offset = section.offset + cursor.position();
        let init_expr = parse_init_expr(&mut cursor, section.offset)?;
        globals.push(Global {
            global_type,
            init_expr,
            init_offset,
        });
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::GlobalSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(globals)
}

fn parse_global_type(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<GlobalType, DecodeError> {
    let val_type = parse_val_type(cursor, base_offset)?;
    let mutability = parse_mutability(cursor, base_offset)?;
    Ok(GlobalType {
        val_type,
        mutability,
    })
}

fn parse_val_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<ValType, DecodeError> {
    parse_binary_val_type(cursor, base_offset, DecodeContext::GlobalSection)
}

fn parse_mutability(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Mutability, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::GlobalSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match byte {
        0x00 => Ok(Mutability::Const),
        0x01 => Ok(Mutability::Var),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::GlobalSection,
            kind: DecodeErrorKind::InvalidMutability { byte },
        }),
    }
}

fn parse_init_expr<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<&'a [u8], DecodeError> {
    let start = cursor.position();
    loop {
        let opcode_offset = cursor.position();
        let byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + opcode_offset),
            context: DecodeContext::GlobalSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;

        if byte == 0x0B {
            let end = cursor.position();
            return Ok(&cursor.original()[start..end]);
        }
    }
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::GlobalSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;
    use crate::types::NumType;

    fn raw_global_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Global,
            offset: 30,
            data,
        }
    }

    #[test]
    fn parse_empty_global_section() {
        let section = raw_global_section(&[0x00]);
        let globals = parse_global_section(&section).unwrap();
        assert!(globals.is_empty());
    }

    #[test]
    fn parse_single_i32_global() {
        let section = raw_global_section(&[0x01, 0x7F, 0x00, 0x41, 0x2A, 0x0B]);
        let globals = parse_global_section(&section).unwrap();
        assert_eq!(globals.len(), 1);
        assert_eq!(globals[0].global_type.val_type, ValType::Num(NumType::I32));
        assert_eq!(globals[0].global_type.mutability, Mutability::Const);
        assert_eq!(globals[0].init_expr, &[0x41, 0x2A, 0x0B]);
        assert_eq!(globals[0].init_offset, 33);
    }

    #[test]
    fn reject_invalid_mutability() {
        let section = raw_global_section(&[0x01, 0x7F, 0x02, 0x41, 0x00, 0x0B]);
        let err = parse_global_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::InvalidMutability { byte: 0x02 }
        ));
    }

    #[test]
    fn reject_unterminated_init_expr() {
        let section = raw_global_section(&[0x01, 0x7F, 0x00, 0x41, 0x00]);
        let err = parse_global_section(&section).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnexpectedEof);
    }
}
