// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! Instruction decoding.
//!
//! Decodes a subset of WebAssembly instructions sufficient to start Phase 1
//! validation work. Function bodies continue to store raw bytes; decoded
//! instructions are produced on demand from [`CodeBody`].
//!
//! See [Spec §5.4](https://webassembly.github.io/spec/core/binary/instructions.html).

use alloc::vec::Vec;

use crate::binary::leb128::{self, Cursor};
use crate::binary::typeparser::{
    parse_heap_type, parse_val_type as parse_binary_val_type, parse_val_type_with_first_byte,
};
use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};
use crate::types::{
    BlockType, CodeBody, ElemIdx, FuncIdx, GlobalIdx, HeapType, LabelIdx, LocalIdx, MemArg, MemIdx,
    RefType, TableIdx, TagIdx, TypeIdx, ValType,
};

/// A decoded instruction paired with its absolute byte offset in the module.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedInstr {
    pub offset: ByteOffset,
    pub instr: Instr,
}

/// A catch clause of a `try_table` instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct TryTableCatch {
    /// Which form of catch this clause is.
    pub kind: TryTableCatchKind,
    /// Tag index; `None` for `catch_all` / `catch_all_ref`.
    pub tag: Option<TagIdx>,
    /// Branch target label, relative like a `br` label.
    pub label: u32,
}

/// The four catch-clause forms of `try_table`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryTableCatchKind {
    /// `catch $tag $label` — match tag, branch with payload.
    Catch,
    /// `catch_ref $tag $label` — match tag, branch with payload + exnref.
    CatchRef,
    /// `catch_all $label` — match anything, branch empty.
    CatchAll,
    /// `catch_all_ref $label` — match anything, branch with exnref.
    CatchAllRef,
}

