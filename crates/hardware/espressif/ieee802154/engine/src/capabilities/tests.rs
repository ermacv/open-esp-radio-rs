use super::*;
use crate::pib::Ieee802154PibDefaults;

const LEVELS: TransmitPowerLevels = match TransmitPowerLevels::new(&[-24, 0, 20]) {
    Ok(levels) => levels,
    Err(_) => panic!("valid levels"),
};

#[test]
fn single_interface_table_keeps_the_recovered_values() {
    let caps = capabilities(LEVELS, false);
    assert_eq!(caps.operations, RADIO_CAPABILITIES);
    assert!(!caps.operations.contains(RadioCapabilities::MULTI_PAN));
    assert_eq!(caps.interfaces, 1);
    assert_eq!(caps.transmit_power.map(|power| power.reset_dbm), Some(20));
    assert_eq!(
        caps.cca_threshold,
        Some(CcaThresholdCapability {
            lowest_dbm: -120,
            highest_dbm: 0,
            reset_dbm: -75,
        })
    );
    assert_eq!(caps.receive_sensitivity_dbm, -104);
    assert_eq!(
        caps.csl,
        Some(CslCapability {
            accuracy_ppm: 50,
            uncertainty_10us: 50,
        })
    );
    assert_eq!(caps.enhanced_ack_header_ies, 16);
    assert_eq!(ENH_ACK_PROBING_CAPACITY, 10);
}

#[test]
fn multipan_table_adds_multi_pan_and_four_interfaces() {
    let caps = capabilities(LEVELS, true);
    assert_eq!(
        caps.operations,
        RADIO_CAPABILITIES.union(RadioCapabilities::MULTI_PAN)
    );
    assert_eq!(caps.interfaces, 4);
}

#[test]
fn cca_reset_is_the_pib_default() {
    assert_eq!(
        Ieee802154PibDefaults::default().cca_threshold_dbm(),
        CCA_RESET_DBM
    );
}
