//! ZGFX (RDP8) Bulk Data Compression

mod api;
mod circular_buffer;
mod compressor;
mod control_messages;
mod wrapper;

use std::io::{self, Write as _};

pub use api::{CompressionMode, compress_and_wrap_egfx, wrap_uncompressed_recorded};
pub use compressor::Compressor;
pub use wrapper::{wrap_compressed, wrap_uncompressed};

use self::circular_buffer::FixedCircularBuffer;
use self::control_messages::{BulkEncodedData, CompressionFlags, SegmentedDataPdu};

/// Sliding window size shared by compressor and decompressor.
pub(crate) const HISTORY_SIZE: usize = 2_500_000;

/// Ceiling on the decompressed output of a single ZGFX segment.
///
/// This is a resource limit, not a wire requirement. A match-copy chain can
/// expand a few bytes of input into unbounded output, so the decompressor needs
/// some ceiling or a small hostile segment can exhaust memory. Left unbounded, a
/// fuzz input reached a 1.5 GB peak.
///
/// MS-RDPEGFX 3.1.9.1.2 ("RDP 8.0 compressor limits") does supply a number: a
/// compliant compressor MUST NOT produce any single segment representing more
/// than 65,535 uncompressed bytes, whether sent as SINGLE or as one part of a
/// MULTIPART stream. This crate's own compressor honors that bound: `wrapper.rs`
/// panics if asked to wrap a compressed segment past it, and
/// `compress_and_wrap_egfx`, the only production caller, falls back to
/// uncompressed rather than risk exceeding it.
///
/// So 65,535 bounds every segment a conforming peer will ever send. 64 MiB is
/// still used here, not that number, because this ceiling exists to catch
/// non-conforming or hostile input, not to police conforming input: a decoder
/// that hard-rejects at exactly the compressor-side limit has no margin for a
/// peer implementation with a minor, benign spec deviation. 64 MiB sits far
/// above any plausible frame while still bounding the damage, so it can only
/// fire on an attack. Same footing as the compositor's total-byte budget: an
/// explicit implementation limit, documented as ours rather than dressed up as
/// a spec requirement.
pub(crate) const MAX_DECOMPRESSED_PER_SEGMENT: usize = 64 * 1024 * 1024;

/// Ceiling on the total decompressed output of a single multipart ZGFX message.
///
/// Each segment is already bounded by [`MAX_DECOMPRESSED_PER_SEGMENT`], but
/// summing enough small segments could otherwise drive the aggregate up to
/// whatever the declared `uncompressedSize` field claims, and that field is a
/// 32-bit value fully controlled by the sender (up to roughly 4.29 GiB). A
/// multipart message built from dozens of small segments, each just under the
/// per-segment ceiling, can exhaust memory long before `uncompressedSize` is
/// reached.
///
/// 256 MiB, same footing as `MAX_COMPOSITOR_BYTES` in ironrdp-egfx's
/// `compositor.rs`: a fixed implementation budget a hostile sender cannot
/// inflate by lying about its own declared total.
pub(crate) const MAX_DECOMPRESSED_TOTAL: usize = 256 * 1024 * 1024;

pub struct Decompressor {
    history: FixedCircularBuffer,
}

impl Decompressor {
    pub fn new() -> Self {
        Self {
            history: FixedCircularBuffer::new(HISTORY_SIZE),
        }
    }

