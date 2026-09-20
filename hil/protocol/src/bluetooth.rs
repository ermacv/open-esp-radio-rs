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

/// Diagnostic peripheral operation within one board boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothPeripheralOperation {
    StartAdvertising {
        termination: BluetoothPeripheralTermination,
        hold_millis: u16,
    },
    /// Enable the fixed-key Controller encryption diagnostic while idle.
    EncryptedAcl {
        enabled: bool,
        failure: Option<BluetoothSecurityFailure>,
    },
    Snapshot,
    /// Select the ATT Host that can retain one RX credit while draining HCI events.
    AclBackpressure {
        enabled: bool,
    },
    /// Arm the next test packet credit, or return the exact retained credit.
    HoldAclCredit {
        hold: bool,
    },
    /// Select the four-credit ATT workload and force real thermal calibration.
    /// Idle only; disabling restores the exact pre-experiment thresholds.
    CalibrationTraffic {
        enabled: bool,
    },
    /// Queue four sequenced 64-byte ACL packets; requires all prior credits returned.
    AclBurst,
    /// Reset and drain the Controller, retire HCI/timer/IRQ registers and join its platform.
    /// This is terminal for this boot; actual cold owners remain retained.
    Retire,
    /// Physically shut down and reinitialize on the same storage, without a board reset.
    Restart,
    /// Service due shared-PHY tracking while retaining the same HCI and powered epoch.
    Maintain,
    /// Measure real calibration branches using diagnostic thresholds, preserving samples.
    Calibrate {
        threshold: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BluetoothPeripheralResult {
    Started {
        address: [u8; 6],
    },
    EncryptedAclConfigured {
        enabled: bool,
        failure: Option<BluetoothSecurityFailure>,
    },
    Snapshot,
    CalibrationTrafficConfigured {
        enabled: bool,
        restored: bool,
    },
    AclBurstQueued,
    AclBackpressureConfigured {
        enabled: bool,
    },
    AclCreditHoldConfigured {
        hold: bool,
    },
    HciRejected {
        command_stage: u8,
    },
    Timeout,
    LeaseExpired,
    /// A cold release and powered initialization completed on the original allocations.
    Restarted {
        cycles: u32,
        new_reset_completed: bool,
        old_commands_closed: bool,
        old_events_closed: bool,
        old_acl_credits_closed: bool,
    },
    /// The same HCI completed Reset after a due quiescent tracking window.
    Maintained {
        cycles: u32,
        due_tracking_completed: bool,
        tracking_inhibited: bool,
        same_hci_reset_completed: bool,
        common_calibrated: bool,
        bluetooth_calibrated: bool,
    },
    /// All ownership transitions completed; each Host authority was probed afterward.
    Retired {
        radio_cold: bool,
        commands_closed: bool,
        events_closed: bool,
        acl_credits_closed: bool,
    },
}

/// Completed child invocations and their largest observed duration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothPhyOperation {
    pub completed: u16,
    pub maximum_micros: u32,
}

/// Measurements from actual PHY execution and guarded RUN, in microseconds.
/// Operation elapsed times nest; observed maxima are not worst-case bounds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothPhyMaintenanceEvidence {
    pub transactions: u32,
    pub restored: u32,
    pub common_calibrations: u32,
    /// Latest completed RX DC product, retained across subsequent light tracking.
    #[serde(default)]
    pub latest_rx_quality: Option<crate::PhyRxGainQualityEvidence>,
    pub bluetooth_calibrations: u32,
    pub maximum_execution_micros: u32,
    pub maximum_restoration_micros: u32,
    pub maximum_to_run_micros: u32,
    pub maximum_poll_micros: u32,
    pub admitted_at_micros: u64,
    /// IRQ/register/timer retirement, not an independent RF-off observation.
    #[serde(default)]
    pub quiesced_at_micros: Option<u64>,
    #[serde(default)]
    pub phy_started_at_micros: Option<u64>,
    #[serde(default)]
    pub phy_finished_at_micros: Option<u64>,
    pub execution_deadline_micros: Option<u64>,
    pub restoration_deadline_micros: Option<u64>,
    pub physical_finished_at_micros: Option<u64>,
    pub run_at_micros: Option<u64>,
    pub dcode: BluetoothPhyOperation,
    pub rx_gain: BluetoothPhyOperation,
    pub tx_dc_pwdet: BluetoothPhyOperation,
    pub rfpll: BluetoothPhyOperation,
    pub calibration: BluetoothPhyOperation,
    pub temperature: BluetoothPhyOperation,
    pub invalid: bool,
}

