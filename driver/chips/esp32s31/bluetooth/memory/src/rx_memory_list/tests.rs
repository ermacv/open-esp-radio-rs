use open_esp_radio_esp32s31_hal::BluetoothMemoryListSelector;

use super::BluetoothRxMemoryListClass;

#[test]
fn scan_and_non_scan_classes_map_to_the_two_active_selectors() {
    assert_eq!(
        BluetoothRxMemoryListClass::Scanning.selector(),
        BluetoothMemoryListSelector::One
    );
    assert_eq!(
        BluetoothRxMemoryListClass::NonScanning.selector(),
        BluetoothMemoryListSelector::Two
    );
}
