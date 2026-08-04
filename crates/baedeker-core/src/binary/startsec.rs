// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Start section parsing.
//!
//! Decodes the optional start function index from the start section.
//! See [Spec §5.5.11](https://webassembly.github.io/spec/core/binary/modules.html#start-section).

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::FuncIdx;

pub fn parse_start_section(section: &RawSection<'_>) -> Result<FuncIdx, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let func_idx = leb128::decode_u32(&mut cursor).map_err(|mut e| {
        e.context = DecodeContext::StartSection;
        e.offset = ByteOffset(section.offset + e.offset.0);
        e
    })?;

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::StartSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(FuncIdx(func_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;

    fn raw_start_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Start,
            offset: 30,
            data,
        }
    }

    #[test]
    fn parse_start_section_with_single_function_index() {
        let section = raw_start_section(&[0x02]);
        let start = parse_start_section(&section).unwrap();
        assert_eq!(start, FuncIdx(2));
    }

    #[test]
    fn reject_trailing_bytes() {
        let section = raw_start_section(&[0x00, 0x00]);
        let err = parse_start_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::SectionSizeMismatch { .. }
        ));
    }
}
