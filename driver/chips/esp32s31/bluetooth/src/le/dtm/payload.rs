//! Allocation-free Direct Test Mode transmitter payload preparation.
//!
//! Complete current and initial ESP32-S31 `dtm_tx_create_ctx` bodies select
//! the eight HCI patterns below. Their two 255-byte PRBS tables are identical
//! across those public revisions. The generators reproduce the complete table
//! images without retaining a vendor allocator, object layout or extracted
//! table in production code.

#![forbid(unsafe_code)]

/// Why an HCI DTM payload-pattern selector cannot be represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmPayloadPatternError {
    /// The reviewed transmitter accepts exactly selector images `0..=7`.
    UnsupportedHciSelector,
}

/// The eight payload patterns accepted by the reviewed DTM transmitter.
///
/// Multi-bit names describe the transmitted least-significant-bit-first bit
/// sequence. The corresponding in-memory repeated byte is documented on each
/// variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BluetoothDtmPayloadPattern {
    /// PRBS9, HCI selector zero.
    Prbs9 = 0,
    /// Repeated `11110000`, stored as `0x0f`, HCI selector one.
    Repeated11110000 = 1,
    /// Repeated `10101010`, stored as `0x55`, HCI selector two.
    Repeated10101010 = 2,
    /// PRBS15, HCI selector three.
    Prbs15 = 3,
    /// Repeated `11111111`, stored as `0xff`, HCI selector four.
    RepeatedAllOnes = 4,
    /// Repeated `00000000`, stored as `0x00`, HCI selector five.
    RepeatedAllZeros = 5,
    /// Repeated `00001111`, stored as `0xf0`, HCI selector six.
    Repeated00001111 = 6,
    /// Repeated `01010101`, stored as `0xaa`, HCI selector seven.
    Repeated01010101 = 7,
}

impl BluetoothDtmPayloadPattern {
    /// Decode the complete selector domain accepted by the TX command body.
    pub const fn from_hci_selector(selector: u8) -> Result<Self, BluetoothDtmPayloadPatternError> {
        match selector {
            0 => Ok(Self::Prbs9),
            1 => Ok(Self::Repeated11110000),
            2 => Ok(Self::Repeated10101010),
            3 => Ok(Self::Prbs15),
            4 => Ok(Self::RepeatedAllOnes),
            5 => Ok(Self::RepeatedAllZeros),
            6 => Ok(Self::Repeated00001111),
            7 => Ok(Self::Repeated01010101),
            _ => Err(BluetoothDtmPayloadPatternError::UnsupportedHciSelector),
        }
    }

    /// Return the HCI payload-pattern selector image.
    pub const fn hci_selector(self) -> u8 {
        self as u8
    }

    /// Prepare exactly `length` bytes in caller-owned storage.
    ///
    /// Generation is bounded by the eight-bit HCI length image. No allocation,
    /// hardware wait, scheduler publication or ownership transfer occurs.
    pub fn prepare<'storage>(
        self,
        length: BluetoothDtmPayloadLength,
        storage: &'storage mut [u8],
    ) -> Result<BluetoothDtmPreparedPayload<'storage>, BluetoothDtmPayloadPreparationError> {
        let payload_length = length.as_usize();
        if storage.len() < payload_length {
            return Err(BluetoothDtmPayloadPreparationError::StorageTooShort);
        }

        let payload = &mut storage[..payload_length];
        self.fill_reviewed(payload);

        Ok(BluetoothDtmPreparedPayload {
            pattern: self,
            length,
            storage,
        })
    }

    pub(crate) fn fill_reviewed(self, payload: &mut [u8]) {
        match self {
            Self::Prbs9 => fill_prbs(payload, 9, 0, 4),
            Self::Repeated11110000 => payload.fill(0x0f),
            Self::Repeated10101010 => payload.fill(0x55),
            Self::Prbs15 => fill_prbs(payload, 15, 6, 10),
            Self::RepeatedAllOnes => payload.fill(0xff),
            Self::RepeatedAllZeros => payload.fill(0x00),
            Self::Repeated00001111 => payload.fill(0xf0),
            Self::Repeated01010101 => payload.fill(0xaa),
        }
    }
}

/// The complete eight-bit DTM payload-length image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmPayloadLength(u8);

impl BluetoothDtmPayloadLength {
    /// Retain an HCI payload-length byte without widening its domain.
    pub const fn from_hci_image(length: u8) -> Self {
        Self(length)
    }

    /// Return the HCI payload-length image.
    pub const fn hci_image(self) -> u8 {
        self.0
    }

    /// Return the bounded host index used for caller-owned storage.
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Why caller-owned storage could not hold the requested DTM payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmPayloadPreparationError {
    /// The destination is shorter than the complete HCI length image.
    StorageTooShort,
}

/// A prepared, CPU-owned DTM payload view.
///
/// This affine borrow prevents mutation through the original storage binding
/// while prepared. It is not a hardware-owned descriptor or publication token.
#[derive(Debug)]
pub struct BluetoothDtmPreparedPayload<'storage> {
    pattern: BluetoothDtmPayloadPattern,
    length: BluetoothDtmPayloadLength,
    storage: &'storage mut [u8],
}

impl<'storage> BluetoothDtmPreparedPayload<'storage> {
    /// Return the selected HCI pattern.
    pub const fn pattern(&self) -> BluetoothDtmPayloadPattern {
        self.pattern
    }

    /// Return the complete HCI length image.
    pub const fn length(&self) -> BluetoothDtmPayloadLength {
        self.length
    }

    /// Borrow only the initialized payload prefix.
    pub fn bytes(&self) -> &[u8] {
        &self.storage[..self.length.as_usize()]
    }

    /// End preparation and return the complete caller-owned storage slice.
    pub fn release(self) -> &'storage mut [u8] {
        self.storage
    }
}

fn fill_prbs(payload: &mut [u8], width: u32, first_tap: u32, second_tap: u32) {
    let mut state = (1_u16 << width) - 1;

    for output in payload {
        let mut image = 0_u8;
        for output_bit in 0..8 {
            image |= ((state & 1) as u8) << output_bit;
            let feedback = ((state >> first_tap) ^ (state >> second_tap)) & 1;
            state = (state >> 1) | (feedback << (width - 1));
        }
        *output = image;
    }
}

#[cfg(test)]
mod tests;
