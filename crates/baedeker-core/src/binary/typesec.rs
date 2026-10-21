// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Type section parsing.
//!
//! Decodes WebAssembly type entries (function, struct, array, and subtype
//! definitions, including `rec` groups) from the type section.
//! See [Spec §5.5.4](https://webassembly.github.io/spec/core/binary/modules.html#type-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::parse_val_type;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{CompType, FieldType, FuncType, StorageType, SubType, ValType};

const FUNC_TYPE_TAG: u8 = 0x60;
const STRUCT_TYPE_TAG: u8 = 0x5F;
const ARRAY_TYPE_TAG: u8 = 0x5E;
const SUB_TYPE_TAG: u8 = 0x50;
const SUB_FINAL_TYPE_TAG: u8 = 0x4F;
const REC_GROUP_TAG: u8 = 0x4E;
const PACK_I8_TAG: u8 = 0x78;
const PACK_I16_TAG: u8 = 0x77;

/// A parsed type section: the flattened subtype definitions plus the extent
/// of each explicit `rec` group entry (start index, length). Rec-group
/// members are flattened into `types` in declaration order; the extents
/// carry the provenance needed for iso-recursive-style tag type equality at
/// link time (two tag types are equal only when their rec-group signatures
/// and positions match, or both are outside any rec group).
#[derive(Debug, Default, PartialEq)]
pub struct TypeSection {
    pub types: Vec<SubType>,
    pub rec_groups: Vec<(u32, u32)>,
}

/// Parse a type section into subtype entries.
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
        // A rectype is either a `rec` group (`0x4E` + `vec subtype`) or a
        // single bare subtype.
        let form = *cursor.remaining().first().ok_or(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if form == REC_GROUP_TAG {
            cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(section.offset + cursor.position()),
                context: DecodeContext::TypeSection,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            let inner_offset = cursor.position();
            let inner = leb128::decode_u32(&mut cursor).map_err(|mut e| {
                e.context = DecodeContext::TypeSection;
                e.offset = ByteOffset(section.offset + inner_offset + e.offset.0);
                e
            })?;
            let start = out.types.len() as u32;
            for _ in 0..inner {
                out.types.push(parse_sub_type(&mut cursor, section.offset)?);
            }
            out.rec_groups.push((start, inner));
        } else {
            out.types.push(parse_sub_type(&mut cursor, section.offset)?);
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

fn parse_sub_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<SubType, DecodeError> {
    let Some(&first) = cursor.remaining().first() else {
        return Err(DecodeError {
            offset: ByteOffset(base_offset + cursor.position()),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        });
    };

    if first == SUB_TYPE_TAG || first == SUB_FINAL_TYPE_TAG {
        cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + cursor.position()),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        let supertypes = parse_u32_vec(cursor, base_offset)?;
        let comp = parse_comp_type(cursor, base_offset)?;
        Ok(SubType {
            is_final: first == SUB_FINAL_TYPE_TAG,
            supertypes,
            comp,
        })
    } else {
        Ok(SubType {
            is_final: true,
            supertypes: Vec::new(),
            comp: parse_comp_type(cursor, base_offset)?,
        })
    }
}

fn parse_u32_vec(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Vec<u32>, DecodeError> {
    let count = leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::TypeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })?;

    let mut values = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        values.push(leb128::decode_u32(cursor).map_err(|mut e| {
            e.context = DecodeContext::TypeSection;
            e.offset = ByteOffset(base_offset + e.offset.0);
            e
        })?);
    }
    Ok(values)
}

fn parse_comp_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<CompType, DecodeError> {
    let tag_offset = cursor.position();
    let tag = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + tag_offset),
        context: DecodeContext::TypeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match tag {
        FUNC_TYPE_TAG => {
            let params = parse_valtype_vec(cursor, base_offset)?;
            let results = parse_valtype_vec(cursor, base_offset)?;
            Ok(CompType::Func(FuncType { params, results }))
        }
        STRUCT_TYPE_TAG => Ok(CompType::Struct(parse_fieldtype_vec(cursor, base_offset)?)),
        ARRAY_TYPE_TAG => Ok(CompType::Array(parse_fieldtype(cursor, base_offset)?)),
        other => Err(DecodeError {
            offset: ByteOffset(base_offset + tag_offset),
            context: DecodeContext::TypeSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: FUNC_TYPE_TAG,
                found: other,
            },
        }),
    }
}

