use super::*;

fn frame(counter: u16) -> [u8; 15] {
    let mut frame = [0x41, 0x88, 0, 0x45, 0x4f, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0];
    frame[9..13].copy_from_slice(&IEEE802154_STREAM_MAGIC);
    frame[13..15].copy_from_slice(&counter.to_le_bytes());
    frame
}

#[test]
fn a_frame_without_the_magic_is_not_a_stream_frame() {
    assert_eq!(ieee802154_stream_counter(&[0x41, 0x88, 0, 1, 2, 3]), None);
    assert_eq!(ieee802154_stream_counter(&frame(0x1234)), Some(0x1234));
}

#[test]
fn the_receipt_counts_distinct_counters_duplicates_and_gaps() {
    let mut tracker = Ieee802154StreamTracker::default();
    for counter in [0, 1, 2, 5, 6, 2, 10] {
        tracker.record(&frame(counter));
    }
    tracker.record(&frame(IEEE802154_STREAM_CAPACITY));
    tracker.record(&[0x41, 0x88, 0]);
    assert_eq!(
        tracker.receipt(),
        Ieee802154SessionStreamReceipt {
            received: 6,
            duplicates: 1,
            out_of_range: 1,
            span: 11,
            missing_runs: 2,
            longest_missing_run: 3,
        }
    );
}

#[test]
fn an_empty_stream_has_no_span_and_no_gaps() {
    assert_eq!(
        Ieee802154StreamTracker::default().receipt(),
        Ieee802154SessionStreamReceipt::default()
    );
}
