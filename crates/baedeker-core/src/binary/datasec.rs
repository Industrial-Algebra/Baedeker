// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Data section parsing.
//!
//! Decodes defined data segments and their initialization payloads.
//! See [Spec §5.5.14](https://webassembly.github.io/spec/core/binary/modules.html#data-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{DataMode, DataSegment, MemIdx};

pub fn parse_data_section<'a>(
    section: &RawSection<'a>,
) -> Result<Vec<DataSegment<'a>>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut segments = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        segments.push(parse_data_segment(&mut cursor, section.offset)?);
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::DataSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(segments)
}

pub fn parse_data_count_section(section: &RawSection<'_>) -> Result<u32, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = leb128::decode_u32(&mut cursor).map_err(|mut e| {
        e.context = DecodeContext::DataCountSection;
        e.offset = ByteOffset(section.offset + e.offset.0);
        e
    })?;

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::DataCountSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(count)
}

fn parse_data_segment<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<DataSegment<'a>, DecodeError> {
    let flag = decode_u32_in_section(cursor, base_offset)?;
    match flag {
        0 => {
            let offset_offset = base_offset + cursor.position();
            let offset_expr = parse_init_expr(cursor, base_offset)?;
            let init = parse_byte_vec(cursor, base_offset)?;
            let init_offset = base_offset + cursor.position() - init.len();
            Ok(DataSegment {
                mode: DataMode::Active {
                    memory: MemIdx(0),
                    offset_expr,
                    offset_offset,
                },
                init,
                init_offset,
            })
        }
        1 => {
            let init = parse_byte_vec(cursor, base_offset)?;
            let init_offset = base_offset + cursor.position() - init.len();
            Ok(DataSegment {
                mode: DataMode::Passive,
                init,
                init_offset,
            })
        }
        2 => {
            let memory = MemIdx(decode_u32_in_section(cursor, base_offset)?);
            let offset_offset = base_offset + cursor.position();
            let offset_expr = parse_init_expr(cursor, base_offset)?;
            let init = parse_byte_vec(cursor, base_offset)?;
            let init_offset = base_offset + cursor.position() - init.len();
            Ok(DataSegment {
                mode: DataMode::Active {
                    memory,
                    offset_expr,
                    offset_offset,
                },
                init,
                init_offset,
            })
        }
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset),
            context: DecodeContext::DataSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: flag as u8,
            },
        }),
    }
}

fn parse_init_expr<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<&'a [u8], DecodeError> {
    let start = cursor.position();
    loop {
        let pos = cursor.position();
        let byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + pos),
            context: DecodeContext::DataSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if byte == 0x0B {
            return Ok(&cursor.original()[start..cursor.position()]);
        }
    }
}

fn parse_byte_vec<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<&'a [u8], DecodeError> {
    let len = decode_u32_in_section(cursor, base_offset)? as usize;
    let pos = cursor.position();
    cursor.read_bytes(len).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + pos),
        context: DecodeContext::DataSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::DataSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;

    fn raw_data_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Data,
            offset: 40,
            data,
        }
    }

    #[test]
    fn parse_passive_data_segment() {
        let section = raw_data_section(&[0x01, 0x01, 0x03, b'a', b'b', b'c']);
        let segments = parse_data_section(&section).unwrap();
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0].mode, DataMode::Passive));
        assert_eq!(segments[0].init, b"abc");
    }

    #[test]
    fn parse_active_data_segment() {
        let section = raw_data_section(&[0x01, 0x00, 0x41, 0x00, 0x0B, 0x02, 0xAA, 0xBB]);
        let segments = parse_data_section(&section).unwrap();
        assert_eq!(segments.len(), 1);
        match &segments[0].mode {
            DataMode::Active {
                memory,
                offset_expr,
                ..
            } => {
                assert_eq!(*memory, MemIdx(0));
                assert_eq!(*offset_expr, &[0x41, 0x00, 0x0B]);
            }
            DataMode::Passive => panic!("expected active"),
        }
        assert_eq!(segments[0].init, &[0xAA, 0xBB]);
    }
}
