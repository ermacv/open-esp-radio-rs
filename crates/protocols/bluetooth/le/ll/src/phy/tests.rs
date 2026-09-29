use super::*;

#[test]
fn a_single_supported_bit_names_a_phy() {
    assert_eq!(phy_from_single_mask(1), Some(ConnectionPhy::Le1M));
    assert_eq!(phy_from_single_mask(2), Some(ConnectionPhy::Le2M));
    // Coded, several bits or none name no connection PHY here.
    for mask in [0, 3, 4, 6, 0x80] {
        assert_eq!(phy_from_single_mask(mask), None);
    }
    for phy in [ConnectionPhy::Le1M, ConnectionPhy::Le2M] {
        assert_eq!(phy_from_single_mask(phy_mask(phy)), Some(phy));
    }
}

#[test]
fn preferences_keep_only_supported_phys_and_never_prefer_none() {
    let preference = LePhyPreference::new(0b110, 0b100);
    assert_eq!(preference.transmit(), 0b10);
    // Only Coded: nothing supported is preferred, so every PHY is.
    assert_eq!(preference.receive(), LE_SUPPORTED_PHYS);
    assert_eq!(LePhyPreference::new(0, 0), LePhyPreference::ANY);
}

#[test]
fn a_transition_changes_when_either_direction_does() {
    let two = ConnectionPhys {
        transmit: ConnectionPhy::Le2M,
        receive: ConnectionPhy::Le2M,
    };
    assert!(LePhyTransition::new(ConnectionPhys::LE_1M, two).changed());
    assert!(!LePhyTransition::new(two, two).changed());
    let receive_only = ConnectionPhys {
        transmit: ConnectionPhy::Le1M,
        receive: ConnectionPhy::Le2M,
    };
    assert!(LePhyTransition::new(ConnectionPhys::LE_1M, receive_only).changed());
}
