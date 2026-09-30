use super::*;

#[test]
fn ht_rates_exist_only_at_20_and_40_mhz() {
    let mcs = HtMcs::new(7).unwrap();
    assert!(HtRate::new(mcs, PpduBandwidth::Mhz40, true).is_some());
    assert!(HtRate::new(mcs, PpduBandwidth::Mhz80, false).is_none());
    assert_eq!(HtMcs::new(31).unwrap().spatial_streams(), 4);
    assert_eq!(HtMcs::new(32), None);
}

#[test]
fn he_dcm_is_limited_to_its_standard_mcs_and_streams() {
    let rate = |mcs, streams, dcm| {
        HeRate::new(
            HeMcs::new(mcs).unwrap(),
            SpatialStreams::new(streams).unwrap(),
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Bcc,
            dcm,
        )
    };
    for mcs in 0..=11 {
        assert!(rate(mcs, 1, false).is_some());
        assert_eq!(rate(mcs, 1, true).is_some(), matches!(mcs, 0 | 1 | 3 | 4));
    }
    assert!(rate(0, 2, true).is_some());
    assert!(rate(0, 3, true).is_none());
    assert_eq!(HeMcs::new(12), None);
    assert_eq!(SpatialStreams::new(0), None);
    assert_eq!(SpatialStreams::new(9), None);
}

#[test]
fn legacy_rates_report_their_modulation_and_rate() {
    assert!(!LegacyRate::Cck11M(DsssPreamble::Short).is_ofdm());
    assert!(LegacyRate::Ofdm6M.is_ofdm());
    assert_eq!(LegacyRate::Cck5M5(DsssPreamble::Long).kbps(), 5_500);
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Ofdm54M).bandwidth(),
        PpduBandwidth::Mhz20
    );
}
