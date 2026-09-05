use open_esp_radio_esp32s31_pac::RadioHardware;

use super::Ieee802154Owned;

#[derive(Debug)]
struct FakePlatform;

#[test]
fn untouched_owner_releases_the_complete_neutral_root() {
    let owned = Ieee802154Owned::from_hardware(FakePlatform, RadioHardware::for_validation());
    let (_platform, hardware) = owned
        .release()
        .expect("an untouched IEEE 802.15.4 route can be released");

    let ieee = hardware.into_ieee802154();
    let (task, interrupts) = ieee.separate_interrupt_owner();
    let _hardware = task
        .into_cold(interrupts)
        .release()
        .expect("an untouched IEEE 802.15.4 route can be released");
}
