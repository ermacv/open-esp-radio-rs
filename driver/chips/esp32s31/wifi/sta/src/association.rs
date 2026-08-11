//! ESP32-S31 STA Association policy derived from scan and PHY calibration.

use open_esp_radio_ieee80211::{
    scan::ScanRecord,
    station::{
        HeUlMuPowerCapability, HeUlMuPowerCapabilityError, StaAssociationPhy,
        StaAssociationPreference, StaPowerCapability, StaPowerCapabilityError,
        select_sta_association,
    },
};

use open_esp_radio_esp32s31_wifi::tx::WifiTxPowerProfile;

/// Recovered minimum power advertised by the ESP32-S31 HE STA.
///
/// Complete vendor `hal_he_init` installs this value through
/// `hal_set_tx_min_pwr`; the maximum remains derived from calibrated rate 16.
pub const ESP32S31_STA_MINIMUM_TX_POWER_DBM: i8 = -11;

/// Listen interval used by the qualified ESP32-S31 infrastructure STA path.
pub const ESP32S31_STA_LISTEN_INTERVAL: u16 = 3;

/// Complete association inputs derived from scan policy and calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaAssociationProfile {
    pub phy: StaAssociationPhy,
    pub power_capability: Option<StaPowerCapability>,
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
    pub rate_16_through_25: Option<[i8; 10]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaAssociationProfileError {
    PowerCapability(StaPowerCapabilityError),
    HeUlMuPower(HeUlMuPowerCapabilityError),
}

/// Select an Association PHY and derive every HE power field from calibration.
pub fn esp32s31_sta_association_profile<P: WifiTxPowerProfile>(
    access_point: &ScanRecord,
    preference: StaAssociationPreference,
    power: &P,
) -> Result<Esp32s31StaAssociationProfile, Esp32s31StaAssociationProfileError> {
    let phy = select_sta_association(access_point, preference).phy;
    if phy != StaAssociationPhy::He20 {
        return Ok(Esp32s31StaAssociationProfile {
            phy,
            power_capability: None,
            he_ul_mu_power: None,
            rate_16_through_25: None,
        });
    }

    let rate_power = core::array::from_fn(|offset| power.power_pair(16 + offset as u8).primary);
    let power_capability =
        StaPowerCapability::new(ESP32S31_STA_MINIMUM_TX_POWER_DBM, rate_power[0])
            .map_err(Esp32s31StaAssociationProfileError::PowerCapability)?;
    let he_ul_mu_power = HeUlMuPowerCapability::from_rate_power_indices(rate_power)
        .map_err(Esp32s31StaAssociationProfileError::HeUlMuPower)?;
    Ok(Esp32s31StaAssociationProfile {
        phy,
        power_capability: Some(power_capability),
        he_ul_mu_power: Some(he_ul_mu_power),
        rate_16_through_25: Some(rate_power),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_wifi::tx::WifiTxPowerPair;

    const HE20_MCS9_CAPABILITY: [u8; 24] = [
        255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
        0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
    ];

    struct Power;

    impl WifiTxPowerProfile for Power {
        fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
            let primary = [20, 20, 20, 19, 19, 18, 18, 16, 15, 20]
                [usize::from(rate_code.saturating_sub(16).min(9))];
            WifiTxPowerPair {
                primary,
                alternate: primary,
            }
        }
    }

    #[test]
    fn he_profile_owns_calibrated_power_derivation() {
        let mut access_point = ScanRecord {
            channel: 6,
            ..ScanRecord::EMPTY
        };
        access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
            .copy_from_slice(&HE20_MCS9_CAPABILITY);
        access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
        let operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
        access_point.he_operation_ie[..operation.len()].copy_from_slice(&operation);
        access_point.he_operation_ie_len = operation.len() as u8;

        let profile = esp32s31_sta_association_profile(
            &access_point,
            StaAssociationPreference::PreferHe20,
            &Power,
        )
        .unwrap();
        assert_eq!(profile.phy, StaAssociationPhy::He20);
        assert_eq!(
            profile.rate_16_through_25,
            Some([20, 20, 20, 19, 19, 18, 18, 16, 15, 20])
        );
        assert_eq!(profile.power_capability.unwrap().minimum_dbm(), -11);
        assert_eq!(profile.power_capability.unwrap().maximum_dbm(), 20);
        assert_eq!(
            profile.he_ul_mu_power.unwrap().relative_to_rate_16(),
            [0, 0, 1, 1, 2, 2, 4, 5, 0]
        );
    }
}
