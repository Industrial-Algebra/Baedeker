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
                // For 5th byte at shift=28, only bits 0-3 carry data.
                // Bits 4-6 must be sign-extended from bit 3.
                let sign_and_unused = byte & 0x70;
                if sign_and_unused != 0 && sign_and_unused != 0x70 {
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
                // 10th byte at shift=63: only bit 0 carries data, bit 6 is sign.
                let sign_and_unused = byte & 0x7E;
                if sign_and_unused != 0 && sign_and_unused != 0x7E {
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
