// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Export section parsing.
//!
//! Decodes exports and their descriptors from the export section.
//! See [Spec §5.5.10](https://webassembly.github.io/spec/core/binary/modules.html#export-section).

use alloc::{borrow::ToOwned, string::String, vec::Vec};

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{Export, ExportDesc, FuncIdx, GlobalIdx, MemIdx, TableIdx};

pub fn parse_export_section(section: &RawSection<'_>) -> Result<Vec<Export>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut exports = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let name = parse_name(&mut cursor, section.offset)?;
        let desc = parse_export_desc(&mut cursor, section.offset)?;
        exports.push(Export { name, desc });
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::ExportSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(exports)
}

fn parse_name(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<String, DecodeError> {
    let length = decode_u32_in_section(cursor, base_offset)? as usize;
    let offset = cursor.position();
    let bytes = cursor.read_bytes(length).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::ExportSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::ExportSection,
            kind: DecodeErrorKind::InvalidUtf8,
        })
}

fn parse_export_desc(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<ExportDesc, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::ExportSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    let idx = decode_u32_in_section(cursor, base_offset)?;
    match byte {
        0x00 => Ok(ExportDesc::Func(FuncIdx(idx))),
        0x01 => Ok(ExportDesc::Table(TableIdx(idx))),
        0x02 => Ok(ExportDesc::Mem(MemIdx(idx))),
        0x03 => Ok(ExportDesc::Global(GlobalIdx(idx))),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::ExportSection,
            kind: DecodeErrorKind::UnknownExportDesc { byte },
        }),
    }
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::ExportSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;

    fn raw_export_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Export,
            offset: 28,
            data,
        }
    }

    #[test]
    fn parse_empty_export_section() {
        let section = raw_export_section(&[0x00]);
        let exports = parse_export_section(&section).unwrap();
        assert!(exports.is_empty());
    }

    #[test]
    fn parse_function_and_memory_exports() {
        let section = raw_export_section(&[
            0x02, 0x03, b'a', b'd', b'd', 0x00, 0x01, 0x03, b'm', b'e', b'm', 0x02, 0x00,
        ]);

        let exports = parse_export_section(&section).unwrap();
        assert_eq!(exports.len(), 2);
        assert_eq!(exports[0].name, "add");
        assert_eq!(exports[0].desc, ExportDesc::Func(FuncIdx(1)));
        assert_eq!(exports[1].name, "mem");
        assert_eq!(exports[1].desc, ExportDesc::Mem(MemIdx(0)));
    }

    #[test]
    fn reject_unknown_export_descriptor() {
        let section = raw_export_section(&[0x01, 0x01, b'x', 0x04, 0x00]);
        let err = parse_export_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnknownExportDesc { byte: 0x04 }
        ));
    }
}
