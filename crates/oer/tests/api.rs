#[cfg(feature = "wifi")]
#[test]
fn wifi_facade_preserves_implementation_type_identity() {
    fn through_facade(value: oer::wifi::WifiConfig) -> radio::wifi::WifiConfig {
        value
    }
    let _: fn(radio::wifi::WifiConfig) -> radio::wifi::WifiConfig = through_facade;
}

#[cfg(feature = "owned-xarxa")]
#[test]
fn embassy_radio_facade_preserves_supervisor_reexport_type_identity() {
    let _: fn(
        radio_embassy::EmbassyWifiSupervisorCommand,
    ) -> oer::embassy::radio::EmbassyWifiSupervisorCommand = |command| command;
    let _: fn(
        oer::embassy::radio::EmbassyWifiSupervisorResponse<()>,
    ) -> radio_embassy::EmbassyWifiSupervisorResponse<()> = |response| response;
}

#[cfg(feature = "wifi")]
#[test]
fn association_policy_and_wire_codec_share_the_same_types() {
    use oer::ieee80211::sta::association::{PhyMode, Preference, select_phy};

    let preference: mac::station::association::Preference = Preference::PreferHe20;
    let mode: mac::station::association::PhyMode = select_phy(preference, true, true, true);
    assert_eq!(mode, PhyMode::He20);
}

#[cfg(feature = "esp32s31")]
#[test]
fn chip_namespace_preserves_hal_type_identity() {
    use oer::chips::esp32s31::hal;

    let _: fn(chip_hal::root::RadioHardware) -> hal::root::RadioHardware = |owner| owner;
}

#[cfg(feature = "esp32s31-wifi")]
#[test]
fn chip_wifi_namespace_preserves_backend_type_identity() {
    use oer::chips::esp32s31::driver;

    let _: fn(chip_ap::engine::ApEngineError) -> driver::ieee80211::ap::engine::ApEngineError =
        |error| error;
    let _: fn(chip_sta::profile::Selection) -> driver::ieee80211::sta::profile::Selection =
        |selection| selection;
}

#[cfg(feature = "bluetooth")]
#[test]
fn bluetooth_facade_preserves_hci_and_link_layer_type_identity() {
    use oer::bluetooth::{hci as api, le::ll};

    let _: fn(hci::LeDtmPhy) -> api::LeDtmPhy = |phy| phy;
    let _: fn(le_ll::LeDeviceAddress) -> ll::LeDeviceAddress = |address| address;
}

#[cfg(feature = "ieee802154")]
#[test]
fn ieee802154_facade_preserves_channel_type_identity() {
    let _: fn(ieee802154::Channel) -> oer::ieee802154::Channel = |channel| channel;
}

#[cfg(feature = "esp32s31-bluetooth")]
#[test]
fn bluetooth_backend_preserves_controller_event_cell_identity() {
    let _: fn(
        chip_bluetooth::runtime_resources::ControllerEventCells,
    )
        -> oer::chips::esp32s31::driver::bluetooth::runtime_resources::ControllerEventCells =
        |cells| cells;
}

#[cfg(feature = "esp32s31-ieee802154")]
#[test]
fn chip_ieee802154_namespace_preserves_engine_type_identity() {
    let _: fn(
        chip_ieee802154::pib::Ieee802154PibDefaults,
    ) -> oer::chips::esp32s31::driver::ieee802154::pib::Ieee802154PibDefaults = |defaults| defaults;
}

#[cfg(feature = "embassy-ieee802154")]
#[test]
fn ieee802154_system_exposes_its_construction_inputs() {
    use oer::systems::esp32s31::embassy::ieee802154;

    let _: fn(chip_ieee802154::pib::Ieee802154PibDefaults) -> ieee802154::Ieee802154PibDefaults =
        |defaults| defaults;
    let _: fn(
        ieee802154_composition::Ieee802154PhyMaintenance,
    ) -> ieee802154::Ieee802154PhyMaintenance = |outcome| outcome;
}

#[cfg(feature = "openthread")]
#[test]
fn openthread_radio_is_reachable_through_the_ieee802154_system() {
    use oer::systems::esp32s31::embassy::ieee802154::openthread;

    let _: fn(
        ieee802154_openthread::frames::TransmitFailure,
    ) -> openthread::frames::TransmitFailure = |failure| failure;
}