impl BluetoothPhyMaintenanceEvidence {
    /// Five disjoint intervals of the latest guarded peripheral transaction:
    /// admission to IRQ retirement, handoff to PHY, PHY execution, inverse
    /// handoff, and restoration to RUN. Their sum is admission-to-RUN elapsed
    /// time, not request latency, RF-off time, CPU time or a worst-case bound.
    /// Missing, reordered, failed or late observations have no valid partition.
    pub fn exclusive_intervals(&self) -> Option<[u64; 5]> {
        if self.invalid {
            return None;
        }
        let edges = [
            self.admitted_at_micros,
            self.quiesced_at_micros?,
            self.phy_started_at_micros?,
            self.phy_finished_at_micros?,
            self.physical_finished_at_micros?,
            self.run_at_micros?,
        ];
        if edges[4] >= self.execution_deadline_micros?
            || edges[5] >= self.restoration_deadline_micros?
        {
            return None;
        }
        let mut intervals = [0; 5];
        for (interval, pair) in intervals.iter_mut().zip(edges.windows(2)) {
            *interval = pair[1].checked_sub(pair[0])?;
        }
        Some(intervals)
    }
}

/// Dedicated plaintext traffic evidence; counts include the MTU response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothCalibrationTrafficEvidence {
    pub enabled: bool,
    pub connected: bool,
    pub interval_micros: u32,
    pub mtu_exchanged: bool,
    pub sent: u32,
    pub completed: u32,
    pub faults: u32,
}

/// Test Host observations; retained credits and event delivery are independent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothAclBackpressureEvidence {
    pub enabled: bool,
    pub armed: bool,
    pub held: bool,
    pub mtu_exchanged: bool,
    pub received: u32,
    pub returned: u32,
    pub sent: u32,
    pub completed: u32,
    pub supervision_timeout_millis: u32,
    pub held_at_millis: u32,
    pub disconnected_at_millis: u32,
    pub disconnected_while_held: bool,
    pub faults: u32,
}

/// Fragmented ATT Write Command used solely as the backpressure stimulus.
pub const fn bluetooth_backpressure_packet() -> [u8; 64] {
    let mut packet = bluetooth_calibration_notification(0);
    packet[4] = 0x52;
    packet
}

/// One complete L2CAP ATT notification, crossing three legacy LL packets.
pub const fn bluetooth_calibration_notification(sequence: u32) -> [u8; 64] {
    let mut packet = [0; 64];
    packet[0] = 60;
    packet[2] = 4;
    packet[4] = 0x1b;
    packet[5] = 1;
    let bytes = sequence.to_le_bytes();
    let mut i = 0;
    while i < 4 {
        packet[7 + i] = bytes[i];
        i += 1;
    }
    i = 11;
    while i < 64 {
        packet[i] = (sequence as u8).wrapping_add(i as u8);
        i += 1;
    }
    packet
}

/// Public, non-secret key material for the finite encrypted ACL fixture only.
pub const BLUETOOTH_TEST_LTK: [u8; 16] = [0x35; 16];
pub const BLUETOOTH_TEST_RAND: [u8; 8] = [0x27; 8];
pub const BLUETOOTH_TEST_EDIV: u16 = 0x1937;
/// Distinct public key identity for in-connection refresh of the diagnostic session.
pub const BLUETOOTH_REFRESH_LTK: [u8; 16] = [0x6a; 16];
pub const BLUETOOTH_REFRESH_RAND: [u8; 8] = [0x58; 8];
pub const BLUETOOTH_REFRESH_EDIV: u16 = 0x2849;

