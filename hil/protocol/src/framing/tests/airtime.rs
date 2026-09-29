use super::*;
use crate::{WifiAirtimePeer, WifiAirtimePeerEvidence, WifiAirtimeReport};

#[test]
fn worst_case_airtime_records_fit_existing_wire_frames() {
    for body in [
        Event::WifiAirtimePeer(WifiAirtimePeerEvidence {
            peer: WifiAirtimePeer::Unicast {
                address: [255; 6],
                association_id: u16::MAX,
                association_epoch: u32::MAX,
            },
            grants: u64::MAX,
            granted_micros: u64::MAX,
            settlements: u64::MAX,
            settled_grants_micros: u64::MAX,
            charged_micros: u64::MAX,
            cancellations: u64::MAX,
            cancelled_grants_micros: u64::MAX,
            balance_after_last_event_micros: i64::MIN,
            outstanding: u32::MAX,
            outstanding_micros: u64::MAX,
            maximum_outstanding: u32::MAX,
        }),
        Event::WifiAirtimeReport(WifiAirtimeReport {
            peer_records: u8::MAX,
            dropped_events: u64::MAX,
            saturated: true,
        }),
    ] {
        let expected = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, body);
        let mut encoder = FrameEncoder::new();
        let mut decoder = FrameDecoder::new();
        let mut actual = None;
        decoder.feed(encoder.encode(&expected).unwrap(), |record| {
            actual = Some(record.unwrap())
        });
        assert_eq!(actual, Some(expected));
    }
}

#[test]
fn worst_case_aggregate_fill_record_fits_one_wire_frame() {
    let body = Event::WifiApAggregateFill(crate::WifiApAggregateFill {
        generation: u32::MAX,
        association_id: u16::MAX,
        aggregates: u32::MAX,
        subframes: u32::MAX,
        maximum_subframes: u8::MAX,
        histogram: [u32::MAX; 5],
    });
    let expected = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, body);
    let mut encoder = FrameEncoder::new();
    let mut decoder = FrameDecoder::new();
    let mut actual = None;
    decoder.feed(encoder.encode(&expected).unwrap(), |record| {
        actual = Some(record.unwrap())
    });
    assert_eq!(actual, Some(expected));
}
