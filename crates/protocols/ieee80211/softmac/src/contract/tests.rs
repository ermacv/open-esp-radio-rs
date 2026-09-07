use super::*;

const CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        station_interfaces: 1,
        access_point_interfaces: 0,
        simultaneous_station_access_point: false,
        standalone_monitor: false,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: false,
        protocol_validated_monitor_tap: false,
    },
    operations: MacOperationOwnership {
        tx_fcs_generation: MacOperationOwner::Hardware,
        immediate_ack_response: MacOperationOwner::Hardware,
        csma_ca_backoff_countdown: MacOperationOwner::Hardware,
        unicast_retry_policy: MacOperationOwner::Software,
        tx_sequence_assignment: MacOperationOwner::Software,
        ccmp_key_selection: MacOperationOwner::Software,
        ccmp_packet_number: MacOperationOwner::Software,
        ccmp_transform: MacOperationOwner::Hardware,
        rx_block_ack_matching: MacOperationOwner::Hardware,
        rx_reorder: MacOperationOwner::Software,
        tx_block_ack_capture: MacOperationOwner::Hardware,
        tx_ampdu_retry_selection: MacOperationOwner::Software,
    },
    resources: MacResourceLimits {
        channel_contexts: 1,
        ordinary_tx_queues: 4,
        rx_block_ack_entries: 8,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        tx_block_ack_max_window: 32,
        tx_ampdu_max_subframes: 32,
        station_pairwise_ccmp_slots: 1,
        station_group_ccmp_slots: 1,
        access_point_pairwise_ccmp_slots: 1,
        access_point_group_ccmp_slots: 1,
        access_point_association_entries: 1,
        access_point_encrypted_clients: 1,
    },
};

#[test]
fn zero_and_oversized_block_ack_windows_are_not_supported() {
    assert!(!CAPABILITIES.supports_rx_block_ack_window(0));
    assert!(CAPABILITIES.supports_rx_block_ack_window(64));
    assert!(!CAPABILITIES.supports_rx_block_ack_window(65));
    assert!(CAPABILITIES.supports_tx_block_ack_window(32));
    assert!(!CAPABILITIES.supports_tx_block_ack_window(33));
}

#[test]
fn implemented_roles_and_monitor_taps_are_explicit() {
    assert!(
        CAPABILITIES
            .interfaces
            .supports_role(interface::VifRole::Station)
    );
    assert!(
        !CAPABILITIES
            .interfaces
            .supports_role(interface::VifRole::AccessPoint)
    );
    assert!(
        !CAPABILITIES
            .interfaces
            .supports_monitor_tap(interface::MonitorTapPoint::Raw)
    );
}

#[test]
fn terminal_status_distinguishes_an_exchange_from_one_attempt() {
    let status = MacTxStatus {
        result: MacTxResult::Transmitted,
        attempts: 3,
        final_rate: 7_u8,
        acknowledged: Some(true),
        ack_snr_db: Some(18),
        airtime_micros: None,
    };
    assert_eq!(status.attempts, 3);
    assert_eq!(status.result, MacTxResult::Transmitted);
}

#[test]
fn tx_plan_contains_protocol_policy_but_no_hardware_queue_encoding() {
    let plan = MacTxPlan {
        access_category: WmmAccessCategory::Video,
        initial_rate: 7_u8,
        publication_limit: 4,
        publication_timeout_micros: 250_000,
    };
    assert_eq!(plan.access_category, WmmAccessCategory::Video);
    assert_eq!(plan.initial_rate, 7);
    assert_eq!(plan.publication_limit, 4);
}

#[test]
fn receive_metadata_keeps_absence_and_provenance_distinct() {
    let staged = MacRxMetadata {
        channel: MacRxEvidence::HardwareObserved(6),
        rate: MacRxEvidence::HardwareObserved(11_u8),
        rssi_dbm: MacRxEvidence::HardwareObserved(-47),
        crypto: MacRxEvidence::Unavailable,
        s_mpdu: MacRxEvidence::Unavailable,
        ampdu: MacRxEvidence::Unavailable,
        amsdu: MacRxEvidence::Unavailable,
    };
    assert!(staged.channel.is_available());
    assert!(!staged.crypto.is_available());

    let validated = MacRxMetadata {
        crypto: MacRxEvidence::ProtocolValidated(MacRxCryptoStatus::DecryptedAndIntegrityVerified),
        s_mpdu: MacRxEvidence::HardwareObserved(true),
        amsdu: MacRxEvidence::ProtocolValidated(false),
        ..staged
    };
    assert_ne!(validated.crypto, staged.crypto);
    assert_eq!(
        validated.s_mpdu.as_ref(),
        MacRxEvidence::HardwareObserved(&true)
    );
    assert_eq!(validated.ampdu, MacRxEvidence::Unavailable);
    assert_eq!(validated.amsdu, MacRxEvidence::ProtocolValidated(false));
}

#[test]
fn ampdu_status_joins_block_ack_and_one_ordinary_retry() {
    let status = MacAmpduTxStatus {
        result: MacAmpduTxResult::Delivered,
        original_subframes: 3,
        aggregate_attempts: 2,
        aggregate_rate: 7_u8,
        block_acknowledged_subframes: 2,
        ordinary_retry: Some(MacTxStatus {
            result: MacTxResult::Transmitted,
            attempts: 2,
            final_rate: 5,
            acknowledged: Some(true),
            ack_snr_db: Some(12),
            airtime_micros: None,
        }),
    };
    assert_eq!(status.delivered_subframes(), 3);
    assert_eq!(status.total_publication_attempts(), 4);
    assert!(status.fully_delivered());
}