/// The `try_table` instruction payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TryTable {
    pub block_type: BlockType,
    pub catches: Vec<TryTableCatch>,
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
    BrOnNull(LabelIdx),
    BrOnNonNull(LabelIdx),
    BrTable {
        targets: Vec<LabelIdx>,
        default: LabelIdx,
    },
    Return,
    Call(FuncIdx),
    ReturnCall(FuncIdx),
    CallRef(TypeIdx),
    ReturnCallRef(TypeIdx),
    CallIndirect {
        type_idx: TypeIdx,
        table_idx: TableIdx,
    },
    ReturnCallIndirect {
        type_idx: TypeIdx,
        table_idx: TableIdx,
    },
    Drop,
    Select,
    SelectTyped(Vec<ValType>),
    LocalGet(LocalIdx),
    LocalSet(LocalIdx),
    LocalTee(LocalIdx),
    GlobalGet(GlobalIdx),
    GlobalSet(GlobalIdx),
    TableGet(TableIdx),
    TableSet(TableIdx),
    V128Load(MemArg),
    V128Load8x8S(MemArg),
    V128Load8x8U(MemArg),
    V128Load16x4S(MemArg),
    V128Load16x4U(MemArg),
    V128Load32x2S(MemArg),
    V128Load32x2U(MemArg),
    V128Load8Splat(MemArg),
    V128Load16Splat(MemArg),
    V128Load32Splat(MemArg),
    V128Load64Splat(MemArg),
    V128Load32Zero(MemArg),
    V128Load64Zero(MemArg),
    V128Store(MemArg),
    V128Load8Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Load16Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Load32Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Load64Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Store8Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Store16Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Store32Lane {
        memarg: MemArg,
        lane: u8,
    },
    V128Store64Lane {
        memarg: MemArg,
        lane: u8,
    },
    I32Load(MemArg),
    I64Load(MemArg),
    F32Load(MemArg),
    F64Load(MemArg),
    I32Load8S(MemArg),
    I32Load8U(MemArg),
    I32Load16S(MemArg),
    I32Load16U(MemArg),
    I64Load8S(MemArg),
    I64Load8U(MemArg),
    I64Load16S(MemArg),
    I64Load16U(MemArg),
    I64Load32S(MemArg),
    I64Load32U(MemArg),
    I32Store(MemArg),
    I64Store(MemArg),
    F32Store(MemArg),
    F64Store(MemArg),
    I32Store8(MemArg),
    I32Store16(MemArg),
    I64Store8(MemArg),
    I64Store16(MemArg),
    I64Store32(MemArg),
    MemoryInit(crate::types::DataIdx, MemIdx),
    DataDrop(crate::types::DataIdx),
    MemoryCopy {
        dst: MemIdx,
        src: MemIdx,
    },
    MemoryFill(MemIdx),
    TableInit {
        elem_idx: ElemIdx,
        table_idx: TableIdx,
    },
    ElemDrop(ElemIdx),
    TableCopy {
        dst: TableIdx,
        src: TableIdx,
    },
    TableGrow(TableIdx),
    TableSize(TableIdx),
    TableFill(TableIdx),
    MemorySize(MemIdx),
    MemoryGrow(MemIdx),
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),
    RefNull(RefType),
    RefIsNull,
    RefFunc(FuncIdx),
    RefAsNonNull,
    /// `ref.test (ref null? ht)` — test a reference against a heap type.
    RefTest {
        nullable: bool,
        ht: HeapType,
    },
    /// `ref.cast (ref null? ht)` — cast a reference, trapping on failure.
    RefCast {
        nullable: bool,
        ht: HeapType,
    },
    /// `br_on_cast` / `br_on_cast_fail` — branch on cast success/failure.
    BrOnCast {
        fail: bool,
        src: RefType,
        tgt: RefType,
        label: LabelIdx,
    },
    /// `ref.eq` — equality comparison of two eq references.
    RefEq,
    /// `any.convert_extern` — re-tag an extern reference as an any reference.
    AnyConvertExtern,
    /// `extern.convert_any` — re-tag an any reference as an extern reference.
    ExternConvertAny,
    V128Const([u8; 16]),
    I8x16Splat,
    I16x8Splat,
    I32x4Splat,
    I64x2Splat,
    F32x4Splat,
    F64x2Splat,
    I32x4ExtractLane(u8),
    I32x4ReplaceLane(u8),
    F32x4ExtractLane(u8),
    F32x4ReplaceLane(u8),
    V128Not,
    V128And,
    V128Or,
    V128Xor,
    I8x16Add,
    I8x16Sub,
    I16x8Add,
    I16x8Mul,
    I16x8Sub,
    I32x4Add,
    I32x4Sub,
    I32x4Mul,
    I64x2Add,
    I64x2Mul,
    I64x2Sub,
    F32x4Add,
    F32x4Sub,
    F32x4Mul,
    F32x4Div,
    F64x2Add,
    F64x2Sub,
    F64x2Mul,
    F64x2Div,
    V128Andnot,
    I8x16AddSatS,
    I8x16AddSatU,
    I8x16SubSatS,
    I8x16SubSatU,
    I8x16MinS,
    I8x16MinU,
    I8x16MaxS,
    I8x16MaxU,
    I8x16AvgrU,
    I16x8AddSatS,
    I16x8AddSatU,
    I16x8SubSatS,
    I16x8SubSatU,
    I16x8MinS,
    I16x8MinU,
    I16x8MaxS,
    I16x8MaxU,
    I16x8AvgrU,
    I16x8Q15MulrSatS,
    I32x4MinS,
    I32x4MinU,
    I32x4MaxS,
    I32x4MaxU,
    I8x16ExtractLaneS(u8),
    I8x16ExtractLaneU(u8),
    I8x16ReplaceLane(u8),
    I16x8ExtractLaneS(u8),
    I16x8ExtractLaneU(u8),
    I16x8ReplaceLane(u8),
    I64x2ExtractLane(u8),
    I64x2ReplaceLane(u8),
    F64x2ExtractLane(u8),
    F64x2ReplaceLane(u8),
    V128Bitselect,
    V128AnyTrue,
    I8x16Abs,
    I8x16Neg,
    I8x16Popcnt,
    I8x16AllTrue,
    I8x16Bitmask,
    I8x16NarrowI16x8S,
    I8x16NarrowI16x8U,
    I16x8ExtAddPairwiseI8x16S,
    I16x8ExtAddPairwiseI8x16U,
    I16x8Abs,
    I16x8Neg,
    I16x8AllTrue,
    I16x8Bitmask,
    I16x8NarrowI32x4S,
    I16x8NarrowI32x4U,
    I16x8ExtendLowI8x16S,
    I16x8ExtendHighI8x16S,
    I16x8ExtendLowI8x16U,
    I16x8ExtendHighI8x16U,
    I16x8ExtMulLowI8x16S,
    I16x8ExtMulHighI8x16S,
    I16x8ExtMulLowI8x16U,
    I16x8ExtMulHighI8x16U,
    I32x4ExtAddPairwiseI16x8S,
    I32x4ExtAddPairwiseI16x8U,
    I32x4Abs,
    I32x4Neg,
    I32x4AllTrue,
    I32x4Bitmask,
    I32x4ExtendLowI16x8S,
    I32x4ExtendHighI16x8S,
    I32x4ExtendLowI16x8U,
    I32x4ExtendHighI16x8U,
    I32x4DotI16x8S,
    I32x4ExtMulLowI16x8S,
    I32x4ExtMulHighI16x8S,
    I32x4ExtMulLowI16x8U,
    I32x4ExtMulHighI16x8U,
    I64x2Abs,
    I64x2Neg,
    I64x2AllTrue,
    I64x2Bitmask,
    I64x2ExtendLowI32x4S,
    I64x2ExtendHighI32x4S,
    I64x2ExtendLowI32x4U,
    I64x2ExtendHighI32x4U,
    I64x2ExtMulLowI32x4S,
    I64x2ExtMulHighI32x4S,
    I64x2ExtMulLowI32x4U,
    I64x2ExtMulHighI32x4U,
    I8x16Eq,
    I8x16Ne,
    I8x16LtS,
    I8x16LtU,
    I8x16GtS,
    I8x16GtU,
    I8x16LeS,
    I8x16LeU,
    I8x16GeS,
    I8x16GeU,
    I16x8Eq,
    I16x8Ne,
    I16x8LtS,
    I16x8LtU,
    I16x8GtS,
    I16x8GtU,
    I16x8LeS,
    I16x8LeU,
    I16x8GeS,
    I16x8GeU,
    I32x4Eq,
    I32x4Ne,
    I32x4LtS,
    I32x4LtU,
    I32x4GtS,
    I32x4GtU,
    I32x4LeS,
    I32x4LeU,
    I32x4GeS,
    I32x4GeU,
    F32x4Eq,
    F32x4Ne,
    F32x4Lt,
    F32x4Gt,
    F32x4Le,
    F32x4Ge,
    F64x2Eq,
    F64x2Ne,
    F64x2Lt,
    F64x2Gt,
    F64x2Le,
    F64x2Ge,
    I8x16Shl,
    I8x16ShrS,
    I8x16ShrU,
    I16x8Shl,
    I16x8ShrS,
    I16x8ShrU,
    I32x4Shl,
    I32x4ShrS,
    I32x4ShrU,
    I64x2Shl,
    I64x2ShrS,
    I64x2ShrU,
    I64x2Eq,
    I64x2Ne,
    I64x2LtS,
    I64x2GtS,
    I64x2LeS,
    I64x2GeS,
    F32x4Abs,
    F32x4Neg,
    F32x4Sqrt,
    F32x4Ceil,
    F32x4Floor,
    F32x4Trunc,
    F32x4Nearest,
    F64x2Abs,
    F64x2Neg,
    F64x2Sqrt,
    F64x2Ceil,
    F64x2Floor,
    F64x2Trunc,
    F64x2Nearest,
    F32x4Min,
    F32x4Max,
    F32x4PMin,
    F32x4PMax,
    F64x2Min,
    F64x2Max,
    F64x2PMin,
    F64x2PMax,
    I32x4TruncSatF32x4S,
    I32x4TruncSatF32x4U,
    F32x4ConvertI32x4S,
    F32x4ConvertI32x4U,
    I32x4TruncSatF64x2SZero,
    I32x4TruncSatF64x2UZero,
    F32x4DemoteF64x2Zero,
    F64x2PromoteLowF32x4,
    F64x2ConvertLowI32x4S,
    F64x2ConvertLowI32x4U,
    I8x16Swizzle,
    I8x16Shuffle([u8; 16]),
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
    I64Eqz,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LtU,
    I64GtS,
    I64GtU,
    I64LeS,
    I64LeU,
    I64GeS,
    I64GeU,
    F32Eq,
    F32Ne,
    F32Lt,
    F32Gt,
    F32Le,
    F32Ge,
    F64Eq,
    F64Ne,
    F64Lt,
    F64Gt,
    F64Le,
    F64Ge,
    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,
    I64Clz,
    I64Ctz,
    I64Popcnt,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrS,
    I64ShrU,
    I64Rotl,
    I64Rotr,
    F32Abs,
    F32Neg,
    F32Ceil,
    F32Floor,
    F32Trunc,
    F32Nearest,
    F32Sqrt,
    F32Add,
    F32Sub,
    F32Mul,
    F32Div,
    F32Min,
    F32Max,
    F32Copysign,
    F64Abs,
    F64Neg,
    F64Ceil,
    F64Floor,
    F64Trunc,
    F64Nearest,
    F64Sqrt,
    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    F64Min,
    F64Max,
    F64Copysign,
    I32WrapI64,
    I32TruncF32S,
    I32TruncF32U,
    I32TruncF64S,
    I32TruncF64U,
    I64ExtendI32S,
    I64ExtendI32U,
    I64TruncF32S,
    I64TruncF32U,
    I64TruncF64S,
    I64TruncF64U,
    F32ConvertI32S,
    F32ConvertI32U,
    F32ConvertI64S,
    F32ConvertI64U,
    F32DemoteF64,
    F64ConvertI32S,
    F64ConvertI32U,
    F64ConvertI64S,
    F64ConvertI64U,
    F64PromoteF32,
    I32ReinterpretF32,
    I64ReinterpretF64,
    F32ReinterpretI32,
    F64ReinterpretI64,
    I32Extend8S,
    I32Extend16S,
    I64Extend8S,
    I64Extend16S,
    I64Extend32S,
    I32TruncSatF32S,
    I32TruncSatF32U,
    I32TruncSatF64S,
    I32TruncSatF64U,
    I64TruncSatF32S,
    I64TruncSatF32U,
    I64TruncSatF64S,
    I64TruncSatF64U,
    /// `try_table` — protected block with catch clauses (wasm 3.0 EH).
    TryTable(TryTable),
    /// `throw $tag` — raise an exception with the tag's parameters.
    Throw(TagIdx),
    /// `throw_ref` — re-raise the exception referenced by an exnref.
    ThrowRef,
    /// `struct.new $x` — allocate a struct from its field operands.
    StructNew(TypeIdx),
    /// `struct.new_default $x` — allocate a struct with default field values.
    StructNewDefault(TypeIdx),
    /// `struct.get $x $i` — read an unpacked struct field.
    StructGet {
        type_idx: TypeIdx,
        field: u32,
    },
    /// `struct.get_s $x $i` — read a packed struct field, sign-extended.
    StructGetS {
        type_idx: TypeIdx,
        field: u32,
    },
    /// `struct.get_u $x $i` — read a packed struct field, zero-extended.
    StructGetU {
        type_idx: TypeIdx,
        field: u32,
    },
    /// `struct.set $x $i` — write a struct field.
    StructSet {
        type_idx: TypeIdx,
        field: u32,
    },
    /// `array.new $x` — allocate an array of `len` copies of the operand.
    ArrayNew(TypeIdx),
    /// `array.new_default $x` — allocate an array of default elements.
    ArrayNewDefault(TypeIdx),
    /// `array.new_fixed $x $n` — allocate an array from `n` operands.
    ArrayNewFixed {
        type_idx: TypeIdx,
        count: u32,
    },
    /// `array.new_data $x $y` — allocate an array from a data segment.
    ArrayNewData {
        type_idx: TypeIdx,
        data: crate::types::DataIdx,
    },
    /// `array.new_elem $x $y` — allocate an array from an element segment.
    ArrayNewElem {
        type_idx: TypeIdx,
        elem: ElemIdx,
    },
    /// `array.get $x` — read an unpacked array element.
    ArrayGet(TypeIdx),
    /// `array.get_s $x` — read a packed array element, sign-extended.
    ArrayGetS(TypeIdx),
    /// `array.get_u $x` — read a packed array element, zero-extended.
    ArrayGetU(TypeIdx),
    /// `array.set $x` — write an array element.
    ArraySet(TypeIdx),
    /// `array.len` — the length of an abstract array reference.
    ArrayLen,
    /// `array.fill $x` — fill a range with a value.
    ArrayFill(TypeIdx),
    /// `array.copy $x $y` — copy between arrays.
    ArrayCopy {
        dst: TypeIdx,
        src: TypeIdx,
    },
    /// `array.init_data $x $y` — initialize from a data segment.
    ArrayInitData {
        type_idx: TypeIdx,
        data: crate::types::DataIdx,
    },
    /// `array.init_elem $x $y` — initialize from an element segment.
    ArrayInitElem {
        type_idx: TypeIdx,
        elem: ElemIdx,
    },
    /// `ref.i31` — box an i32 into a 31-bit immediate reference.
    RefI31,
    /// `i31.get_s` — sign-extended payload.
    I31GetS,
    /// `i31.get_u` — zero-extended payload.
    I31GetU,
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
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) | Instr::TryTable(_) => {
                block_depth += 1
            }
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
        0x1F => Instr::TryTable(parse_try_table(cursor, base_offset)?),
        0x08 => Instr::Throw(parse_tag_idx(cursor, base_offset)?),
        0x0A => Instr::ThrowRef,
        0x0B => Instr::End,
        0x0C => Instr::Br(LabelIdx(decode_u32(cursor, base_offset)?)),
        0x0D => Instr::BrIf(LabelIdx(decode_u32(cursor, base_offset)?)),
        0x0E => {
            let target_count = decode_u32(cursor, base_offset)?;
            let mut targets = Vec::with_capacity(cursor.capacity_hint(target_count));
            for _ in 0..target_count {
                targets.push(LabelIdx(decode_u32(cursor, base_offset)?));
            }
            let default = LabelIdx(decode_u32(cursor, base_offset)?);
            Instr::BrTable { targets, default }
        }
        0x0F => Instr::Return,
        0x10 => Instr::Call(FuncIdx(decode_u32(cursor, base_offset)?)),
        0x12 => Instr::ReturnCall(FuncIdx(decode_u32(cursor, base_offset)?)),
        0x14 => Instr::CallRef(TypeIdx(decode_u32(cursor, base_offset)?)),
        0x15 => Instr::ReturnCallRef(TypeIdx(decode_u32(cursor, base_offset)?)),
        0x11 => Instr::CallIndirect {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            table_idx: parse_table_idx(cursor, base_offset)?,
        },
        0x13 => Instr::ReturnCallIndirect {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            table_idx: parse_table_idx(cursor, base_offset)?,
        },
        0x1A => Instr::Drop,
        0x1B => Instr::Select,
        0x1C => Instr::SelectTyped(parse_result_types(cursor, base_offset)?),
        0x20 => Instr::LocalGet(LocalIdx(decode_u32(cursor, base_offset)?)),
        0x21 => Instr::LocalSet(LocalIdx(decode_u32(cursor, base_offset)?)),
        0x22 => Instr::LocalTee(LocalIdx(decode_u32(cursor, base_offset)?)),
        0x23 => Instr::GlobalGet(GlobalIdx(decode_u32(cursor, base_offset)?)),
        0x24 => Instr::GlobalSet(GlobalIdx(decode_u32(cursor, base_offset)?)),
        0x25 => Instr::TableGet(parse_table_idx(cursor, base_offset)?),
        0x26 => Instr::TableSet(parse_table_idx(cursor, base_offset)?),
        0xFC => decode_bulk_memory_instr(cursor, base_offset)?,
        0xFD => decode_simd_instr(cursor, base_offset)?,
        0xFB => decode_gc_instr(cursor, base_offset)?,
        0x28 => Instr::I32Load(parse_memarg(cursor, base_offset)?),
        0x29 => Instr::I64Load(parse_memarg(cursor, base_offset)?),
        0x2A => Instr::F32Load(parse_memarg(cursor, base_offset)?),
        0x2B => Instr::F64Load(parse_memarg(cursor, base_offset)?),
        0x2C => Instr::I32Load8S(parse_memarg(cursor, base_offset)?),
        0x2D => Instr::I32Load8U(parse_memarg(cursor, base_offset)?),
        0x2E => Instr::I32Load16S(parse_memarg(cursor, base_offset)?),
        0x2F => Instr::I32Load16U(parse_memarg(cursor, base_offset)?),
        0x30 => Instr::I64Load8S(parse_memarg(cursor, base_offset)?),
        0x31 => Instr::I64Load8U(parse_memarg(cursor, base_offset)?),
        0x32 => Instr::I64Load16S(parse_memarg(cursor, base_offset)?),
        0x33 => Instr::I64Load16U(parse_memarg(cursor, base_offset)?),
        0x34 => Instr::I64Load32S(parse_memarg(cursor, base_offset)?),
        0x35 => Instr::I64Load32U(parse_memarg(cursor, base_offset)?),
        0x36 => Instr::I32Store(parse_memarg(cursor, base_offset)?),
        0x37 => Instr::I64Store(parse_memarg(cursor, base_offset)?),
        0x38 => Instr::F32Store(parse_memarg(cursor, base_offset)?),
        0x39 => Instr::F64Store(parse_memarg(cursor, base_offset)?),
        0x3A => Instr::I32Store8(parse_memarg(cursor, base_offset)?),
        0x3B => Instr::I32Store16(parse_memarg(cursor, base_offset)?),
        0x3C => Instr::I64Store8(parse_memarg(cursor, base_offset)?),
        0x3D => Instr::I64Store16(parse_memarg(cursor, base_offset)?),
        0x3E => Instr::I64Store32(parse_memarg(cursor, base_offset)?),
        0x3F => Instr::MemorySize(parse_mem_idx(cursor, base_offset)?),
        0x40 => Instr::MemoryGrow(parse_mem_idx(cursor, base_offset)?),
        0x41 => Instr::I32Const(decode_i32(cursor, base_offset)?),
        0x42 => Instr::I64Const(decode_i64(cursor, base_offset)?),
        0x43 => Instr::F32Const(decode_f32(cursor, base_offset)?),
        0x44 => Instr::F64Const(decode_f64(cursor, base_offset)?),
        0xD0 => Instr::RefNull(RefType::from_parts(
            true,
            parse_heap_type(cursor, base_offset, DecodeContext::CodeSection)?,
        )),
        0xD1 => Instr::RefIsNull,
        0xD2 => Instr::RefFunc(FuncIdx(decode_u32(cursor, base_offset)?)),
        0xD3 => Instr::RefEq,
        0xD4 => Instr::RefAsNonNull,
        0xD5 => Instr::BrOnNull(LabelIdx(decode_u32(cursor, base_offset)?)),
        0xD6 => Instr::BrOnNonNull(LabelIdx(decode_u32(cursor, base_offset)?)),
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
        0x50 => Instr::I64Eqz,
        0x51 => Instr::I64Eq,
        0x52 => Instr::I64Ne,
        0x53 => Instr::I64LtS,
        0x54 => Instr::I64LtU,
        0x55 => Instr::I64GtS,
        0x56 => Instr::I64GtU,
        0x57 => Instr::I64LeS,
        0x58 => Instr::I64LeU,
        0x59 => Instr::I64GeS,
        0x5A => Instr::I64GeU,
        0x5B => Instr::F32Eq,
        0x5C => Instr::F32Ne,
        0x5D => Instr::F32Lt,
        0x5E => Instr::F32Gt,
        0x5F => Instr::F32Le,
        0x60 => Instr::F32Ge,
        0x61 => Instr::F64Eq,
        0x62 => Instr::F64Ne,
        0x63 => Instr::F64Lt,
        0x64 => Instr::F64Gt,
        0x65 => Instr::F64Le,
        0x66 => Instr::F64Ge,
        0x67 => Instr::I32Clz,
        0x68 => Instr::I32Ctz,
        0x69 => Instr::I32Popcnt,
        0x6A => Instr::I32Add,
        0x6B => Instr::I32Sub,
        0x6C => Instr::I32Mul,
        0x6D => Instr::I32DivS,
        0x6E => Instr::I32DivU,
        0x6F => Instr::I32RemS,
        0x70 => Instr::I32RemU,
        0x71 => Instr::I32And,
        0x72 => Instr::I32Or,
        0x73 => Instr::I32Xor,
        0x74 => Instr::I32Shl,
        0x75 => Instr::I32ShrS,
        0x76 => Instr::I32ShrU,
        0x77 => Instr::I32Rotl,
        0x78 => Instr::I32Rotr,
        0x79 => Instr::I64Clz,
        0x7A => Instr::I64Ctz,
        0x7B => Instr::I64Popcnt,
        0x7C => Instr::I64Add,
        0x7D => Instr::I64Sub,
        0x7E => Instr::I64Mul,
        0x7F => Instr::I64DivS,
        0x80 => Instr::I64DivU,
        0x81 => Instr::I64RemS,
        0x82 => Instr::I64RemU,
        0x83 => Instr::I64And,
        0x84 => Instr::I64Or,
        0x85 => Instr::I64Xor,
        0x86 => Instr::I64Shl,
        0x87 => Instr::I64ShrS,
        0x88 => Instr::I64ShrU,
        0x89 => Instr::I64Rotl,
        0x8A => Instr::I64Rotr,
        0x8B => Instr::F32Abs,
        0x8C => Instr::F32Neg,
        0x8D => Instr::F32Ceil,
        0x8E => Instr::F32Floor,
        0x8F => Instr::F32Trunc,
        0x90 => Instr::F32Nearest,
        0x91 => Instr::F32Sqrt,
        0x92 => Instr::F32Add,
        0x93 => Instr::F32Sub,
        0x94 => Instr::F32Mul,
        0x95 => Instr::F32Div,
        0x96 => Instr::F32Min,
        0x97 => Instr::F32Max,
        0x98 => Instr::F32Copysign,
        0x99 => Instr::F64Abs,
        0x9A => Instr::F64Neg,
        0x9B => Instr::F64Ceil,
        0x9C => Instr::F64Floor,
        0x9D => Instr::F64Trunc,
        0x9E => Instr::F64Nearest,
        0x9F => Instr::F64Sqrt,
        0xA0 => Instr::F64Add,
        0xA1 => Instr::F64Sub,
        0xA2 => Instr::F64Mul,
        0xA3 => Instr::F64Div,
        0xA4 => Instr::F64Min,
        0xA5 => Instr::F64Max,
        0xA6 => Instr::F64Copysign,
        0xA7 => Instr::I32WrapI64,
        0xA8 => Instr::I32TruncF32S,
        0xA9 => Instr::I32TruncF32U,
        0xAA => Instr::I32TruncF64S,
        0xAB => Instr::I32TruncF64U,
        0xAC => Instr::I64ExtendI32S,
        0xAD => Instr::I64ExtendI32U,
        0xAE => Instr::I64TruncF32S,
        0xAF => Instr::I64TruncF32U,
        0xB0 => Instr::I64TruncF64S,
        0xB1 => Instr::I64TruncF64U,
        0xB2 => Instr::F32ConvertI32S,
        0xB3 => Instr::F32ConvertI32U,
        0xB4 => Instr::F32ConvertI64S,
        0xB5 => Instr::F32ConvertI64U,
        0xB6 => Instr::F32DemoteF64,
        0xB7 => Instr::F64ConvertI32S,
        0xB8 => Instr::F64ConvertI32U,
        0xB9 => Instr::F64ConvertI64S,
        0xBA => Instr::F64ConvertI64U,
        0xBB => Instr::F64PromoteF32,
        0xBC => Instr::I32ReinterpretF32,
        0xBD => Instr::I64ReinterpretF64,
        0xBE => Instr::F32ReinterpretI32,
        0xBF => Instr::F64ReinterpretI64,
        0xC0 => Instr::I32Extend8S,
        0xC1 => Instr::I32Extend16S,
        0xC2 => Instr::I64Extend8S,
        0xC3 => Instr::I64Extend16S,
        0xC4 => Instr::I64Extend32S,
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

