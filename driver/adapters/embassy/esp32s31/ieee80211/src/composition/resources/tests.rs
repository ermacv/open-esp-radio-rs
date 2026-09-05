use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

#[test]
fn marker_and_public_dimensions_cannot_drift() {
    assert_eq!(
        Esp32s31DefaultWifiResourceProfile::RX_DESCRIPTOR_COUNT,
        ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
    );
    assert_eq!(
        Esp32s31DefaultWifiResourceProfile::TX_AMPDU_FRAME_COUNT,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT
    );
    assert_eq!(Esp32s31DefaultWifiResourceProfile::RX_REORDER_WINDOW, 16);
    assert_eq!(Esp32s31DefaultWifiResourceProfile::RX_STAGE_SLOT_COUNT, 32);
    assert_eq!(Esp32s31DefaultWifiResourceProfile::RX_DESCRIPTOR_COUNT, 96);
    assert_eq!(ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH, 67);
    assert_eq!(ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH, 128);
    const {
        assert!(
            ESP32S31_DEFAULT_NETWORK_PACKET_POOL_CAPACITY
                > ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH
        );
        assert!(
            ESP32S31_DEFAULT_NETWORK_RX_PACKET_POOL_CAPACITY
                > ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH
        );
    }
    assert_eq!(
        ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH
            - ESP32S31_PERMANENT_NETWORK_ENDPOINTS
            - ESP32S31_NETWORK_TX_PIPELINE_CREDITS,
        2 * ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT,
        "two aggregate arenas remain visible while Core1 owns one unpublished TX token"
    );
}

#[test]
fn default_station_memory_is_acquired_as_one_owner_graph() {
    static MEMORY: Esp32s31DefaultWifiMemory<NoopRawMutex> = Esp32s31DefaultWifiMemory::new();

    let lease = MEMORY.claim().expect("fresh station memory is available");
    assert_eq!(
        lease.rx_dma.buffers().len(),
        ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
    );
    assert_eq!(lease.scan_frame.len(), ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY);
    assert_eq!(lease.ap_beacon.len(), WPA2_BEACON_CAPACITY);
    assert!(matches!(
        MEMORY.claim(),
        Err(Esp32s31DefaultWifiMemoryError::InUse)
    ));
}
