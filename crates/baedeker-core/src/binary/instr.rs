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
use crate::types::{BlockType, CodeBody, FuncIdx, GlobalIdx, LabelIdx, LocalIdx, MemIdx, ValType};

/// A decoded instruction paired with its absolute byte offset in the module.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedInstr {
    pub offset: ByteOffset,
    pub instr: Instr,
}

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
    BrTable {
        targets: Vec<LabelIdx>,
        default: LabelIdx,
    },
    Return,
    Call(FuncIdx),
    Drop,
    Select,
    SelectTyped(Vec<ValType>),
    LocalGet(LocalIdx),
    LocalSet(LocalIdx),
    LocalTee(LocalIdx),
    GlobalGet(GlobalIdx),
    GlobalSet(GlobalIdx),
    MemorySize(MemIdx),
    MemoryGrow(MemIdx),
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
    I64Add,
}

impl<'a> CodeBody<'a> {
    /// Decode this function body's raw instruction bytes.
    pub fn instructions(&self) -> Result<Vec<Instr>, DecodeError> {
        self.instructions_with_offsets()
            .map(|instrs| instrs.into_iter().map(|decoded| decoded.instr).collect())
    }

    /// Decode this function body's raw instruction bytes with absolute module offsets.
    pub fn instructions_with_offsets(&self) -> Result<Vec<DecodedInstr>, DecodeError> {
        decode_instr_sequence_with_offsets(self.body, self.body_offset)
    }
}

/// Decode a function-body instruction sequence until its terminating `end`.
pub fn decode_instr_sequence(bytes: &[u8], base_offset: usize) -> Result<Vec<Instr>, DecodeError> {
    decode_instr_sequence_with_offsets(bytes, base_offset)
        .map(|instrs| instrs.into_iter().map(|decoded| decoded.instr).collect())
}

/// Decode a function-body instruction sequence until its terminating `end`, preserving offsets.
pub fn decode_instr_sequence_with_offsets(
    bytes: &[u8],
    base_offset: usize,
) -> Result<Vec<DecodedInstr>, DecodeError> {
    let mut cursor = Cursor::new(bytes);
    let mut instrs = Vec::new();
    let mut block_depth = 0usize;

    loop {
        let decoded = decode_instr_with_offset(&mut cursor, base_offset)?;
        match decoded.instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => block_depth += 1,
            Instr::End => {
                if block_depth == 0 {
                    instrs.push(decoded);
                    return Ok(instrs);
                }
                block_depth -= 1;
            }
            _ => {}
        }

        instrs.push(decoded);

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
    decode_instr_with_offset(cursor, base_offset).map(|decoded| decoded.instr)
}

