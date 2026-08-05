// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Function section parsing.
//!
//! Decodes function declarations from the function section.
//! See [Spec §5.5.6](https://webassembly.github.io/spec/core/binary/modules.html#function-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::TypeIdx;

pub fn parse_function_section(section: &RawSection<'_>) -> Result<Vec<TypeIdx>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = leb128::decode_u32(&mut cursor).map_err(|mut e| {
        e.context = DecodeContext::FunctionSection;
        e.offset = ByteOffset(section.offset + e.offset.0);
        e
    })?;

    let mut functions = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let type_idx = leb128::decode_u32(&mut cursor).map_err(|mut e| {
            e.context = DecodeContext::FunctionSection;
            e.offset = ByteOffset(section.offset + e.offset.0);
            e
        })?;
        functions.push(TypeIdx(type_idx));
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::FunctionSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(functions)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::binary::section::SectionId;

    fn raw_function_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Function,
            offset: 30,
            data,
        }
    }

    #[test]
    fn parse_empty_function_section() {
        let section = raw_function_section(&[0x00]);
        let functions = parse_function_section(&section).unwrap();
        assert!(functions.is_empty());
    }

    #[test]
    fn parse_function_type_indices() {
        let section = raw_function_section(&[0x03, 0x00, 0x02, 0x7F]);
        let functions = parse_function_section(&section).unwrap();
        assert_eq!(functions, vec![TypeIdx(0), TypeIdx(2), TypeIdx(127)]);
    }

    #[test]
    fn reject_trailing_bytes() {
        let section = raw_function_section(&[0x00, 0xFF]);
        let err = parse_function_section(&section).unwrap_err();
        assert_eq!(
            err.kind,
            DecodeErrorKind::SectionSizeMismatch {
                expected: 2,
                consumed: 1,
            }
        );
    }
}
