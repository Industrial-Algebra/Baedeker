// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Code section parsing.
//!
//! Decodes function body records from the code section.
//! Instruction sequences remain raw byte slices until instruction decoding is implemented.
//! See [Spec §5.5.13](https://webassembly.github.io/spec/core/binary/modules.html#code-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::parse_val_type as parse_binary_val_type;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{CodeBody, LocalDecl, ValType};

/// Maximum total locals per function body. Matches the engine limits used
/// by wasmtime and V8; the spec's binary format allows up to `u32::MAX`,
/// which is not a survivable allocation request.
const MAX_TOTAL_LOCALS: u64 = 50_000;

pub fn parse_code_section<'a>(section: &RawSection<'a>) -> Result<Vec<CodeBody<'a>>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_code_section(&mut cursor, section.offset)?;

    let mut codes = Vec::with_capacity(cursor.capacity_hint(count));
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

    let mut locals = Vec::with_capacity(body_cursor.capacity_hint(local_count));
    let mut total_locals: u64 = 0;
    for _ in 0..local_count {
        let count = decode_u32_in_code_section(&mut body_cursor, base_offset + body_offset)?;
        let val_type = parse_val_type(&mut body_cursor, base_offset + body_offset)?;
        total_locals += u64::from(count);
        // The spec permits up to u32::MAX locals, but lowering allocates
        // per-local registers — hundreds of millions of locals turns a tiny
        // binary into gigabytes of allocation. Engines cap this (wasmtime
        // and V8 both use 50,000); so do we.
        if total_locals > MAX_TOTAL_LOCALS {
            return Err(DecodeError {
                offset: ByteOffset(base_offset + body_offset),
                context: DecodeContext::CodeSection,
                kind: DecodeErrorKind::TooManyLocals,
            });
        }
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
    parse_binary_val_type(cursor, base_offset, DecodeContext::CodeSection)
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

    /// Fuzz regression: a body declaring billions of local-decl groups must
    /// fail with EOF instead of pre-allocating gigabytes from the untrusted
    /// count.
    #[test]
    fn reject_huge_local_decl_count_without_oom() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
            0x03, 0x02, 0x01, 0x00, // func 0 : type 0
            0x0a, 0x07, 0x01, 0x05, 0xff, 0xff, 0xff, 0xff, 0x0b, // code
        ];
        // Must error at decode or lowering — never abort on a huge
        // allocation.
        if let Ok(module) = crate::binary::module::Module::decode(&bytes) {
            assert!(module.lower().is_err());
        }
    }

    /// Fuzz regression: 0x0FFF_FFFF locals in one group fits the spec's
    /// `u32::MAX` rule but must hit the engine's 50k cap, not a 3GB
    /// register allocation during lowering.
    #[test]
    fn reject_local_count_above_engine_cap() {
        let section = raw_code_section(&[
            0x01, // one body
            0x07, // body size
            0x01, // one local decl group
            0xff, 0xff, 0xff, 0xff, 0x00, // count = 0x0FFF_FFFF
            0x7f, // i32
            0x0B, // end
        ]);
        let error = parse_code_section(&section).unwrap_err();
        assert_eq!(error.kind, DecodeErrorKind::TooManyLocals);
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
