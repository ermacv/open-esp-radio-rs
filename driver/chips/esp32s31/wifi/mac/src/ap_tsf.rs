//! Closed low-MAC ownership of access-point TSF lifecycle edges.

use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal;
use open_esp_radio_esp32s31_pac::RadioRegisters;

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

// The AP runtime currently owns one `RadioRegisters` value for receive,
// crypto and timing. Keep the physical owner opaque by adapting it through
// the same closed HAL used by independently composed clients.
impl ApTsfHardware for RadioRegisters {
    fn reset_and_start_access_point_tsf(&mut self) {
        WifiMacHal::new(self).reset_and_start_access_point_tsf();
    }

    fn stop_access_point_tsf(&mut self) {
        WifiMacHal::new(self).stop_access_point_tsf();
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
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingHardware {
        starts: usize,
        stops: usize,
    }

    impl ApTsfHardware for RecordingHardware {
        fn reset_and_start_access_point_tsf(&mut self) {
            self.starts += 1;
        }

        fn stop_access_point_tsf(&mut self) {
            self.stops += 1;
        }
    }

    #[test]
    fn low_mac_publishes_exactly_one_start_and_stop_edge() {
        let mut hardware = RecordingHardware::default();

        reset_and_start_access_point_tsf(&mut hardware);
        stop_access_point_tsf(&mut hardware);

        assert_eq!(hardware.starts, 1);
        assert_eq!(hardware.stops, 1);
    }
}
