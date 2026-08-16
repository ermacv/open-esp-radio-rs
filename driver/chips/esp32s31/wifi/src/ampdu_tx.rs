//! Role-neutral A-MPDU publication configuration.
//!
//! Descriptor retention, delimiter formatting and completion live in the MAC
//! crate. This layer makes the remaining role boundary explicit: only the
//! selected virtual interface and hardware key slot differ between an AP and
//! a station publication.

use open_esp_radio_esp32s31_hal::types::MacInterface;
use open_esp_radio_esp32s31_wifi_mac::tx::{HtAmpduTxConfig, HtProtectionSpacing, HtRate};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduTxRoleAdapter {
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduPublicationInputs {
    pub rate: HtRate,
    pub aggregate_length: u16,
    pub subframes: u8,
    pub protection_spacing: HtProtectionSpacing,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub rts_power_primary: u8,
    pub rts_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

pub fn ht_ampdu_publication_config(
    role: AmpduTxRoleAdapter,
    inputs: HtAmpduPublicationInputs,
) -> Option<HtAmpduTxConfig> {
    let mut config = HtAmpduTxConfig::new(inputs.rate, inputs.aggregate_length, inputs.subframes)?;
    config.protection_spacing = inputs.protection_spacing;
    config.data_power_primary = inputs.data_power_primary;
    config.data_power_alternate = inputs.data_power_alternate;
    config.rts_power_primary = inputs.rts_power_primary;
    config.rts_power_alternate = inputs.rts_power_alternate;
    config.aifsn = inputs.aifsn;
    config.contention_window = inputs.contention_window;
    config.interface = role.interface;
    config.scheduler_priority = inputs.scheduler_priority;
    config.pti = inputs.packet_priority;
    config.pti_count = 1;
    config.hardware_key_selector = role.hardware_key_selector;
    Some(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_wifi_mac::tx::{HtChannelWidth, HtGuardInterval, HtMcs};

    #[test]
    fn role_adapter_changes_only_interface_and_key_authority() {
        let inputs = HtAmpduPublicationInputs {
            rate: HtRate {
                mcs: HtMcs::Mcs7,
                channel_width: HtChannelWidth::Mhz20,
                guard_interval: HtGuardInterval::Long800Ns,
            },
            aggregate_length: 4_096,
            subframes: 4,
            protection_spacing: HtProtectionSpacing::Density0To4,
            data_power_primary: 1,
            data_power_alternate: 2,
            rts_power_primary: 3,
            rts_power_alternate: 4,
            aifsn: 3,
            contention_window: 15,
            scheduler_priority: 1,
            packet_priority: 1,
        };
        let station = ht_ampdu_publication_config(
            AmpduTxRoleAdapter {
                interface: MacInterface::Station,
                hardware_key_selector: 2,
            },
            inputs,
        )
        .unwrap();
        let access_point = ht_ampdu_publication_config(
            AmpduTxRoleAdapter {
                interface: MacInterface::AccessPoint,
                hardware_key_selector: 8,
            },
            inputs,
        )
        .unwrap();
        assert_eq!(station.interface, MacInterface::Station);
        assert_eq!(access_point.interface, MacInterface::AccessPoint);
        assert_eq!(station.hardware_key_selector, 2);
        assert_eq!(access_point.hardware_key_selector, 8);
        assert_eq!(station.aggregate_length, access_point.aggregate_length);
        assert_eq!(station.rate, access_point.rate);
    }
}
