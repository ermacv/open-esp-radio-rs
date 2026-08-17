//! Role-neutral A-MPDU publication configuration.
//!
//! Descriptor retention, delimiter formatting and completion live in the MAC
//! crate. This layer makes the remaining role boundary explicit: only the
//! selected virtual interface and hardware key slot differ between an AP and
//! a station publication.

use open_esp_radio_esp32s31_hal::types::MacInterface;
use open_esp_radio_esp32s31_wifi_mac::tx::{HtAmpduTxConfig, HtProtectionSpacing, HtRate};
use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::TX_BLOCK_ACK_MAX_WINDOW;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduTxRoleAdapter {
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
}

/// Complete role-owned HT policy for one fresh aggregate.
///
/// Peer lookup and frame encoding remain in AP/STA code. Once admitted, the
/// interface, key, PHY rate and bounded BlockAck window move together so the
/// scheduler cannot independently recompute one of them for standby
/// publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduTxRolePolicy {
    role: AmpduTxRoleAdapter,
    rate: HtRate,
    frame_limit: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxRolePolicyError {
    EmptyBlockAckWindow,
    BlockAckWindowTooWide(u16),
    EmptyConfiguredLimit,
    ConfiguredLimitTooWide(u8),
    InvalidArenaCapacity(usize),
}

impl HtAmpduTxRolePolicy {
    pub fn new(
        role: AmpduTxRoleAdapter,
        rate: HtRate,
        negotiated_window: u16,
        configured_limit: u8,
        arena_capacity: usize,
    ) -> Result<Self, HtAmpduTxRolePolicyError> {
        if negotiated_window == 0 {
            return Err(HtAmpduTxRolePolicyError::EmptyBlockAckWindow);
        }
        if negotiated_window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(HtAmpduTxRolePolicyError::BlockAckWindowTooWide(
                negotiated_window,
            ));
        }
        if configured_limit == 0 {
            return Err(HtAmpduTxRolePolicyError::EmptyConfiguredLimit);
        }
        if arena_capacity == 0 || arena_capacity > usize::from(TX_BLOCK_ACK_MAX_WINDOW) {
            return Err(HtAmpduTxRolePolicyError::InvalidArenaCapacity(
                arena_capacity,
            ));
        }
        if u16::from(configured_limit) > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(HtAmpduTxRolePolicyError::ConfiguredLimitTooWide(
                configured_limit,
            ));
        }
        let frame_limit = usize::from(configured_limit)
            .min(usize::from(negotiated_window))
            .min(arena_capacity) as u8;
        Ok(Self {
            role,
            rate,
            frame_limit,
        })
    }

    pub const fn role(self) -> AmpduTxRoleAdapter {
        self.role
    }

    pub const fn rate(self) -> HtRate {
        self.rate
    }

    pub const fn frame_limit(self) -> u8 {
        self.frame_limit
    }
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

    #[test]
    fn role_policy_binds_key_rate_and_every_batch_limit() {
        let rate = HtRate {
            mcs: HtMcs::Mcs7,
            channel_width: HtChannelWidth::Mhz20,
            guard_interval: HtGuardInterval::Long800Ns,
        };
        let policy = HtAmpduTxRolePolicy::new(
            AmpduTxRoleAdapter {
                interface: MacInterface::AccessPoint,
                hardware_key_selector: 8,
            },
            rate,
            16,
            12,
            8,
        )
        .unwrap();

        assert_eq!(policy.rate(), rate);
        assert_eq!(policy.role().interface, MacInterface::AccessPoint);
        assert_eq!(policy.role().hardware_key_selector, 8);
        assert_eq!(policy.frame_limit(), 8);
    }

    #[test]
    fn role_policy_rejects_unrepresentable_windows_instead_of_truncating() {
        let rate = HtRate {
            mcs: HtMcs::Mcs7,
            channel_width: HtChannelWidth::Mhz20,
            guard_interval: HtGuardInterval::Long800Ns,
        };
        assert_eq!(
            HtAmpduTxRolePolicy::new(
                AmpduTxRoleAdapter {
                    interface: MacInterface::Station,
                    hardware_key_selector: 2,
                },
                rate,
                33,
                32,
                32,
            ),
            Err(HtAmpduTxRolePolicyError::BlockAckWindowTooWide(33))
        );
    }
}
