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
mod tests {
    use super::{
        BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern, BluetoothDtmPayloadPatternError,
        BluetoothDtmPayloadPreparationError,
    };

    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    #[test]
    fn every_hci_pattern_selector_roundtrips_and_bounds_the_domain() {
        let patterns = [
            BluetoothDtmPayloadPattern::Prbs9,
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadPattern::Repeated10101010,
            BluetoothDtmPayloadPattern::Prbs15,
            BluetoothDtmPayloadPattern::RepeatedAllOnes,
            BluetoothDtmPayloadPattern::RepeatedAllZeros,
            BluetoothDtmPayloadPattern::Repeated00001111,
            BluetoothDtmPayloadPattern::Repeated01010101,
        ];

        for (selector, pattern) in patterns.into_iter().enumerate() {
            assert_eq!(pattern.hci_selector(), selector as u8);
            assert_eq!(
                BluetoothDtmPayloadPattern::from_hci_selector(selector as u8),
                Ok(pattern)
            );
        }
        assert_eq!(
            BluetoothDtmPayloadPattern::from_hci_selector(8),
            Err(BluetoothDtmPayloadPatternError::UnsupportedHciSelector)
        );
    }

    #[test]
    fn repeated_patterns_match_all_complete_vendor_branches() {
        let cases = [
            (BluetoothDtmPayloadPattern::Repeated11110000, 0x0f),
            (BluetoothDtmPayloadPattern::Repeated10101010, 0x55),
            (BluetoothDtmPayloadPattern::RepeatedAllOnes, 0xff),
            (BluetoothDtmPayloadPattern::RepeatedAllZeros, 0x00),
            (BluetoothDtmPayloadPattern::Repeated00001111, 0xf0),
            (BluetoothDtmPayloadPattern::Repeated01010101, 0xaa),
        ];

        for (pattern, expected) in cases {
            let mut storage = [0xa5; 255];
            let prepared = pattern
                .prepare(BluetoothDtmPayloadLength::from_hci_image(255), &mut storage)
                .expect("full HCI payload fits");
            assert_eq!(prepared.bytes(), &[expected; 255]);
        }
    }

    #[test]
    fn prbs9_matches_the_complete_cross_revision_table_fingerprint() {
        let mut storage = [0; 255];
        let prepared = BluetoothDtmPayloadPattern::Prbs9
            .prepare(BluetoothDtmPayloadLength::from_hci_image(255), &mut storage)
            .expect("full PRBS9 payload fits");

        assert_eq!(
            &prepared.bytes()[..16],
            &[
                0xff, 0xc1, 0xfb, 0xe8, 0x4c, 0x90, 0x72, 0x8b, 0xe7, 0xb3, 0x51, 0x89, 0x63, 0xab,
                0x23, 0x23
            ]
        );
        assert_eq!(fnv1a64(prepared.bytes()), 0x94db_648c_b178_dce3);
    }

    #[test]
    fn prbs15_matches_the_complete_cross_revision_table_fingerprint() {
        let mut storage = [0; 255];
        let prepared = BluetoothDtmPayloadPattern::Prbs15
            .prepare(BluetoothDtmPayloadLength::from_hci_image(255), &mut storage)
            .expect("full PRBS15 payload fits");

        assert_eq!(
            &prepared.bytes()[..16],
            &[
                0xff, 0x7f, 0xf0, 0x3e, 0x3a, 0x13, 0xa4, 0xdc, 0xe2, 0xf9, 0x6c, 0x54, 0xe2, 0xd8,
                0xea, 0xc8
            ]
        );
        assert_eq!(fnv1a64(prepared.bytes()), 0x4655_41b8_492c_b9ba);
    }

    #[test]
    fn preparation_is_fail_closed_and_returns_the_whole_storage() {
        let mut short = [0xa5; 3];
        assert!(matches!(
            BluetoothDtmPayloadPattern::RepeatedAllZeros
                .prepare(BluetoothDtmPayloadLength::from_hci_image(4), &mut short,),
            Err(BluetoothDtmPayloadPreparationError::StorageTooShort)
        ));
        assert_eq!(short, [0xa5; 3]);

        let mut storage = [0xa5; 8];
        let prepared = BluetoothDtmPayloadPattern::RepeatedAllZeros
            .prepare(BluetoothDtmPayloadLength::from_hci_image(3), &mut storage)
            .expect("prefix fits");
        assert_eq!(
            prepared.pattern(),
            BluetoothDtmPayloadPattern::RepeatedAllZeros
        );
        assert_eq!(prepared.length().hci_image(), 3);
        assert_eq!(prepared.bytes(), [0, 0, 0]);
        assert_eq!(prepared.release(), [0, 0, 0, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5]);
    }

    #[test]
    fn zero_length_preparation_is_defined_for_every_pattern() {
        for selector in 0..=7 {
            let pattern = BluetoothDtmPayloadPattern::from_hci_selector(selector)
                .expect("selector belongs to the complete domain");
            let mut storage = [];
            let prepared = pattern
                .prepare(BluetoothDtmPayloadLength::from_hci_image(0), &mut storage)
                .expect("zero bytes require no storage");
            assert!(prepared.bytes().is_empty());
        }
    }
}
