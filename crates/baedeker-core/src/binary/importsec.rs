//! Import section parsing.
//!
//! Decodes imports and their descriptors from the import section.
//! See [Spec §5.5.5](https://webassembly.github.io/spec/core/binary/modules.html#import-section).

use alloc::{borrow::ToOwned, string::String, vec::Vec};

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::{
    parse_ref_type as parse_binary_ref_type, parse_val_type as parse_binary_val_type,
};
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{
    GlobalType, Import, ImportDesc, Limits, MemType, Mutability, RefType, TableType, TypeIdx,
    ValType,
};

pub fn parse_import_section(section: &RawSection<'_>) -> Result<Vec<Import>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset, DecodeContext::ImportSection)?;

    let mut imports = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let module = parse_name(&mut cursor, section.offset)?;
        let name = parse_name(&mut cursor, section.offset)?;
        let desc = parse_import_desc(&mut cursor, section.offset)?;
        imports.push(Import { module, name, desc });
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::ImportSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(imports)
}

fn parse_import_desc(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<ImportDesc, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::ImportSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match byte {
        0x00 => {
            let type_idx =
                decode_u32_in_section(cursor, base_offset, DecodeContext::ImportSection)?;
            Ok(ImportDesc::Func(TypeIdx(type_idx)))
        }
        0x01 => Ok(ImportDesc::Table(parse_table_type(cursor, base_offset)?)),
        0x02 => Ok(ImportDesc::Mem(MemType {
            limits: parse_limits(cursor, base_offset)?,
        })),
        0x03 => Ok(ImportDesc::Global(parse_global_type(cursor, base_offset)?)),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::ImportSection,
            kind: DecodeErrorKind::UnknownImportDesc { byte },
        }),
    }
}

fn parse_table_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TableType, DecodeError> {
    let elem = parse_ref_type(cursor, base_offset)?;
    let limits = parse_limits(cursor, base_offset)?;
    Ok(TableType {
        elem,
        limits,
        init: None,
    })
}

fn parse_global_type(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<GlobalType, DecodeError> {
    let val_type = parse_val_type(cursor, base_offset)?;
    let mutability = parse_mutability(cursor, base_offset)?;
    Ok(GlobalType {
        val_type,
        mutability,
    })
}

fn parse_limits(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Limits, DecodeError> {
    let tag_offset = cursor.position();
    let tag = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + tag_offset),
        context: DecodeContext::ImportSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match tag {
        0x00 => {
            let min = decode_u32_in_section(cursor, base_offset, DecodeContext::ImportSection)?;
            Ok(Limits { min, max: None })
        }
        0x01 => {
            let min = decode_u32_in_section(cursor, base_offset, DecodeContext::ImportSection)?;
            let max = decode_u32_in_section(cursor, base_offset, DecodeContext::ImportSection)?;
            Ok(Limits {
                min,
                max: Some(max),
            })
        }
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + tag_offset),
            context: DecodeContext::ImportSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: tag,
            },
        }),
    }
}

fn parse_name(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<String, DecodeError> {
    let length = decode_u32_in_section(cursor, base_offset, DecodeContext::ImportSection)? as usize;
    let offset = cursor.position();
    let bytes = cursor.read_bytes(length).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::ImportSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::ImportSection,
            kind: DecodeErrorKind::InvalidUtf8,
        })
}

fn parse_val_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<ValType, DecodeError> {
    parse_binary_val_type(cursor, base_offset, DecodeContext::ImportSection)
}

fn parse_ref_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<RefType, DecodeError> {
    parse_binary_ref_type(cursor, base_offset, DecodeContext::ImportSection)
}

fn parse_mutability(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Mutability, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::ImportSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match byte {
        0x00 => Ok(Mutability::Const),
        0x01 => Ok(Mutability::Var),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::ImportSection,
            kind: DecodeErrorKind::InvalidMutability { byte },
        }),
    }
}

fn decode_u32_in_section(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
    context: DecodeContext,
) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = context;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::section::SectionId;
    use crate::types::NumType;

    fn raw_import_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Import,
            offset: 20,
            data,
        }
    }

    #[test]
    fn parse_empty_import_section() {
        let section = raw_import_section(&[0x00]);
        let imports = parse_import_section(&section).unwrap();
        assert!(imports.is_empty());
    }

    #[test]
    fn parse_function_and_memory_imports() {
        let section = raw_import_section(&[
            0x02, 0x03, b'e', b'n', b'v', 0x05, b'p', b'r', b'i', b'n', b't', 0x00, 0x01, 0x03,
            b'e', b'n', b'v', 0x03, b'm', b'e', b'm', 0x02, 0x01, 0x01, 0x02,
        ]);

        let imports = parse_import_section(&section).unwrap();
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].module, "env");
        assert_eq!(imports[0].name, "print");
        assert_eq!(imports[0].desc, ImportDesc::Func(TypeIdx(1)));
        assert_eq!(imports[1].name, "mem");
        assert_eq!(
            imports[1].desc,
            ImportDesc::Mem(MemType {
                limits: Limits {
                    min: 1,
                    max: Some(2),
                },
            })
        );
    }

    #[test]
    fn parse_global_import() {
        let section =
            raw_import_section(&[0x01, 0x03, b'e', b'n', b'v', 0x01, b'g', 0x03, 0x7F, 0x01]);
        let imports = parse_import_section(&section).unwrap();
        assert_eq!(
            imports[0].desc,
            ImportDesc::Global(GlobalType {
                val_type: ValType::Num(NumType::I32),
                mutability: Mutability::Var,
            })
        );
    }

    #[test]
    fn reject_unknown_import_descriptor() {
        let section = raw_import_section(&[0x01, 0x01, b'm', 0x01, b'n', 0x09]);
        let err = parse_import_section(&section).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnknownImportDesc { byte: 0x09 });
    }

    #[test]
    fn reject_invalid_utf8_name() {
        let section = raw_import_section(&[0x01, 0x01, 0xFF, 0x01, b'n', 0x00, 0x00]);
        let err = parse_import_section(&section).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::InvalidUtf8);
    }

    #[test]
    fn reject_invalid_mutability() {
        let section = raw_import_section(&[0x01, 0x01, b'm', 0x01, b'g', 0x03, 0x7F, 0x02]);
        let err = parse_import_section(&section).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::InvalidMutability { byte: 0x02 });
    }
}