fn parse_fieldtype_vec(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Vec<FieldType>, DecodeError> {
    let count = leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::TypeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })?;

    let mut fields = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        fields.push(parse_fieldtype(cursor, base_offset)?);
    }
    Ok(fields)
}

fn parse_fieldtype(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<FieldType, DecodeError> {
    let storage = parse_storagetype(cursor, base_offset)?;
    let mut_offset = cursor.position();
    let mut_byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + mut_offset),
        context: DecodeContext::TypeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    let mutable = match mut_byte {
        0 => false,
        1 => true,
        other => {
            return Err(DecodeError {
                offset: ByteOffset(base_offset + mut_offset),
                context: DecodeContext::TypeSection,
                kind: DecodeErrorKind::InvalidMutability { byte: other },
            });
        }
    };
    Ok(FieldType { mutable, storage })
}

fn parse_storagetype(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<StorageType, DecodeError> {
    // Packed storage types (`i8`/`i16`) never overlap the value-type
    // encodings, so a single peek disambiguates the reference grammar's
    // `either [valtype; packtype]`.
    match cursor.remaining().first() {
        Some(&PACK_I8_TAG) => {
            cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(base_offset + cursor.position()),
                context: DecodeContext::TypeSection,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            Ok(StorageType::I8)
        }
        Some(&PACK_I16_TAG) => {
            cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(base_offset + cursor.position()),
                context: DecodeContext::TypeSection,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            Ok(StorageType::I16)
        }
        _ => Ok(StorageType::Val(parse_val_type(
            cursor,
            base_offset,
            DecodeContext::TypeSection,
        )?)),
    }
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
    use alloc::vec;

    fn raw_type_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Type,
            offset: 10,
            data,
        }
    }

    fn func(types: &[SubType], idx: usize) -> &FuncType {
        types[idx].as_func().expect("expected a function type")
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
        assert!(func(&types, 0).params.is_empty());
        assert!(func(&types, 0).results.is_empty());
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
        let ty = func(&types, 0);

        assert_eq!(ty.params.len(), 3);
        assert_eq!(ty.params[0], ValType::Num(NumType::I32));
        assert_eq!(ty.params[1], ValType::Num(NumType::I64));
        assert_eq!(ty.params[2], ValType::Vec(VecType::V128));
        assert_eq!(ty.results[0], ValType::Num(NumType::F64));
        assert_eq!(ty.results[1], ValType::Ref(RefType::FuncRef));
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
        let ty = func(&types, 0);

        assert_eq!(
            ty.params[0],
            ValType::Ref(RefType::concrete(true, TypeIdx(0)))
        );
        assert_eq!(ty.results[0], ValType::Ref(RefType::func(false)));
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

    #[test]
    fn parse_rec_group_and_bare_types() {
        // (rec (type (func))) (type (struct)) (type (array i32))
        let section = raw_type_section(&[
            0x03, // three rectypes
            0x4E, 0x01, 0x60, 0x00, 0x00, // rec { (func) }
            0x5F, 0x00, // (struct)
            0x5E, 0x7F, 0x00, // (array i32)
        ]);
        let parsed = parse_type_section(&section).unwrap();
        assert_eq!(parsed.types.len(), 3);
        assert_eq!(parsed.rec_groups, vec![(0, 1)]);
        assert!(parsed.types[0].as_func().is_some());
        assert!(matches!(parsed.types[1].comp, CompType::Struct(_)));
        assert!(matches!(parsed.types[2].comp, CompType::Array(_)));
    }

    #[test]
    fn parse_sub_with_supertypes() {
        // (type (func)) (sub final 0 (func))
        let section = raw_type_section(&[
            0x02, 0x60, 0x00, 0x00, // (func)
            0x4F, 0x01, 0x00, 0x60, 0x00, 0x00, // sub final {0} (func)
        ]);
        let parsed = parse_type_section(&section).unwrap();
        assert_eq!(parsed.types.len(), 2);
        assert!(parsed.types[0].is_final);
        assert!(parsed.types[1].is_final);
        assert_eq!(parsed.types[1].supertypes, vec![0]);
    }

    #[test]
    fn parse_empty_rec_group() {
        let section = raw_type_section(&[0x01, 0x4E, 0x00]);
        let parsed = parse_type_section(&section).unwrap();
        assert!(parsed.types.is_empty());
        assert_eq!(parsed.rec_groups, vec![(0, 0)]);
    }
}
