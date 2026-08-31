//! Positional result word in one returned DTM RX packet slot.

#![forbid(unsafe_code)]

/// Validated result word consumed by the reviewed ESP32-S31 DTM RX callback.
///
/// Complete current linked body
/// `ble-controller:r_sym_ble_kdHGLPeGDJlAvxmbjQ6e` reads this word at returned
/// packet-buffer offset `+0x0c`. It accepts the word only when its low 24 bits
/// are zero and copies the high byte from offset `+0x0f` into DTM state.
/// Dead-stripped raw-archive body
/// `r_sym_ble_PptSRbXfefQwMVyO5jxP` independently corroborates that positional
/// transform but is not its linked effect authority. This controller-memory
/// projection intentionally does not name the byte as RSSI, length, CRC or
/// status.
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
