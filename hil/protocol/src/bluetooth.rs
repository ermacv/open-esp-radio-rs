//! Host-controlled DTM on LE 1M, channel 0, 37-byte PRBS9 payloads.
//! An active test has a fixed 30-second lease; expiry is a failed observation.

use serde::{Deserialize, Serialize};

/// Diagnostic connection establishment probe; ending it requires a board reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothPeripheralOperation {
    StartAdvertising,
    Snapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothPeripheralResult {
    Started { address: [u8; 6] },
    Snapshot,
    HciRejected { command_stage: u8 },
    Timeout,
    LeaseExpired,
}

/// Publication counts are software observations, not successful peer exchanges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothPeripheralEvidence {
    pub operation: BluetoothPeripheralOperation,
    pub result: BluetoothPeripheralResult,
    pub advertising_runs: u32,
    pub peripheral_runs: u32,
    pub peripheral_disconnections: u32,
    /// Successful, profile-valid LE Connection Complete events consumed by the Host.
    pub connection_complete_events: u32,
    /// Successful, profile-valid Disconnection Complete events consumed by the Host.
    pub disconnection_complete_events: u32,
    /// Connection/disconnection events that failed the HIL profile checks or decoding.
    pub host_event_faults: u32,
    pub last_disconnect_reason: Option<u8>,
    pub retries: u32,
    pub terminal: bool,
    pub saturated: bool,
    pub detail_truncated: bool,
    pub detail: heapless::String<128>,
}

impl BluetoothPeripheralEvidence {
    pub fn started_address(&self, requested: BluetoothPeripheralOperation) -> Option<[u8; 6]> {
        (self.operation == requested).then_some(())?;
        match self.result {
            BluetoothPeripheralResult::Started { address } => Some(address),
            _ => None,
        }
    }