    pub fn decompress(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, ZgfxError> {
        let segmented_data = SegmentedDataPdu::from_buffer(input)?;

        match segmented_data {
            SegmentedDataPdu::Single(segment) => self.handle_segment(&segment, output),
            SegmentedDataPdu::Multipart {
                uncompressed_size,
                segments,
            } => {
                let mut bytes_written = 0;
                for segment in segments {
                    let written = self.handle_segment(&segment, output)?;
                    bytes_written += written;

                    // Early detection: a header declaring uncompressedSize=N but segments
                    // producing more than N short-circuits here rather than after the full
                    // allocation. The declared u32 size remains the wire-format upper bound;
                    // realistic-traffic limits below that are a caller-layer concern.
                    if bytes_written > uncompressed_size {
                        return Err(ZgfxError::MultipartTotalExceedsDeclared {
                            written: bytes_written,
                            declared: uncompressed_size,
                        });
                    }

                    // Separate from the check above: uncompressed_size is attacker-controlled,
                    // so a hostile sender can set it arbitrarily high and still pass that check.
                    // This bounds the actual aggregate allocation regardless of what the sender
                    // declares.
                    if bytes_written > MAX_DECOMPRESSED_TOTAL {
                        return Err(ZgfxError::MultipartTotalExceedsBudget {
                            written: bytes_written,
                            budget: MAX_DECOMPRESSED_TOTAL,
                        });
                    }
                }

                if bytes_written != uncompressed_size {
                    Err(ZgfxError::InvalidDecompressedSize {
                        decompressed_size: bytes_written,
                        uncompressed_size,
                    })
                } else {
                    Ok(bytes_written)
                }
            }
        }
    }

    fn handle_segment(&mut self, segment: &BulkEncodedData<'_>, output: &mut Vec<u8>) -> Result<usize, ZgfxError> {
        if !segment.data.is_empty() {
            if segment.compression_flags.contains(CompressionFlags::COMPRESSED) {
                self.decompress_segment(segment.data, output)
            } else {
                self.history.write_all(segment.data)?;
                output.extend_from_slice(segment.data);

                Ok(segment.data.len())
            }
        } else {
            Ok(0)
        }
    }

    fn decompress_segment(&mut self, encoded_data: &[u8], output: &mut Vec<u8>) -> Result<usize, ZgfxError> {
        let Some((&trailer, data)) = encoded_data.split_last() else {
            return Ok(0);
        };

        // The low 3 bits of the last byte indicate the number of unused bits in the final
        // byte (0-7); MS-RDPEGFX 3.1.9.1.2.4 reserves the five high-order bits. Mask them
        // off rather than reading the whole byte, or a sender that leaves them non-zero
        // (nothing in the spec requires zeroing a reserved field) makes this look like a
        // huge trailing-bit count and silently truncates real token data instead of
        // decoding it.
        // Use checked arithmetic so attacker-controlled trailing-bit counts that exceed the
        // available bit budget surface as a typed error rather than an unsigned underflow panic.
        let trailing_unused = usize::from(trailer & 0x07);
        let bit_count = 8usize
            .checked_mul(data.len())
            .and_then(|n| n.checked_sub(trailing_unused))
            .ok_or(ZgfxError::InvalidTrailingBitCount(trailing_unused))?;

        let start = output.len();
        let mut segment = Segment {
            bits: BitReader::new(data, bit_count),
            history: &mut self.history,
            output,
            start,
            recorded: start,
        };
        let result = segment.decode();
        // Bytes decoded before an error are in the history too, as the receiver of a
        // well-formed stream would have them.
        segment.record();
        result
    }
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

/// One compressed segment being decoded into `output`.
///
/// Decoded bytes reach `history` in bulk rather than one at a time: `output[recorded..]` holds
/// the bytes decoded since the last [`Segment::record`], which runs before anything reads the
/// history and once the segment ends.
struct Segment<'a, 'h> {
    bits: BitReader<'a>,
    history: &'h mut FixedCircularBuffer,
    output: &'h mut Vec<u8>,
    /// Length of `output` before this segment.
    start: usize,
    /// Length of `output` already written to `history`.
    recorded: usize,
}

impl Segment<'_, '_> {
    fn decode(&mut self) -> Result<usize, ZgfxError> {
        while self.bits.remaining() > 0 {
            // A segment whose remaining bits are shorter than every prefix that could follow
            // them surfaces as TokenBitsNotFound: the code is prefix-free, so the lookup finds
            // the only candidate and its length is then checked against the bit budget.
            let token = TOKEN_LOOKUP[usize::from(self.bits.peek_token())]
                .and_then(|index| TOKEN_TABLE.get(usize::from(index)))
                .filter(|token| usize::from(token.prefix_len) <= self.bits.remaining())
                .ok_or(ZgfxError::TokenBitsNotFound)?;
            self.bits.consume(u32::from(token.prefix_len));

            match token.ty {
                TokenType::NullLiteral => {
                    // The prefix value is encoded with a "0" prefix,
                    // then read 8 bits containing the byte to output.
                    let value = u8::try_from(self.bits.read(8)?).expect("8 bits fit in a byte");
                    self.output.push(value);
                }
                TokenType::Literal { literal_value } => self.output.push(literal_value),
                TokenType::Match {
                    distance_value_size,
                    distance_base,
                } => {
                    let written = self.written();
                    let max_remaining = MAX_DECOMPRESSED_PER_SEGMENT.checked_sub(written).ok_or(
                        ZgfxError::SegmentDecompressedSizeExceedsLimit {
                            decompressed: written,
                            limit: MAX_DECOMPRESSED_PER_SEGMENT,
                        },
                    )?;
                    self.handle_match(distance_value_size, distance_base, max_remaining)?;
                }
            }

            // The per-token check above bounds a single match copy; this bounds their
            // sum, which is what a hostile stream actually exploits. Catches the
            // cumulative NullLiteral / Literal / Match contribution that could
            // otherwise inflate a small input into a multi-gigabyte allocation.
            if self.written() > MAX_DECOMPRESSED_PER_SEGMENT {
                return Err(ZgfxError::SegmentDecompressedSizeExceedsLimit {
                    decompressed: self.written(),
                    limit: MAX_DECOMPRESSED_PER_SEGMENT,
                });
            }
        }

        Ok(self.written())
    }

    fn written(&self) -> usize {
        self.output.len() - self.start
    }

    /// Write the bytes decoded since the last call to the history.
    fn record(&mut self) {
        self.history
            .write_all(&self.output[self.recorded..])
            .expect("circular buffer does not fail");
        self.recorded = self.output.len();
    }

    fn handle_match(
        &mut self,
        distance_value_size: u32,
        distance_base: u32,
        max_remaining: usize,
    ) -> Result<(), ZgfxError> {
        // Each token has been assigned a different base distance
        // and number of additional value bits to be added to compute the full distance.
        let distance_value = self.bits.read(distance_value_size)?;
        let full_distance = distance_base
            .checked_add(distance_value)
            .ok_or(ZgfxError::InvalidIntegralConversion("token's full distance"))?;
        let distance = usize::try_from(full_distance)
            .map_err(|_| ZgfxError::InvalidIntegralConversion("token's full distance"))?;

        if distance == 0 {
            self.read_unencoded_bytes(max_remaining)
        } else {
            // Bound the match distance against the history size to prevent the
            // circular buffer's position arithmetic from underflowing on an
            // attacker-controlled distance. This is defense-in-depth alongside
            // the bound check inside FixedCircularBuffer::read_with_offset.
            if distance > HISTORY_SIZE {
                return Err(ZgfxError::MatchDistanceOutOfRange {
                    distance,
                    history_size: HISTORY_SIZE,
                });
            }
            self.read_encoded_bytes(distance, max_remaining)
        }
    }

    fn read_unencoded_bytes(&mut self, max_remaining: usize) -> Result<(), ZgfxError> {
        // A match distance of zero is a special case,
        // which indicates that an unencoded run of bytes follows.
        // The count of bytes is encoded as a 15-bit value.
        let length = usize::try_from(self.bits.read(15)?).expect("15 bits fit in usize");

        // Enforce the per-segment cap before any allocation. length is a 15-bit
        // value (max 32,767) so it can never exceed MAX_DECOMPRESSED_PER_SEGMENT,
        // but it may exceed max_remaining if the prior tokens already filled most
        // of the budget.
        if length > max_remaining {
            return Err(ZgfxError::SegmentDecompressedSizeExceedsLimit {
                decompressed: MAX_DECOMPRESSED_PER_SEGMENT
                    .saturating_sub(max_remaining)
                    .saturating_add(length),
                limit: MAX_DECOMPRESSED_PER_SEGMENT,
            });
        }

        let bytes = self.bits.read_aligned_bytes(length)?;
        self.output.extend_from_slice(bytes);
        Ok(())
    }

    fn read_encoded_bytes(&mut self, distance: usize, max_remaining: usize) -> Result<(), ZgfxError> {
        // A match length prefix follows the token and indicates
        // how many additional bits will be needed to get the full length
        // (the number of bytes to be copied): a run of ones ended by a zero.
        let length_token_size = self.bits.skip_ones();
        if self.bits.remaining() == 0 {
            return Err(ZgfxError::IncompleteBitStream {
                needed: length_token_size.saturating_add(1),
                remaining: length_token_size,
            });
        }
        self.bits.consume(1);

        let length = if length_token_size == 0 {
            // special case
            3
        } else {
            // The length-value bit width must fit a usize. Bound it before reading so an
            // attacker cannot drive the exponent into a shift overflow.
            let value_bits = length_token_size
                .checked_add(1)
                .ok_or(ZgfxError::LengthTokenSizeTooLarge(length_token_size))?;
            let usize_bits =
                usize::try_from(usize::BITS).map_err(|_| ZgfxError::LengthTokenSizeTooLarge(length_token_size))?;
            if value_bits >= usize_bits {
                return Err(ZgfxError::LengthTokenSizeTooLarge(length_token_size));
            }
            let exponent =
                u32::try_from(value_bits).map_err(|_| ZgfxError::LengthTokenSizeTooLarge(length_token_size))?;
            let value = usize::try_from(self.bits.read_wide(exponent)?)
                .map_err(|_| ZgfxError::LengthTokenSizeTooLarge(length_token_size))?;
            let base = 1usize
                .checked_shl(exponent)
                .ok_or(ZgfxError::LengthTokenSizeTooLarge(length_token_size))?;
            base.checked_add(value)
                .ok_or(ZgfxError::LengthTokenSizeTooLarge(length_token_size))?
        };

        // Enforce the per-segment cap before allocating. A match-copy of length N
        // grows the output by N bytes; rejecting before the copy prevents an
        // attacker-controlled match length from inflating the output beyond the
        // spec-defined bound.
        if length > max_remaining {
            return Err(ZgfxError::SegmentDecompressedSizeExceedsLimit {
                decompressed: MAX_DECOMPRESSED_PER_SEGMENT
                    .saturating_sub(max_remaining)
                    .saturating_add(length),
                limit: MAX_DECOMPRESSED_PER_SEGMENT,
            });
        }

        if distance <= self.written() {
            // The source is in this segment's output, which ends with the newest history bytes.
            copy_match_within(self.output, distance, length);
        } else {
            self.record();
            self.history.read_with_offset(distance, length, self.output)?;
        }
        Ok(())
    }
}

/// Append `length` bytes copied from `distance` bytes back in `output`.
///
/// The copy may overlap the bytes it produces, which repeats the last `distance` bytes.
///
/// INVARIANT: `0 < distance <= output.len()`.
fn copy_match_within(output: &mut Vec<u8>, distance: usize, length: usize) {
    output.reserve(length);
    let source = output.len() - distance;
    let mut left = length;
    while left > 0 {
        // Everything from `source` on repeats with period `distance`, and each pass appends a
        // whole number of periods after the previous one, so the next pass can copy twice as much.
        let chunk = left.min(output.len() - source);
        output.extend_from_within(source..source + chunk);
        left -= chunk;
    }
}

/// Reads a segment's bits most significant first, up to a hard end-of-bits limit.
struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next byte of `data` to load into `acc`.
    next: usize,
    /// Loaded bits, most significant first.
    ///
    /// INVARIANT: the bits below the top `acc_len` are zero.
    acc: u64,
    acc_len: u32,
    /// Bits left in the segment. Fewer than are loaded and unloaded when the last byte
    /// is partly padding.
    remaining: usize,
    total: usize,
}

impl<'a> BitReader<'a> {
    /// INVARIANT: `bit_count <= data.len() * 8`.
    fn new(data: &'a [u8], bit_count: usize) -> Self {
        Self {
            data,
            next: 0,
            acc: 0,
            acc_len: 0,
            remaining: bit_count,
            total: bit_count,
        }
    }

