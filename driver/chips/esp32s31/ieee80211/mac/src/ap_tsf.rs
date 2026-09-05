//! Closed low-MAC ownership of access-point TSF lifecycle edges.

use open_esp_radio_esp32s31_hal::{RadioRuntimeOwner, wifi_mac::WifiMacHal};

/// Minimal hardware capability required to own the AP TSF domain.
pub trait ApTsfHardware {
    fn reset_and_start_access_point_tsf(&mut self);
    fn stop_access_point_tsf(&mut self);
}

impl ApTsfHardware for WifiMacHal<'_> {
    fn reset_and_start_access_point_tsf(&mut self) {
        WifiMacHal::reset_and_start_access_point_tsf(self);
    }

    fn stop_access_point_tsf(&mut self) {
        WifiMacHal::stop_access_point_tsf(self);
    }
}

impl ApTsfHardware for RadioRuntimeOwner {
    fn reset_and_start_access_point_tsf(&mut self) {
        self.wifi_mac_hal().reset_and_start_access_point_tsf();
    }

    fn stop_access_point_tsf(&mut self) {
        self.wifi_mac_hal().stop_access_point_tsf();
    }
}

/// Reset AP timing and begin a fresh protocol-role epoch.
pub fn reset_and_start_access_point_tsf(hardware: &mut impl ApTsfHardware) {
    hardware.reset_and_start_access_point_tsf();
}

/// Stop AP timing before relinquishing or changing the AP protocol role.
///
/// This leaf intentionally contains no raw address or register image. The HAL
/// owns the finite PAC transaction recovered from
/// `libpp.a[hal_tsf.o]::hal_disable_softap_tsf`.
pub fn stop_access_point_tsf(hardware: &mut impl ApTsfHardware) {
    hardware.stop_access_point_tsf();
}

#[cfg(test)]
mod tests;
