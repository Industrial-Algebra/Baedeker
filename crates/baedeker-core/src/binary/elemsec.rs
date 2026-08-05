// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Element section parsing.
//!
//! Decodes defined element segments from the element section.
//! See [Spec §5.5.12](https://webassembly.github.io/spec/core/binary/modules.html#element-section).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::section::RawSection;
use crate::binary::typeparser::parse_ref_type as parse_binary_ref_type;
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{
    ElementExpr, ElementInit, ElementMode, ElementSegment, FuncIdx, RefType, TableIdx,
};

pub fn parse_element_section<'a>(
    section: &RawSection<'a>,
) -> Result<Vec<ElementSegment<'a>>, DecodeError> {
    let mut cursor = Cursor::new(section.data);
    let count = decode_u32_in_section(&mut cursor, section.offset)?;

    let mut segments = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        segments.push(parse_element_segment(&mut cursor, section.offset)?);
    }

    if !cursor.is_empty() {
        return Err(DecodeError {
            offset: ByteOffset(section.offset + cursor.position()),
            context: DecodeContext::ElementSection,
            kind: DecodeErrorKind::SectionSizeMismatch {
                expected: section.data.len() as u32,
                consumed: cursor.position() as u32,
            },
        });
    }

    Ok(segments)
}

fn parse_element_segment<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<ElementSegment<'a>, DecodeError> {
    let flag = decode_u32_in_section(cursor, base_offset)?;
    match flag {
        0 => {
            let offset_offset = base_offset + cursor.position();
            let offset_expr = parse_init_expr(cursor, base_offset)?;
            let init = parse_funcidx_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Active {
                    table: TableIdx(0),
                    offset_expr,
                    offset_offset,
                },
                elem_type: RefType::FuncRef,
                init: ElementInit::FuncIndices(init),
            })
        }
        1 => {
            let elem_type = parse_elemkind(cursor, base_offset)?;
            let init = parse_funcidx_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Passive,
                elem_type,
                init: ElementInit::FuncIndices(init),
            })
        }
        2 => {
            let table = TableIdx(decode_u32_in_section(cursor, base_offset)?);
            let offset_offset = base_offset + cursor.position();
            let offset_expr = parse_init_expr(cursor, base_offset)?;
            let elem_type = parse_elemkind(cursor, base_offset)?;
            let init = parse_funcidx_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Active {
                    table,
                    offset_expr,
                    offset_offset,
                },
                elem_type,
                init: ElementInit::FuncIndices(init),
            })
        }
        3 => {
            let elem_type = parse_elemkind(cursor, base_offset)?;
            let init = parse_funcidx_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Declarative,
                elem_type,
                init: ElementInit::FuncIndices(init),
            })
        }
        4 => {
            let offset_offset = base_offset + cursor.position();
            let offset_expr = parse_init_expr(cursor, base_offset)?;
            let init = parse_expr_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Active {
                    table: TableIdx(0),
                    offset_expr,
                    offset_offset,
                },
                elem_type: RefType::FuncRef,
                init: ElementInit::Expressions(init),
            })
        }
        5 => {
            let elem_type = parse_ref_type(cursor, base_offset)?;
            let init = parse_expr_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Passive,
                elem_type,
                init: ElementInit::Expressions(init),
            })
        }
        6 => {
            let table = TableIdx(decode_u32_in_section(cursor, base_offset)?);
            let offset_offset = base_offset + cursor.position();
            let offset_expr = parse_init_expr(cursor, base_offset)?;
            let elem_type = parse_ref_type(cursor, base_offset)?;
            let init = parse_expr_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Active {
                    table,
                    offset_expr,
                    offset_offset,
                },
                elem_type,
                init: ElementInit::Expressions(init),
            })
        }
        7 => {
            let elem_type = parse_ref_type(cursor, base_offset)?;
            let init = parse_expr_vec(cursor, base_offset)?;
            Ok(ElementSegment {
                mode: ElementMode::Declarative,
                elem_type,
                init: ElementInit::Expressions(init),
            })
        }
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset),
            context: DecodeContext::ElementSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: flag as u8,
            },
        }),
    }
}

fn parse_funcidx_vec(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Vec<FuncIdx>, DecodeError> {
    let count = decode_u32_in_section(cursor, base_offset)?;
    let mut funcs = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        funcs.push(FuncIdx(decode_u32_in_section(cursor, base_offset)?));
    }
    Ok(funcs)
}

