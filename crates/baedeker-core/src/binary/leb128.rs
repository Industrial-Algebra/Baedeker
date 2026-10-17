// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! LEB128 (Little Endian Base 128) encoder/decoder.
//!
//! WASM uses LEB128 extensively for compact integer encoding in the binary format.
//! See [Spec §5.2.2](https://webassembly.github.io/spec/core/binary/values.html#integers).

use crate::error::{ByteOffset, DecodeContext, DecodeError, DecodeErrorKind};

/// A cursor over a byte slice, tracking the current read position.
#[derive(Debug)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Create a new cursor at position 0.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Current byte offset.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Remaining bytes.
    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// Pre-allocation capacity for `count` entries yet to be parsed.
    ///
    /// Malformed binaries can declare counts in the billions; blindly
    /// trusting them in `Vec::with_capacity` turns a tiny input into a
    /// multi-gigabyte allocation. Every entry needs at least one byte in
    /// the stream, so the remaining input length is a sound upper bound.
    pub fn capacity_hint(&self, count: u32) -> usize {
        (count as usize).min(self.remaining().len())
    }

    /// Original backing slice.
    pub fn original(&self) -> &'a [u8] {
        self.data
    }

    /// Whether we've consumed all input.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Read a single byte, advancing the cursor.
    pub fn read_byte(&mut self) -> Result<u8, DecodeError> {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            Ok(b)
        } else {
            Err(DecodeError {
                offset: ByteOffset(self.pos),
                context: DecodeContext::Leb128,
                kind: DecodeErrorKind::UnexpectedEof,
            })
        }
    }

    /// Read exactly `n` bytes as a slice, advancing the cursor.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.pos + n <= self.data.len() {
            let slice = &self.data[self.pos..self.pos + n];
            self.pos += n;
            Ok(slice)
        } else {
            Err(DecodeError {
                offset: ByteOffset(self.pos),
                context: DecodeContext::Leb128,
                kind: DecodeErrorKind::UnexpectedEof,
            })
        }
    }

    /// Advance the cursor by `n` bytes.
    pub fn advance(&mut self, n: usize) -> Result<(), DecodeError> {
        if self.pos + n <= self.data.len() {
            self.pos += n;
            Ok(())
        } else {
            Err(DecodeError {
                offset: ByteOffset(self.pos),
                context: DecodeContext::Leb128,
                kind: DecodeErrorKind::UnexpectedEof,
            })
        }
    }
}

/// Decode an unsigned LEB128-encoded u32.
///
/// The encoding uses at most 5 bytes. The final byte's unused high bits
/// must be zero (no overlong encodings).
/// See [Spec §5.2.2](https://webassembly.github.io/spec/core/binary/values.html#integers).
pub fn decode_u32(cursor: &mut Cursor<'_>) -> Result<u32, DecodeError> {
    let start = cursor.position();
    let mut result: u32 = 0;
    let mut shift: u32 = 0;

    for i in 0..5 {
        let byte = cursor.read_byte().map_err(|mut e| {
            e.context = DecodeContext::Leb128;
            e.offset = ByteOffset(start);
            e
        })?;

        let low_bits = u32::from(byte & 0x7F);

        // Check for overflow on the 5th byte (shift=28): only 4 low bits are valid.
        if i == 4 && (byte & 0xF0) != 0 {
            return Err(DecodeError {
                offset: ByteOffset(start),
                context: DecodeContext::Leb128,
                kind: DecodeErrorKind::Leb128Overflow,
            });
        }

        result |= low_bits << shift;

        if byte & 0x80 == 0 {
            return Ok(result);
        }

        shift += 7;
    }

    Err(DecodeError {
        offset: ByteOffset(start),
        context: DecodeContext::Leb128,
        kind: DecodeErrorKind::Leb128TooLong,
    })
}

/// Decode an unsigned LEB128-encoded u64.
///
/// The encoding uses at most 10 bytes.
pub fn decode_u64(cursor: &mut Cursor<'_>) -> Result<u64, DecodeError> {
    let start = cursor.position();
    let mut result: u64 = 0;
    let mut shift: u32 = 0;

    for i in 0..10 {
        let byte = cursor.read_byte().map_err(|mut e| {
            e.context = DecodeContext::Leb128;
            e.offset = ByteOffset(start);
            e
        })?;

        let low_bits = u64::from(byte & 0x7F);

        // 10th byte (shift=63): only 1 low bit is valid.
        if i == 9 && (byte & 0xFE) != 0 {
            return Err(DecodeError {
                offset: ByteOffset(start),
                context: DecodeContext::Leb128,
                kind: DecodeErrorKind::Leb128Overflow,
            });
        }

        result |= low_bits << shift;

        if byte & 0x80 == 0 {
            return Ok(result);
        }

        shift += 7;
    }

    Err(DecodeError {
        offset: ByteOffset(start),
        context: DecodeContext::Leb128,
        kind: DecodeErrorKind::Leb128TooLong,
    })
}

