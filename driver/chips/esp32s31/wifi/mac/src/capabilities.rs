//! Exact portable MAC-service profile exposed by the ESP32-S31 driver.
//!
//! This is deliberately not a datasheet feature list.  Every field describes
//! the complete source-owned service that exists today, including its bounded
//! Rust storage.  In particular, aggregate and retry operations are split at
//! their real hardware/software boundary instead of being advertised by one
//! ambiguous offload boolean.

use open_esp_radio_wifi_softmac::{
    MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
    MacServiceCapabilities,
};

use crate::{
    rx_ampdu::{RX_BLOCK_ACK_MAX_WINDOW, STA_RX_BLOCK_ACK_BANK_COUNT},
    rx_ampdu_hw::S31_RX_BLOCK_ACK_MAX_TID,
    tx_ampdu::{TX_AMPDU_SLOT_CAPACITY, TX_BLOCK_ACK_MAX_WINDOW},
};

/// MAC-service capabilities implemented by the current ESP32-S31 driver.
///
/// Evidence for the split ownership is kept beside the implementation:
///
/// - TX encoders exclude the hardware FCS; normal RX policy enables auto-ACK;
/// - `tx_runtime` owns retry limits, rate ladders and contention updates;
/// - portable station state owns sequence spaces, while the typed CCMP slot
///   owns PN publication and selects a hardware key;
/// - hardware BA tables/completion registers match sequence spaces and capture
///   bitmaps, while `rx_ampdu` and `tx_runtime` own reorder/retention policy.
pub const ESP32S31_MAC_SERVICE_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        // This is the currently implemented owner graph, not the number of
        // address-match slots visible in hardware.
        station_interfaces: 1,
        access_point_interfaces: 1,
        simultaneous_station_access_point: false,
        standalone_monitor: true,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: true,
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
        // The current radio/channel owner has one home/current selector and no
        // simultaneous multi-channel scheduler.
        channel_contexts: 1,
        ordinary_tx_queues: 4,
        rx_block_ack_entries: STA_RX_BLOCK_ACK_BANK_COUNT as u8,
        rx_block_ack_max_tid: S31_RX_BLOCK_ACK_MAX_TID,
        rx_block_ack_max_window: RX_BLOCK_ACK_MAX_WINDOW,
        tx_block_ack_max_window: TX_BLOCK_ACK_MAX_WINDOW,
        tx_ampdu_max_subframes: TX_AMPDU_SLOT_CAPACITY as u16,
        // These are the slots intentionally exposed by the open STA driver,
        // not the size of the underlying hardware key table.
        station_pairwise_ccmp_slots: 1,
        station_group_ccmp_slots: 1,
        access_point_pairwise_ccmp_slots: 15,
        access_point_group_ccmp_slots: 1,
        access_point_association_entries: 15,
        access_point_encrypted_clients: 15,
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_exposes_the_real_split_mac_boundaries() {
        let operations = ESP32S31_MAC_SERVICE_CAPABILITIES.operations;
        assert_eq!(
            operations.immediate_ack_response,
            MacOperationOwner::Hardware
        );
        assert_eq!(operations.unicast_retry_policy, MacOperationOwner::Software);
        assert_eq!(
            operations.tx_sequence_assignment,
            MacOperationOwner::Software
        );
        assert_eq!(operations.ccmp_transform, MacOperationOwner::Hardware);
        assert_eq!(operations.rx_reorder, MacOperationOwner::Software);
        assert_eq!(operations.tx_block_ack_capture, MacOperationOwner::Hardware);
        assert_eq!(
            operations.tx_ampdu_retry_selection,
            MacOperationOwner::Software
        );
    }

    #[test]
    fn profile_limits_are_derived_from_the_owners_that_enforce_them() {
        let resources = ESP32S31_MAC_SERVICE_CAPABILITIES.resources;
        assert_eq!(resources.ordinary_tx_queues, 4);
        assert_eq!(
            resources.rx_block_ack_entries as usize,
            STA_RX_BLOCK_ACK_BANK_COUNT
        );
        assert_eq!(resources.rx_block_ack_max_window, RX_BLOCK_ACK_MAX_WINDOW);
        assert_eq!(resources.tx_block_ack_max_window, TX_BLOCK_ACK_MAX_WINDOW);
        assert_eq!(
            resources.tx_ampdu_max_subframes as usize,
            TX_AMPDU_SLOT_CAPACITY
        );
        assert_eq!(resources.access_point_pairwise_ccmp_slots, 15);
        assert_eq!(resources.access_point_association_entries, 15);
        assert_eq!(resources.access_point_encrypted_clients, 15);
    }

    #[test]
    fn profile_advertises_only_materialized_interface_and_monitor_topologies() {
        let interfaces = ESP32S31_MAC_SERVICE_CAPABILITIES.interfaces;
        assert_eq!(interfaces.station_interfaces, 1);
        assert_eq!(interfaces.access_point_interfaces, 1);
        assert!(!interfaces.simultaneous_station_access_point);
        assert!(interfaces.standalone_monitor);
        assert!(!interfaces.monitor_with_interfaces);
        assert!(!interfaces.raw_monitor_tap);
        assert!(interfaces.normalized_monitor_tap);
        assert!(!interfaces.protocol_validated_monitor_tap);
    }
}
