// Copyright (C) 2026 Industrial Algebra\n// SPDX-License-Identifier: Apache-2.0\n
//! Shared parsers for value/reference/heap types.
//!
//! This keeps the section decoders aligned on the same typed-reference decoding
//! rules while preserving precise decode contexts and offsets.

use crate::binary::leb128;
use crate::binary::leb128::Cursor;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{HeapType, NumType, RefType, TypeIdx, ValType, VecType};

pub fn parse_val_type(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
    context: DecodeContext,
) -> Result<ValType, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    parse_val_type_with_first_byte(cursor, byte, base_offset + offset, context)
}

pub fn parse_val_type_with_first_byte(
    cursor: &mut Cursor<'_>,
    first: u8,
    absolute_offset: usize,
    context: DecodeContext,
) -> Result<ValType, DecodeError> {
    match first {
        0x7F => Ok(ValType::Num(NumType::I32)),
        0x7E => Ok(ValType::Num(NumType::I64)),
        0x7D => Ok(ValType::Num(NumType::F32)),
        0x7C => Ok(ValType::Num(NumType::F64)),
        0x7B => Ok(ValType::Vec(VecType::V128)),
        0x70 | 0x6F | 0x63 | 0x64 => Ok(ValType::Ref(parse_ref_type_with_first_byte(
            cursor,
            first,
            absolute_offset,
            context,
        )?)),
        byte => Err(DecodeError {
            offset: ByteOffset(absolute_offset),
            context,
            kind: DecodeErrorKind::UnknownValType { byte },
        }),
    }
}

pub fn parse_ref_type(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
    context: DecodeContext,
) -> Result<RefType, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    parse_ref_type_with_first_byte(cursor, byte, base_offset + offset, context)
}

pub fn parse_ref_type_with_first_byte(
    cursor: &mut Cursor<'_>,
    first: u8,
    absolute_offset: usize,
    context: DecodeContext,
) -> Result<RefType, DecodeError> {
    match first {
        0x70 => Ok(RefType::FuncRef),
        0x6F => Ok(RefType::ExternRef),
        0x63 => Ok(RefType::from_parts(
            true,
            parse_heap_type_at(cursor, context, absolute_offset + 1)?,
        )),
        0x64 => Ok(RefType::from_parts(
            false,
            parse_heap_type_at(cursor, context, absolute_offset + 1)?,
        )),
        byte => Err(DecodeError {
            offset: ByteOffset(absolute_offset),
            context,
            kind: DecodeErrorKind::UnknownRefType { byte },
        }),
    }
}

pub fn parse_heap_type(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
    context: DecodeContext,
) -> Result<HeapType, DecodeError> {
    parse_heap_type_at(cursor, context, base_offset + cursor.position())
}

fn parse_heap_type_at(
    cursor: &mut Cursor<'_>,
    context: DecodeContext,
    absolute_offset: usize,
) -> Result<HeapType, DecodeError> {
    let Some(&first) = cursor.remaining().first() else {
        return Err(DecodeError {
            offset: ByteOffset(absolute_offset),
            context,
            kind: DecodeErrorKind::UnexpectedEof,
        });
    };

    match first {
        0x70 => {
            cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(absolute_offset),
                context,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            Ok(HeapType::Func)
        }
        0x6F => {
            cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(absolute_offset),
                context,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            Ok(HeapType::Extern)
        }
        0x6E | 0x71 | 0x72 | 0x73 | 0x6D | 0x6B | 0x6A | 0x6C | 0x69 | 0x74 | 0x68 | 0x75 => {
            Err(DecodeError {
                offset: ByteOffset(absolute_offset),
                context,
                kind: DecodeErrorKind::UnknownRefType { byte: first },
            })
        }
        _ => {
            let start = cursor.position();
            leb128::decode_u32(cursor)
                .map(TypeIdx)
                .map(HeapType::Type)
                .map_err(|mut e| {
                    e.context = context;
                    e.offset = ByteOffset(absolute_offset + (e.offset.0 - start));
                    e
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_ref_types() {
        let mut cursor = Cursor::new(&[0x70, 0x6F]);
        assert_eq!(
            parse_ref_type(&mut cursor, 10, DecodeContext::TypeSection).unwrap(),
            RefType::FuncRef
        );
        assert_eq!(
            parse_ref_type(&mut cursor, 10, DecodeContext::TypeSection).unwrap(),
            RefType::ExternRef
        );
    }

    #[test]
    fn parse_typed_concrete_reference_type() {
        let mut cursor = Cursor::new(&[0x63, 0x00]);
        assert_eq!(
            parse_ref_type(&mut cursor, 12, DecodeContext::TypeSection).unwrap(),
            RefType::concrete(true, TypeIdx(0))
        );
    }

    #[test]
    fn parse_non_null_abstract_function_reference_type() {
        let mut cursor = Cursor::new(&[0x64, 0x70]);
        assert_eq!(
            parse_ref_type(&mut cursor, 14, DecodeContext::TypeSection).unwrap(),
            RefType::func(false)
        );
    }
}