/// Decode a signed LEB128-encoded i32.
///
/// The encoding uses at most 5 bytes. Sign extension is applied based on
/// the sign bit of the final byte.
pub fn decode_i32(cursor: &mut Cursor<'_>) -> Result<i32, DecodeError> {
    let start = cursor.position();
    let mut result: i32 = 0;
    let mut shift: u32 = 0;

    for i in 0..5 {
        let byte = cursor.read_byte().map_err(|mut e| {
            e.context = DecodeContext::Leb128;
            e.offset = ByteOffset(start);
            e
        })?;

        let low_bits = i32::from(byte & 0x7F);
        result |= low_bits << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            // On the final byte, check that the unused high bits are consistent
            // with the sign bit (either all 0s or all 1s).
            if i == 4 {
                // 5th byte at shift=28: bits 0-3 carry data (bit 3 = the
                // i32 sign bit); bits 4-6 must sign-extend bit 3 exactly.
                let sign = byte & 0x08;
                let extension = byte & 0x70;
                if (sign == 0 && extension != 0) || (sign != 0 && extension != 0x70) {
                    return Err(DecodeError {
                        offset: ByteOffset(start),
                        context: DecodeContext::Leb128,
                        kind: DecodeErrorKind::Leb128Overflow,
                    });
                }
            } else if shift < 32 && (byte & 0x40) != 0 {
                // Sign-extend negative values.
                result |= !0 << shift;
            }
            return Ok(result);
        }
    }

    Err(DecodeError {
        offset: ByteOffset(start),
        context: DecodeContext::Leb128,
        kind: DecodeErrorKind::Leb128TooLong,
    })
}

/// Decode a signed LEB128-encoded `s33` value (concrete heap-type indices,
/// block types). The encoding uses at most 5 bytes; the 5th byte carries
/// value bits 28-31 and the sign bit at value bit 32, so its bits 5-6 must
/// sign-extend bit 4 exactly. Returns `i64` so the full `[-2^32, 2^32-1]`
/// range is representable.
///
/// This is the raw 33-bit value; the pinned concrete-heap-index reader wraps
/// it to `i32` (see [`decode_s33_wrapped`]).
pub fn decode_s33(cursor: &mut Cursor<'_>) -> Result<i64, DecodeError> {
    let start = cursor.position();
    let mut result: i64 = 0;
    let mut shift: u32 = 0;

    for i in 0..5 {
        let byte = cursor.read_byte().map_err(|mut e| {
            e.context = DecodeContext::Leb128;
            e.offset = ByteOffset(start);
            e
        })?;

        let low_bits = i64::from(byte & 0x7F);
        result |= low_bits << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            if i == 4 {
                // 5th byte at shift=28: bits 0-3 carry value bits 28-31,
                // bit 4 is the sign bit (value bit 32); bits 5-6 must
                // sign-extend it exactly.
                let sign = byte & 0x10;
                let extension = byte & 0x60;
                if (sign == 0 && extension != 0) || (sign != 0 && extension != 0x60) {
                    return Err(DecodeError {
                        offset: ByteOffset(start),
                        context: DecodeContext::Leb128,
                        kind: DecodeErrorKind::Leb128Overflow,
                    });
                }
                if sign != 0 {
                    result |= !0i64 << shift;
                }
            } else if (byte & 0x40) != 0 {
                result |= !0i64 << shift;
            }
            return Ok(result);
        }
    }

    Err(DecodeError {
        offset: ByteOffset(start),
        context: DecodeContext::Leb128,
        kind: DecodeErrorKind::Leb128TooLong,
    })
}

/// Decode a concrete heap-type index exactly as the pinned reference does:
/// read an `s33` (see [`decode_s33`]), then apply
/// `Convert.I32_.wrap_i64` so the 33-bit signed value is narrowed to `i32`
/// (`interpreter/binary/decode.ml`). A **negative** wrapped value is a
/// malformed type index; a nonnegative one is the index. Callers are
/// responsible for rejecting the negative case.
pub fn decode_s33_wrapped(cursor: &mut Cursor<'_>) -> Result<i32, DecodeError> {
    decode_s33(cursor).map(|value| value as i32)
}

