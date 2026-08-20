//! Register-only boundary for one same-channel STA plus SoftAP configuration.
//!
//! This module deliberately stops below runtime ownership. It gives later
//! orchestration one finite operation instead of allowing it to interleave
//! the STA=0 and AP=1 receive-context transactions by hand.

use open_esp_radio_esp32s31_hal::wifi_mac::MacStaApReceivePlan;
use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal;

/// Minimal hardware authority required by the combined receive-context plan.
pub trait StaApRegisterHardware {
    fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan);
    fn disable_station_receive_registers(&mut self);
    fn disable_access_point_receive_registers(&mut self);
}

impl StaApRegisterHardware for WifiMacHal<'_> {
    fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan) {
        self.configure_sta_ap_receive_plan(plan);
    }

    fn disable_station_receive_registers(&mut self) {
        self.disable_station_receive_policy();
    }

    fn disable_access_point_receive_registers(&mut self) {
        self.disable_access_point_receive_policy();
    }
}

/// Leave the station role while retaining the complete SoftAP register bank.
pub fn disable_station_receive_registers<H: StaApRegisterHardware>(hardware: &mut H) {
    hardware.disable_station_receive_registers();
}

/// Leave the SoftAP role while retaining the complete station register bank.
pub fn disable_access_point_receive_registers<H: StaApRegisterHardware>(hardware: &mut H) {
    hardware.disable_access_point_receive_registers();
}

/// Apply both reviewed MAC receive contexts as one LMAC operation.
///
/// The caller must separately prove that STA and SoftAP share one physical
/// channel and must own the common RX, TX, interrupt and beacon schedulers.
pub fn configure_sta_ap_receive_registers<H: StaApRegisterHardware>(
    hardware: &mut H,
    station_address: [u8; 6],
    station_bssid: [u8; 6],
    access_point_address: [u8; 6],
) {
    hardware.apply_sta_ap_receive_registers(MacStaApReceivePlan::observed_mode_one(
        station_address,
        station_bssid,
        access_point_address,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Hardware {
        plans: std::vec::Vec<MacStaApReceivePlan>,
        disabled: std::vec::Vec<crate::MacInterface>,
    }

    impl StaApRegisterHardware for Hardware {
        fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan) {
            self.plans.push(plan);
        }

        fn disable_station_receive_registers(&mut self) {
            self.disabled.push(crate::MacInterface::Station);
        }

        fn disable_access_point_receive_registers(&mut self) {
            self.disabled.push(crate::MacInterface::AccessPoint);
        }
    }

    #[test]
    fn lmac_publishes_the_combined_plan_as_one_operation() {
        let station_address = [0x02, 0, 0, 0, 0, 1];
        let station_bssid = [0x02, 0, 0, 0, 0, 2];
        let access_point_address = [0x02, 0, 0, 0, 0, 3];
        let plan = MacStaApReceivePlan::observed_mode_one(
            station_address,
            station_bssid,
            access_point_address,
        );
        let mut hardware = Hardware::default();

        configure_sta_ap_receive_registers(
            &mut hardware,
            station_address,
            station_bssid,
            access_point_address,
        );

        assert_eq!(hardware.plans.as_slice(), &[plan]);
    }

    #[test]
    fn each_role_can_leave_without_reconfiguring_the_surviving_bank() {
        let mut hardware = Hardware::default();

        disable_station_receive_registers(&mut hardware);
        disable_access_point_receive_registers(&mut hardware);

        assert!(hardware.plans.is_empty());
        assert_eq!(
            hardware.disabled,
            [
                crate::MacInterface::Station,
                crate::MacInterface::AccessPoint
            ]
        );
    }
}