fn parse_expr_vec<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<Vec<ElementExpr<'a>>, DecodeError> {
    let count = decode_u32_in_section(cursor, base_offset)?;
    let mut exprs = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        let offset = base_offset + cursor.position();
        let expr = parse_init_expr(cursor, base_offset)?;
        exprs.push(ElementExpr { expr, offset });
    }
    Ok(exprs)
}

fn parse_init_expr<'a>(
    cursor: &mut Cursor<'a>,
    base_offset: usize,
) -> Result<&'a [u8], DecodeError> {
    let start = cursor.position();
    loop {
        let pos = cursor.position();
        let byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + pos),
            context: DecodeContext::ElementSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        if byte == 0x0B {
            return Ok(&cursor.original()[start..cursor.position()]);
        }
    }
}

fn parse_elemkind(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<RefType, DecodeError> {
    let offset = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::ElementSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match byte {
        0x00 => Ok(RefType::FuncRef),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + offset),
            context: DecodeContext::ElementSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: byte,
            },
        }),
    }
}

fn parse_ref_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<RefType, DecodeError> {
    parse_binary_ref_type(cursor, base_offset, DecodeContext::ElementSection)
}

fn decode_u32_in_section(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::ElementSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::binary::section::SectionId;

    fn raw_element_section(data: &[u8]) -> RawSection<'_> {
        RawSection {
            id: SectionId::Element,
            offset: 34,
            data,
        }
    }

    #[test]
    fn parse_active_funcidx_elements() {
        let section = raw_element_section(&[0x01, 0x00, 0x41, 0x00, 0x0B, 0x02, 0x00, 0x01]);
        let elems = parse_element_section(&section).unwrap();
        assert_eq!(elems.len(), 1);
        match &elems[0].mode {
            ElementMode::Active {
                table, offset_expr, ..
            } => {
                assert_eq!(*table, TableIdx(0));
                assert_eq!(*offset_expr, &[0x41, 0x00, 0x0B]);
            }
            _ => panic!("expected active element segment"),
        }
        assert_eq!(elems[0].elem_type, RefType::FuncRef);
        assert_eq!(
            elems[0].init,
            ElementInit::FuncIndices(vec![FuncIdx(0), FuncIdx(1)])
        );
    }

    #[test]
    fn parse_passive_expr_elements() {
        let section = raw_element_section(&[0x01, 0x05, 0x70, 0x01, 0xD2, 0x70, 0x0B]);
        let elems = parse_element_section(&section).unwrap();
        assert_eq!(elems.len(), 1);
        assert!(matches!(elems[0].mode, ElementMode::Passive));
        assert_eq!(elems[0].elem_type, RefType::FuncRef);
        match &elems[0].init {
            ElementInit::Expressions(exprs) => {
                assert_eq!(exprs.len(), 1);
                assert_eq!(exprs[0].expr, &[0xD2, 0x70, 0x0B]);
            }
            _ => panic!("expected expression initializers"),
        }
    }

    #[test]
    fn parse_typed_active_expr_elements() {
        let section = raw_element_section(&[
            0x01, 0x06, 0x00, 0x41, 0x00, 0x0B, 0x63, 0x01, 0x01, 0xD2, 0x00, 0x0B,
        ]);
        let elems = parse_element_section(&section).unwrap();
        assert_eq!(elems.len(), 1);
        match &elems[0].mode {
            ElementMode::Active {
                table, offset_expr, ..
            } => {
                assert_eq!(*table, TableIdx(0));
                assert_eq!(*offset_expr, &[0x41, 0x00, 0x0B]);
            }
            _ => panic!("expected active element segment"),
        }
        assert_eq!(
            elems[0].elem_type,
            RefType::concrete(true, crate::types::TypeIdx(1))
        );
        match &elems[0].init {
            ElementInit::Expressions(exprs) => {
                assert_eq!(exprs.len(), 1);
                assert_eq!(exprs[0].expr, &[0xD2, 0x00, 0x0B]);
            }
            _ => panic!("expected expression initializers"),
        }
    }

    #[test]
    fn reject_unknown_reference_type() {
        let section = raw_element_section(&[0x01, 0x05, 0x6E, 0x00]);
        let err = parse_element_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnknownRefType { byte: 0x6E }
        ));
    }

    #[test]
    fn reject_invalid_elemkind() {
        let section = raw_element_section(&[0x01, 0x01, 0x01, 0x00]);
        let err = parse_element_section(&section).unwrap_err();
        assert!(matches!(
            err.kind,
            DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: 0x01,
            }
        ));
    }
}
