use super::*;

fn round_trip(features: DiagnosticFeatures) -> DiagnosticFeatures {
    let mut buffer = [0_u8; 16];
    let encoded = postcard::to_slice(&features, &mut buffer).unwrap();
    postcard::from_bytes(encoded).unwrap()
}

#[test]
fn a_feature_set_round_trips_as_its_features() {
    assert_eq!(
        round_trip(DiagnosticFeatures::empty()),
        DiagnosticFeatures::empty()
    );
    let both: DiagnosticFeatures = DiagnosticFeature::ALL.into_iter().collect();
    assert_eq!(round_trip(both), both);
    let station_exit = DiagnosticFeatures::empty().with(DiagnosticFeature::StationExit, true);
    assert_eq!(round_trip(station_exit), station_exit);
    assert!(station_exit.contains(DiagnosticFeature::StationExit));
    assert!(!station_exit.contains(DiagnosticFeature::RxOwnership));
    assert!(
        station_exit
            .with(DiagnosticFeature::StationExit, false)
            .is_empty()
    );
}

#[test]
fn an_unknown_or_repeated_feature_fails_closed() {
    // A sequence of one feature whose variant index this host does not know.
    let unknown = DiagnosticFeature::ALL.len() as u8;
    assert!(postcard::from_bytes::<DiagnosticFeatures>(&[1, unknown]).is_err());
    // The same feature twice.
    assert!(postcard::from_bytes::<DiagnosticFeatures>(&[2, 1, 1]).is_err());
}