/// One deliberate initial-key failure or missing refresh key; subsequent connections use valid keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BluetoothSecurityFailure {
    MissingKey,
    WrongKey,
    MissingRefreshKey,
    /// Corrupt one active data MIC after RF receive, before software authentication.
    ActiveDataMic,
}

impl core::str::FromStr for BluetoothSecurityFailure {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "missing-key" => Ok(Self::MissingKey),
            "wrong-key" => Ok(Self::WrongKey),
            "missing-refresh-key" => Ok(Self::MissingRefreshKey),
            "active-data-mic" => Ok(Self::ActiveDataMic),
            _ => Err("expected missing-key, wrong-key, missing-refresh-key or active-data-mic"),
        }
    }
}

/// Host-observed security transitions; key bytes never enter run evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothEncryptionEvidence {
    pub enabled: bool,
    pub encrypted: bool,
    pub key_requests: u32,
    pub key_replies: u32,
    pub negative_replies: u32,
    pub wrong_key_replies: u32,
    pub encryption_changes: u32,
    pub key_refreshes: u32,
    pub faults: u32,
    /// One-shot diagnostic corruption of a received data MIC, not an RF injection.
    #[serde(default)]
    pub mic_injections: u32,
    #[serde(default)]
    pub mic_injection_armed: bool,
}

