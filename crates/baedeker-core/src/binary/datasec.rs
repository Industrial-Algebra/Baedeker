// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Data section parsing.
//!
//! Decodes defined data segments and their initialization payloads.
//! See [Spec §5.5.14](https://webassembly.github.io/spec/core/binary/modules.html#data-section).

use alloc::vec::Vec;

use crate::binary::instr::decode_const_expr_bytes;
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

/// Parse a data segment's offset/init expression, returning its raw bytes
/// (including the terminating `end`).
///
/// The boundary comes from the shared instruction walk, never from scanning
/// for the first `0x0B`: GC-era constant expressions (`array.new_fixed` with
/// many operands, `struct.new` with wide field values) contain `0x0B` bytes
/// inside immediates, which a raw scan truncates.
fn parse_init_expr<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<&'a [u8], DecodeError> {
    decode_const_expr_bytes(cursor, base_offset, DecodeContext::DataSection)
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

    /// The offset expression's boundary is found by decoding instructions, not
    /// by scanning for the first `0x0B`: `i32.const 11` encodes as
    /// `0x41 0x0B`, and the `0x0B` is an OPERAND byte, not the terminator.
    /// (The old scan truncated the expression there and then read the real
    /// terminator as the payload length — the `unexpected end of input`
    /// decode failure the U4 smith campaign recorded on seeds 36/89/275.)
    #[test]
    fn offset_expr_operand_byte_0x0b_is_not_a_terminator() {
        // count=1, flag=0, i32.const 11, end, len=2, "ab"
        let section = raw_data_section(&[0x01, 0x00, 0x41, 0x0B, 0x0B, 0x02, b'a', b'b']);
        let segments = parse_data_section(&section).unwrap();
        assert_eq!(segments.len(), 1);
        match &segments[0].mode {
            DataMode::Active { offset_expr, .. } => {
                assert_eq!(*offset_expr, &[0x41, 0x0B, 0x0B]);
            }
            DataMode::Passive => panic!("expected active"),
        }
        assert_eq!(segments[0].init, b"ab");
    }

    /// A GC-era constant expression with wide immediates is sized correctly:
    /// `array.new_fixed $t 100` with 100 `i32.const 11` operands. Every
    /// operand carries a `0x0B` payload byte, so a raw scan would stop inside
    /// the first operand.
    ///
    /// A data-segment offset must type-check to `i32`, so this is a DECODER
    /// boundary test (validation is a separate stage and rejects the offset);
    /// the same expression appears as a valid element-segment initializer in
    /// `elemsec`'s `expr_vec` path.
    #[test]
    fn array_new_fixed_length_100_offset_expr_is_sized_correctly() {
        let mut bytes = alloc::vec![0x01u8, 0x00]; // count=1, flag=0 (active, memory 0)
        for _ in 0..100 {
            bytes.extend_from_slice(&[0x41, 0x0B]); // i32.const 11
        }
        bytes.extend_from_slice(&[0xFB, 0x08, 0x00, 0x64]); // array.new_fixed $t=0, n=100
        bytes.push(0x0B); // end
        let expr_len = bytes.len() - 2;
        bytes.extend_from_slice(&[0x02, 0xAA, 0xBB]); // payload len=2

        let section = raw_data_section(&bytes);
        let segments = parse_data_section(&section).unwrap();
        assert_eq!(segments.len(), 1);
        match &segments[0].mode {
            DataMode::Active { offset_expr, .. } => {
                assert_eq!(offset_expr.len(), expr_len);
                assert_eq!(offset_expr[expr_len - 1], 0x0B);
                assert_eq!(
                    &offset_expr[expr_len - 5..expr_len - 1],
                    &[0xFB, 0x08, 0x00, 0x64]
                );
            }
            DataMode::Passive => panic!("expected active"),
        }
        assert_eq!(segments[0].init, &[0xAA, 0xBB]);
    }
}