    pub fn is_snapshot(&self, requested: BluetoothPeripheralOperation) -> bool {
        self.operation == requested && self.result == BluetoothPeripheralResult::Snapshot
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothDtmOperation {
    Reset,
    Receive,
    Transmit,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothDtmResult {
    Complete { received_packets: Option<u16> },
    HciRejected,
    Timeout,
    LeaseExpired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothDtmEvidence {
    pub operation: BluetoothDtmOperation,
    pub result: BluetoothDtmResult,
    pub rx_diagnostics: BluetoothDtmRxDiagnostics,
}

/// Cumulative production RX recycle observations since this firmware boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothDtmRxDiagnostics {
    pub sequence_checks: u32,
    pub sequence_deadline_rejections: u32,
    pub last_sequence_lead_ticks: i32,
    pub successful_events: u32,
    pub failed_events: u32,
    pub last_failure_status: u32,
    pub empty_events: u32,
    pub counted_packets: u32,
    pub rejected_packets: u32,
}

impl BluetoothDtmEvidence {
    pub fn completed(self, requested: BluetoothDtmOperation) -> bool {
        self.operation == requested
            && matches!(self.result,
            BluetoothDtmResult::Complete { received_packets }
                if received_packets.is_some() == (requested == BluetoothDtmOperation::End))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peripheral_publication_and_terminal_evidence_survive_framing() {
        for result in [
            BluetoothPeripheralResult::Started {
                address: [1, 2, 3, 4, 5, 6],
            },
            BluetoothPeripheralResult::Snapshot,
            BluetoothPeripheralResult::HciRejected { command_stage: 4 },
            BluetoothPeripheralResult::Timeout,
            BluetoothPeripheralResult::LeaseExpired,
        ] {
            let expected = crate::Envelope::new(
                7,
                8,
                0,
                9,
                crate::Event::BluetoothPeripheral(BluetoothPeripheralEvidence {
                    operation: BluetoothPeripheralOperation::Snapshot,
                    result,
                    advertising_runs: u32::MAX,
                    peripheral_runs: 2,
                    peripheral_disconnections: 1,
                    connection_complete_events: 1,
                    disconnection_complete_events: 1,
                    host_event_faults: 0,
                    last_disconnect_reason: Some(8),
                    retries: 3,
                    terminal: true,
                    saturated: true,
                    detail_truncated: false,
                    detail: "peripheral active: TimingPolicyUnavailable"
                        .try_into()
                        .unwrap(),
                }),
            );
            let mut encoder = crate::FrameEncoder::new();
            let mut decoder = crate::FrameDecoder::new();
            let mut observed = None;
            decoder.feed(encoder.encode(&expected).unwrap(), |frame| {
                observed = Some(frame.unwrap())
            });
            assert_eq!(observed, Some(expected));
        }
    }

    #[test]
    fn peripheral_completion_helpers_reject_wrong_operations_and_results() {
        let evidence = BluetoothPeripheralEvidence {
            operation: BluetoothPeripheralOperation::StartAdvertising,
            result: BluetoothPeripheralResult::Started {
                address: [1, 2, 3, 4, 5, 6],
            },
            advertising_runs: 1,
            peripheral_runs: 0,
            peripheral_disconnections: 0,
            connection_complete_events: 0,
            disconnection_complete_events: 0,
            host_event_faults: 0,
            last_disconnect_reason: None,
            retries: 0,
            terminal: false,
            saturated: false,
            detail_truncated: false,
            detail: heapless::String::new(),
        };
        assert_eq!(
            evidence.started_address(BluetoothPeripheralOperation::StartAdvertising),
            Some([1, 2, 3, 4, 5, 6])
        );
        assert_eq!(
            evidence.started_address(BluetoothPeripheralOperation::Snapshot),
            None
        );
        assert!(!evidence.is_snapshot(BluetoothPeripheralOperation::StartAdvertising));
        assert!(
            BluetoothPeripheralEvidence {
                operation: BluetoothPeripheralOperation::Snapshot,
                result: BluetoothPeripheralResult::Snapshot,
                ..evidence
            }
            .is_snapshot(BluetoothPeripheralOperation::Snapshot)
        );
    }

    #[test]
    fn rx_deadline_diagnostics_survive_framed_transport() {
        let expected = crate::Envelope::new(
            3,
            4,
            0,
            5,
            crate::Event::BluetoothDtm(BluetoothDtmEvidence {
                operation: BluetoothDtmOperation::End,
                result: BluetoothDtmResult::Complete {
                    received_packets: Some(0),
                },
                rx_diagnostics: BluetoothDtmRxDiagnostics {
                    sequence_checks: u32::MAX,
                    sequence_deadline_rejections: 1371,
                    last_sequence_lead_ticks: -1,
                    successful_events: 2,
                    ..Default::default()
                },
            }),
        );
        let mut encoder = crate::FrameEncoder::new();
        let mut decoder = crate::FrameDecoder::new();
        let mut observed = None;
        decoder.feed(encoder.encode(&expected).unwrap(), |result| {
            observed = Some(result.unwrap());
        });
        assert_eq!(observed, Some(expected));
    }
    #[test]
    fn only_correlated_complete_results_with_correct_count_scope_pass() {
        let evidence = BluetoothDtmEvidence {
            rx_diagnostics: BluetoothDtmRxDiagnostics::default(),
            operation: BluetoothDtmOperation::End,
            result: BluetoothDtmResult::Complete {
                received_packets: Some(0),
            },
        };
        assert!(evidence.completed(BluetoothDtmOperation::End));
        assert!(!evidence.completed(BluetoothDtmOperation::Receive));
        for result in [
            BluetoothDtmResult::HciRejected,
            BluetoothDtmResult::Timeout,
            BluetoothDtmResult::LeaseExpired,
            BluetoothDtmResult::Complete {
                received_packets: None,
            },
        ] {
            assert!(
                !BluetoothDtmEvidence { result, ..evidence }.completed(BluetoothDtmOperation::End)
            );
        }
    }
}
