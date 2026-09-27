use super::*;
use oer_ieee802154_engine::channel::Ieee802154Channel;

fn channel() -> Ieee802154Channel {
    Ieee802154Channel::new(20).expect("standard channel")
}

#[test]
fn the_esp32s31_provider_levels_resolve_every_request() {
    let levels = ESP32S31_TX_POWER_LEVELS;
    assert_eq!(levels.len(), 16);
    // A request between two levels floors to the lower one.
    let default = levels.resolve(channel(), 20);
    assert_eq!(
        (default.effective_dbm(), default.selected_provider_index()),
        (18, 14)
    );
    assert_eq!(levels.resolve(channel(), 0).selected_provider_index(), 8);
    assert_eq!(levels.resolve(channel(), -128).selected_provider_index(), 0);
    assert_eq!(levels.resolve(channel(), 127).effective_dbm(), 21);
}
