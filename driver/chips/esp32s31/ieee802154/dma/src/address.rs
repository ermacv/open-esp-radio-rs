//! Validated internal-SRAM DMA addresses.

use crate::FRAME_BUFFER_SIZE;

/// Inclusive lower bound of ESP32-S31 internal DMA-capable SRAM.
pub const DMA_LOW: u32 = 0x2f00_0000;
/// Exclusive upper bound of ESP32-S31 internal DMA-capable SRAM.
pub const DMA_HIGH: u32 = 0x2f08_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaAddressError {
    OutOfRange,
    Unaligned,
    RegionTooLarge,
}

/// A four-byte-aligned address whose complete 128-byte frame lies in internal
/// DMA-capable SRAM.
///
/// The field is private, so invalid address tokens cannot be constructed.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::DmaFrameAddress;
///
/// let _forged = DmaFrameAddress { address: 0x1234_5678 };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaFrameAddress {
    address: u32,
}

impl DmaFrameAddress {
    pub const fn try_new(address: u32) -> Result<Self, DmaAddressError> {
        if address & 3 != 0 {
            return Err(DmaAddressError::Unaligned);
        }
        if address < DMA_LOW || address >= DMA_HIGH || FRAME_BUFFER_SIZE as u32 > DMA_HIGH - address
        {
            return Err(DmaAddressError::OutOfRange);
        }
        Ok(Self { address })
    }

    /// Numeric address for the final register-writing boundary.
    ///
    /// Safe MMIO APIs should accept the direction-specific borrowed address
    /// token rather than accepting this numeric value from callers.
    pub const fn as_u32(self) -> u32 {
        self.address
    }

    pub(crate) const fn checked_frame_offset(
        self,
        frame_index: usize,
    ) -> Result<Self, DmaAddressError> {
        let Some(byte_offset) = frame_index.checked_mul(FRAME_BUFFER_SIZE) else {
            return Err(DmaAddressError::RegionTooLarge);
        };
        if byte_offset > u32::MAX as usize {
            return Err(DmaAddressError::RegionTooLarge);
        }
        let byte_offset = byte_offset as u32;
        let Some(address) = self.address.checked_add(byte_offset) else {
            return Err(DmaAddressError::RegionTooLarge);
        };
        Self::try_new(address)
    }

    pub(crate) const fn validates_frame_count(
        self,
        frame_count: usize,
    ) -> Result<(), DmaAddressError> {
        if frame_count == 0 {
            return Err(DmaAddressError::RegionTooLarge);
        }
        match self.checked_frame_offset(frame_count - 1) {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_frame_range_boundaries_are_enforced() {
        assert_eq!(DmaFrameAddress::try_new(DMA_LOW).unwrap().as_u32(), DMA_LOW);
        assert_eq!(
            DmaFrameAddress::try_new(DMA_HIGH - FRAME_BUFFER_SIZE as u32)
                .unwrap()
                .as_u32(),
            DMA_HIGH - FRAME_BUFFER_SIZE as u32
        );
        assert_eq!(
            DmaFrameAddress::try_new(DMA_LOW - 4),
            Err(DmaAddressError::OutOfRange)
        );
        assert_eq!(
            DmaFrameAddress::try_new(DMA_HIGH - FRAME_BUFFER_SIZE as u32 + 4),
            Err(DmaAddressError::OutOfRange)
        );
        assert_eq!(
            DmaFrameAddress::try_new(DMA_LOW + 1),
            Err(DmaAddressError::Unaligned)
        );
    }

    #[test]
    fn complete_pool_span_must_fit() {
        let base = DmaFrameAddress::try_new(DMA_HIGH - 3 * FRAME_BUFFER_SIZE as u32).unwrap();
        assert_eq!(base.validates_frame_count(3), Ok(()));
        assert_eq!(
            base.validates_frame_count(4),
            Err(DmaAddressError::OutOfRange)
        );
        assert_eq!(
            base.validates_frame_count(usize::MAX),
            Err(DmaAddressError::RegionTooLarge)
        );
    }
}
