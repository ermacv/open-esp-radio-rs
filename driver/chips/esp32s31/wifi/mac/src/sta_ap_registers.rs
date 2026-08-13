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
}

impl StaApRegisterHardware for WifiMacHal<'_> {
    fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan) {
        self.configure_sta_ap_receive_plan(plan);
    }
}

/// Apply both reviewed MAC receive contexts as one LMAC operation.
///
/// The caller must separately prove that STA and SoftAP share one physical
/// channel and must own the common RX, TX, interrupt and beacon schedulers.
pub fn configure_sta_ap_receive_registers<H: StaApRegisterHardware>(
    hardware: &mut H,
    plan: MacStaApReceivePlan,
) {
    hardware.apply_sta_ap_receive_registers(plan);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Hardware {
        plans: std::vec::Vec<MacStaApReceivePlan>,
    }

    impl StaApRegisterHardware for Hardware {
        fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan) {
            self.plans.push(plan);
        }
    }

    #[test]
    fn lmac_publishes_the_combined_plan_as_one_operation() {
        let plan = MacStaApReceivePlan {
            station_address: [0x02, 0, 0, 0, 0, 1],
            station_bssid: [0x02, 0, 0, 0, 0, 2],
            station_policy_mode: crate::MacStaPolicyMode::Mode1,
            access_point_address: [0x02, 0, 0, 0, 0, 3],
        };
        let mut hardware = Hardware::default();

        configure_sta_ap_receive_registers(&mut hardware, plan);

        assert_eq!(hardware.plans.as_slice(), &[plan]);
    }
}
