//! Public Wi-Fi topology values.
//!
//! Only implemented subsystems belong in the API. Bluetooth, IEEE 802.15.4
//! and coexistence will gain their own owner types when a concrete runner
//! exists; placeholder switches are deliberately absent.

pub use open_esp_radio_wifi_softmac::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneMonitorPlan, WifiStationConfig,
};

#[cfg(all(test, feature = "esp32s31-wifi"))]
mod tests {
    use super::*;

    #[test]
    fn esp32s31_accepts_only_implemented_wifi_topologies() {
        let station = WifiConfig::station(WifiStationConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap(),
        ))
        .validate(crate::esp32s31::wifi::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
        .unwrap();
        assert!(station.station().is_some());

        let access_point = WifiConfig::access_point(WifiAccessPointConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 2]).unwrap(),
        ));
        assert_eq!(
            access_point.validate(
                crate::esp32s31::wifi::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
            ),
            Err(WifiConfigError::UnsupportedAccessPoint),
        );

        let monitor = WifiConfig::monitor(WifiMonitorConfig::normalized())
            .validate(crate::esp32s31::wifi::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
            .unwrap();
        assert!(monitor.standalone_monitor().is_some());
    }
}
