//! Code section parsing.
//!
//! Decodes function body records from the code section.
//! Instruction sequences remain raw byte slices until instruction decoding is implemented.
//! See [Spec §5.5.13](https://webassembly.github.io/spec/core/binary/modules.html#code-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{CodeBody, LocalDecl, ValType};

pub fn parse_code_section<'a>(section: &RawSection<'a>) -> Result<Vec<CodeBody<'a>>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_code_section(&mut cursor, section.offset)?;

    let mut codes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        codes.push(parse_code_body(&mut cursor, section.offset)?);
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(codes)
}

fn parse_code_body<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<CodeBody<'a>, DecodeError> {
    let body_size = decode_u32_in_code_section(cursor, base_offset)? as usize;
    let body_offset = cursor.position();
    let body_bytes = cursor.read_bytes(body_size).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + body_offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    let mut body_cursor = Cursor::new(body_bytes);
    let local_count = decode_u32_in_code_section(&mut body_cursor, base_offset + body_offset)?;

    let mut locals = Vec::with_capacity(local_count as usize);
    for _ in 0..local_count {
        let count = decode_u32_in_code_section(&mut body_cursor, base_offset + body_offset)?;
        let val_type = parse_val_type(&mut body_cursor, base_offset + body_offset)?;
        locals.push(LocalDecl { count, val_type });
    }

    let instr_offset = body_cursor.position();
    let body = &body_bytes[instr_offset..];

    Ok(CodeBody {
        locals,
        body,
        body_offset: base_offset + body_offset + instr_offset,
    })
}

fn parse_val_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<ValType, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    ValType::from_encoding(byte).ok_or(DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnknownValType { byte },
    })
}

fn decode_u32_in_code_section(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::CodeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;
    use crate::types::NumType;

    fn raw_code_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Code,
            offset: 40,
            data,
        }
    }

    #[test]
    fn parse_empty_code_section() {
        let section = raw_code_section(&[0x00]);
        let codes = parse_code_section(&section).unwrap();
        assert!(codes.is_empty());
    }

    #[test]
    fn parse_single_body_without_locals() {
        let section = raw_code_section(&[
            0x01, // one body
            0x02, // body size
            0x00, // zero local decls
            0x0B, // end
        ]);
        let codes = parse_code_section(&section).unwrap();
        assert_eq!(codes.len(), 1);
        assert!(codes[0].locals.is_empty());
        assert_eq!(codes[0].body, &[0x0B]);
        assert_eq!(codes[0].body_offset, 43);
    }

    #[test]
    fn parse_body_with_locals() {
        let section = raw_code_section(&[
            0x01, // one body
            0x06, // body size
            0x02, // two local decl groups
            0x01, 0x7F, // 1 i32
            0x02, 0x7E, // 2 i64
            0x0B, // end
        ]);
        let codes = parse_code_section(&section).unwrap();
        assert_eq!(codes[0].locals.len(), 2);
        assert_eq!(
            codes[0].locals[0],
            LocalDecl {
                count: 1,
                val_type: ValType::Num(NumType::I32),
            }
        );
        assert_eq!(
            codes[0].locals[1],
            LocalDecl {
                count: 2,
                val_type: ValType::Num(NumType::I64),
            }
        );
        assert_eq!(codes[0].body, &[0x0B]);
    }

    #[test]
    fn reject_invalid_local_type() {
        let section = raw_code_section(&[
            0x01, // one body
            0x03, // body size
            0x01, // one local decl
            0x01, 0x01, // invalid valtype
        ]);
        let err = parse_code_section(&section).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnknownValType { byte: 0x01 });
    }

    #[test]
    fn reject_trailing_bytes() {
        let section = raw_code_section(&[0x00, 0xFF]);
        let err = parse_code_section(&section).unwrap_err();
        assert_eq!(
            err.kind,
            DecodeErrorKind::SectionSizeMismatch {
                expected: 2,
                consumed: 1,
            }
        );
    }
}