    fn remaining(&self) -> usize {
        self.remaining
    }

    /// Load whole bytes until more than 56 bits are loaded or `data` ends.
    fn refill(&mut self) {
        if let Some(chunk) = self.data.get(self.next..self.next + 8) {
            let word = u64::from_be_bytes(chunk.try_into().expect("8-byte chunk"));
            let bytes = (64 - self.acc_len) / 8;
            let len = self.acc_len + bytes * 8;
            let mask = if len == 64 { u64::MAX } else { !(u64::MAX >> len) };
            self.acc |= (word >> self.acc_len) & mask;
            self.acc_len = len;
            self.next += usize::try_from(bytes).expect("at most 8");
        } else {
            while self.acc_len <= 56 {
                let Some(&byte) = self.data.get(self.next) else {
                    break;
                };
                self.acc |= u64::from(byte) << (56 - self.acc_len);
                self.acc_len += 8;
                self.next += 1;
            }
        }
    }

    /// The next 9 bits, the longest token prefix, with zeros past the end of the data.
    fn peek_token(&mut self) -> u16 {
        if self.acc_len < 9 {
            self.refill();
        }
        u16::try_from(self.acc >> (64 - 9)).expect("9 bits fit in u16")
    }

    /// Drop `count` bits.
    ///
    /// INVARIANT: `count <= self.remaining` and `count` bits are loaded, which holds right after a
    /// peek or refill for `count <= 32`.
    fn consume(&mut self, count: u32) {
        debug_assert!(count <= self.acc_len && usize::try_from(count).is_ok_and(|c| c <= self.remaining));
        self.acc = self.acc.checked_shl(count).unwrap_or(0);
        self.acc_len -= count;
        self.remaining -= usize::try_from(count).expect("u32 fits usize");
    }

