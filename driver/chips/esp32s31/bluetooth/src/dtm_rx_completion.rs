//! Bounded receive accounting for the ESP32-S31 DTM recycle callback.
//!
//! This is Lower Link Layer state above the controller-memory parser. Each
//! call consumes exactly one lower returned-packet observation and contains no
//! loop, allocation, MMIO, waker or RTOS dependency. The session itself is
//! retained by the affine event chain, so an observation cannot be accounted
//! twice or applied to another graph.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmRxResultProjection;
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmRxResultProjectionError;

/// Initial positional byte installed by the complete current DTM environment
/// initializer.
pub const BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE: u8 = 0x7f;

/// Pure receive-count state retained across one active DTM receiver test.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothDtmReceiverSession {
    received_packet_count: u16,
    last_returned_byte: u8,
}

impl BluetoothDtmReceiverSession {
    /// Construct the exact initial count and positional-byte image.
    pub const fn new() -> Self {
        Self {
            received_packet_count: 0,
            last_returned_byte: BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE,
        }
    }

    /// Account one semantic projection retained by its lower packet owner.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn account_projection(
        &mut self,
        projection: Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError>,
    ) -> BluetoothDtmRxCompletionOutcome {
        match projection {
            Ok(result) => {
                self.last_returned_byte = result.returned_byte();
                self.received_packet_count = self.received_packet_count.wrapping_add(1);
                BluetoothDtmRxCompletionOutcome::Counted {
                    received_packet_count: self.received_packet_count,
                    returned_byte: self.last_returned_byte,
                }
            }
            Err(error) => BluetoothDtmRxCompletionOutcome::NotCounted { error },
        }
    }

    /// Count serialized as the two-byte LE Test End return parameter.
    pub const fn received_packet_count(&self) -> u16 {
        self.received_packet_count
    }

    /// Return the last accepted positional byte without assigning semantics.
    pub const fn last_returned_byte(&self) -> u8 {
        self.last_returned_byte
    }
}

impl Default for BluetoothDtmReceiverSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of one completed lower RX drain/rotation transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the DTM RX completion outcome must reach the test owner"]
pub enum BluetoothDtmRxCompletionOutcome {
    /// The scheduler event returned no completed packet.
    NoReturnedPacket,
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

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError,
    };

    use super::{
        BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE, BluetoothDtmReceiverSession,
        BluetoothDtmRxCompletionOutcome,
    };

    #[test]
    fn initial_state_matches_the_complete_current_environment_initializer() {
        let state = BluetoothDtmReceiverSession::new();

        assert_eq!(state.received_packet_count(), 0);
        assert_eq!(
            state.last_returned_byte(),
            BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE
        );
    }

    #[test]
    fn accepted_word_updates_byte_and_count_once() {
        let mut state = BluetoothDtmReceiverSession::new();

        assert_eq!(
            state.account_projection(BluetoothDtmRxResultProjection::from_word(0xa500_0000)),
            BluetoothDtmRxCompletionOutcome::Counted {
                received_packet_count: 1,
                returned_byte: 0xa5,
            }
        );
        assert_eq!(state.received_packet_count(), 1);
        assert_eq!(state.last_returned_byte(), 0xa5);
    }

    #[test]
    fn rejected_projection_preserves_state_after_rearm() {
        let mut state = BluetoothDtmReceiverSession::new();
        let accepted =
            state.account_projection(BluetoothDtmRxResultProjection::from_word(0x3100_0000));
        let rejected =
            state.account_projection(BluetoothDtmRxResultProjection::from_word(0xff00_0001));

        assert!(matches!(
            accepted,
            BluetoothDtmRxCompletionOutcome::Counted { .. }
        ));
        assert_eq!(
            rejected,
            BluetoothDtmRxCompletionOutcome::NotCounted {
                error: BluetoothDtmRxResultProjectionError::NonzeroLowTwentyFourBits,
            }
        );
        assert_eq!(state.received_packet_count(), 1);
        assert_eq!(state.last_returned_byte(), 0x31);
    }

    #[test]
    fn count_uses_the_complete_wrapping_u16_transition() {
        let mut state = BluetoothDtmReceiverSession::new();

        for _ in 0..=u16::MAX {
            let outcome = state.account_projection(BluetoothDtmRxResultProjection::from_word(0));
            assert!(matches!(
                outcome,
                BluetoothDtmRxCompletionOutcome::Counted { .. }
            ));
        }

        assert_eq!(state.received_packet_count(), 0);
        assert_eq!(state.last_returned_byte(), 0);
    }
}
