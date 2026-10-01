use super::*;
fn transition() -> EventReport<'static> {
    EventReport {
        token: 1,
        kind: EventType::TRANSITION,
        status: EventReportStatus::SUCCESSFUL,
        timing: Some(EventTiming {
            tsf: 123,
            utc_offset: [1; 10],
            time_error: [2; 5],
        }),
        data: EventData::Transition(TransitionEvent {
            source: [2; 6],
            target: [4; 6],
            time_tu: 256,
            reason: 6,
            result: 0,
            source_rcpi: 1,
            source_rsni: 2,
            target_rcpi: 3,
            target_rsni: 4,
        }),
    }
}
#[test]
fn both_frequent_transition_forms_require_an_explicit_transmit_choice() {
    let mut r = transition();
    r.status = EventReportStatus::FREQUENT_TRANSITION;
    let mut out = [0xaa; 64];
    assert!(r.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 64]);
    let n = r
        .encode_frequent(FrequentTransitionFormat::WithLastEvent, &mut out)
        .unwrap();
    assert_eq!(EventReport::parse(&out[2..n]).unwrap(), r);
    r.timing = None;
    r.data = EventData::Empty;
    let n = r
        .encode_frequent(FrequentTransitionFormat::StatusOnly, &mut out)
        .unwrap();
    assert_eq!(n, 5);
    assert_eq!(EventReport::parse(&out[2..n]).unwrap(), r);
    assert!(
        r.encode_frequent(FrequentTransitionFormat::WithLastEvent, &mut out)
            .is_err()
    );
}
#[test]
fn all_original_event_payloads_roundtrip_and_capacity_preflight_is_atomic() {
    let data = [
        transition().data,
        EventData::Rsna(RsnaEvent {
            target: [2; 6],
            authentication: [0, 15, 172, 1],
            eap: EapMethod::Expanded {
                vendor_id: [0, 0, 1],
                vendor_type: [0, 0, 0, 2],
            },
            result: 3,
            rsne: &[48, 4, 1, 0],
        }),
        EventData::PeerLink(PeerLinkEvent {
            peer: [2; 6],
            operating_class: 81,
            channel: 6,
            tx_power_dbm: -5,
            connection_seconds: 0x112233,
            status: PeerLinkStatus::DIRECT_ACTIVE,
        }),
        EventData::WnmLog(b"<14>Oct  1 12:00:00 host 02:00:00:00:00:01: associated"),
    ];
    for data in data {
        let r = EventReport {
            kind: data.kind().unwrap(),
            data,
            ..transition()
        };
        let mut out = [0; 257];
        let n = r.encode(&mut out).unwrap();
        assert_eq!(EventReport::parse(&out[2..n]).unwrap(), r);
        let mut small = [0xaa; 4];
        assert!(r.encode(&mut small).is_err());
        assert_eq!(small, [0xaa; 4]);
    }
}
#[test]
fn conditions_unknown_types_and_empty_reports_are_preserved() {
    let request = EventRequest {
        token: 1,
        kind: EventType::TRANSITION,
        response_limit: 0,
        conditions: Elements::parse(&[2, 2, 1, 0, 200, 1, 9]).unwrap(),
    };
    let mut out = [0; 32];
    let n = request.encode(&mut out).unwrap();
    assert_eq!(EventRequest::parse(&out[2..n]).unwrap(), request);
    assert!(EventRequest::parse(&[1, 0, 0, 2, 1, 0]).is_err());
    let other = EventReport::parse(&[0, 4, 0, 1, 2, 3]).unwrap();
    assert_eq!(other.data, EventData::Other(&[1, 2, 3]));
    assert_eq!(
        EventReportFrame::parse(&[10, 1, 1])
            .unwrap()
            .reports()
            .unwrap()
            .count(),
        0
    );
    assert!(EventReportFrame::parse(&[10, 1, 0, 79, 3, 1, 0, 2]).is_err());
}
#[test]
fn action_encoding_also_requires_the_explicit_frequent_format() {
    for format in [
        FrequentTransitionFormat::WithLastEvent,
        FrequentTransitionFormat::StatusOnly,
    ] {
        let mut report = transition();
        report.status = EventReportStatus::FREQUENT_TRANSITION;
        if format == FrequentTransitionFormat::StatusOnly {
            report.timing = None;
            report.data = EventData::Empty;
        }
        let mut ie = [0; 257];
        let n = report.encode_frequent(format, &mut ie).unwrap();
        let frame = EventReportFrame {
            dialog_token: 1,
            elements: Elements::parse(&ie[..n]).unwrap(),
        };
        let mut out = [0xaa; 260];
        assert!(frame.encode(&mut out).is_err());
        assert_eq!(out, [0xaa; 260]);
        let n = frame.encode_frequent(format, &mut out).unwrap();
        assert_eq!(EventReportFrame::parse(&out[..n]).unwrap(), frame);
        let other = if format == FrequentTransitionFormat::StatusOnly {
            FrequentTransitionFormat::WithLastEvent
        } else {
            FrequentTransitionFormat::StatusOnly
        };
        assert!(frame.encode_frequent(other, &mut out).is_err());
    }
}
