//! Instruction decoding.
//!
//! Decodes a subset of WebAssembly instructions sufficient to start Phase 1
//! validation work. Function bodies continue to store raw bytes; decoded
//! instructions are produced on demand from [`CodeBody`].
//!
//! See [Spec §5.4](https://webassembly.github.io/spec/core/binary/instructions.html).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{BlockType, CodeBody, FuncIdx, LabelIdx, LocalIdx, ValType};

/// A decoded WebAssembly instruction.
#[derive(Debug, Clone, PartialEq)]
pub enum Instr {
    Unreachable,
    Nop,
    Block(BlockType),
    Loop(BlockType),
    If(BlockType),
    Else,
    End,
    Br(LabelIdx),
    BrIf(LabelIdx),
    Return,
    Call(FuncIdx),
    Drop,
    Select,
    LocalGet(LocalIdx),
    LocalSet(LocalIdx),
    LocalTee(LocalIdx),
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I32Add,
}

impl<'a> CodeBody<'a> {
    /// Decode this function body's raw instruction bytes.
    pub fn instructions(&self) -> Result<Vec<Instr>, DecodeError> {
        decode_instr_sequence(self.body, self.body_offset)
    }
}

/// Decode a function-body instruction sequence until its terminating `end`.
pub fn decode_instr_sequence(bytes: &[u8], base_offset: usize) -> Result<Vec<Instr>, DecodeError> {
    let mut cursor = Cursor::new(bytes);
    let mut instrs = Vec::new();
    let mut block_depth = 0usize;

    loop {
        let instr = decode_instr(&mut cursor, base_offset)?;
        match instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => block_depth += 1,
            Instr::End => {
                if block_depth == 0 {
                    instrs.push(Instr::End);
                    return Ok(instrs);
                }
                block_depth -= 1;
            }
            _ => {}
        }

        instrs.push(instr);

        if cursor.is_empty() {
            return Err(DecodeError {
                offset: ByteOffset(base_offset + cursor.position()),
                context: DecodeContext::CodeSection,
                kind: DecodeErrorKind::UnexpectedEof,
            });
        }
    }
}

/// Decode a single instruction.
pub fn decode_instr(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Instr, DecodeError> {
    let opcode_offset = cursor.position();
    let opcode = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + opcode_offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    match opcode {
        0x00 => Ok(Instr::Unreachable),
        0x01 => Ok(Instr::Nop),
        0x02 => Ok(Instr::Block(parse_block_type(cursor, base_offset)?)),
        0x03 => Ok(Instr::Loop(parse_block_type(cursor, base_offset)?)),
        0x04 => Ok(Instr::If(parse_block_type(cursor, base_offset)?)),
        0x05 => Ok(Instr::Else),
        0x0B => Ok(Instr::End),
        0x0C => Ok(Instr::Br(LabelIdx(decode_u32(cursor, base_offset)?))),
        0x0D => Ok(Instr::BrIf(LabelIdx(decode_u32(cursor, base_offset)?))),
        0x0F => Ok(Instr::Return),
        0x10 => Ok(Instr::Call(FuncIdx(decode_u32(cursor, base_offset)?))),
        0x1A => Ok(Instr::Drop),
        0x1B => Ok(Instr::Select),
        0x20 => Ok(Instr::LocalGet(LocalIdx(decode_u32(cursor, base_offset)?))),
        0x21 => Ok(Instr::LocalSet(LocalIdx(decode_u32(cursor, base_offset)?))),
        0x22 => Ok(Instr::LocalTee(LocalIdx(decode_u32(cursor, base_offset)?))),
        0x41 => Ok(Instr::I32Const(decode_i32(cursor, base_offset)?)),
        0x42 => Ok(Instr::I64Const(decode_i64(cursor, base_offset)?)),
        0x43 => Ok(Instr::F32Const(decode_f32(cursor, base_offset)?)),
        0x44 => Ok(Instr::F64Const(decode_f64(cursor, base_offset)?)),
        0x45 => Ok(Instr::I32Eqz),
        0x46 => Ok(Instr::I32Eq),
        0x47 => Ok(Instr::I32Ne),
        0x48 => Ok(Instr::I32LtS),
        0x49 => Ok(Instr::I32LtU),
        0x4A => Ok(Instr::I32GtS),
        0x4B => Ok(Instr::I32GtU),
        0x4C => Ok(Instr::I32LeS),
        0x4D => Ok(Instr::I32LeU),
        0x4E => Ok(Instr::I32GeS),
        0x4F => Ok(Instr::I32GeU),
        0x6A => Ok(Instr::I32Add),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset + opcode_offset),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnknownOpcode { byte: opcode },
        }),
    }
}