    fn check(&self, count: usize) -> Result<(), ZgfxError> {
        if count > self.remaining {
            Err(ZgfxError::IncompleteBitStream {
                needed: count,
                remaining: self.remaining,
            })
        } else {
            Ok(())
        }
    }

    /// Read `count` bits, `1 <= count <= 32`.
    fn read(&mut self, count: u32) -> Result<u32, ZgfxError> {
        debug_assert!((1..=32).contains(&count));
        self.check(usize::try_from(count).expect("u32 fits usize"))?;
        if self.acc_len < count {
            self.refill();
        }
        let value = u32::try_from(self.acc >> (64 - count)).expect("at most 32 bits");
        self.consume(count);
        Ok(value)
    }

    /// Read `count` bits, `1 <= count <= 64`.
    fn read_wide(&mut self, count: u32) -> Result<u64, ZgfxError> {
        self.check(usize::try_from(count).expect("u32 fits usize"))?;
        if count <= 32 {
            return self.read(count).map(u64::from);
        }
        let high = u64::from(self.read(count - 32)?);
        let low = u64::from(self.read(32)?);
        Ok((high << 32) | low)
    }

    /// Drop the run of one bits at the read position and return its length.
    fn skip_ones(&mut self) -> usize {
        let mut count = 0;
        loop {
            if self.acc_len < 32 {
                self.refill();
            }
            let loaded = usize::try_from(self.acc_len)
                .expect("u32 fits usize")
                .min(self.remaining);
            if loaded == 0 {
                return count;
            }
            let ones = usize::try_from(self.acc.leading_ones())
                .expect("u32 fits usize")
                .min(loaded);
            self.consume(u32::try_from(ones).expect("at most 64"));
            count += ones;
            if ones < loaded {
                return count;
            }
        }
    }

