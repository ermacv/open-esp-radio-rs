//! Positional result word in one returned DTM RX packet slot.

#![forbid(unsafe_code)]

/// Validated result word consumed by the reviewed ESP32-S31 DTM RX callback.
///
/// Complete current body `r_sym_ble_PptSRbXfefQwMVyO5jxP` reads this word at
/// returned packet-buffer offset `+0x0c`. It accepts the word only when its
/// low 24 bits are zero and copies the high byte from offset `+0x0f` into DTM
/// state. This controller-memory projection intentionally does not name that
/// byte as RSSI, length, CRC or status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxResultProjection(u32);

/// Why a positional DTM RX result word is not counted by the reviewed path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRxResultProjectionError {
    /// At least one of the low 24 bits is nonzero.
    NonzeroLowTwentyFourBits,
}

impl BluetoothDtmRxResultProjection {
    /// Apply the exact validation performed at returned-buffer offset `+0x0c`.
    pub const fn from_word(word: u32) -> Result<Self, BluetoothDtmRxResultProjectionError> {
        if word & 0x00ff_ffff != 0 {
            return Err(BluetoothDtmRxResultProjectionError::NonzeroLowTwentyFourBits);
        }
        Ok(Self(word))
    }

    /// Return the still-positional byte at returned-buffer offset `+0x0f`.
    pub const fn returned_byte(self) -> u8 {
        (self.0 >> 24) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::{BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError};

    #[test]
    fn result_projection_preserves_only_the_reviewed_high_byte() {
        assert_eq!(
            BluetoothDtmRxResultProjection::from_word(0)
                .expect("an all-zero result word is accepted")
                .returned_byte(),
            0
        );
        assert_eq!(
            BluetoothDtmRxResultProjection::from_word(0xab00_0000)
                .expect("the high byte is positional payload")
                .returned_byte(),
            0xab
        );
        assert_eq!(
            BluetoothDtmRxResultProjection::from_word(0x0000_0001),
            Err(BluetoothDtmRxResultProjectionError::NonzeroLowTwentyFourBits)
        );
        assert_eq!(
            BluetoothDtmRxResultProjection::from_word(0x00ff_ffff),
            Err(BluetoothDtmRxResultProjectionError::NonzeroLowTwentyFourBits)
        );
    }
}
