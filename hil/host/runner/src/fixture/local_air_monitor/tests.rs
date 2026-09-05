use super::*;

#[test]
fn target_egress_timing_pairs_ppdu_tail_block_ack_and_next_ppdu() {
    let evidence = target_egress_timing_from_fields(
        "100.000000\t2\t8\n\
         100.000100\t2\t8\n\
         100.000140\t1\t9\n\
         100.000300\t2\t8\n\
         100.000500\t1\t9\n\
         100.000900\t2\t8\n",
    );

    assert_eq!(evidence.target_data_frames, 4);
    assert_eq!(evidence.peer_block_ack_frames, 2);
    assert!(evidence.target_data_pairing_available);
    assert_eq!(
        evidence.peer_block_ack_interarrival,
        Some(AirIntervalSummary {
            samples: 1,
            total_micros: 360,
            minimum_micros: 360,
            p50_micros: 360,
            p95_micros: 360,
            p99_micros: 360,
            maximum_micros: 360,
        })
    );
    assert_eq!(
        evidence.data_to_block_ack,
        Some(AirIntervalSummary {
            samples: 2,
            total_micros: 240,
            minimum_micros: 40,
            p50_micros: 40,
            p95_micros: 200,
            p99_micros: 200,
            maximum_micros: 200,
        })
    );
    assert_eq!(
        evidence.block_ack_to_next_data,
        Some(AirIntervalSummary {
            samples: 2,
            total_micros: 560,
            minimum_micros: 160,
            p50_micros: 160,
            p95_micros: 400,
            p99_micros: 400,
            maximum_micros: 400,
        })
    );
}

#[test]
fn block_ack_cadence_survives_missing_target_data_decode() {
    let evidence = target_egress_timing_from_fields(
        "100.000000\t2\t8\n\
         100.000100\t1\t9\n\
         100.000500\t1\t9\n\
         100.001100\t1\t9\n",
    );

    assert_eq!(evidence.target_data_frames, 1);
    assert_eq!(evidence.peer_block_ack_frames, 3);
    assert!(!evidence.target_data_pairing_available);
    assert_eq!(evidence.data_to_block_ack, None);
    assert_eq!(evidence.block_ack_to_next_data, None);
    assert_eq!(
        evidence.peer_block_ack_interarrival,
        Some(AirIntervalSummary {
            samples: 2,
            total_micros: 1_000,
            minimum_micros: 400,
            p50_micros: 400,
            p95_micros: 600,
            p99_micros: 600,
            maximum_micros: 600,
        })
    );
}

#[test]
fn unpaired_block_ack_disables_pair_derived_timing_even_with_many_data_records() {
    let evidence = target_egress_timing_from_fields(
        "100.000000\t2\t8\n\
         100.000010\t2\t8\n\
         100.000020\t2\t8\n\
         100.000100\t1\t9\n\
         100.000500\t1\t9\n",
    );

    assert_eq!(evidence.target_data_frames, 3);
    assert_eq!(evidence.peer_block_ack_frames, 2);
    assert!(!evidence.target_data_pairing_available);
    assert_eq!(evidence.data_to_block_ack, None);
    assert_eq!(evidence.block_ack_to_next_data, None);
}

#[test]
fn epoch_parser_is_integer_and_microsecond_bounded() {
    assert_eq!(epoch_micros("12"), Some(12_000_000));
    assert_eq!(epoch_micros("12.3"), Some(12_300_000));
    assert_eq!(epoch_micros("12.345678999"), Some(12_345_678));
    assert_eq!(epoch_micros("broken"), None);
    assert_eq!(parse_tshark_u8("0x09"), Some(9));
}

#[test]
fn retry_grouping_counts_one_logical_mpdu() {
    let key = MacFrameKey {
        tid: 0,
        sequence: 12,
        fragment: 0,
    };
    let mut last = BTreeMap::new();
    last.insert(key, 1.0_f64);
    assert!(1.02 - last[&key] <= RETRY_GROUP_SECONDS);
    assert!(1.2 - last[&key] > RETRY_GROUP_SECONDS);
}

#[test]
fn accepts_tshark_boolean_and_numeric_retry_values() {
    assert!(parse_retry_flag("1"));
    assert!(parse_retry_flag("True"));
    assert!(parse_retry_flag("true"));
    assert!(!parse_retry_flag("0"));
    assert!(!parse_retry_flag("False"));
    assert!(!parse_retry_flag(""));
}

#[test]
fn monitor_action_follows_openwrt_primary_channel() {
    assert_eq!(
        resolve_observer_action_from_iw(
            "type AP\n\tchannel 6 (2437 MHz), width: 40 MHz, center1: 2447 MHz\n"
        )
        .unwrap(),
        "observer-ht40-6"
    );
    assert_eq!(
        resolve_observer_action_from_iw(
            "type AP\n\tchannel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz\n"
        )
        .unwrap(),
        "observer-ht40-13"
    );
    assert!(resolve_observer_action_from_iw("channel 3 (2422 MHz)").is_err());
}

#[test]
fn block_ack_tracker_unwraps_windows_and_deduplicates_overlap() {
    let mut tracker = BlockAckTracker::default();
    tracker.observe(4_090, [u8::MAX; 8]);
    tracker.observe(10, [u8::MAX; 8]);
    assert_eq!(tracker.frames, 2);
    assert_eq!(tracker.full_frames, 2);
    assert_eq!(tracker.tail_frames, 0);
    assert_eq!(tracker.hole_frames, 0);
    assert_eq!(tracker.backward_starts, 0);
    assert_eq!(tracker.acknowledged.len(), 80);

    tracker.observe(9, [1; 8]);
    assert_eq!(tracker.backward_starts, 1);
    assert_eq!(tracker.frames, 2);
}

#[test]
fn decodes_little_bit_order_block_ack_bitmap_bytes() {
    let bitmap = decode_block_ack_bitmap("7f00000000000000").unwrap();
    let mut tracker = BlockAckTracker::default();
    tracker.observe(1, bitmap);
    assert_eq!(tracker.tail_frames, 1);
    assert_eq!(tracker.hole_frames, 0);
    assert_eq!(tracker.acknowledged.len(), 7);
}

#[test]
fn distinguishes_normal_block_ack_tail_from_internal_loss_hole() {
    assert!(!block_ack_bitmap_has_internal_hole([
        0xff, 0x7f, 0, 0, 0, 0, 0, 0,
    ]));
    assert!(block_ack_bitmap_has_internal_hole([
        0xff, 0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ]));

    let mut tracker = BlockAckTracker::default();
    tracker.observe(1, [0xff, 0x7f, 0, 0, 0, 0, 0, 0]);
    tracker.observe(16, [0xff, 0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(tracker.full_frames, 0);
    assert_eq!(tracker.tail_frames, 1);
    assert_eq!(tracker.hole_frames, 1);
}