    /// Skip to the next byte boundary, then take `length` whole bytes.
    fn read_aligned_bytes(&mut self, length: usize) -> Result<&'a [u8], ZgfxError> {
        let consumed = self.total - self.remaining;
        let padding = (8 - consumed % 8) % 8;
        self.check(padding)?;
        let start = (consumed + padding) / 8;
        let bit_count = length
            .checked_mul(8)
            .ok_or(ZgfxError::InvalidIntegralConversion("unencoded byte-run bit count"))?;
        if bit_count > self.remaining - padding {
            return Err(ZgfxError::IncompleteBitStream {
                needed: bit_count,
                remaining: self.remaining - padding,
            });
        }

        let bytes = &self.data[start..start + length];
        self.next = start + length;
        self.acc = 0;
        self.acc_len = 0;
        self.remaining -= padding + bit_count;
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
struct Token {
    /// The prefix bits, in the low `prefix_len` bits.
    prefix: u16,
    prefix_len: u8,
    ty: TokenType,
}

#[derive(Clone, Copy)]
enum TokenType {
    NullLiteral,
    Literal {
        literal_value: u8,
    },
    Match {
        distance_value_size: u32,
        distance_base: u32,
    },
}

/// Bits [`TOKEN_LOOKUP`] is indexed by: the longest prefix in [`TOKEN_TABLE`].
const TOKEN_LOOKUP_BITS: u8 = 9;

const fn literal(prefix: u16, prefix_len: u8, literal_value: u8) -> Token {
    Token {
        prefix,
        prefix_len,
        ty: TokenType::Literal { literal_value },
    }
}

const fn distance(prefix: u16, prefix_len: u8, distance_value_size: u32, distance_base: u32) -> Token {
    Token {
        prefix,
        prefix_len,
        ty: TokenType::Match {
            distance_value_size,
            distance_base,
        },
    }
}

/// The token prefixes of MS-RDPEGFX 3.1.9.1.2.4. No prefix is the start of another.
static TOKEN_TABLE: [Token; 40] = [
    Token {
        prefix: 0b0,
        prefix_len: 1,
        ty: TokenType::NullLiteral,
    },
    literal(0b11000, 5, 0x00),
    literal(0b11001, 5, 0x01),
    literal(0b110100, 6, 0x02),
    literal(0b110101, 6, 0x03),
    literal(0b110110, 6, 0xff),
    literal(0b1101110, 7, 0x04),
    literal(0b1101111, 7, 0x05),
    literal(0b1110000, 7, 0x06),
    literal(0b1110001, 7, 0x07),
    literal(0b1110010, 7, 0x08),
    literal(0b1110011, 7, 0x09),
    literal(0b1110100, 7, 0x0a),
    literal(0b1110101, 7, 0x0b),
    literal(0b1110110, 7, 0x3a),
    literal(0b1110111, 7, 0x3b),
    literal(0b1111000, 7, 0x3c),
    literal(0b1111001, 7, 0x3d),
    literal(0b1111010, 7, 0x3e),
    literal(0b1111011, 7, 0x3f),
    literal(0b1111100, 7, 0x40),
    literal(0b1111101, 7, 0x80),
    literal(0b11111100, 8, 0x0c),
    literal(0b11111101, 8, 0x38),
    literal(0b11111110, 8, 0x39),
    literal(0b11111111, 8, 0x66),
    distance(0b10001, 5, 5, 0),
    distance(0b10010, 5, 7, 32),
    distance(0b10011, 5, 9, 160),
    distance(0b10100, 5, 10, 672),
    distance(0b10101, 5, 12, 1_696),
    distance(0b101100, 6, 14, 5_792),
    distance(0b101101, 6, 15, 22_176),
    distance(0b1011100, 7, 18, 54_944),
    distance(0b1011101, 7, 20, 317_088),
    distance(0b10111100, 8, 20, 1_365_664),
    distance(0b10111101, 8, 21, 2_414_240),
    distance(0b101111100, 9, 22, 4_511_392),
    distance(0b101111101, 9, 23, 8_705_696),
    distance(0b101111110, 9, 24, 17_094_304),
];

/// The [`TOKEN_TABLE`] index of the token each `TOKEN_LOOKUP_BITS`-bit value starts with, if any.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "const evaluation has no From or TryFrom; TOKEN_TABLE has fewer than 256 entries"
)]
static TOKEN_LOOKUP: [Option<u8>; 1 << TOKEN_LOOKUP_BITS] = {
    let mut lookup = [None; 1 << TOKEN_LOOKUP_BITS];
    let mut index = 0;
    while index < TOKEN_TABLE.len() {
        let token = TOKEN_TABLE[index];
        let free_bits = TOKEN_LOOKUP_BITS - token.prefix_len;
        let first = (token.prefix as usize) << free_bits;
        let mut suffix = 0;
        while suffix < 1 << free_bits {
            assert!(lookup[first + suffix].is_none(), "token prefixes overlap");
            lookup[first + suffix] = Some(index as u8);
            suffix += 1;
        }
        index += 1;
    }
    lookup
};