/// Decode a single instruction with its absolute module offset.
pub fn decode_instr_with_offset(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<DecodedInstr, DecodeError> {
    let opcode_offset = cursor.position();
    let opcode = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + opcode_offset),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;

    let instr = match opcode {
        0x00 => Instr::Unreachable,
        0x01 => Instr::Nop,
        0x02 => Instr::Block(parse_block_type(cursor, base_offset)?),
        0x03 => Instr::Loop(parse_block_type(cursor, base_offset)?),
        0x04 => Instr::If(parse_block_type(cursor, base_offset)?),
        0x05 => Instr::Else,
        0x0B => Instr::End,
        0x0C => Instr::Br(LabelIdx(decode_u32(cursor, base_offset)?)),
        0x0D => Instr::BrIf(LabelIdx(decode_u32(cursor, base_offset)?)),
        0x0E => {
            let target_count = decode_u32(cursor, base_offset)?;
            let mut targets = Vec::with_capacity(target_count as usize);
            for _ in 0..target_count {
                targets.push(LabelIdx(decode_u32(cursor, base_offset)?));
            }
            let default = LabelIdx(decode_u32(cursor, base_offset)?);
            Instr::BrTable { targets, default }
        }
        0x0F => Instr::Return,
        0x10 => Instr::Call(FuncIdx(decode_u32(cursor, base_offset)?)),
        0x1A => Instr::Drop,
        0x1B => Instr::Select,
        0x1C => Instr::SelectTyped(parse_result_types(cursor, base_offset)?),
        0x20 => Instr::LocalGet(LocalIdx(decode_u32(cursor, base_offset)?)),
        0x21 => Instr::LocalSet(LocalIdx(decode_u32(cursor, base_offset)?)),
        0x22 => Instr::LocalTee(LocalIdx(decode_u32(cursor, base_offset)?)),
        0x23 => Instr::GlobalGet(GlobalIdx(decode_u32(cursor, base_offset)?)),
        0x24 => Instr::GlobalSet(GlobalIdx(decode_u32(cursor, base_offset)?)),
        0x3F => Instr::MemorySize(parse_mem_idx(cursor, base_offset)?),
        0x40 => Instr::MemoryGrow(parse_mem_idx(cursor, base_offset)?),
        0x41 => Instr::I32Const(decode_i32(cursor, base_offset)?),
        0x42 => Instr::I64Const(decode_i64(cursor, base_offset)?),
        0x43 => Instr::F32Const(decode_f32(cursor, base_offset)?),
        0x44 => Instr::F64Const(decode_f64(cursor, base_offset)?),
        0x45 => Instr::I32Eqz,
        0x46 => Instr::I32Eq,
        0x47 => Instr::I32Ne,
        0x48 => Instr::I32LtS,
        0x49 => Instr::I32LtU,
        0x4A => Instr::I32GtS,
        0x4B => Instr::I32GtU,
        0x4C => Instr::I32LeS,
        0x4D => Instr::I32LeU,
        0x4E => Instr::I32GeS,
        0x4F => Instr::I32GeU,
        0x6A => Instr::I32Add,
        0x7C => Instr::I64Add,
        _ => {
            return Err(DecodeError {
                offset: ByteOffset(base_offset + opcode_offset),
                context: DecodeContext::CodeSection,
                kind: DecodeErrorKind::UnknownOpcode { byte: opcode },
            });
        }
    };

    Ok(DecodedInstr {
        offset: ByteOffset(base_offset + opcode_offset),
        instr,
    })
}

fn parse_mem_idx(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<MemIdx, DecodeError> {
    let pos = cursor.position();
    let byte = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + pos),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    if byte != 0x00 {
        return Err(DecodeError {
            offset: ByteOffset(base_offset + pos),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnexpectedByte {
                expected: 0x00,
                found: byte,
            },
        });
    }
    Ok(MemIdx(0))
}

fn parse_result_types(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Vec<ValType>, DecodeError> {
    let count = decode_u32(cursor, base_offset)?;
    let mut types = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let pos = cursor.position();
        let byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + pos),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        let ty = ValType::from_encoding(byte).ok_or(DecodeError {
            offset: ByteOffset(base_offset + pos),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnknownValType { byte },
        })?;
        types.push(ty);
    }
    Ok(types)
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
    fn decode_br_table() {
        let instrs = decode_instr_sequence(&[0x0E, 0x02, 0x00, 0x01, 0x02, 0x0B], 220).unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::BrTable {
                    targets: vec![LabelIdx(0), LabelIdx(1)],
                    default: LabelIdx(2),
                },
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_typed_select() {
        let instrs = decode_instr_sequence(&[0x1C, 0x01, 0x7E, 0x0B], 230).unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::SelectTyped(vec![ValType::Num(NumType::I64)]),
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_globals_and_memory_ops() {
        let instrs =
            decode_instr_sequence(&[0x23, 0x00, 0x24, 0x01, 0x3F, 0x00, 0x40, 0x00, 0x0B], 240)
                .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::GlobalGet(GlobalIdx(0)),
                Instr::GlobalSet(GlobalIdx(1)),
                Instr::MemorySize(MemIdx(0)),
                Instr::MemoryGrow(MemIdx(0)),
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
    fn decode_instruction_offsets() {
        let body = CodeBody {
            locals: Vec::new(),
            body: &[0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B],
            body_offset: 100,
        };

        let instrs = body.instructions_with_offsets().unwrap();
        assert_eq!(instrs[0].offset, ByteOffset(100));
        assert_eq!(instrs[1].offset, ByteOffset(102));
        assert_eq!(instrs[2].offset, ByteOffset(104));
        assert_eq!(instrs[3].offset, ByteOffset(105));
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