fn decode_gc_instr(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Instr, DecodeError> {
    let opcode = decode_u32(cursor, base_offset)?;
    match opcode {
        0x00 => Ok(Instr::StructNew(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x01 => Ok(Instr::StructNewDefault(TypeIdx(decode_u32(
            cursor,
            base_offset,
        )?))),
        0x02 => Ok(Instr::StructGet {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            field: decode_u32(cursor, base_offset)?,
        }),
        0x03 => Ok(Instr::StructGetS {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            field: decode_u32(cursor, base_offset)?,
        }),
        0x04 => Ok(Instr::StructGetU {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            field: decode_u32(cursor, base_offset)?,
        }),
        0x05 => Ok(Instr::StructSet {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            field: decode_u32(cursor, base_offset)?,
        }),
        0x06 => Ok(Instr::ArrayNew(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x07 => Ok(Instr::ArrayNewDefault(TypeIdx(decode_u32(
            cursor,
            base_offset,
        )?))),
        0x08 => Ok(Instr::ArrayNewFixed {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            count: decode_u32(cursor, base_offset)?,
        }),
        0x09 => Ok(Instr::ArrayNewData {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            data: crate::types::DataIdx(decode_u32(cursor, base_offset)?),
        }),
        0x0A => Ok(Instr::ArrayNewElem {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            elem: ElemIdx(decode_u32(cursor, base_offset)?),
        }),
        0x0B => Ok(Instr::ArrayGet(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x0C => Ok(Instr::ArrayGetS(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x0D => Ok(Instr::ArrayGetU(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x0E => Ok(Instr::ArraySet(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x0F => Ok(Instr::ArrayLen),
        0x10 => Ok(Instr::ArrayFill(TypeIdx(decode_u32(cursor, base_offset)?))),
        0x11 => Ok(Instr::ArrayCopy {
            dst: TypeIdx(decode_u32(cursor, base_offset)?),
            src: TypeIdx(decode_u32(cursor, base_offset)?),
        }),
        0x12 => Ok(Instr::ArrayInitData {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            data: crate::types::DataIdx(decode_u32(cursor, base_offset)?),
        }),
        0x13 => Ok(Instr::ArrayInitElem {
            type_idx: TypeIdx(decode_u32(cursor, base_offset)?),
            elem: ElemIdx(decode_u32(cursor, base_offset)?),
        }),
        0x1C => Ok(Instr::RefI31),
        0x1D => Ok(Instr::I31GetS),
        0x1E => Ok(Instr::I31GetU),
        // 0x14–0x17: cast/test identity checks; 0x18/0x19: br_on_cast(_fail);
        // 0x1a/0x1b: extern/any conversions. Shapes and operand order follow
        // `interpreter/binary/decode.ml`.
        0x14..=0x17 => {
            let ht = parse_heap_type(cursor, base_offset, DecodeContext::CodeSection)?;
            match opcode {
                0x14 => Ok(Instr::RefTest {
                    nullable: false,
                    ht,
                }),
                0x15 => Ok(Instr::RefTest { nullable: true, ht }),
                0x16 => Ok(Instr::RefCast {
                    nullable: false,
                    ht,
                }),
                _ => Ok(Instr::RefCast { nullable: true, ht }),
            }
        }
        0x18 | 0x19 => {
            let flags = cursor.read_byte().map_err(|_| DecodeError {
                offset: ByteOffset(base_offset + cursor.position()),
                context: DecodeContext::CodeSection,
                kind: DecodeErrorKind::UnexpectedEof,
            })?;
            if flags & 0xfc != 0 {
                return Err(DecodeError {
                    offset: ByteOffset(base_offset),
                    context: DecodeContext::CodeSection,
                    kind: DecodeErrorKind::MalformedCastFlags { flags },
                });
            }
            let label = LabelIdx(decode_u32(cursor, base_offset)?);
            let src_null = flags & 0x01 != 0;
            let tgt_null = flags & 0x02 != 0;
            let src = RefType::from_parts(
                src_null,
                parse_heap_type(cursor, base_offset, DecodeContext::CodeSection)?,
            );
            let tgt = RefType::from_parts(
                tgt_null,
                parse_heap_type(cursor, base_offset, DecodeContext::CodeSection)?,
            );
            Ok(Instr::BrOnCast {
                fail: opcode == 0x19,
                src,
                tgt,
                label,
            })
        }
        0x1A => Ok(Instr::AnyConvertExtern),
        0x1B => Ok(Instr::ExternConvertAny),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnknownOpcode { byte: 0xFB },
        }),
    }
}

fn decode_bulk_memory_instr(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Instr, DecodeError> {
    let opcode = decode_u32(cursor, base_offset)?;
    match opcode {
        0 => Ok(Instr::I32TruncSatF32S),
        1 => Ok(Instr::I32TruncSatF32U),
        2 => Ok(Instr::I32TruncSatF64S),
        3 => Ok(Instr::I32TruncSatF64U),
        4 => Ok(Instr::I64TruncSatF32S),
        5 => Ok(Instr::I64TruncSatF32U),
        6 => Ok(Instr::I64TruncSatF64S),
        7 => Ok(Instr::I64TruncSatF64U),
        8 => {
            let data = crate::types::DataIdx(decode_u32(cursor, base_offset)?);
            let mem = parse_mem_idx(cursor, base_offset)?;
            Ok(Instr::MemoryInit(data, mem))
        }
        9 => Ok(Instr::DataDrop(crate::types::DataIdx(decode_u32(
            cursor,
            base_offset,
        )?))),
        10 => {
            let dst = parse_mem_idx(cursor, base_offset)?;
            let src = parse_mem_idx(cursor, base_offset)?;
            Ok(Instr::MemoryCopy { dst, src })
        }
        11 => Ok(Instr::MemoryFill(parse_mem_idx(cursor, base_offset)?)),
        12 => Ok(Instr::TableInit {
            elem_idx: ElemIdx(decode_u32(cursor, base_offset)?),
            table_idx: parse_table_idx(cursor, base_offset)?,
        }),
        13 => Ok(Instr::ElemDrop(ElemIdx(decode_u32(cursor, base_offset)?))),
        14 => {
            let dst = parse_table_idx(cursor, base_offset)?;
            let src = parse_table_idx(cursor, base_offset)?;
            Ok(Instr::TableCopy { dst, src })
        }
        15 => Ok(Instr::TableGrow(parse_table_idx(cursor, base_offset)?)),
        16 => Ok(Instr::TableSize(parse_table_idx(cursor, base_offset)?)),
        17 => Ok(Instr::TableFill(parse_table_idx(cursor, base_offset)?)),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnknownOpcode { byte: 0xFC },
        }),
    }
}

fn decode_simd_instr(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<Instr, DecodeError> {
    let opcode = decode_u32(cursor, base_offset)?;
    match opcode {
        0 => Ok(Instr::V128Load(parse_memarg(cursor, base_offset)?)),
        1 => Ok(Instr::V128Load8x8S(parse_memarg(cursor, base_offset)?)),
        2 => Ok(Instr::V128Load8x8U(parse_memarg(cursor, base_offset)?)),
        3 => Ok(Instr::V128Load16x4S(parse_memarg(cursor, base_offset)?)),
        4 => Ok(Instr::V128Load16x4U(parse_memarg(cursor, base_offset)?)),
        5 => Ok(Instr::V128Load32x2S(parse_memarg(cursor, base_offset)?)),
        6 => Ok(Instr::V128Load32x2U(parse_memarg(cursor, base_offset)?)),
        7 => Ok(Instr::V128Load8Splat(parse_memarg(cursor, base_offset)?)),
        8 => Ok(Instr::V128Load16Splat(parse_memarg(cursor, base_offset)?)),
        9 => Ok(Instr::V128Load32Splat(parse_memarg(cursor, base_offset)?)),
        10 => Ok(Instr::V128Load64Splat(parse_memarg(cursor, base_offset)?)),
        11 => Ok(Instr::V128Store(parse_memarg(cursor, base_offset)?)),
        12 => Ok(Instr::V128Const(parse_v128_const(cursor, base_offset)?)),
        15 => Ok(Instr::I8x16Splat),
        16 => Ok(Instr::I16x8Splat),
        17 => Ok(Instr::I32x4Splat),
        18 => Ok(Instr::I64x2Splat),
        19 => Ok(Instr::F32x4Splat),
        20 => Ok(Instr::F64x2Splat),
        27 => Ok(Instr::I32x4ExtractLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        28 => Ok(Instr::I32x4ReplaceLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        31 => Ok(Instr::F32x4ExtractLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        32 => Ok(Instr::F32x4ReplaceLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        21 => Ok(Instr::I8x16ExtractLaneS(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        22 => Ok(Instr::I8x16ExtractLaneU(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        23 => Ok(Instr::I8x16ReplaceLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        24 => Ok(Instr::I16x8ExtractLaneS(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        25 => Ok(Instr::I16x8ExtractLaneU(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        26 => Ok(Instr::I16x8ReplaceLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        29 => Ok(Instr::I64x2ExtractLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        30 => Ok(Instr::I64x2ReplaceLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        33 => Ok(Instr::F64x2ExtractLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        34 => Ok(Instr::F64x2ReplaceLane(parse_lane_idx(
            cursor,
            base_offset,
        )?)),
        79 => Ok(Instr::V128Andnot),
        111 => Ok(Instr::I8x16AddSatS),
        112 => Ok(Instr::I8x16AddSatU),
        114 => Ok(Instr::I8x16SubSatS),
        115 => Ok(Instr::I8x16SubSatU),
        118 => Ok(Instr::I8x16MinS),
        119 => Ok(Instr::I8x16MinU),
        120 => Ok(Instr::I8x16MaxS),
        121 => Ok(Instr::I8x16MaxU),
        123 => Ok(Instr::I8x16AvgrU),
        130 => Ok(Instr::I16x8Q15MulrSatS),
        143 => Ok(Instr::I16x8AddSatS),
        144 => Ok(Instr::I16x8AddSatU),
        146 => Ok(Instr::I16x8SubSatS),
        147 => Ok(Instr::I16x8SubSatU),
        149 => Ok(Instr::I16x8Mul),
        150 => Ok(Instr::I16x8MinS),
        151 => Ok(Instr::I16x8MinU),
        152 => Ok(Instr::I16x8MaxS),
        153 => Ok(Instr::I16x8MaxU),
        155 => Ok(Instr::I16x8AvgrU),
        182 => Ok(Instr::I32x4MinS),
        183 => Ok(Instr::I32x4MinU),
        184 => Ok(Instr::I32x4MaxS),
        185 => Ok(Instr::I32x4MaxU),
        77 => Ok(Instr::V128Not),
        78 => Ok(Instr::V128And),
        80 => Ok(Instr::V128Or),
        81 => Ok(Instr::V128Xor),
        110 => Ok(Instr::I8x16Add),
        113 => Ok(Instr::I8x16Sub),
        142 => Ok(Instr::I16x8Add),
        145 => Ok(Instr::I16x8Sub),
        174 => Ok(Instr::I32x4Add),
        177 => Ok(Instr::I32x4Sub),
        181 => Ok(Instr::I32x4Mul),
        206 => Ok(Instr::I64x2Add),
        213 => Ok(Instr::I64x2Mul),
        209 => Ok(Instr::I64x2Sub),
        228 => Ok(Instr::F32x4Add),
        229 => Ok(Instr::F32x4Sub),
        230 => Ok(Instr::F32x4Mul),
        231 => Ok(Instr::F32x4Div),
        240 => Ok(Instr::F64x2Add),
        241 => Ok(Instr::F64x2Sub),
        242 => Ok(Instr::F64x2Mul),
        243 => Ok(Instr::F64x2Div),
        82 => Ok(Instr::V128Bitselect),
        83 => Ok(Instr::V128AnyTrue),
        96 => Ok(Instr::I8x16Abs),
        97 => Ok(Instr::I8x16Neg),
        98 => Ok(Instr::I8x16Popcnt),
        99 => Ok(Instr::I8x16AllTrue),
        100 => Ok(Instr::I8x16Bitmask),
        101 => Ok(Instr::I8x16NarrowI16x8S),
        102 => Ok(Instr::I8x16NarrowI16x8U),
        124 => Ok(Instr::I16x8ExtAddPairwiseI8x16S),
        125 => Ok(Instr::I16x8ExtAddPairwiseI8x16U),
        128 => Ok(Instr::I16x8Abs),
        129 => Ok(Instr::I16x8Neg),
        131 => Ok(Instr::I16x8AllTrue),
        132 => Ok(Instr::I16x8Bitmask),
        133 => Ok(Instr::I16x8NarrowI32x4S),
        134 => Ok(Instr::I16x8NarrowI32x4U),
        135 => Ok(Instr::I16x8ExtendLowI8x16S),
        136 => Ok(Instr::I16x8ExtendHighI8x16S),
        137 => Ok(Instr::I16x8ExtendLowI8x16U),
        138 => Ok(Instr::I16x8ExtendHighI8x16U),
        156 => Ok(Instr::I16x8ExtMulLowI8x16S),
        157 => Ok(Instr::I16x8ExtMulHighI8x16S),
        158 => Ok(Instr::I16x8ExtMulLowI8x16U),
        159 => Ok(Instr::I16x8ExtMulHighI8x16U),
        126 => Ok(Instr::I32x4ExtAddPairwiseI16x8S),
        127 => Ok(Instr::I32x4ExtAddPairwiseI16x8U),
        160 => Ok(Instr::I32x4Abs),
        161 => Ok(Instr::I32x4Neg),
        163 => Ok(Instr::I32x4AllTrue),
        164 => Ok(Instr::I32x4Bitmask),
        167 => Ok(Instr::I32x4ExtendLowI16x8S),
        168 => Ok(Instr::I32x4ExtendHighI16x8S),
        169 => Ok(Instr::I32x4ExtendLowI16x8U),
        170 => Ok(Instr::I32x4ExtendHighI16x8U),
        186 => Ok(Instr::I32x4DotI16x8S),
        188 => Ok(Instr::I32x4ExtMulLowI16x8S),
        189 => Ok(Instr::I32x4ExtMulHighI16x8S),
        190 => Ok(Instr::I32x4ExtMulLowI16x8U),
        191 => Ok(Instr::I32x4ExtMulHighI16x8U),
        192 => Ok(Instr::I64x2Abs),
        193 => Ok(Instr::I64x2Neg),
        195 => Ok(Instr::I64x2AllTrue),
        196 => Ok(Instr::I64x2Bitmask),
        199 => Ok(Instr::I64x2ExtendLowI32x4S),
        200 => Ok(Instr::I64x2ExtendHighI32x4S),
        201 => Ok(Instr::I64x2ExtendLowI32x4U),
        202 => Ok(Instr::I64x2ExtendHighI32x4U),
        220 => Ok(Instr::I64x2ExtMulLowI32x4S),
        221 => Ok(Instr::I64x2ExtMulHighI32x4S),
        222 => Ok(Instr::I64x2ExtMulLowI32x4U),
        223 => Ok(Instr::I64x2ExtMulHighI32x4U),
        84 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Load8Lane { memarg, lane })
        }
        85 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Load16Lane { memarg, lane })
        }
        86 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Load32Lane { memarg, lane })
        }
        87 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Load64Lane { memarg, lane })
        }
        88 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Store8Lane { memarg, lane })
        }
        89 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Store16Lane { memarg, lane })
        }
        90 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Store32Lane { memarg, lane })
        }
        91 => {
            let (memarg, lane) = parse_memarg_lane(cursor, base_offset)?;
            Ok(Instr::V128Store64Lane { memarg, lane })
        }
        92 => Ok(Instr::V128Load32Zero(parse_memarg(cursor, base_offset)?)),
        93 => Ok(Instr::V128Load64Zero(parse_memarg(cursor, base_offset)?)),
        35 => Ok(Instr::I8x16Eq),
        36 => Ok(Instr::I8x16Ne),
        37 => Ok(Instr::I8x16LtS),
        38 => Ok(Instr::I8x16LtU),
        39 => Ok(Instr::I8x16GtS),
        40 => Ok(Instr::I8x16GtU),
        41 => Ok(Instr::I8x16LeS),
        42 => Ok(Instr::I8x16LeU),
        43 => Ok(Instr::I8x16GeS),
        44 => Ok(Instr::I8x16GeU),
        45 => Ok(Instr::I16x8Eq),
        46 => Ok(Instr::I16x8Ne),
        47 => Ok(Instr::I16x8LtS),
        48 => Ok(Instr::I16x8LtU),
        49 => Ok(Instr::I16x8GtS),
        50 => Ok(Instr::I16x8GtU),
        51 => Ok(Instr::I16x8LeS),
        52 => Ok(Instr::I16x8LeU),
        53 => Ok(Instr::I16x8GeS),
        54 => Ok(Instr::I16x8GeU),
        55 => Ok(Instr::I32x4Eq),
        56 => Ok(Instr::I32x4Ne),
        57 => Ok(Instr::I32x4LtS),
        58 => Ok(Instr::I32x4LtU),
        59 => Ok(Instr::I32x4GtS),
        60 => Ok(Instr::I32x4GtU),
        61 => Ok(Instr::I32x4LeS),
        62 => Ok(Instr::I32x4LeU),
        63 => Ok(Instr::I32x4GeS),
        64 => Ok(Instr::I32x4GeU),
        65 => Ok(Instr::F32x4Eq),
        66 => Ok(Instr::F32x4Ne),
        67 => Ok(Instr::F32x4Lt),
        68 => Ok(Instr::F32x4Gt),
        69 => Ok(Instr::F32x4Le),
        70 => Ok(Instr::F32x4Ge),
        71 => Ok(Instr::F64x2Eq),
        72 => Ok(Instr::F64x2Ne),
        73 => Ok(Instr::F64x2Lt),
        74 => Ok(Instr::F64x2Gt),
        75 => Ok(Instr::F64x2Le),
        76 => Ok(Instr::F64x2Ge),
        107 => Ok(Instr::I8x16Shl),
        108 => Ok(Instr::I8x16ShrS),
        109 => Ok(Instr::I8x16ShrU),
        139 => Ok(Instr::I16x8Shl),
        140 => Ok(Instr::I16x8ShrS),
        141 => Ok(Instr::I16x8ShrU),
        171 => Ok(Instr::I32x4Shl),
        172 => Ok(Instr::I32x4ShrS),
        173 => Ok(Instr::I32x4ShrU),
        203 => Ok(Instr::I64x2Shl),
        204 => Ok(Instr::I64x2ShrS),
        205 => Ok(Instr::I64x2ShrU),
        214 => Ok(Instr::I64x2Eq),
        215 => Ok(Instr::I64x2Ne),
        216 => Ok(Instr::I64x2LtS),
        217 => Ok(Instr::I64x2GtS),
        218 => Ok(Instr::I64x2LeS),
        219 => Ok(Instr::I64x2GeS),
        13 => {
            let mut lanes = [0u8; 16];
            for lane in lanes.iter_mut() {
                *lane = parse_lane_idx(cursor, base_offset)?;
            }
            Ok(Instr::I8x16Shuffle(lanes))
        }
        14 => Ok(Instr::I8x16Swizzle),
        94 => Ok(Instr::F32x4DemoteF64x2Zero),
        95 => Ok(Instr::F64x2PromoteLowF32x4),
        103 => Ok(Instr::F32x4Ceil),
        104 => Ok(Instr::F32x4Floor),
        105 => Ok(Instr::F32x4Trunc),
        106 => Ok(Instr::F32x4Nearest),
        116 => Ok(Instr::F64x2Ceil),
        117 => Ok(Instr::F64x2Floor),
        122 => Ok(Instr::F64x2Trunc),
        148 => Ok(Instr::F64x2Nearest),
        224 => Ok(Instr::F32x4Abs),
        225 => Ok(Instr::F32x4Neg),
        227 => Ok(Instr::F32x4Sqrt),
        232 => Ok(Instr::F32x4Min),
        233 => Ok(Instr::F32x4Max),
        234 => Ok(Instr::F32x4PMin),
        235 => Ok(Instr::F32x4PMax),
        236 => Ok(Instr::F64x2Abs),
        237 => Ok(Instr::F64x2Neg),
        239 => Ok(Instr::F64x2Sqrt),
        244 => Ok(Instr::F64x2Min),
        245 => Ok(Instr::F64x2Max),
        246 => Ok(Instr::F64x2PMin),
        247 => Ok(Instr::F64x2PMax),
        248 => Ok(Instr::I32x4TruncSatF32x4S),
        249 => Ok(Instr::I32x4TruncSatF32x4U),
        250 => Ok(Instr::F32x4ConvertI32x4S),
        251 => Ok(Instr::F32x4ConvertI32x4U),
        252 => Ok(Instr::I32x4TruncSatF64x2SZero),
        253 => Ok(Instr::I32x4TruncSatF64x2UZero),
        254 => Ok(Instr::F64x2ConvertLowI32x4S),
        255 => Ok(Instr::F64x2ConvertLowI32x4U),
        _ => Err(DecodeError {
            offset: ByteOffset(base_offset),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnknownSimdOpcode { opcode },
        }),
    }
}

fn parse_memarg(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<MemArg, DecodeError> {
    let flags = decode_u32(cursor, base_offset)?;
    let (align, memory) = if flags & (1 << 6) != 0 {
        (flags ^ (1 << 6), MemIdx(decode_u32(cursor, base_offset)?))
    } else {
        (flags, MemIdx(0))
    };
    let offset = decode_u64(cursor, base_offset)?;
    Ok(MemArg {
        align,
        offset,
        memory,
    })
}

fn parse_memarg_lane(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<(MemArg, u8), DecodeError> {
    let memarg = parse_memarg(cursor, base_offset)?;
    let pos = cursor.position();
    let lane = cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + pos),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    Ok((memarg, lane))
}

fn parse_lane_idx(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u8, DecodeError> {
    let pos = cursor.position();
    cursor.read_byte().map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + pos),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })
}

fn parse_v128_const(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<[u8; 16], DecodeError> {
    let pos = cursor.position();
    let bytes = cursor.read_bytes(16).map_err(|_| DecodeError {
        offset: ByteOffset(base_offset + pos),
        context: DecodeContext::CodeSection,
        kind: DecodeErrorKind::UnexpectedEof,
    })?;
    Ok(bytes.try_into().expect("16 bytes"))
}

fn parse_mem_idx(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<MemIdx, DecodeError> {
    Ok(MemIdx(decode_u32(cursor, base_offset)?))
}

fn parse_table_idx(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TableIdx, DecodeError> {
    Ok(TableIdx(decode_u32(cursor, base_offset)?))
}

fn parse_result_types(
    cursor: &mut Cursor<'_>,
    base_offset: usize,
) -> Result<Vec<ValType>, DecodeError> {
    let count = decode_u32(cursor, base_offset)?;
    let mut types = Vec::with_capacity(cursor.capacity_hint(count));
    for _ in 0..count {
        types.push(parse_binary_val_type(
            cursor,
            base_offset,
            DecodeContext::CodeSection,
        )?);
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

    if matches!(first, 0x7B..=0x7F | 0x70 | 0x6F | 0x63 | 0x64 | 0x69) {
        return Ok(BlockType::Val(parse_val_type_with_first_byte(
            cursor,
            first,
            base_offset + start,
            DecodeContext::CodeSection,
        )?));
    }

    let type_idx = decode_block_type_idx(cursor, first, base_offset + start)?;
    Ok(BlockType::TypeIdx(type_idx))
}

fn parse_tag_idx(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TagIdx, DecodeError> {
    let pos = cursor.position();
    let idx = leb128::decode_u32(cursor).map_err(|mut e| {
        e.offset = ByteOffset(base_offset + pos);
        e
    })?;
    Ok(TagIdx(idx))
}

fn parse_try_table(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<TryTable, DecodeError> {
    let block_type = parse_block_type(cursor, base_offset)?;
    let pos = cursor.position();
    let count = leb128::decode_u32(cursor).map_err(|mut e| {
        e.offset = ByteOffset(base_offset + pos);
        e
    })?;
    let mut catches = Vec::new();
    for _ in 0..count {
        let clause_pos = cursor.position();
        let kind_byte = cursor.read_byte().map_err(|_| DecodeError {
            offset: ByteOffset(base_offset + clause_pos),
            context: DecodeContext::CodeSection,
            kind: DecodeErrorKind::UnexpectedEof,
        })?;
        let (kind, has_tag) = match kind_byte {
            0x00 => (TryTableCatchKind::Catch, true),
            0x01 => (TryTableCatchKind::CatchRef, true),
            0x02 => (TryTableCatchKind::CatchAll, false),
            0x03 => (TryTableCatchKind::CatchAllRef, false),
            byte => {
                return Err(DecodeError {
                    offset: ByteOffset(base_offset + clause_pos),
                    context: DecodeContext::CodeSection,
                    kind: DecodeErrorKind::UnknownOpcode { byte },
                });
            }
        };
        let tag = if has_tag {
            Some(parse_tag_idx(cursor, base_offset)?)
        } else {
            None
        };
        let label_pos = cursor.position();
        let label = leb128::decode_u32(cursor).map_err(|mut e| {
            e.offset = ByteOffset(base_offset + label_pos);
            e
        })?;
        catches.push(TryTableCatch { kind, tag, label });
    }
    Ok(TryTable {
        block_type,
        catches,
    })
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

fn decode_u64(cursor: &mut Cursor<'_>, base_offset: usize) -> Result<u64, DecodeError> {
    leb128::decode_u64(cursor).map_err(|mut e| {
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
        let instrs = decode_instr_sequence(
            &[
                0x23, 0x00, 0x24, 0x01, 0x28, 0x02, 0x00, 0x2C, 0x00, 0x01, 0x35, 0x02, 0x02, 0x36,
                0x02, 0x04, 0x3A, 0x00, 0x08, 0x3E, 0x02, 0x0C, 0x3F, 0x00, 0x40, 0x00, 0x0B,
            ],
            240,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::GlobalGet(GlobalIdx(0)),
                Instr::GlobalSet(GlobalIdx(1)),
                Instr::I32Load(MemArg {
                    align: 2,
                    offset: 0,
                    memory: MemIdx(0),
                }),
                Instr::I32Load8S(MemArg {
                    align: 0,
                    offset: 1,
                    memory: MemIdx(0),
                }),
                Instr::I64Load32U(MemArg {
                    align: 2,
                    offset: 2,
                    memory: MemIdx(0),
                }),
                Instr::I32Store(MemArg {
                    align: 2,
                    offset: 4,
                    memory: MemIdx(0),
                }),
                Instr::I32Store8(MemArg {
                    align: 0,
                    offset: 8,
                    memory: MemIdx(0),
                }),
                Instr::I64Store32(MemArg {
                    align: 2,
                    offset: 12,
                    memory: MemIdx(0),
                }),
                Instr::MemorySize(MemIdx(0)),
                Instr::MemoryGrow(MemIdx(0)),
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_all_scalar_memory_opcodes() {
        let instrs = decode_instr_sequence(
            &[
                0x28, 0x02, 0x00, 0x29, 0x03, 0x00, 0x2A, 0x02, 0x00, 0x2B, 0x03, 0x00, 0x2C, 0x00,
                0x00, 0x2D, 0x00, 0x00, 0x2E, 0x01, 0x00, 0x2F, 0x01, 0x00, 0x30, 0x00, 0x00, 0x31,
                0x00, 0x00, 0x32, 0x01, 0x00, 0x33, 0x01, 0x00, 0x34, 0x02, 0x00, 0x35, 0x02, 0x00,
                0x36, 0x02, 0x00, 0x37, 0x03, 0x00, 0x38, 0x02, 0x00, 0x39, 0x03, 0x00, 0x3A, 0x00,
                0x00, 0x3B, 0x01, 0x00, 0x3C, 0x00, 0x00, 0x3D, 0x01, 0x00, 0x3E, 0x02, 0x00, 0x0B,
            ],
            280,
        )
        .unwrap();

        assert_eq!(instrs.len(), 24);
        assert!(matches!(instrs[0], Instr::I32Load(_)));
        assert!(matches!(instrs[1], Instr::I64Load(_)));
        assert!(matches!(instrs[2], Instr::F32Load(_)));
        assert!(matches!(instrs[3], Instr::F64Load(_)));
        assert!(matches!(instrs[4], Instr::I32Load8S(_)));
        assert!(matches!(instrs[5], Instr::I32Load8U(_)));
        assert!(matches!(instrs[6], Instr::I32Load16S(_)));
        assert!(matches!(instrs[7], Instr::I32Load16U(_)));
        assert!(matches!(instrs[8], Instr::I64Load8S(_)));
        assert!(matches!(instrs[9], Instr::I64Load8U(_)));
        assert!(matches!(instrs[10], Instr::I64Load16S(_)));
        assert!(matches!(instrs[11], Instr::I64Load16U(_)));
        assert!(matches!(instrs[12], Instr::I64Load32S(_)));
        assert!(matches!(instrs[13], Instr::I64Load32U(_)));
        assert!(matches!(instrs[14], Instr::I32Store(_)));
        assert!(matches!(instrs[15], Instr::I64Store(_)));
        assert!(matches!(instrs[16], Instr::F32Store(_)));
        assert!(matches!(instrs[17], Instr::F64Store(_)));
        assert!(matches!(instrs[18], Instr::I32Store8(_)));
        assert!(matches!(instrs[19], Instr::I32Store16(_)));
        assert!(matches!(instrs[20], Instr::I64Store8(_)));
        assert!(matches!(instrs[21], Instr::I64Store16(_)));
        assert!(matches!(instrs[22], Instr::I64Store32(_)));
        assert!(matches!(instrs[23], Instr::End));
    }

    #[test]
    fn decode_ref_instructions() {
        let instrs = decode_instr_sequence(
            &[
                0xD0, 0x70, 0xD1, 0xD2, 0x00, 0xD4, 0xD5, 0x01, 0xD6, 0x02, 0x0B,
            ],
            90,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::RefNull(RefType::FuncRef),
                Instr::RefIsNull,
                Instr::RefFunc(FuncIdx(0)),
                Instr::RefAsNonNull,
                Instr::BrOnNull(LabelIdx(1)),
                Instr::BrOnNonNull(LabelIdx(2)),
                Instr::End
            ]
        );
    }

    #[test]
    fn decode_typed_ref_null_instruction() {
        let instrs = decode_instr_sequence(&[0xD0, 0x00, 0x0B], 92).unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::RefNull(RefType::concrete(true, TypeIdx(0))),
                Instr::End
            ]
        );
    }

    #[test]
    fn decode_call_indirect_and_table_ops() {
        let instrs = decode_instr_sequence(
            &[
                0x12, 0x00, 0x14, 0x03, 0x15, 0x04, 0x11, 0x01, 0x00, 0x13, 0x02, 0x00, 0x25, 0x00,
                0x26, 0x00, 0x0B,
            ],
            95,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::ReturnCall(FuncIdx(0)),
                Instr::CallRef(TypeIdx(3)),
                Instr::ReturnCallRef(TypeIdx(4)),
                Instr::CallIndirect {
                    type_idx: TypeIdx(1),
                    table_idx: TableIdx(0),
                },
                Instr::ReturnCallIndirect {
                    type_idx: TypeIdx(2),
                    table_idx: TableIdx(0),
                },
                Instr::TableGet(TableIdx(0)),
                Instr::TableSet(TableIdx(0)),
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_broader_numeric_ops() {
        let instrs = decode_instr_sequence(
            &[
                0x50, 0x51, 0x5B, 0x61, 0x67, 0x6B, 0x76, 0x79, 0x7D, 0x88, 0x8B, 0x92, 0x98, 0x99,
                0xA0, 0xA6, 0x0B,
            ],
            140,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::I64Eqz,
                Instr::I64Eq,
                Instr::F32Eq,
                Instr::F64Eq,
                Instr::I32Clz,
                Instr::I32Sub,
                Instr::I32ShrU,
                Instr::I64Clz,
                Instr::I64Sub,
                Instr::I64ShrU,
                Instr::F32Abs,
                Instr::F32Add,
                Instr::F32Copysign,
                Instr::F64Abs,
                Instr::F64Add,
                Instr::F64Copysign,
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_conversions_and_reinterpretations() {
        let instrs = decode_instr_sequence(
            &[
                0xA7, 0xA8, 0xAB, 0xAC, 0xB1, 0xB2, 0xB5, 0xB6, 0xB7, 0xBA, 0xBB, 0xBC, 0xBD, 0xBE,
                0xBF, 0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xFC, 0x00, 0xFC, 0x03, 0xFC, 0x04, 0xFC, 0x07,
                0x0B,
            ],
            180,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::I32WrapI64,
                Instr::I32TruncF32S,
                Instr::I32TruncF64U,
                Instr::I64ExtendI32S,
                Instr::I64TruncF64U,
                Instr::F32ConvertI32S,
                Instr::F32ConvertI64U,
                Instr::F32DemoteF64,
                Instr::F64ConvertI32S,
                Instr::F64ConvertI64U,
                Instr::F64PromoteF32,
                Instr::I32ReinterpretF32,
                Instr::I64ReinterpretF64,
                Instr::F32ReinterpretI32,
                Instr::F64ReinterpretI64,
                Instr::I32Extend8S,
                Instr::I32Extend16S,
                Instr::I64Extend8S,
                Instr::I64Extend16S,
                Instr::I64Extend32S,
                Instr::I32TruncSatF32S,
                Instr::I32TruncSatF64U,
                Instr::I64TruncSatF32S,
                Instr::I64TruncSatF64U,
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_bulk_memory_ops() {
        let instrs = decode_instr_sequence(
            &[
                0xFC, 0x08, 0x00, 0x00, 0xFC, 0x09, 0x00, 0xFC, 0x0A, 0x00, 0x00, 0xFC, 0x0B, 0x00,
                0xFC, 0x0C, 0x00, 0x00, 0xFC, 0x0D, 0x00, 0xFC, 0x0E, 0x00, 0x00, 0xFC, 0x0F, 0x00,
                0xFC, 0x10, 0x00, 0xFC, 0x11, 0x00, 0x0B,
            ],
            315,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::MemoryInit(crate::types::DataIdx(0), MemIdx(0)),
                Instr::DataDrop(crate::types::DataIdx(0)),
                Instr::MemoryCopy {
                    dst: MemIdx(0),
                    src: MemIdx(0)
                },
                Instr::MemoryFill(MemIdx(0)),
                Instr::TableInit {
                    elem_idx: ElemIdx(0),
                    table_idx: TableIdx(0),
                },
                Instr::ElemDrop(ElemIdx(0)),
                Instr::TableCopy {
                    dst: TableIdx(0),
                    src: TableIdx(0),
                },
                Instr::TableGrow(TableIdx(0)),
                Instr::TableSize(TableIdx(0)),
                Instr::TableFill(TableIdx(0)),
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_nonzero_memory_indices() {
        let instrs = decode_instr_sequence(
            &[
                0x3F, 0x01, 0x40, 0x02, 0xFC, 0x08, 0x00, 0x03, 0xFC, 0x0A, 0x01, 0x02, 0xFC, 0x0B,
                0x04, 0x0B,
            ],
            402,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::MemorySize(MemIdx(1)),
                Instr::MemoryGrow(MemIdx(2)),
                Instr::MemoryInit(crate::types::DataIdx(0), MemIdx(3)),
                Instr::MemoryCopy {
                    dst: MemIdx(1),
                    src: MemIdx(2),
                },
                Instr::MemoryFill(MemIdx(4)),
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_nonzero_memargs() {
        let instrs = decode_instr_sequence(
            &[
                0x28, 0x42, 0x01, 0x05, 0x36, 0x42, 0x02, 0x00, 0xFD, 0x00, 0x44, 0x03, 0x00, 0xFD,
                0x54, 0x40, 0x04, 0x01, 0x07, 0x0B,
            ],
            427,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::I32Load(MemArg {
                    align: 2,
                    offset: 5,
                    memory: MemIdx(1),
                }),
                Instr::I32Store(MemArg {
                    align: 2,
                    offset: 0,
                    memory: MemIdx(2),
                }),
                Instr::V128Load(MemArg {
                    align: 4,
                    offset: 0,
                    memory: MemIdx(3),
                }),
                Instr::V128Load8Lane {
                    memarg: MemArg {
                        align: 0,
                        offset: 1,
                        memory: MemIdx(4),
                    },
                    lane: 7,
                },
                Instr::End,
            ]
        );
    }

    #[test]
    fn decode_vector_memory_ops() {
        let instrs = decode_instr_sequence(
            &[
                0xFD, 0x00, 0x04, 0x00, 0xFD, 0x0B, 0x04, 0x00, 0xFD, 0x54, 0x00, 0x00, 0x0F, 0xFD,
                0x58, 0x00, 0x00, 0x0F, 0xFD, 0x5C, 0x02, 0x00, 0xFD, 0x0C, 0x00, 0x01, 0x02, 0x03,
                0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x0B,
            ],
            320,
        )
        .unwrap();
        assert_eq!(
            instrs,
            vec![
                Instr::V128Load(MemArg {
                    align: 4,
                    offset: 0,
                    memory: MemIdx(0),
                }),
                Instr::V128Store(MemArg {
                    align: 4,
                    offset: 0,
                    memory: MemIdx(0),
                }),
                Instr::V128Load8Lane {
                    memarg: MemArg {
                        align: 0,
                        offset: 0,
                        memory: MemIdx(0),
                    },
                    lane: 15,
                },
                Instr::V128Store8Lane {
                    memarg: MemArg {
                        align: 0,
                        offset: 0,
                        memory: MemIdx(0),
                    },
                    lane: 15,
                },
                Instr::V128Load32Zero(MemArg {
                    align: 2,
                    offset: 0,
                    memory: MemIdx(0),
                }),
                Instr::V128Const([
                    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
                    0x0D, 0x0E, 0x0F,
                ]),
                Instr::End,
            ]
        );
    }

    #[test]
    fn reject_unknown_simd_opcode() {
        let err = decode_instr_sequence(&[0xFD, 0xE2, 0x01, 0x0B], 410).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnknownSimdOpcode { opcode: 226 });
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
