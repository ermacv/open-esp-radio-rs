use super::*;
#[test]
fn spectrum_measurements_cannot_be_sent_in_radio_measurement_actions() {
    assert_eq!(
        RadioMeasurementRequest::parse(&[5, 0, 1, 0, 0, 38, 3, 1, 2, 0]),
        Err(WireError::InconsistentFields)
    );
    assert_eq!(
        RadioMeasurementReport::parse(&[5, 1, 1, 39, 3, 1, 4, 2]),
        Err(WireError::InconsistentFields)
    );
    let extended = [5, 0, 1, 0, 0, 38, 3, 1, 2, 200];
    assert!(RadioMeasurementRequest::parse(&extended).is_ok());
}

#[test]
fn channel_and_noise_requests_decode_independent_wire_fixtures() {
    let wire = [
        5, 0, 7, 2, 0, 38, 13, 9, 16, 3, 81, 6, 8, 0, 20, 0, 1, 2, 2, 90,
    ];
    let request = RadioMeasurementRequest::parse(&wire).unwrap();
    assert_eq!(request.repetitions, 2);
    let element = request.measurements().unwrap().next().unwrap();
    assert!(element.mode.duration_mandatory());
    let MeasurementRequestData::ChannelLoad(data) = element.data().unwrap() else {
        panic!()
    };
    assert_eq!(data.randomization_tu, 8);
    assert_eq!(data.duration_tu, 20);
    assert_eq!(data.reporting_condition().unwrap(), Some((2, 90)));
    let mut out = [0; 64];
    let len = request.encode(&mut out).unwrap();
    assert_eq!(&out[..len], &wire);
    let mut noise = wire;
    noise[9] = 4;
    assert!(matches!(
        RadioMeasurementRequest::parse(&noise)
            .unwrap()
            .measurements()
            .unwrap()
            .next()
            .unwrap()
            .data()
            .unwrap(),
        MeasurementRequestData::NoiseHistogram(_)
    ));
}

#[test]
fn beacon_request_keeps_binary_ssid_channel_reports_and_vendor_fields() {
    let wire = [
        5, 0, 8, 0, 0, 38, 33, 2, 0, 5, 81, 255, 10, 0, 40, 0, 1, 255, 255, 255, 255, 255, 255, 0,
        3, b'a', 0, b'b', 2, 1, 2, 51, 3, 81, 1, 6, 221, 2, 7, 8,
    ];
    let request = RadioMeasurementRequest::parse(&wire).unwrap();
    let MeasurementRequestData::Beacon(data) = request
        .measurements()
        .unwrap()
        .next()
        .unwrap()
        .data()
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(data.bssid, [255; 6]);
    assert_eq!(
        data.channel.subelements.unique(0).unwrap().unwrap(),
        b"a\0b"
    );
    assert_eq!(
        data.channel.subelements.unique(51).unwrap().unwrap(),
        [81, 1, 6]
    );
    let mut out = [0; 64];
    let len = request.encode(&mut out).unwrap();
    assert_eq!(&out[..len], &wire);
}

#[test]
fn control_rejections_and_autonomous_report_zero_tokens_are_valid() {
    let control = [5, 0, 1, 0, 0, 38, 3, 7, 2, 3];
    assert_eq!(
        RadioMeasurementRequest::parse(&control)
            .unwrap()
            .measurements()
            .unwrap()
            .next()
            .unwrap()
            .data()
            .unwrap(),
        MeasurementRequestData::Control
    );
    let refused = [5, 1, 0, 39, 3, 0, 4, 5];
    let report = RadioMeasurementReport::parse(&refused).unwrap();
    assert_eq!(report.dialog_token, 0);
    assert_eq!(
        report.reports().unwrap().next().unwrap().data().unwrap(),
        MeasurementReportData::Rejected
    );
    assert!(RadioMeasurementReport::parse(&[5, 1, 1, 39, 4, 1, 2, 5, 1]).is_err());
}

#[test]
fn report_fields_tsf_units_and_all_histogram_bins_round_trip() {
    let header = MeasurementReportHeader {
        operating_class: 81,
        channel: 6,
        start_tsf: 0x0807060504030201,
        duration_tu: 123,
    };
    let noise = NoiseHistogramReport {
        header,
        antenna_id: 3,
        anpi: 40,
        ipi_density: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        subelements: Elements::parse(&[221, 2, 6, 7]).unwrap(),
    };
    let mut out = [0; 64];
    let len = noise.encode(&mut out).unwrap();
    assert_eq!(&out[2..10], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(NoiseHistogramReport::parse(&out[..len]).unwrap(), noise);
    let beacon = BeaconMeasurementReport {
        header,
        frame_information: 7,
        rcpi: 255,
        rsni: 255,
        bssid: [2, 3, 4, 5, 6, 7],
        antenna_id: 2,
        parent_tsf: 0x12345678,
        subelements: Elements::parse(&[164, 1, 1]).unwrap(),
    };
    let len = beacon.encode(&mut out).unwrap();
    assert_eq!(BeaconMeasurementReport::parse(&out[..len]).unwrap(), beacon);
    let load = ChannelLoadReport {
        header,
        load: 200,
        subelements: Elements::EMPTY,
    };
    let len = load.encode(&mut out).unwrap();
    assert_eq!(ChannelLoadReport::parse(&out[..len]).unwrap(), load);
}

#[test]
fn link_measurement_tpc_and_signed_power_follow_wire_order() {
    let wire = [5, 3, 9, 35, 2, 0xfc, 12, 2, 3, 100, 80, 221, 2, 6, 7];
    let report = LinkMeasurementReport::parse(&wire).unwrap();
    assert_eq!(report.transmit_power_dbm, -4);
    assert_eq!(report.link_margin_db, 12);
    let mut out = [0; 64];
    let len = report.encode(&mut out).unwrap();
    assert_eq!(&out[..len], &wire);
    let mut bad = wire;
    bad[4] = 3;
    assert!(LinkMeasurementReport::parse(&bad).is_err());
    let request = LinkMeasurementRequest::parse(&[5, 2, 7, 0xf9, 20]).unwrap();
    assert_eq!(request.transmit_power_dbm, -7);
}

#[test]
fn validation_and_output_capacity_precede_any_write() {
    let mut out = [0xaa; 20];
    let malformed = RadioMeasurementRequest {
        dialog_token: 1,
        repetitions: 0,
        elements: Elements::parse(&[38, 4, 1, 0, 5, 1]).unwrap(),
    };
    assert!(malformed.measurements().is_err());
    assert!(malformed.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 20]);
    let request = RadioMeasurementRequest::parse(&[5, 0, 1, 0, 0, 38, 3, 1, 2, 5]).unwrap();
    assert!(request.encode(&mut out[..4]).is_err());
    assert_eq!(out, [0xaa; 20]);
    for len in 0..11 {
        assert!(LinkMeasurementReport::parse(&[5, 3, 1, 35, 2, 0, 0, 0, 0, 0, 0][..len]).is_err());
    }
    assert!(ChannelMeasurementRequest::parse(&[81, 6, 0, 0, 1, 0, 1, 1, 3]).is_err());
}
