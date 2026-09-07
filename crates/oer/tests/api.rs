#[cfg(feature = "wifi")]
#[test]
fn wifi_facade_preserves_implementation_type_identity() {
    fn through_facade(value: oer::wifi::WifiConfig) -> radio::wifi::WifiConfig {
        value
    }
    let _: fn(radio::wifi::WifiConfig) -> radio::wifi::WifiConfig = through_facade;
}

#[cfg(feature = "wifi")]
#[test]
fn association_policy_and_wire_codec_share_the_same_types() {
    use oer::ieee80211::sta::association::{PhyMode, Preference, select_phy};

    let preference: mac::station::association::Preference = Preference::PreferHe20;
    let mode: mac::station::association::PhyMode = select_phy(preference, true, true);
    assert_eq!(mode, PhyMode::He20);
}

#[cfg(feature = "esp32s31")]
#[test]
fn chip_namespaces_preserve_backend_type_identity() {
    use oer::chips::esp32s31::{driver, hal};

    let _: fn(chip_ap::engine::ApEngineError) -> driver::ieee80211::ap::engine::ApEngineError =
        |error| error;
    let _: fn(chip_sta::profile::Selection) -> driver::ieee80211::sta::profile::Selection =
        |selection| selection;
    let _: fn(chip_hal::owner::Radio<()>) -> hal::owner::Radio<()> = |owner| owner;
}
