// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

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
        0x63 | 0x64 | 0x69..=0x74 => Ok(ValType::Ref(parse_ref_type_with_first_byte(
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

/// Map a single-byte abstract heap-type encoding to its [`HeapType`].
///
/// Abstract heap types are encoded as signed 7-bit values, so their byte
/// encodings are `0x69..=0x74` (`noexn` .. `exn`).
fn abstract_heap_type(byte: u8) -> Option<HeapType> {
    Some(match byte {
        0x74 => HeapType::NoExn,
        0x73 => HeapType::NoFunc,
        0x72 => HeapType::NoExtern,
        0x71 => HeapType::None,
        0x70 => HeapType::Func,
        0x6F => HeapType::Extern,
        0x6E => HeapType::Any,
        0x6D => HeapType::Eq,
        0x6C => HeapType::I31,
        0x6B => HeapType::Struct,
        0x6A => HeapType::Array,
        0x69 => HeapType::Exn,
        _ => return None,
    })
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
        0x69..=0x74 => Ok(RefType::from_parts(
            true,
            abstract_heap_type(first).expect("range checked"),
        )),
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

    // Abstract heap types are single-byte negative `s7` values and occupy a
    // DISJOINT opcode space from the concrete-index `s33` reader. Match the
    // exact abstract bytes before the index path so that e.g. `0x7f`
    // (an `s33` of -1) is treated as a malformed index, never as an abstract
    // heap type.
    if first & 0x80 == 0 {
        if let Some(heap) = abstract_heap_type(first) {
            cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(absolute_offset),
                context,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            return Ok(heap);
        }
    }

    // Otherwise the byte starts a concrete type index, decoded per the pin's
    // `s33`: the 33-bit signed value is WRAPPED to `i32` (`Convert.I32_.wrap_i64`
    // at `interpreter/binary/decode.ml`), and a negative wrapped value is a
    // malformed type index (`typeuse` rejects `i < 0`). A nonnegative wrapped
    // value is the index. Byte-count (6th byte = too long) and the 5th byte's
    // unused-bit sign-extension check live in `decode_s33`.
    let start = cursor.position();
    let wrapped = leb128::decode_s33_wrapped(cursor).map_err(|mut e| {
        e.context = context;
        e.offset = ByteOffset(absolute_offset + (e.offset.0 - start));
        e
    })?;
    if wrapped < 0 {
        return Err(DecodeError {
            offset: ByteOffset(absolute_offset),
            context,
            kind: DecodeErrorKind::UnknownRefType { byte: first },
        });
    }
    Ok(HeapType::Type(TypeIdx(wrapped as u32)))
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

    /// Review r3b P1: concrete heap indices decode per the pin's `s33`, which
    /// wraps the 33-bit signed value to `i32`; a negative wrapped value is a
    /// malformed type index, a nonnegative one is the index.
    #[test]
    fn parse_concrete_heap_type_pinned_wrap_table() {
        // Wraps to -1 → malformed.
        let mut cursor = Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection)
                .unwrap_err()
                .kind,
            DecodeErrorKind::UnknownRefType { byte: 0xff }
        );
        // Wraps to i32::MIN → malformed.
        let mut cursor = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x08]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection)
                .unwrap_err()
                .kind,
            DecodeErrorKind::UnknownRefType { byte: 0x80 }
        );
        // `-2^32` wraps to 0 → index 0.
        let mut cursor = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x70]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap(),
            HeapType::Type(TypeIdx(0))
        );
        // Bits 5-6 do not sign-extend bit 4 at n=5 → integer too large.
        let mut cursor = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x10]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection)
                .unwrap_err()
                .kind,
            DecodeErrorKind::Leb128Overflow
        );
        // Wraps to -1 → malformed.
        let mut cursor = Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x7f]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection)
                .unwrap_err()
                .kind,
            DecodeErrorKind::UnknownRefType { byte: 0xff }
        );
        // Plain `00` → index 0.
        let mut cursor = Cursor::new(&[0x00]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap(),
            HeapType::Type(TypeIdx(0))
        );
        // A single byte `0x7f` is the `s33` value -1 → malformed type index.
        // It MUST NOT be confused with the abstract heap-type bytes, which
        // occupy the disjoint `0x69..=0x74` opcode space.
        let mut cursor = Cursor::new(&[0x7f]);
        assert_eq!(
            parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection)
                .unwrap_err()
                .kind,
            DecodeErrorKind::UnknownRefType { byte: 0x7f }
        );
    }

    /// Abstract heap-type single bytes remain abstract, not concrete indices.
    #[test]
    fn parse_abstract_single_bytes_stay_abstract() {
        for (byte, expected) in [
            (0x74u8, HeapType::NoExn),
            (0x70, HeapType::Func),
            (0x69, HeapType::Exn),
        ] {
            let bytes = [byte];
            let mut cursor = Cursor::new(&bytes);
            assert_eq!(
                parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap(),
                expected
            );
        }
    }

    /// Values above `u32::MAX` (bit 32 set without sign extension) stay
    /// malformed.
    #[test]
    fn parse_concrete_heap_type_over_u32_max_is_malformed() {
        let mut cursor = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x10]);
        let err = parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::Leb128Overflow);
    }

    /// Negative multi-byte (non-abstract) encodings remain malformed.
    #[test]
    fn parse_negative_multibyte_heap_type_is_malformed() {
        let mut cursor = Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x7f]);
        let err = parse_heap_type(&mut cursor, 0, DecodeContext::TypeSection).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnknownRefType { byte: 0xff });
    }
}