#[non_exhaustive]
#[derive(Debug)]
pub enum ZgfxError {
    IOError(io::Error),
    InvalidCompressionType,
    InvalidSegmentedDescriptor,
    InvalidDecompressedSize {
        decompressed_size: usize,
        uncompressed_size: usize,
    },
    TokenBitsNotFound,
    InvalidIntegralConversion(&'static str),
    InvalidTrailingBitCount(usize),
    SegmentSizeExceedsBuffer {
        size: usize,
        remaining: usize,
    },
    IncompleteBitStream {
        needed: usize,
        remaining: usize,
    },
    MatchDistanceOutOfRange {
        distance: usize,
        history_size: usize,
    },
    LengthTokenSizeTooLarge(usize),
    SegmentDecompressedSizeExceedsLimit {
        decompressed: usize,
        limit: usize,
    },
    MultipartTotalExceedsDeclared {
        written: usize,
        declared: usize,
    },
    MultipartTotalExceedsBudget {
        written: usize,
        budget: usize,
    },
}

impl core::fmt::Display for ZgfxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IOError(_error) => write!(f, "IO error"),
            Self::InvalidCompressionType => write!(f, "invalid compression type"),
            Self::InvalidSegmentedDescriptor => write!(f, "invalid segmented descriptor"),
            Self::InvalidDecompressedSize {
                decompressed_size,
                uncompressed_size,
            } => write!(
                f,
                "decompressed size of segments ({decompressed_size}) does not equal to uncompressed size ({uncompressed_size})",
            ),
            Self::TokenBitsNotFound => write!(f, "token bits not found"),
            Self::InvalidIntegralConversion(type_name) => {
                write!(f, "invalid `{type_name}`: out of range integral type conversion")
            }
            Self::InvalidTrailingBitCount(count) => {
                write!(f, "invalid trailing-bit count {count} exceeds available bit budget")
            }
            Self::SegmentSizeExceedsBuffer { size, remaining } => {
                write!(
                    f,
                    "multipart segment claims {size} bytes but only {remaining} bytes remain in the buffer"
                )
            }
            Self::IncompleteBitStream { needed, remaining } => {
                write!(
                    f,
                    "incomplete bitstream: needed {needed} bits, only {remaining} bits remain in the segment"
                )
            }
            Self::MatchDistanceOutOfRange { distance, history_size } => {
                write!(
                    f,
                    "match distance {distance} exceeds history buffer size {history_size}"
                )
            }
            Self::LengthTokenSizeTooLarge(size) => {
                write!(f, "match-length token size {size} exceeds the supported bit budget")
            }
            Self::SegmentDecompressedSizeExceedsLimit { decompressed, limit } => {
                write!(
                    f,
                    "segment decompressed size {decompressed} exceeds the {limit} byte per-segment ceiling"
                )
            }
            Self::MultipartTotalExceedsDeclared { written, declared } => {
                write!(
                    f,
                    "multipart decompressed bytes {written} exceed the declared uncompressedSize {declared}"
                )
            }
            Self::MultipartTotalExceedsBudget { written, budget } => {
                write!(
                    f,
                    "multipart decompressed bytes {written} exceed the {budget} byte total budget"
                )
            }
        }
    }
}

impl core::error::Error for ZgfxError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::IOError(error) => Some(error),
            Self::InvalidCompressionType => None,
            Self::InvalidSegmentedDescriptor => None,
            Self::InvalidDecompressedSize { .. } => None,
            Self::TokenBitsNotFound => None,
            Self::InvalidIntegralConversion(_) => None,
            Self::InvalidTrailingBitCount(_) => None,
            Self::SegmentSizeExceedsBuffer { .. } => None,
            Self::IncompleteBitStream { .. } => None,
            Self::MatchDistanceOutOfRange { .. } => None,
            Self::LengthTokenSizeTooLarge(_) => None,
            Self::SegmentDecompressedSizeExceedsLimit { .. } => None,
            Self::MultipartTotalExceedsDeclared { .. } => None,
            Self::MultipartTotalExceedsBudget { .. } => None,
        }
    }
}

