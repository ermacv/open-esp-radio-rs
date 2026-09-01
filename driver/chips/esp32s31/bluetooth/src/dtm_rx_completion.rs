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
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmRxRssi;

/// Pure receive-count state retained across one active DTM receiver test.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothDtmReceiverSession {
    received_packet_count: u16,
    last_rssi: Option<BluetoothDtmRxRssi>,
}

impl BluetoothDtmReceiverSession {
    /// Construct an empty semantic session.
    ///
    /// The vendor environment stores `0x7f` before the first accepted packet.
    /// Rust retains absence as `None` instead of exposing that storage sentinel
    /// as a measured RSSI sample.
    pub const fn new() -> Self {
        Self {
            received_packet_count: 0,
            last_rssi: None,
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
                self.last_rssi = Some(result.rssi());
                self.received_packet_count = self.received_packet_count.wrapping_add(1);
                BluetoothDtmRxCompletionOutcome::Counted {
                    received_packet_count: self.received_packet_count,
                    rssi: result.rssi(),
                }
            }
            Err(error) => BluetoothDtmRxCompletionOutcome::NotCounted { error },
        }
    }

    /// Count serialized as the two-byte LE Test End return parameter.
    pub const fn received_packet_count(&self) -> u16 {
        self.received_packet_count
    }

    /// Return the last accepted signed RSSI, if one packet has been counted.
    pub const fn last_rssi(&self) -> Option<BluetoothDtmRxRssi> {
        self.last_rssi
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
    /// The result word updated the signed RSSI and wrapping packet count.
    Counted {
        /// Count after this buffer was accepted.
        received_packet_count: u16,
        /// Signed controller RSSI copied from packet-buffer offset `+0x0f`.
        rssi: BluetoothDtmRxRssi,
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
        BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError, BluetoothDtmRxRssi,
    };

    use super::{BluetoothDtmReceiverSession, BluetoothDtmRxCompletionOutcome};

    #[test]
    fn initial_state_has_no_accepted_rssi_sample() {
        let state = BluetoothDtmReceiverSession::new();

        assert_eq!(state.received_packet_count(), 0);
        assert_eq!(state.last_rssi(), None);
    }

    #[test]
    fn accepted_word_updates_signed_rssi_and_count_once() {
        let mut state = BluetoothDtmReceiverSession::new();

        assert_eq!(
            state.account_projection(BluetoothDtmRxResultProjection::from_word(0xa500_0000)),
            BluetoothDtmRxCompletionOutcome::Counted {
                received_packet_count: 1,
                rssi: BluetoothDtmRxRssi::from_controller_value(-91),
            }
        );
        assert_eq!(state.received_packet_count(), 1);
        assert_eq!(
            state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
            Some(-91)
        );
    }

    #[test]
    fn accepted_rssi_preserves_the_signed_controller_domain() {
        let mut state = BluetoothDtmReceiverSession::new();

        assert_eq!(
            state.account_projection(BluetoothDtmRxResultProjection::from_word(0xff00_0000)),
            BluetoothDtmRxCompletionOutcome::Counted {
                received_packet_count: 1,
                rssi: BluetoothDtmRxRssi::from_controller_value(-1),
            }
        );
        assert_eq!(
            state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
            Some(-1)
        );
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
        assert_eq!(
            state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
            Some(49)
        );
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
        assert_eq!(
            state.last_rssi().map(BluetoothDtmRxRssi::controller_value),
            Some(0)
        );
    }
}
