//! Allocation-free A-MPDU byte accounting recovered from the PP formatter.

use super::block_ack::TX_AMPDU_SLOT_CAPACITY;

pub(super) const HARDWARE_HE_CONTROL_LENGTH: u16 = 4;
const HT_MPDU_LENGTH_MASK: u32 = 0x3fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduLengthError {
    InvalidLimits,
    Empty,
    ZeroMpduLength,
    WindowFull,
    AggregateTooLong(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduLength {
    pub bytes: u16,
    pub subframes: u8,
}

/// Exact basic-HT A-MPDU byte accounting recovered from the pinned PP blob.
///
/// Bits 0..13 of `payload_word` carry the MPDU length. `empty_delimiters` is
/// the byte immediately following that word in the PP metadata prefix. Every
/// non-final MPDU contributes its four-byte delimiter, its length rounded to
/// four bytes, and the requested empty delimiters. `finish` removes the final
/// padding and empty delimiters, leaving the last MPDU's mandatory delimiter.
///
/// The accumulator has a caller-selected fixed window and byte ceiling. It
/// never allocates, accesses pointers, reads time, retries, waits or invokes a
/// callback.
pub struct HtAmpduLengthAccumulator {
    bytes_with_tail: u32,
    tail_bytes: u16,
    count: u8,
    max_subframes: u8,
    max_bytes: u16,
}

impl HtAmpduLengthAccumulator {
    pub const fn new(max_subframes: u8, max_bytes: u16) -> Result<Self, HtAmpduLengthError> {
        if max_subframes == 0 || max_subframes as usize > TX_AMPDU_SLOT_CAPACITY || max_bytes == 0 {
            return Err(HtAmpduLengthError::InvalidLimits);
        }
        Ok(Self {
            bytes_with_tail: 0,
            tail_bytes: 0,
            count: 0,
            max_subframes,
            max_bytes,
        })
    }

    pub const fn push(
        &mut self,
        payload_word: u32,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduLengthError> {
        self.push_with_hardware_he_control(payload_word, empty_delimiters, false)
    }

    /// Add one MPDU whose HE-Control field may be inserted by MAC hardware.
    ///
    /// SOURCE: complete `libpp.a[pp_he.o]::
    /// ppCalSubFrameLength` reads `metadata[7] & 1`, multiplies it by four,
    /// and adds it after the delimiter and rounded metadata length. The low
    /// fourteen-bit MPDU length remains unchanged.
    pub const fn push_with_hardware_he_control(
        &mut self,
        payload_word: u32,
        empty_delimiters: u8,
        hardware_he_control: bool,
    ) -> Result<(), HtAmpduLengthError> {
        if self.count >= self.max_subframes {
            return Err(HtAmpduLengthError::WindowFull);
        }
        let mpdu_bytes = payload_word & HT_MPDU_LENGTH_MASK;
        if mpdu_bytes == 0 {
            return Err(HtAmpduLengthError::ZeroMpduLength);
        }
        let padding = (4 - (mpdu_bytes & 3)) & 3;
        let empty_bytes = (empty_delimiters as u32) * 4;
        let inserted_bytes = if hardware_he_control {
            HARDWARE_HE_CONTROL_LENGTH as u32
        } else {
            0
        };
        let contribution = mpdu_bytes + padding + empty_bytes + 4 + inserted_bytes;
        let next = self.bytes_with_tail + contribution;
        let final_bytes = next - padding - empty_bytes;
        if final_bytes > self.max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(final_bytes));
        }
        self.bytes_with_tail = next;
        self.tail_bytes = (padding + empty_bytes) as u16;
        self.count += 1;
        Ok(())
    }

    pub const fn finish(&self) -> Result<HtAmpduLength, HtAmpduLengthError> {
        if self.count == 0 {
            return Err(HtAmpduLengthError::Empty);
        }
        let bytes = self.bytes_with_tail - self.tail_bytes as u32;
        if bytes > self.max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(bytes));
        }
        Ok(HtAmpduLength {
            bytes: bytes as u16,
            subframes: self.count,
        })
    }
}
