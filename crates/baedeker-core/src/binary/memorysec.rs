//! Memory section parsing.
//!
//! Decodes defined memories from the memory section.
//! See [Spec §5.5.7](https://webassembly.github.io/spec/core/binary/modules.html#memory-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{Limits, MemType};

pub fn parse_memory_section(section: &RawSection<'_>) -> Result<Vec<MemType>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut memories = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        memories.push(MemType {
            limits: parse_limits(&mut cursor, section.offset)?,
        });
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::MemorySection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(memories)
}

fn parse_limits(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Limits, DecodeError> {
    let tag_offset = cursor.position();
    let tag = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + tag_offset),
        context: DecodeContext::MemorySection,
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
            context: DecodeContext::MemorySection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: tag,
            },
        }),
    }
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::MemorySection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;

    fn raw_memory_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Memory,
            offset: 24,
            data,
        }
    }

    #[test]
    fn parse_empty_memory_section() {
        let section = raw_memory_section(&[0x00]);
        let memories = parse_memory_section(&section).unwrap();
        assert!(memories.is_empty());
    }

    #[test]
    fn parse_single_min_only_memory() {
        let section = raw_memory_section(&[0x01, 0x00, 0x02]);
        let memories = parse_memory_section(&section).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].limits.min, 2);
        assert_eq!(memories[0].limits.max, None);
    }

    #[test]
    fn parse_single_bounded_memory() {
        let section = raw_memory_section(&[0x01, 0x01, 0x01, 0x03]);
        let memories = parse_memory_section(&section).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].limits.min, 1);
        assert_eq!(memories[0].limits.max, Some(3));
    }

    #[test]
    fn reject_invalid_limits_tag() {
        let section = raw_memory_section(&[0x01, 0x02, 0x01]);
        let err = parse_memory_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: 0x02
            }
        ));
    }
}
