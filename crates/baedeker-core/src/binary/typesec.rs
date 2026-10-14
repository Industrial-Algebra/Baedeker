// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Type section parsing.
//!
//! Decodes WebAssembly function type entries from the type section.
//! See [Spec §5.5.4](https://webassembly.github.io/spec/core/binary/modules.html#type-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::parse_val_type;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{FuncType, ValType};

const FUNC_TYPE_TAG: u8 = 0x60;
const REC_GROUP_TAG: u8 = 0x4E;

/// A parsed type section: the flattened function types plus the extent of
/// each `rec` group entry (start index, length). Rec-group members are
/// flattened into `types` in declaration order; the extents carry the
/// provenance needed for iso-recursive-style tag type equality at link
/// time (two tag types are equal only when their rec-group signatures and
/// positions match, or both are outside any rec group).
#[derive(Debug, Default, PartialEq)]
pub struct TypeSection {
    pub types: Vec<FuncType>,
    pub rec_groups: Vec<(u32, u32)>,
}

/// Parse a type section into function type entries.
pub fn parse_type_section(section: &RawSection<'_>) -> Result<TypeSection, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = leb128::decode_u32(&mut cursor).map_err(|mut e| {
        e.context = DecodeContext::TypeSection;
        e.offset = ByteOffset(section.offset + e.offset.0);
        e
    })?;

    let mut out = TypeSection {
        types: Vec::with_capacity(cursor.capacity_hint(count)),
        rec_groups: Vec::new(),
    };
    for _ in 0..count {
        // Peek the entry form: `0x60` is a bare function type; `0x4E` opens
        // a `rec` group whose members follow under an inner count and are
        // flattened into the type list. Other forms (GC subtypes) remain
        // unsupported.
        let form_offset = cursor.position();
        let form = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(section.offset + form_offset),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if form == REC_GROUP_TAG {
            let inner_offset = cursor.position();
            let inner = leb128::decode_u32(&mut cursor).map_err(|mut e| {
                e.context = DecodeContext::TypeSection;
                e.offset = ByteOffset(section.offset + inner_offset + e.offset.0);
                e
            })?;
            let start = out.types.len() as u32;
            for _ in 0..inner {
                out.types
                    .push(parse_func_type(&mut cursor, section.offset)?);
            }
            out.rec_groups.push((start, inner));
        } else if form == FUNC_TYPE_TAG {
            let params = parse_valtype_vec(&mut cursor, section.offset)?;
            let results = parse_valtype_vec(&mut cursor, section.offset)?;
            out.types.push(FuncType { params, results });
        } else {
            return Err(DecodeError {
                offset: ByteOffset(section.offset + form_offset),
                context: DecodeContext::TypeSection,
                kind: DecodeErrorKind::UnexpectedByte {
                    expected: FUNC_TYPE_TAG,
                    found: form,
                },
            });
        }
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(out)
}

fn parse_func_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<FuncType, DecodeError> {
    let tag_offset = cursor.position();
    let tag = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + tag_offset),
        context: DecodeContext::TypeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    if tag != FUNC_TYPE_TAG {
        return Err(DecodeError {
            offset: ByteOffset(base_offset + tag_offset),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: FUNC_TYPE_TAG,
                found: tag,
            },
        });
    }

    let params = parse_valtype_vec(cursor, base_offset)?;
    let results = parse_valtype_vec(cursor, base_offset)?;

    Ok(FuncType { params, results })
}

fn parse_valtype_vec(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Vec<ValType>, DecodeError> {
    let count = leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::TypeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })?;

    let mut types = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        types.push(parse_val_type(
            cursor,
            base_offset,
            DecodeContext::TypeSection,
        )?);
    }

    Ok(types)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;
    use crate::types::{NumType, RefType, TypeIdx, VecType};

    fn raw_type_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Type,
            offset: 10,
            data,
        }
    }

    #[test]
    fn parse_empty_type_section() {
        let section = raw_type_section(&[0x00]);
        let types = parse_type_section(&section).unwrap().types;
        assert!(types.is_empty());
    }

    #[test]
    fn parse_single_empty_func_type() {
        let section = raw_type_section(&[0x01, 0x60, 0x00, 0x00]);
        let types = parse_type_section(&section).unwrap().types;

        assert_eq!(types.len(), 1);
        assert!(types[0].params.is_empty());
        assert!(types[0].results.is_empty());
    }

    #[test]
    fn parse_multi_value_func_type() {
        let section = raw_type_section(&[
            0x01, // one type
            0x60, // functype
            0x03, // three params
            0x7F, 0x7E, 0x7B, // i32, i64, v128
            0x02, // two results
            0x7C, 0x70, // f64, funcref
        ]);
        let types = parse_type_section(&section).unwrap().types;

        assert_eq!(types[0].params.len(), 3);
        assert_eq!(types[0].params[0], ValType::Num(NumType::I32));
        assert_eq!(types[0].params[1], ValType::Num(NumType::I64));
        assert_eq!(types[0].params[2], ValType::Vec(VecType::V128));
        assert_eq!(types[0].results[0], ValType::Num(NumType::F64));
        assert_eq!(types[0].results[1], ValType::Ref(RefType::FuncRef));
    }

    #[test]
    fn parse_typed_reference_func_type() {
        let section = raw_type_section(&[
            0x01, // one type
            0x60, // functype
            0x01, // one param
            0x63, 0x00, // (ref null type 0)
            0x01, // one result
            0x64, 0x70, // (ref func)
        ]);
        let types = parse_type_section(&section).unwrap().types;

        assert_eq!(
            types[0].params[0],
            ValType::Ref(RefType::concrete(true, TypeIdx(0)))
        );
        assert_eq!(types[0].results[0], ValType::Ref(RefType::func(false)));
    }

    #[test]
    fn reject_non_function_type_tag() {
        let section = raw_type_section(&[0x01, 0x61, 0x00, 0x00]);
        let err = parse_type_section(&section).unwrap_err();

        assert_eq!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: FUNC_TYPE_TAG,
                found: 0x61,
            }
        );
    }

    #[test]
    fn reject_unknown_value_type() {
        let section = raw_type_section(&[0x01, 0x60, 0x01, 0x01, 0x00]);
        let err = parse_type_section(&section).unwrap_err();

        assert_eq!(err.kind, DecodeErrorKind::UnknownValType { byte: 0x01 });
    }

    #[test]
    fn reject_trailing_bytes() {
        let section = raw_type_section(&[0x00, 0xFF]);
        let err = parse_type_section(&section).unwrap_err();

        assert_eq!(
            err.kind,
            DecodeErrorKind::SectionSizeMismatch {
                expected: 2,
                consumed: 1,
            }
        );
    }
}