/// Publication counts are software observations, not successful peer exchanges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothPeripheralEvidence {
    pub operation: BluetoothPeripheralOperation,
    pub result: BluetoothPeripheralResult,
    pub advertising_runs: u32,
    pub peripheral_runs: u32,
    pub peripheral_disconnections: u32,
    /// Completed physical maintenance transactions while retaining the ACL session.
    #[serde(default)]
    pub phy_peripheral_maintenance: u32,
    #[serde(default)]
    pub phy_maintenance: Option<BluetoothPhyMaintenanceEvidence>,
    #[serde(default)]
    pub calibration_traffic: Option<BluetoothCalibrationTrafficEvidence>,
    #[serde(default)]
    pub acl_backpressure: Option<BluetoothAclBackpressureEvidence>,
    #[serde(default)]
    pub encryption: Option<BluetoothEncryptionEvidence>,
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

    pub fn is_maintained(&self, requested: BluetoothPeripheralOperation) -> bool {
        matches!(
            requested,
            BluetoothPeripheralOperation::Maintain | BluetoothPeripheralOperation::Calibrate { .. }
        ) && self.operation == requested
            && matches!(
                self.result,
                BluetoothPeripheralResult::Maintained {
                    cycles: 1..,
                    due_tracking_completed: true,
                    tracking_inhibited: false,
                    same_hci_reset_completed: true,
                    ..
                }
            )
            && !self.terminal
            && !self.saturated
            && self.host_event_faults == 0
            && self.host_acl_faults == 0
    }

    pub fn is_restarted(&self, requested: BluetoothPeripheralOperation) -> bool {
        requested == BluetoothPeripheralOperation::Restart
            && self.operation == requested
            && matches!(
                self.result,
                BluetoothPeripheralResult::Restarted {
                    cycles: 1..,
                    new_reset_completed: true,
                    old_commands_closed: true,
                    old_events_closed: true,
                    old_acl_credits_closed: true
                }
            )
            && !self.terminal
            && !self.saturated
            && self.host_event_faults == 0
            && self.host_acl_faults == 0
    }

    pub fn is_retired(&self, requested: BluetoothPeripheralOperation) -> bool {
        requested == BluetoothPeripheralOperation::Retire
            && self.operation == requested
            && self.result
                == BluetoothPeripheralResult::Retired {
                    radio_cold: true,
                    commands_closed: true,
                    events_closed: true,
                    acl_credits_closed: true,
                }
            && !self.terminal
            && !self.saturated
            && self.host_event_faults == 0
            && self.host_acl_faults == 0
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

use crate::ResetReason;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothDtmEvidence {
    pub reset_reason: ResetReason,
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
    fn maintenance_partition_is_exclusive_and_rejects_missing_or_late_edges() {
        let valid = BluetoothPhyMaintenanceEvidence {
            admitted_at_micros: 100,
            quiesced_at_micros: Some(105),
            phy_started_at_micros: Some(110),
            phy_finished_at_micros: Some(150),
            physical_finished_at_micros: Some(160),
            run_at_micros: Some(180),
            execution_deadline_micros: Some(170),
            restoration_deadline_micros: Some(190),
            ..Default::default()
        };
        let intervals = valid.exclusive_intervals().unwrap();
        assert_eq!(intervals, [5, 5, 40, 10, 20]);
        assert_eq!(intervals.into_iter().sum::<u64>(), 80);
        for case in 0..10 {
            let mut bad = valid.clone();
            match case {
                0 => bad.quiesced_at_micros = None,
                1 => bad.phy_started_at_micros = None,
                2 => bad.phy_finished_at_micros = None,
                3 => bad.physical_finished_at_micros = None,
                4 => bad.run_at_micros = None,
                5 => bad.quiesced_at_micros = Some(99),
                6 => bad.phy_finished_at_micros = Some(161),
                7 => bad.execution_deadline_micros = Some(160),
                8 => bad.restoration_deadline_micros = Some(180),
                _ => bad.invalid = true,
            }
            assert_eq!(bad.exclusive_intervals(), None, "case {case}");
        }
    }

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
        fn largest_operation() -> BluetoothPhyOperation {
            BluetoothPhyOperation {
                completed: u16::MAX,
                maximum_micros: u32::MAX,
            }
        }
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
                u64::MAX,
                u32::MAX,
                u64::MAX,
                u32::MAX,
                crate::Event::BluetoothPeripheral(BluetoothPeripheralEvidence {
                    operation: BluetoothPeripheralOperation::StartAdvertising {
                        termination: BluetoothPeripheralTermination::PeerReset,
                        hold_millis: u16::MAX,
                    },
                    result,
                    advertising_runs: u32::MAX,
                    peripheral_runs: u32::MAX,
                    peripheral_disconnections: u32::MAX,
                    phy_peripheral_maintenance: u32::MAX,
                    encryption: Some(BluetoothEncryptionEvidence {
                        enabled: true,
                        encrypted: true,
                        key_requests: u32::MAX,
                        key_replies: u32::MAX,
                        negative_replies: u32::MAX,
                        wrong_key_replies: u32::MAX,
                        encryption_changes: u32::MAX,
                        key_refreshes: u32::MAX,
                        faults: u32::MAX,
                        mic_injections: u32::MAX,
                        mic_injection_armed: true,
                    }),
                    acl_backpressure: Some(BluetoothAclBackpressureEvidence {
                        enabled: true,
                        armed: true,
                        held: true,
                        mtu_exchanged: true,
                        received: u32::MAX,
                        returned: u32::MAX,
                        sent: u32::MAX,
                        completed: u32::MAX,
                        supervision_timeout_millis: u32::MAX,
                        held_at_millis: u32::MAX,
                        disconnected_at_millis: u32::MAX,
                        disconnected_while_held: true,
                        faults: u32::MAX,
                    }),
                    calibration_traffic: Some(BluetoothCalibrationTrafficEvidence {
                        enabled: true,
                        connected: true,
                        interval_micros: u32::MAX,
                        mtu_exchanged: true,
                        sent: u32::MAX,
                        completed: u32::MAX,
                        faults: u32::MAX,
                    }),
                    phy_maintenance: Some(BluetoothPhyMaintenanceEvidence {
                        transactions: u32::MAX,
                        restored: u32::MAX,
                        common_calibrations: u32::MAX,
                        bluetooth_calibrations: u32::MAX,
                        latest_rx_quality: Some(crate::PhyRxGainQualityEvidence::default()),
                        maximum_execution_micros: u32::MAX,
                        maximum_restoration_micros: u32::MAX,
                        maximum_to_run_micros: u32::MAX,
                        maximum_poll_micros: u32::MAX,
                        admitted_at_micros: u64::MAX,
                        quiesced_at_micros: Some(u64::MAX),
                        phy_started_at_micros: Some(u64::MAX),
                        phy_finished_at_micros: Some(u64::MAX),
                        execution_deadline_micros: Some(u64::MAX),
                        restoration_deadline_micros: Some(u64::MAX),
                        physical_finished_at_micros: Some(u64::MAX),
                        run_at_micros: Some(u64::MAX),
                        dcode: largest_operation(),
                        rx_gain: largest_operation(),
                        tx_dc_pwdet: largest_operation(),
                        rfpll: largest_operation(),
                        calibration: largest_operation(),
                        temperature: largest_operation(),
                        invalid: true,
                    }),
                    connection_complete_events: u32::MAX,
                    disconnection_complete_events: u32::MAX,
                    connection_update_complete_events: u32::MAX,
                    channel_map_update_events: u32::MAX,
                    target_disconnect_commands: u32::MAX,
                    target_reset_commands: u32::MAX,
                    host_event_faults: u32::MAX,
                    host_acl_received_packets: u32::MAX,
                    host_acl_queued_packets: u32::MAX,
                    host_acl_transmitted_packets: u32::MAX,
                    host_acl_completed_packets: u32::MAX,
                    host_acl_backpressure_holds: u32::MAX,
                    host_acl_faults: u32::MAX,
                    last_disconnect_reason: Some(u8::MAX),
                    retries: u32::MAX,
                    terminal: true,
                    saturated: true,
                    detail_truncated: false,
                    detail: core::str::from_utf8(&[b'x'; 128])
                        .unwrap()
                        .try_into()
                        .unwrap(),
                }),
            );
            let mut encoder = crate::FrameEncoder::new();
            let mut decoder = crate::FrameDecoder::new();
            let mut observed = None;
            let mut sizing = [0u8; 1024];
            let bytes = postcard::to_slice(&expected.body, &mut sizing)
                .unwrap()
                .len();
            assert!(bytes <= crate::MAX_POSTCARD_BYTES, "body bytes {bytes}");
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
            phy_peripheral_maintenance: 0,
            phy_maintenance: None,
            calibration_traffic: None,
            acl_backpressure: None,
            encryption: None,
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
                ..evidence.clone()
            }
            .is_snapshot(BluetoothPeripheralOperation::Snapshot)
        );
        let retired = BluetoothPeripheralEvidence {
            operation: BluetoothPeripheralOperation::Retire,
            result: BluetoothPeripheralResult::Retired {
                radio_cold: true,
                commands_closed: true,
                events_closed: true,
                acl_credits_closed: true,
            },
            ..evidence
        };
        assert!(retired.is_retired(BluetoothPeripheralOperation::Retire));
        for (cycles, reset, commands, events, credits, expected) in [
            (1, true, true, true, true, true),
            (0, true, true, true, true, false),
            (1, false, true, true, true, false),
            (1, true, false, true, true, false),
            (1, true, true, false, true, false),
            (1, true, true, true, false, false),
        ] {
            let restarted = BluetoothPeripheralEvidence {
                operation: BluetoothPeripheralOperation::Restart,
                result: BluetoothPeripheralResult::Restarted {
                    cycles,
                    new_reset_completed: reset,
                    old_commands_closed: commands,
                    old_events_closed: events,
                    old_acl_credits_closed: credits,
                },
                ..retired.clone()
            };
            assert_eq!(
                restarted.is_restarted(BluetoothPeripheralOperation::Restart),
                expected
            );
            assert!(!restarted.is_restarted(BluetoothPeripheralOperation::Retire));
            for unhealthy in [
                BluetoothPeripheralEvidence {
                    terminal: true,
                    ..restarted.clone()
                },
                BluetoothPeripheralEvidence {
                    saturated: true,
                    ..restarted.clone()
                },
                BluetoothPeripheralEvidence {
                    host_event_faults: 1,
                    ..restarted.clone()
                },
                BluetoothPeripheralEvidence {
                    host_acl_faults: 1,
                    ..restarted
                },
            ] {
                assert!(!unhealthy.is_restarted(BluetoothPeripheralOperation::Restart));
            }
        }

        for (cycles, due, inhibited, reset, expected) in [
            (1, true, false, true, true),
            (0, true, false, true, false),
            (1, false, false, true, false),
            (1, true, true, true, false),
            (1, true, false, false, false),
        ] {
            let maintained = BluetoothPeripheralEvidence {
                operation: BluetoothPeripheralOperation::Maintain,
                result: BluetoothPeripheralResult::Maintained {
                    cycles,
                    due_tracking_completed: due,
                    tracking_inhibited: inhibited,
                    same_hci_reset_completed: reset,
                    common_calibrated: false,
                    bluetooth_calibrated: false,
                },
                ..retired.clone()
            };
            assert_eq!(
                maintained.is_maintained(BluetoothPeripheralOperation::Maintain),
                expected
            );
            assert!(!maintained.is_maintained(BluetoothPeripheralOperation::Restart));
            for unhealthy in [
                BluetoothPeripheralEvidence {
                    terminal: true,
                    ..maintained.clone()
                },
                BluetoothPeripheralEvidence {
                    saturated: true,
                    ..maintained.clone()
                },
                BluetoothPeripheralEvidence {
                    host_event_faults: 1,
                    ..maintained.clone()
                },
                BluetoothPeripheralEvidence {
                    host_acl_faults: 1,
                    ..maintained.clone()
                },
            ] {
                assert!(!unhealthy.is_maintained(BluetoothPeripheralOperation::Maintain));
            }
        }

        assert!(!retired.is_retired(BluetoothPeripheralOperation::Snapshot));
        for (radio_cold, commands_closed, events_closed, acl_credits_closed) in [
            (false, true, true, true),
            (true, false, true, true),
            (true, true, false, true),
            (true, true, true, false),
        ] {
            let incomplete = BluetoothPeripheralEvidence {
                result: BluetoothPeripheralResult::Retired {
                    radio_cold,
                    commands_closed,
                    events_closed,
                    acl_credits_closed,
                },
                ..retired.clone()
            };
            assert!(!incomplete.is_retired(BluetoothPeripheralOperation::Retire));
        }
        for fault in 0..4 {
            let mut unhealthy = retired.clone();
            match fault {
                0 => unhealthy.terminal = true,
                1 => unhealthy.saturated = true,
                2 => unhealthy.host_event_faults = 1,
                _ => unhealthy.host_acl_faults = 1,
            }
            assert!(!unhealthy.is_retired(BluetoothPeripheralOperation::Retire));
        }
        let mut encoder = crate::FrameEncoder::new();
        let mut decoder = crate::FrameDecoder::new();
        let envelope = crate::Envelope::new(1, 2, 0, 3, crate::Event::BluetoothPeripheral(retired));
        let mut observed = None;
        decoder.feed(encoder.encode(&envelope).unwrap(), |frame| {
            observed = Some(frame.unwrap())
        });
        assert_eq!(observed, Some(envelope));
    }

    #[test]
    fn rx_deadline_diagnostics_survive_framed_transport() {
        let expected = crate::Envelope::new(
            3,
            4,
            0,
            5,
            crate::Event::BluetoothDtm(BluetoothDtmEvidence {
                reset_reason: ResetReason::Other,
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
            reset_reason: ResetReason::Other,
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
