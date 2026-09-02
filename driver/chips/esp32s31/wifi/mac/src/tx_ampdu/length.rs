//! Allocation-free A-MPDU byte accounting recovered from the PP formatter.

use core::pin::Pin;

use super::{HtAmpduTxError, HtAmpduTxStorage, block_ack::TX_AMPDU_SLOT_CAPACITY};

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

    /// Return the largest prefix of identical MPDUs accepted by the supplied
    /// window and byte ceiling.
    ///
    /// A conservative scheduler opportunity models every queued frame with
    /// the same maximum size. Replaying [`Self::push_with_hardware_he_control`]
    /// for every prospective subframe makes grant preparation linear in the
    /// BA window even though the first and following contributions are
    /// constant. This is the exact closed form of that replay: the final MPDU
    /// has no alignment or empty-delimiter tail, while every preceding MPDU
    /// contributes that tail once a successor is present.
    pub const fn largest_repeated_prefix(
        max_subframes: u8,
        max_bytes: u16,
        payload_word: u32,
        empty_delimiters: u8,
        hardware_he_control: bool,
    ) -> Result<HtAmpduLength, HtAmpduLengthError> {
        if max_subframes == 0 || max_subframes as usize > TX_AMPDU_SLOT_CAPACITY || max_bytes == 0 {
            return Err(HtAmpduLengthError::InvalidLimits);
        }
        let mpdu_bytes = payload_word & HT_MPDU_LENGTH_MASK;
        if mpdu_bytes == 0 {
            return Err(HtAmpduLengthError::ZeroMpduLength);
        }

        let inserted_bytes = if hardware_he_control {
            HARDWARE_HE_CONTROL_LENGTH as u32
        } else {
            0
        };
        let first = 4 + mpdu_bytes + inserted_bytes;
        if first > max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(first));
        }
        let tail = ((4 - (mpdu_bytes & 3)) & 3) + (empty_delimiters as u32) * 4;
        let following = first + tail;
        let additional_capacity = (max_bytes as u32 - first) / following;
        let window_capacity = (max_subframes - 1) as u32;
        let additional = if additional_capacity < window_capacity {
            additional_capacity
        } else {
            window_capacity
        };
        let subframes = 1 + additional as u8;
        let bytes = first + additional * following;

        Ok(HtAmpduLength {
            bytes: bytes as u16,
            subframes,
        })
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    /// Return the final A-MPDU length after appending one PSDU.
    ///
    /// The previous final MPDU gains its four-byte alignment and requested
    /// empty delimiters only when another MPDU follows it. The new final MPDU
    /// contributes one delimiter and its exact PSDU length. Keeping this
    /// prefix total makes each append O(1); the former validation replayed all
    /// preceding lengths on every `commit_frame`, making a 32-MPDU build
    /// O(n²).
    ///
    /// SOURCE: complete `libpp.a[pp.o]::{ppCalSubFrameLength,
    /// ppCalTxAMPDULength}`, complete `libpp.a[pp_he.o]::
    /// ppCalSubFrameLength`, and the equivalent finite rules in
    /// [`HtAmpduLengthAccumulator`].
    pub(super) fn length_after_append(
        &self,
        psdu_length: u16,
        hardware_he_control: bool,
    ) -> Result<u16, HtAmpduTxError> {
        if psdu_length == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::ZeroMpduLength));
        }
        let mut next = u32::from(self.prepared_length);
        if self.count != 0 {
            let previous = usize::from(self.count - 1);
            let previous_length = u32::from(self.psdu_lengths[previous]);
            next = next
                .checked_add((4 - (previous_length & 3)) & 3)
                .and_then(|length| {
                    length.checked_add(u32::from(self.empty_delimiters[previous]) * 4)
                })
                .ok_or(HtAmpduTxError::Length(
                    HtAmpduLengthError::AggregateTooLong(u32::MAX),
                ))?;
        }
        next = next
            .checked_add(
                4 + u32::from(psdu_length)
                    + if hardware_he_control {
                        u32::from(HARDWARE_HE_CONTROL_LENGTH)
                    } else {
                        0
                    },
            )
            .ok_or(HtAmpduTxError::Length(
                HtAmpduLengthError::AggregateTooLong(u32::MAX),
            ))?;
        if next > u32::from(self.max_aggregate_bytes) {
            return Err(HtAmpduTxError::Length(
                HtAmpduLengthError::AggregateTooLong(next),
            ));
        }
        u16::try_from(next)
            .map_err(|_| HtAmpduTxError::Length(HtAmpduLengthError::AggregateTooLong(next)))
    }

    pub(super) fn recalculate_prepared_length(
        self: Pin<&mut Self>,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        let mut length = HtAmpduLengthAccumulator::new(storage.count, storage.max_aggregate_bytes)
            .map_err(HtAmpduTxError::Length)?;
        for index in 0..usize::from(storage.count) {
            length
                .push_with_hardware_he_control(
                    u32::from(storage.psdu_lengths[index]),
                    storage.empty_delimiters[index],
                    storage.hardware_he_control[index],
                )
                .map_err(HtAmpduTxError::Length)?;
        }
        let aggregate = length.finish().map_err(HtAmpduTxError::Length)?;
        *self.project().prepared_length = aggregate.bytes;
        Ok(aggregate)
    }

    pub(super) fn calculate_aggregate(&self) -> Result<HtAmpduLength, HtAmpduTxError> {
        if self.count == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::Empty));
        }
        if self.prepared_length == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::ZeroMpduLength));
        }
        Ok(HtAmpduLength {
            bytes: self.prepared_length,
            subframes: self.count,
        })
    }
}