impl From<io::Error> for ZgfxError {
    fn from(err: io::Error) -> Self {
        Self::IOError(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCODED_ZGFX_SINGLE: [&[u8]; 5] = [
        include_bytes!("test_assets/encoded.0.bin"),
        include_bytes!("test_assets/encoded.1.bin"),
        include_bytes!("test_assets/encoded.2.bin"),
        include_bytes!("test_assets/encoded.3.bin"),
        include_bytes!("test_assets/encoded.4.bin"),
    ];

    const DECODED_ZGFX_SINGLE: [&[u8]; 5] = [
        include_bytes!("test_assets/decoded.0.bin"),
        include_bytes!("test_assets/decoded.1.bin"),
        include_bytes!("test_assets/decoded.2.bin"),
        include_bytes!("test_assets/decoded.3.bin"),
        include_bytes!("test_assets/decoded.4.bin"),
    ];

    #[test]
    fn zgfx_decompresses_multiple_single_pdus() {
        let pairs = ENCODED_ZGFX_SINGLE
            .iter()
            .copied()
            .zip(DECODED_ZGFX_SINGLE.iter().copied());
        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(pairs.clone().map(|(_, d)| d.len()).max().unwrap());
        for (i, (encode, decode)) in pairs.enumerate() {
            let bytes_written = zgfx.decompress(encode.as_ref(), &mut decompressed).unwrap();
            assert_eq!(decode.len(), bytes_written);
            assert_eq!(decompressed, *decode, "Failed to decompress encoded PDU #{i}");
            decompressed.clear();
        }
    }

    #[test]
    fn zgfx_decompresses_only_one_literal() {
        let buffer = [0b1100_1000, 0x03];
        let expected = vec![0x01];

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    /// MS-RDPEGFX 3.1.9.1.2.4 reserves the five high-order bits of the trailer byte;
    /// only the low 3 bits are the unused-bit count. Same trailer as
    /// `zgfx_decompresses_only_one_literal` (3 unused bits) with the reserved bits set
    /// to a nonzero pattern, which must decode identically rather than being read as a
    /// much larger unused-bit count.
    #[test]
    fn zgfx_ignores_reserved_bits_in_trailer_byte() {
        let buffer = [0b1100_1000, 0x03 | 0b1010_0000];
        let expected = vec![0x01];

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_one_literal_with_null_prefix() {
        let buffer = [0b0011_0010, 0b1000_0000, 0x07];
        let expected = vec![0x65];

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_only_multiple_literals() {
        let buffer = [0b1100_1110, 0b1001_1011, 0b0001_1001, 0b0100_0000, 0x06];
        let expected = vec![0x01, 0x02, 0xff, 0x65];

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_one_literal_with_one_match_distance_1() {
        let buffer = [0b0011_0010, 0b1100_0100, 0b0011_0000, 0x1];
        let expected = vec![0x65; 1 + 4]; // literal (1) + match repeated 4 (length) + 0 times

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_three_literals_with_one_match_distance_3_length_57() {
        let buffer = [
            0b0010_0000,
            0b1001_0000,
            0b1000_1000,
            0b0111_0001,
            0b0001_1111,
            0b1011_0010,
            0x1,
        ];
        let expected = "ABC".repeat(20);
        let expected = expected.as_bytes();

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_one_match_with_match_unencoded_bytes() {
        let expected = "The quick brown fox jumps over the lazy dog".as_bytes();
        let mut buffer = vec![0b1000_1000, 0b0000_0000, 0b00010101, 0b1000_0000];
        buffer.extend_from_slice(expected);
        buffer.extend_from_slice(&[0x00]); // no bits unused

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_multiple_literals_with_match_in_center_with_not_compressed() {
        let buffer = [
            0xE1, // DEBLOCK_MULTIPART
            0x03, 0x00, // 3 segments
            0x2B, 0x00, 0x00, 0x00, // 0x0000002B total bytes uncompressed
            0x11, 0x00, 0x00, 0x00, // first segment is the next 17 bytes:
            0x04, // type 4, not PACKET_COMPRESSED
            0x54, 0x68, 0x65, 0x20, 0x71, 0x75, 0x69, 0x63, 0x6B, 0x20, 0x62, 0x72, 0x6F, 0x77, 0x6E,
            0x20, // "The quick brown "
            0x0E, 0x00, 0x00, 0x00, // second segment is the next 14 bytes:
            0x04, // type 4, not PACKET_COMPRESSED
            0x66, 0x6F, 0x78, 0x20, 0x6A, 0x75, 0x6D, 0x70, 0x73, 0x20, 0x6F, 0x76, 0x65, // "fox jumps ove"
            0x10, 0x00, 0x00, 0x00, // third segment is the next 16 bytes
            0x24, // type 4 + PACKET_COMPRESSED
            0x39, 0x08, 0x0E, 0x91, 0xF8, 0xD8, 0x61, 0x3D, 0x1E, 0x44, 0x06, 0x43, 0x79, 0x9C, // encoded:
            // 0 01110010 = literal 0x72 = "r"
            // 0 00100000 = literal 0x20 = " "
            // 0 01110100 = literal 0x74 = "t"
            //
            // 10001 11111 0 = match, distance = 31, length = 3 "he "
            //
            // 0 01101100 = literal 0x6C = "l"
            // 0 01100001 = literal 0x61 = "a"
            // 0 01111010 = literal 0x7A = "z"
            // 0 01111001 = literal 0x79 = "y"
            // 0 00100000 = literal 0x20 = " "
            // 0 01100100 = literal 0x64 = "d"
            // 0 01101111 = literal 0x6F = "o"
            // 0 01100111 = literal 0x67 = "g"
            0x02, // ignore last two bits of 0x9C byte
        ];
        let expected = "The quick brown fox jumps over the lazy dog".as_bytes();

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        let bytes_written = zgfx.decompress(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(expected.len(), bytes_written);
        assert_eq!(decompressed, expected, "\n{decompressed:x?} != \n{expected:x?}");
    }

    #[test]
    fn zgfx_decompresses_single_match_unencoded_block() {
        let buffer = [
            0xe0, 0x04, 0x13, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x01, 0x06, 0x0a, 0x00, 0x04, 0x00, 0x00, 0x00,
            0x20, 0x00, 0x00, 0x00,
        ];
        let expected = vec![
            0x13, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x01, 0x06, 0x0a, 0x00, 0x04, 0x00, 0x00, 0x00, 0x20, 0x00,
            0x00, 0x00,
        ];

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        let bytes_written = zgfx.decompress(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(expected.len(), bytes_written);
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn zgfx_decompresses_unencoded_block_without_padding() {
        let buffer = [0b1110_0101, 0b0001_0000, 0b0000_0000, 0b00000001, 0b1111_0000, 0x0];
        let expected = vec![0x08, 0xf0];

        let mut zgfx = Decompressor::new();
        let mut decompressed = Vec::with_capacity(expected.len());
        zgfx.decompress_segment(buffer.as_ref(), &mut decompressed).unwrap();
        assert_eq!(decompressed, expected);
    }

    /// MAX_DECOMPRESSED_PER_SEGMENT alone does not bound a multipart message: each
    /// segment here is comfortably under that per-segment ceiling, but five of them
    /// sum past MAX_DECOMPRESSED_TOTAL while staying under the declared
    /// uncompressedSize, so the pre-existing MultipartTotalExceedsDeclared check alone
    /// would let the full allocation happen before erroring. Uses real LZ compression
    /// of a repetitive buffer to keep the wire input small (a few hundred KB) while
    /// still exercising the real decompress path against real segment sizes, rather
    /// than faking bytes_written.
    #[test]
    fn multipart_total_exceeds_budget_before_declared_size_is_reached() {
        let per_segment_decompressed = 60 * 1024 * 1024; // 60 MiB, under the 64 MiB per-segment cap
        let raw = vec![0xABu8; per_segment_decompressed];

        let mut compressor = Compressor::new();
        let compressed = compressor.compress(&raw).unwrap();

        // 5 segments * 60 MiB = 300 MiB, over the 256 MiB budget, under the declared size.
        let segment_count: u16 = 5;
        let per_segment_decompressed_u32 = u32::try_from(per_segment_decompressed).unwrap();
        let total_declared: u32 = per_segment_decompressed_u32 * u32::from(segment_count) + 1000;

        let mut buffer = vec![0xE1u8]; // MULTIPART descriptor
        buffer.extend_from_slice(&segment_count.to_le_bytes());
        buffer.extend_from_slice(&total_declared.to_le_bytes());
        for _ in 0..segment_count {
            let seg_size = u32::try_from(compressed.len() + 1).unwrap(); // +1 for the header byte
            buffer.extend_from_slice(&seg_size.to_le_bytes());
            buffer.push(0x24); // type 4 + PACKET_COMPRESSED
            buffer.extend_from_slice(&compressed);
        }

        let mut zgfx = Decompressor::new();
        let mut output = Vec::new();
        let err = zgfx.decompress(&buffer, &mut output).unwrap_err();

        assert!(
            matches!(err, ZgfxError::MultipartTotalExceedsBudget { .. }),
            "expected MultipartTotalExceedsBudget, got {err:?}"
        );
        // The budget check fires partway through segment 5 (4 * 60 MiB = 240 MiB is
        // still under budget; adding the 5th pushes to 300 MiB), so output holds at
        // most 5 segments' worth, not the full declared 300 MiB + 1000.
        assert!(output.len() <= per_segment_decompressed * 5);
    }
}
