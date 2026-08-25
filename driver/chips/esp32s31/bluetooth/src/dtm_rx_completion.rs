//! Bounded receive accounting for the ESP32-S31 DTM recycle callback.
//!
//! This is Lower Link Layer state above the controller-memory parser. Each
//! call accounts for exactly one already-returned packet buffer and contains
//! no loop, allocation, MMIO, waker or RTOS dependency. The actual buffer
//! owner and the device-to-CPU visibility fence remain deliberately absent.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError,
};

/// Initial positional byte installed by the complete current DTM environment
/// initializer.
pub const BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE: u8 = 0x7f;

/// Pure receive-count state retained across one active DTM receiver test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxCompletionState {
    received_packet_count: u16,
    last_returned_byte: u8,
}

impl BluetoothDtmRxCompletionState {
    /// Construct the exact initial count and positional-byte image.
    pub const fn new() -> Self {
        Self {
            received_packet_count: 0,
            last_returned_byte: BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE,
        }
    }

    /// Apply the complete per-buffer result transition from the current DTM
    /// recycle body.
    ///
    /// A word with zero low 24 bits updates the positional high byte and
    /// increments the 16-bit count with the same wrapping arithmetic as the
    /// reference body. Any other word changes neither value. In both cases the
    /// reference path next enters its append routine. That routine may
    /// substitute the swap-reserve copy when header bit `+0x10.0` is set, so
    /// the outcome requires only that separate decision and does not claim the
    /// original returned header is recyclable.
    pub const fn account_result_word(
        &mut self,
        result_word: u32,
    ) -> BluetoothDtmRxAccountingOutcome {
        match BluetoothDtmRxResultProjection::from_word(result_word) {
            Ok(result) => {
                self.last_returned_byte = result.returned_byte();
                self.received_packet_count = self.received_packet_count.wrapping_add(1);
                BluetoothDtmRxAccountingOutcome::Counted {
                    received_packet_count: self.received_packet_count,
                    returned_byte: self.last_returned_byte,
                }
            }
            Err(error) => BluetoothDtmRxAccountingOutcome::NotCounted { error },
        }
    }

    /// Count serialized as the two-byte LE Test End return parameter.
    pub const fn received_packet_count(self) -> u16 {
        self.received_packet_count
    }

    /// Return the last accepted positional byte without assigning semantics.
    pub const fn last_returned_byte(self) -> u8 {
        self.last_returned_byte
    }
}

impl Default for BluetoothDtmRxCompletionState {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of accounting for one DTM RX packet result word.
///
/// Both variants require the caller's future affine completed-header owner to
/// enter the separately modeled append decision. This value cannot decide
/// whether the ordinary header or swap-reserve copy is appended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "every DTM RX result still requires the separate append decision"]
pub enum BluetoothDtmRxAccountingOutcome {
    /// The result word updated the positional byte and wrapping packet count.
    Counted {
        /// Count after this buffer was accepted.
        received_packet_count: u16,
        /// Positional byte copied from packet-buffer offset `+0x0f`.
        returned_byte: u8,
    },
    /// The low 24 bits prevented this buffer from changing DTM result state.
    NotCounted {
        /// Exact positional validation failure.
        error: BluetoothDtmRxResultProjectionError,
    },
}

impl BluetoothDtmRxAccountingOutcome {
    /// Every result branch enters the append path after accounting.
    pub const fn append_transition_required(self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmRxResultProjectionError;

    use super::{
        BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE, BluetoothDtmRxAccountingOutcome,
        BluetoothDtmRxCompletionState,
    };

    #[test]
    fn initial_state_matches_the_complete_current_environment_initializer() {
        let state = BluetoothDtmRxCompletionState::new();

        assert_eq!(state.received_packet_count(), 0);
        assert_eq!(
            state.last_returned_byte(),
            BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE
        );
    }

    #[test]
    fn accepted_word_updates_byte_and_count_once() {
        let mut state = BluetoothDtmRxCompletionState::new();

        assert_eq!(
            state.account_result_word(0xa500_0000),
            BluetoothDtmRxAccountingOutcome::Counted {
                received_packet_count: 1,
                returned_byte: 0xa5,
            }
        );
        assert_eq!(state.received_packet_count(), 1);
        assert_eq!(state.last_returned_byte(), 0xa5);
    }

    #[test]
    fn rejected_word_preserves_state_but_still_requires_append_decision() {
        let mut state = BluetoothDtmRxCompletionState::new();
        let accepted = state.account_result_word(0x3100_0000);
        let rejected = state.account_result_word(0xff00_0001);

        assert!(accepted.append_transition_required());
        assert_eq!(
            rejected,
            BluetoothDtmRxAccountingOutcome::NotCounted {
                error: BluetoothDtmRxResultProjectionError::NonzeroLowTwentyFourBits,
            }
        );
        assert!(rejected.append_transition_required());
        assert_eq!(state.received_packet_count(), 1);
        assert_eq!(state.last_returned_byte(), 0x31);
    }

    #[test]
    fn count_uses_the_complete_wrapping_u16_transition() {
        let mut state = BluetoothDtmRxCompletionState::new();

        for _ in 0..=u16::MAX {
            let outcome = state.account_result_word(0);
            assert!(outcome.append_transition_required());
        }

        assert_eq!(state.received_packet_count(), 0);
        assert_eq!(state.last_returned_byte(), 0);
    }
}
