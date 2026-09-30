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

#[test]
fn nominal_rates_follow_the_standard_tables() {
    let ht = |mcs, bandwidth, short_gi| {
        PhyRate::Ht(HtRate::new(HtMcs::new(mcs).unwrap(), bandwidth, short_gi).unwrap())
    };
    assert_eq!(ht(0, PpduBandwidth::Mhz20, false).nominal_kbps(), 6_500);
    assert_eq!(ht(7, PpduBandwidth::Mhz20, true).nominal_kbps(), 72_200);
    assert_eq!(ht(7, PpduBandwidth::Mhz40, false).nominal_kbps(), 135_000);
    assert_eq!(ht(15, PpduBandwidth::Mhz40, true).nominal_kbps(), 300_000);

    let he = |mcs, gi_ltf, dcm| {
        PhyRate::He(
            HeRate::new(
                HeMcs::new(mcs).unwrap(),
                SpatialStreams::ONE,
                PpduBandwidth::Mhz20,
                gi_ltf,
                FecCoding::Bcc,
                dcm,
            )
            .unwrap(),
        )
    };
    // 117 bits per 13.6 µs symbol.
    assert_eq!(he(0, HeGiLtf::Ltf2xGi800Ns, false).nominal_kbps(), 8_602);
    assert_eq!(he(9, HeGiLtf::Ltf2xGi800Ns, false).nominal_kbps(), 114_705);
    assert_eq!(
        he(11, HeGiLtf::Ltf4xGi3200Ns, false).nominal_kbps(),
        121_875
    );
    assert_eq!(he(1, HeGiLtf::Ltf2xGi1600Ns, true).nominal_kbps(), 8_125);
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Cck5M5(DsssPreamble::Short)).nominal_kbps(),
        5_500
    );
}
