//! Host-controlled DTM on LE 1M, channel 0, 37-byte PRBS9 payloads.
//! An active test has a fixed 30-second lease; expiry is a failed observation.

use serde::{Deserialize, Serialize};

/// Exact ACL payload used by the peripheral recovery workload.
///
/// The first four bytes form one complete L2CAP-shaped header. The full packet
/// reaches the Controller's declared 251-byte ACL limit and therefore crosses
/// ten legacy 27-byte Link Layer Data PDUs in the no-DLE test profile.
pub const BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES: usize = 251;

/// Number of legacy Data PDUs needed for the recovery workload's ACL payload.
pub const BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS: u32 = 10;

/// Exact post-establishment connection interval requested by the Linux central.
pub const BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS: u16 = 120;

/// Construct one sequenced deterministic payload shared by both HIL Hosts.
pub const fn bluetooth_peripheral_acl_payload_for_sequence(
    sequence: u8,
) -> [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] {
    let mut payload = [0; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES];
    let l2cap_payload = (BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES - 4) as u16;
    let length = l2cap_payload.to_le_bytes();
    payload[0] = length[0];
    payload[1] = length[1];
    payload[2] = 0xff;
    payload[3] = 0xff;
    let mut index = 4;
    while index < payload.len() {
        payload[index] = (((index as u16 * 73 + 19) & 0xff) as u8) ^ sequence;
        index += 1;
    }
    payload
}

/// Construct the first deterministic peripheral ACL payload.
pub const fn bluetooth_peripheral_acl_payload() -> [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] {
    bluetooth_peripheral_acl_payload_for_sequence(0)
}

/// How one peripheral HIL cycle asks the established connection to end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BluetoothPeripheralTermination {
    #[default]
    PeerReset,
    PeerRfkill,
    TargetDisconnect,
    TargetReset,
    /// Historical scenario identity retained only for sealed-run decoding.
    #[serde(rename = "peer-power-off")]
    LegacyPeerPowerOff,
}

/// Diagnostic peripheral operation; ending the whole probe requires a board reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothPeripheralOperation {
    StartAdvertising {
        termination: BluetoothPeripheralTermination,
        hold_millis: u16,
    },
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
    /// Successful, profile-valid Connection Update Complete events consumed by the Host.
    pub connection_update_complete_events: u32,
    /// Channel Map Update instants applied by the Link Layer.
    #[serde(default)]
    pub channel_map_update_events: u32,
    /// Successful target Host Disconnect Command Status responses.
    pub target_disconnect_commands: u32,
    /// Successful target Host Reset Command Complete responses while connected.
    pub target_reset_commands: u32,
    /// Connection, update or disconnection events that failed profile checks or decoding.
    pub host_event_faults: u32,
    /// Nonempty ACL packets delivered by the Controller to the target Host.
    pub host_acl_received_packets: u32,
    /// Validated ACL echo packets accepted from the target Host by HCI.
    pub host_acl_queued_packets: u32,
    /// Target Host ACL packets reported complete after Link Layer acknowledgement.
    #[serde(default)]
    pub host_acl_transmitted_packets: u32,
    /// Controller-to-Host ACL packet credits returned by the target Host.
    pub host_acl_completed_packets: u32,
    /// First-packet credits deliberately held to force bounded Controller backpressure.
    pub host_acl_backpressure_holds: u32,
    /// Controller ACL packets that violated the single-link HIL echo profile or could not be queued.
    pub host_acl_faults: u32,
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
    fn peripheral_acl_probe_fills_the_legacy_fragmentation_envelope() {
        let payload = bluetooth_peripheral_acl_payload();
        let second = bluetooth_peripheral_acl_payload_for_sequence(1);
        assert_eq!(payload.len(), BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES);
        assert_eq!(u16::from_le_bytes([payload[0], payload[1]]), 247);
        assert_eq!(&payload[2..4], &[0xff, 0xff]);
        assert_eq!(
            payload.len().div_ceil(27),
            BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS as usize
        );
        assert!(
            payload[4..]
                .windows(27)
                .all(|window| window[0] != window[26])
        );
        assert_eq!(&payload[..4], &second[..4]);
        assert!(payload[4..].iter().zip(&second[4..]).all(|(a, b)| a != b));
    }

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
                    connection_update_complete_events: 1,
                    channel_map_update_events: 1,
                    target_disconnect_commands: 1,
                    target_reset_commands: 1,
                    host_event_faults: 0,
                    host_acl_received_packets: 1,
                    host_acl_queued_packets: 1,
                    host_acl_transmitted_packets: 1,
                    host_acl_completed_packets: 1,
                    host_acl_backpressure_holds: 1,
                    host_acl_faults: 0,
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
            operation: BluetoothPeripheralOperation::StartAdvertising {
                termination: BluetoothPeripheralTermination::PeerReset,
                hold_millis: 100,
            },
            result: BluetoothPeripheralResult::Started {
                address: [1, 2, 3, 4, 5, 6],
            },
            advertising_runs: 1,
            peripheral_runs: 0,
            peripheral_disconnections: 0,
            connection_complete_events: 0,
            disconnection_complete_events: 0,
            connection_update_complete_events: 0,
            channel_map_update_events: 0,
            target_disconnect_commands: 0,
            target_reset_commands: 0,
            host_event_faults: 0,
            host_acl_received_packets: 0,
            host_acl_queued_packets: 0,
            host_acl_transmitted_packets: 0,
            host_acl_completed_packets: 0,
            host_acl_backpressure_holds: 0,
            host_acl_faults: 0,
            last_disconnect_reason: None,
            retries: 0,
            terminal: false,
            saturated: false,
            detail_truncated: false,
            detail: heapless::String::new(),
        };
        assert_eq!(
            evidence.started_address(BluetoothPeripheralOperation::StartAdvertising {
                termination: BluetoothPeripheralTermination::PeerReset,
                hold_millis: 100,
            }),
            Some([1, 2, 3, 4, 5, 6])
        );
        assert_eq!(
            evidence.started_address(BluetoothPeripheralOperation::Snapshot),
            None
        );
        assert!(
            !evidence.is_snapshot(BluetoothPeripheralOperation::StartAdvertising {
                termination: BluetoothPeripheralTermination::PeerReset,
                hold_millis: 100,
            })
        );
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
