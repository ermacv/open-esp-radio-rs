//! Hardware boundary for switching from scan to associated STA RX policy.

use open_esp_radio_esp32s31_hal::{RadioRuntimeOwner, wifi_mac::WifiMacHal};

pub trait StaLinkRxPolicyHardware {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]);
}

/// Read-only PHY observation used by the STA layer to complete peer policy.
pub trait StaNoiseFloorHardware {
    fn read_noise_floor_dbm(&self) -> i8;
}

impl StaLinkRxPolicyHardware for WifiMacHal<'_> {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.configure_station_receive_policy(bssid);
    }
}

impl StaNoiseFloorHardware for WifiMacHal<'_> {
    fn read_noise_floor_dbm(&self) -> i8 {
        self.read_noise_floor_dbm()
    }
}

impl StaLinkRxPolicyHardware for RadioRuntimeOwner {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.wifi_mac_hal().configure_station_receive_policy(bssid);
    }
}

impl StaNoiseFloorHardware for RadioRuntimeOwner {
    fn read_noise_floor_dbm(&self) -> i8 {
        RadioRuntimeOwner::read_noise_floor_dbm(self)
    }
}

/// Apply the reviewed BSSID/filter transaction after off-channel scan and
/// before the first Authentication transmission.
pub fn configure_sta_link_receive_policy<H: StaLinkRxPolicyHardware>(
    hardware: &mut H,
    bssid: [u8; 6],
) {
    hardware.apply_sta_link_policy(bssid);
}