fn parse_block_type(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<BlockType, DecodeError> {
    let start = cursor.position();
    let first = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + start),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    if first == 0x40 {
        return Ok(BlockType::Empty);
    }

    if let Some(val_type) = ValType::from_encoding(first) {
        return Ok(BlockType::Val(val_type));
    }

    let type_idx = decode_block_type_idx(cursor, first, base_offset + start)?;
    Ok(BlockType::TypeIdx(type_idx))
}

fn decode_block_type_idx(
    cursor: &mut Cursor<'_>,
    first: u8,
    absolute_offset: usize,
) -> Result<u32, DecodeError> {
    let mut result = i64::from(first & 0x7F);
    let mut shift = 7u32;
    let mut byte = first;
    let mut i = 0;

    while byte & 0x80 != 0 {
        i += 1;
        if i >= 5 {
            return Err(DecodeError {
                offset: ByteOffset(absolute_offset),
                context: DecodeContext::CodeSection,
                kind: DecodeErrorKind::Leb128TooLong,
            });
        }

        byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(absolute_offset),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        result |= i64::from(byte & 0x7F) << shift;
        shift += 7;
    }

    if shift < 33 && (byte & 0x40) != 0 {
        result |= (!0i64) << shift;
    }

    if result < 0 {
        return Err(DecodeError {
            offset: ByteOffset(absolute_offset),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::Leb128Overflow,
        });
    }

    Ok(result as u32)
}

fn decode_u32(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u32, DecodeError> {
    leb128::decode_u32(cursor).map_err(|mut e| {
        e.context = DecodeContext::CodeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

fn decode_i32(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<i32, DecodeError> {
    leb128::decode_i32(cursor).map_err(|mut e| {
        e.context = DecodeContext::CodeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

fn decode_i64(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<i64, DecodeError> {
    leb128::decode_i64(cursor).map_err(|mut e| {
        e.context = DecodeContext::CodeSection;
        e.offset = ByteOffset(base_offset + e.offset.0);
        e
    })
}

fn decode_f32(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<f32, DecodeError> {
    let offset = cursor.position();
    let bytes = cursor.read_bytes(4).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    Ok(f32::from_le_bytes(bytes.try_into().expect("4 bytes")))
}

fn decode_f64(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<f64, DecodeError> {
    let offset = cursor.position();
    let bytes = cursor.read_bytes(8).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    Ok(f64::from_le_bytes(bytes.try_into().expect("8 bytes")))
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::types::{CodeBody, NumType};

    #[test]
    fn decode_simple_add_body() {
        let body = CodeBody {
            locals: Vec::new(),
            body: &[0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B],
            body_offset: 100,
        };

        let instrs = body.instructions().unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::LocalGet(LocalIdx(0)),
                Instr::LocalGet(LocalIdx(1)),
                Instr::I32Add,
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_block_with_branch() {
        let instrs = decode_instr_sequence(&[0x02, 0x40, 0x0C, 0x00, 0x0B, 0x0B], 200).unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::Block(BlockType::Empty),
                Instr::Br(LabelIdx(0)),
                Instr::End,
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_value_block_type() {
        let instrs = decode_instr_sequence(&[0x02, 0x7F, 0x41, 0x01, 0x0B, 0x0B], 300).unwrap();
        assert_eq!(
            instrs[0],
            Instr::Block(BlockType::Val(ValType::Num(NumType::I32)))
        );
    }

    #[test]
    fn reject_unknown_opcode() {
        let err = decode_instr_sequence(&[0xFF, 0x0B], 400).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnknownOpcode { byte: 0xFF });
    }

    #[test]
    fn reject_unterminated_sequence() {
        let err = decode_instr_sequence(&[0x20, 0x00], 500).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnexpectedEof);
    }
}
