//! Ownership boundary for role-neutral to active-AP receive policy.

use open_esp_radio_esp32s31_hal::{RadioRuntimeOwner, wifi_mac::WifiMacHal};

pub trait ApRxPolicyHardware {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]);
}

impl ApRxPolicyHardware for WifiMacHal<'_> {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.configure_access_point_receive_policy(access_point);
    }
}

impl ApRxPolicyHardware for RadioRuntimeOwner {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.wifi_mac_hal()
            .configure_access_point_receive_policy(access_point);
    }
}

/// Select the exact vendor policy-eight AP receive frontier.
pub fn configure_ap_receive_policy<H: ApRxPolicyHardware>(hardware: &mut H, access_point: [u8; 6]) {
    hardware.apply_ap_link_policy(access_point);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Hardware(Option<[u8; 6]>);

    impl ApRxPolicyHardware for Hardware {
        fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
            self.0 = Some(access_point);
        }
    }

    #[test]
    fn ap_address_moves_as_one_checked_policy_input() {
        let mut hardware = Hardware::default();
        configure_ap_receive_policy(&mut hardware, [2, 0, 0, 0, 0, 1]);
        assert_eq!(hardware.0, Some([2, 0, 0, 0, 0, 1]));
    }
}