/// Decode a signed LEB128-encoded i64.
///
/// The encoding uses at most 10 bytes.
pub fn decode_i64(cursor: &mut Cursor<'_>) -> Result<i64, DecodeError> {
    let start = cursor.position();
    let mut result: i64 = 0;
    let mut shift: u32 = 0;

    for i in 0..10 {
        let byte = cursor.read_byte().map_err(|mut e| {
            e.context = DecodeContext::Leb128;
            e.offset = ByteOffset(start);
            e
        })?;

        let low_bits = i64::from(byte & 0x7F);
        result |= low_bits << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            if i == 9 {
                // 10th byte at shift=63: bit 0 carries data (the i64 sign
                // bit); bits 1-6 must sign-extend bit 0 exactly.
                let sign = byte & 0x01;
                let extension = byte & 0x7E;
                if (sign == 0 && extension != 0) || (sign != 0 && extension != 0x7E) {
                    return Err(DecodeError {
                        offset: ByteOffset(start),
                        context: DecodeContext::Leb128,
                        kind: DecodeErrorKind::Leb128Overflow,
                    });
                }
            } else if shift < 64 && (byte & 0x40) != 0 {
                result |= !0i64 << shift;
            }
            return Ok(result);
        }
    }

    Err(DecodeError {
        offset: ByteOffset(start),
        context: DecodeContext::Leb128,
        kind: DecodeErrorKind::Leb128TooLong,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── decode_u32 ──────────────────────────────────────────

    #[test]
    fn u32_zero() {
        let mut c = Cursor::new(&[0x00]);
        assert_eq!(decode_u32(&mut c).unwrap(), 0);
    }

    #[test]
    fn u32_single_byte() {
        let mut c = Cursor::new(&[0x08]);
        assert_eq!(decode_u32(&mut c).unwrap(), 8);
    }

    #[test]
    fn u32_max_single_byte() {
        // 127 = 0x7F
        let mut c = Cursor::new(&[0x7F]);
        assert_eq!(decode_u32(&mut c).unwrap(), 127);
    }

    #[test]
    fn u32_two_bytes() {
        // 128 = 0x80 0x01
        let mut c = Cursor::new(&[0x80, 0x01]);
        assert_eq!(decode_u32(&mut c).unwrap(), 128);
    }

    #[test]
    fn u32_624485() {
        // Classic LEB128 test value: 624485 = 0xE5 0x8E 0x26
        let mut c = Cursor::new(&[0xE5, 0x8E, 0x26]);
        assert_eq!(decode_u32(&mut c).unwrap(), 624485);
    }

    #[test]
    fn u32_max_value() {
        // u32::MAX = 4294967295 = 0xFF 0xFF 0xFF 0xFF 0x0F
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert_eq!(decode_u32(&mut c).unwrap(), u32::MAX);
    }

    #[test]
    fn u32_overflow_fifth_byte() {
        // 5th byte has bit 4 set → overflow
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x1F]);
        let err = decode_u32(&mut c).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::Leb128Overflow);
    }

    #[test]
    fn u32_too_long() {
        // 6 continuation bytes
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00]);
        let err = decode_u32(&mut c).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::Leb128Overflow);
    }

    #[test]
    fn u32_unexpected_eof() {
        let mut c = Cursor::new(&[0x80]);
        let err = decode_u32(&mut c).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::UnexpectedEof);
    }

    // ── decode_i32 ──────────────────────────────────────────

    #[test]
    fn i32_zero() {
        let mut c = Cursor::new(&[0x00]);
        assert_eq!(decode_i32(&mut c).unwrap(), 0);
    }

    #[test]
    fn i32_positive() {
        let mut c = Cursor::new(&[0x08]);
        assert_eq!(decode_i32(&mut c).unwrap(), 8);
    }

    #[test]
    fn i32_negative_one() {
        // -1 = 0x7F (sign bit set, single byte)
        let mut c = Cursor::new(&[0x7F]);
        assert_eq!(decode_i32(&mut c).unwrap(), -1);
    }

    #[test]
    fn i32_negative_two() {
        // -2 = 0x7E
        let mut c = Cursor::new(&[0x7E]);
        assert_eq!(decode_i32(&mut c).unwrap(), -2);
    }

    #[test]
    fn i32_negative_128() {
        // -128 = 0x80 0x7F
        let mut c = Cursor::new(&[0x80, 0x7F]);
        assert_eq!(decode_i32(&mut c).unwrap(), -128);
    }

    #[test]
    fn i32_min_value() {
        // i32::MIN = -2147483648 = 0x80 0x80 0x80 0x80 0x78
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x78]);
        assert_eq!(decode_i32(&mut c).unwrap(), i32::MIN);
    }

    #[test]
    fn i32_max_value() {
        // i32::MAX = 2147483647 = 0xFF 0xFF 0xFF 0xFF 0x07
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
        assert_eq!(decode_i32(&mut c).unwrap(), i32::MAX);
    }

    // ── decode_s33 ──────────────────────────────────────

    #[test]
    fn s33_max_u32() {
        // u32::MAX = 4294967295 = 0xFF 0xFF 0xFF 0xFF 0x0F
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert_eq!(decode_s33(&mut c).unwrap(), 0xFFFF_FFFF);
    }

    #[test]
    fn s33_2_pow_31() {
        // 2147483648 = 0x80 0x80 0x80 0x80 0x08
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x08]);
        assert_eq!(decode_s33(&mut c).unwrap(), 0x8000_0000);
    }

    #[test]
    fn s33_over_u32_max_is_overflow() {
        // 4294967296 = 0x80 0x80 0x80 0x80 0x10 — bit 32 set but the
        // upper bits do not sign-extend, so the 33-bit value overflows.
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x10]);
        let err = decode_s33(&mut c).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::Leb128Overflow);
    }

    #[test]
    fn s33_negative_min() {
        // -2^32 = 0x80 0x80 0x80 0x80 0x70
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x70]);
        assert_eq!(decode_s33(&mut c).unwrap(), -(1i64 << 32));
    }

    /// The pinned concrete-index reader wraps the 33-bit value to `i32`
    /// (`Convert.I32_.wrap_i64 (sN 33 s)`). These are the exact classifications
    /// from review r3b's table.
    #[test]
    fn s33_wrap_matches_pin() {
        // 0xffffffff encodes +2^32-1, wraps to -1.
        let mut c = Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(decode_s33_wrapped(&mut c).unwrap(), -1);
        // 0x80000000 encodes +2^31, wraps to i32::MIN.
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x08]);
        assert_eq!(decode_s33_wrapped(&mut c).unwrap(), i32::MIN);
        // -2^32 wraps to 0 (an encoding of index 0).
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x70]);
        assert_eq!(decode_s33_wrapped(&mut c).unwrap(), 0);
        // Bits 5-6 must sign-extend bit 4 at n=5: otherwise integer-too-large.
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x10]);
        assert_eq!(
            decode_s33_wrapped(&mut c).unwrap_err().kind,
            DecodeErrorKind::Leb128Overflow
        );
        // A single byte 0x7f is the s33 value -1 (wraps to -1), not an
        // abstract heap type.
        let mut c = Cursor::new(&[0x7f]);
        assert_eq!(decode_s33_wrapped(&mut c).unwrap(), -1);
    }

    // ── decode_u64 ──────────────────────────────────────────

    #[test]
    fn u64_zero() {
        let mut c = Cursor::new(&[0x00]);
        assert_eq!(decode_u64(&mut c).unwrap(), 0);
    }

    #[test]
    fn u64_max_value() {
        // u64::MAX = 0xFF 0xFF 0xFF 0xFF 0xFF 0xFF 0xFF 0xFF 0xFF 0x01
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
        assert_eq!(decode_u64(&mut c).unwrap(), u64::MAX);
    }

    #[test]
    fn u64_overflow() {
        // 10th byte has extra bits set
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x03]);
        let err = decode_u64(&mut c).unwrap_err();
        assert_eq!(err.kind, DecodeErrorKind::Leb128Overflow);
    }

    // ── decode_i64 ──────────────────────────────────────────

    #[test]
    fn i64_negative_one() {
        let mut c = Cursor::new(&[0x7F]);
        assert_eq!(decode_i64(&mut c).unwrap(), -1i64);
    }

    #[test]
    fn i64_min_value() {
        // i64::MIN encoded as signed LEB128
        let mut c = Cursor::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7F]);
        assert_eq!(decode_i64(&mut c).unwrap(), i64::MIN);
    }

    #[test]
    fn i64_max_value() {
        // i64::MAX encoded as signed LEB128
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        assert_eq!(decode_i64(&mut c).unwrap(), i64::MAX);
    }

    // ── cursor ──────────────────────────────────────────────

    #[test]
    fn cursor_tracks_position() {
        let mut c = Cursor::new(&[0x01, 0x02, 0x03]);
        assert_eq!(c.position(), 0);
        c.read_byte().unwrap();
        assert_eq!(c.position(), 1);
        c.read_bytes(2).unwrap();
        assert_eq!(c.position(), 3);
        assert!(c.is_empty());
    }
}
