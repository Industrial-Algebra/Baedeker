// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Tag section parsing.
//!
//! Decodes exception tag definitions. A tag entry is an attribute byte
//! (must be zero — only exception tags exist in wasm 3.0) followed by a
//! type index whose function type must have empty results (checked at
//! validation, not here).
//! See the exception-handling proposal and
//! [Spec §5.5.X](https://webassembly.github.io/exception-handling/core/binary/modules.html#tag-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::TypeIdx;

pub fn parse_tag_section(section: &RawSection<'_>) -> Result<Vec<TypeIdx>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut tags = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let pos = cursor.position();
        let attribute = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(section.offset + pos),
            context: DecodeContext::SectionBody { id: 13 },
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if attribute != 0x00 {
            return Err(DecodeError {
                offset: ByteOffset(section.offset + pos),
                context: DecodeContext::SectionBody { id: 13 },
                kind: DecodeErrorKind::InvalidTagAttribute { byte: attribute },
            });
        }
        tags.push(TypeIdx(decode_u32_in_section(&mut cursor, section.offset)?));
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::SectionBody { id: 13 },
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(tags)
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::SectionBody { id: 13 };
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}
