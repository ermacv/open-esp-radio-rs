use super::*;

const PEER_A: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER_B: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn agreement(hardware_index: u8, peer: [u8; 6], starting_sequence: u16) -> RxBlockAckSnapshot {
    RxBlockAckSnapshot {
        hardware_index,
        interface: MacInterface::AccessPoint,
        peer,
        tid: 6,
        window: 8,
        starting_sequence,
    }
}

fn segment(address: u32, bytes: &[u8]) -> RxSegment<'_> {
    RxSegment {
        descriptor_address: address,
        descriptor_word0: 0,
        buffer: bytes,
        next_descriptor_address: 0,
    }
}

#[test]
fn one_gap_releases_current_then_retained_frame_in_sequence_order() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(0, PEER_A, 10), |_| {}).unwrap();
    let bytes_10 = [10];
    let bytes_11 = [11];
    let mut released = std::vec::Vec::new();

    let buffered = reorder
        .ingest(
            &storage,
            segment(11, &bytes_11),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 11,
                retry: false,
            },
            None,
            1_000,
            |segment| released.push(segment.descriptor_address),
        )
        .unwrap();
    assert!(buffered.buffered);
    assert!(released.is_empty());
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT - 1);

    let current = reorder
        .ingest(
            &storage,
            segment(10, &bytes_10),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 10,
                retry: false,
            },
            None,
            1_001,
            |segment| released.push(segment.descriptor_address),
        )
        .unwrap();
    assert_eq!(current.dispatched, 1);
    assert_eq!(released, [10]);
    assert!(reorder.has_pending_release());
    assert!(
        reorder.work_due(1_001),
        "an older released owner must be scheduler-visible before a newer MPDU"
    );
    assert!(
        reorder.retained.iter().all(Option::is_none),
        "the pending queue must own the released backing, not a slot alias"
    );
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT - 1);
    assert!(reorder.dispatch_pending(|segment| released.push(segment.descriptor_address)));
    assert_eq!(released, [10, 11]);
    assert!(!reorder.has_pending_release());
    assert!(!reorder.work_due(1_001));
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
    assert_eq!(reorder.next_deadline(), None);
}

#[test]
fn in_order_mpdu_dispatches_without_reorder_backing_or_pending_release() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(0, PEER_A, 10), |_| {}).unwrap();
    let bytes = [10];
    let mut released = std::vec::Vec::new();

    let progress = reorder
        .ingest(
            &storage,
            segment(10, &bytes),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 10,
                retry: false,
            },
            None,
            1_000,
            |segment| released.push(segment.descriptor_address),
        )
        .unwrap();

    assert!(progress.active);
    assert!(!progress.buffered);
    assert_eq!(progress.dispatched, 1);
    assert_eq!(released, [10]);
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
    assert!(!reorder.has_pending_release());
    assert_eq!(reorder.next_deadline(), None);
}

#[test]
fn direct_ingest_is_non_mutating_until_initial_resync_and_on_a_gap() {
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(0, PEER_A, 10), |_| {}).unwrap();
    let key = |sequence| RxBlockAckMpduKey {
        peer: PEER_A,
        tid: 6,
        sequence,
        retry: false,
    };

    assert_eq!(reorder.try_ingest_immediate(key(10), 1_000), Ok(None));
    assert_eq!(reorder.banks.state(0).unwrap().next_sequence(), 10);

    // The complete ingress path owns the one-time baseband resync. Once
    // that edge is complete, direct ingress may advance only an exact
    // in-order frontier.
    reorder.pending_hardware_window_reset[0] = false;
    let progress = reorder
        .try_ingest_immediate(key(10), 1_001)
        .unwrap()
        .expect("the exact frontier is admitted");
    assert!(progress.active);
    assert_eq!(progress.dispatched, 1);
    assert_eq!(reorder.banks.state(0).unwrap().next_sequence(), 11);

    assert_eq!(reorder.try_ingest_immediate(key(12), 1_002), Ok(None));
    assert_eq!(reorder.banks.state(0).unwrap().next_sequence(), 11);
}

#[test]
fn out_of_window_mpdu_advances_only_the_software_reorder_frontier() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(3, PEER_A, 10), |_| {}).unwrap();
    let bytes = [20];

    let progress = reorder
        .ingest(
            &storage,
            segment(20, &bytes),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 20,
                retry: false,
            },
            None,
            1_000,
            |_| panic!("far successor remains buffered"),
        )
        .unwrap();

    assert!(progress.active);
    assert!(progress.buffered);
    assert_eq!(progress.dispatched, 0);
}

#[test]
fn window_advance_that_closes_a_full_run_retains_release_ownership() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(0, PEER_A, 0), |_| {}).unwrap();
    let bytes = [0];
    let mut released = std::vec::Vec::new();

    for sequence in 1..8 {
        let progress = reorder
            .ingest(
                &storage,
                segment(sequence, &bytes),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: sequence as u16,
                    retry: false,
                },
                None,
                sequence as u64,
                |_| panic!("the leading gap retains the partial run"),
            )
            .unwrap();
        assert!(progress.buffered);
    }

    let progress = reorder
        .ingest(
            &storage,
            segment(8, &bytes),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 8,
                retry: false,
            },
            None,
            8,
            |segment| released.push(segment.descriptor_address),
        )
        .unwrap();
    assert!(!progress.buffered);
    assert_eq!(progress.dispatched, 1);
    assert_eq!(released, [1]);

    while reorder.dispatch_pending(|segment| released.push(segment.descriptor_address)) {}
    assert_eq!(released, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
}

#[test]
fn aligned_first_physical_ampdu_does_not_reset_the_hardware_window() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(3, PEER_A, 10), |_| {}).unwrap();
    let bytes = [0];
    let key = |sequence| RxBlockAckMpduKey {
        peer: PEER_A,
        tid: 6,
        sequence,
        retry: false,
    };

    let standalone = reorder
        .ingest(&storage, segment(10, &bytes), key(10), None, 1, |_| {})
        .unwrap();
    assert_eq!(standalone.hardware_window_reset, None);

    let first_ampdu = reorder
        .ingest(&storage, segment(11, &bytes), key(11), Some(2), 2, |_| {})
        .unwrap();
    assert_eq!(first_ampdu.hardware_window_reset, None);

    let next_ampdu = reorder
        .ingest(&storage, segment(12, &bytes), key(12), Some(2), 3, |_| {})
        .unwrap();
    assert_eq!(next_ampdu.hardware_window_reset, None);
}

#[test]
fn stale_first_ht_ampdu_rebases_to_the_negotiated_sequence() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(3, PEER_A, 10), |_| {}).unwrap();
    let bytes = [0];
    let key = |sequence| RxBlockAckMpduKey {
        peer: PEER_A,
        tid: 6,
        sequence,
        retry: false,
    };

    reorder
        .ingest(&storage, segment(10, &bytes), key(10), None, 1, |_| {})
        .unwrap();
    let stale_first_ampdu = reorder
        .ingest(&storage, segment(10, &bytes), key(10), Some(2), 2, |_| {})
        .unwrap();
    assert_eq!(
        stale_first_ampdu.hardware_window_reset,
        Some(Esp32s31AccessPointRxWindowReset {
            hardware_index: 3,
            starting_sequence: 10,
        })
    );
    assert!(!stale_first_ampdu.duplicate);
    assert_eq!(stale_first_ampdu.dispatched, 1);
}

#[test]
fn peer_banks_keep_equal_tid_sequence_spaces_independent() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(0, PEER_A, 20), |_| {}).unwrap();
    reorder.start(agreement(1, PEER_B, 40), |_| {}).unwrap();
    let bytes = [0];

    for (peer, sequence, address) in [(PEER_A, 21, 21), (PEER_B, 41, 41)] {
        reorder
            .ingest(
                &storage,
                segment(address, &bytes),
                RxBlockAckMpduKey {
                    peer,
                    tid: 6,
                    sequence,
                    retry: false,
                },
                None,
                5_000,
                |_| panic!("gap successor must remain retained"),
            )
            .unwrap();
    }
    let mut released = std::vec::Vec::new();
    assert_eq!(
        reorder.expire_due(5_000 + RX_REORDER_GAP_TIMEOUT_MICROS - 1, |segment| {
            released.push(segment.descriptor_address)
        },),
        0
    );
    assert_eq!(
        reorder.expire_due(5_000 + RX_REORDER_GAP_TIMEOUT_MICROS, |segment| released
            .push(segment.descriptor_address),),
        1
    );
    assert_eq!(released, [21]);
    assert_eq!(reorder.next_deadline(), Some(305_000));
    assert_eq!(
        reorder.expire_due(305_000, |segment| released.push(segment.descriptor_address)),
        1
    );
    assert_eq!(released, [21, 41]);
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
}

#[test]
fn peer_teardown_discards_retained_frames_and_releases_backing() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    let agreement = agreement(3, PEER_A, 100);
    reorder.start(agreement, |_| {}).unwrap();
    let bytes = [101];
    reorder
        .ingest(
            &storage,
            segment(101, &bytes),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 101,
                retry: false,
            },
            None,
            0,
            |_| panic!("gap successor must remain retained"),
        )
        .unwrap();
    let current = [100];
    reorder
        .ingest(
            &storage,
            segment(100, &current),
            RxBlockAckMpduKey {
                peer: PEER_A,
                tid: 6,
                sequence: 100,
                retry: false,
            },
            None,
            1,
            |segment| assert_eq!(segment.descriptor_address, 100),
        )
        .unwrap();
    assert!(reorder.has_pending_release());

    assert_eq!(reorder.stop_discard(agreement.identity()), 1);
    assert!(!reorder.has_pending_release());
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
    assert_eq!(reorder.next_deadline(), None);
}

#[test]
fn hardware_rejection_is_safe_only_for_independently_stale_or_owned_sequences() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    reorder.start(agreement(0, PEER_A, 10), |_| {}).unwrap();
    let key = |sequence| RxBlockAckMpduKey {
        peer: PEER_A,
        tid: 6,
        sequence,
        retry: true,
    };
    assert!(!reorder.is_duplicate_or_stale(key(10)));
    assert!(!reorder.is_duplicate_or_stale(key(11)));

    let bytes = [11];
    reorder
        .ingest(&storage, segment(11, &bytes), key(11), None, 0, |_| {})
        .unwrap();
    assert!(reorder.is_duplicate_or_stale(key(11)));

    let bytes = [10];
    reorder
        .ingest(&storage, segment(10, &bytes), key(10), None, 1, |_| {})
        .unwrap();
    assert!(reorder.is_duplicate_or_stale(key(10)));
    assert!(!reorder.is_duplicate_or_stale(key(12)));
    assert!(!reorder.is_duplicate_or_stale(RxBlockAckMpduKey {
        peer: PEER_B,
        ..key(10)
    }));
}

#[test]
fn full_shared_backing_drops_one_frame_without_advancing_sequence_state() {
    let storage = RxReorderFrameStorage::<32>::new();
    let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
    let bytes = [0];
    for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
        let peer = [2, 0, 0, 0, 1, bank as u8];
        reorder
            .start(
                RxBlockAckSnapshot {
                    hardware_index: bank as u8,
                    interface: MacInterface::AccessPoint,
                    peer,
                    tid: 0,
                    window: 64,
                    starting_sequence: 0,
                },
                |_| {},
            )
            .unwrap();
        for sequence in 1..=8 {
            let progress = reorder
                .ingest(
                    &storage,
                    segment((bank * 16 + sequence) as u32, &bytes),
                    RxBlockAckMpduKey {
                        peer,
                        tid: 0,
                        sequence: sequence as u16,
                        retry: false,
                    },
                    None,
                    0,
                    |_| panic!("a leading gap retains every successor"),
                )
                .unwrap();
            assert!(progress.buffered);
        }
    }
    assert_eq!(storage.available_slots(), 0);

    let progress = reorder
        .ingest(
            &storage,
            segment(999, &bytes),
            RxBlockAckMpduKey {
                peer: [2, 0, 0, 0, 1, 0],
                tid: 0,
                sequence: 9,
                retry: false,
            },
            None,
            1,
            |_| panic!("exhausted backing cannot publish out of order"),
        )
        .unwrap();
    assert!(progress.dropped);
    assert!(!progress.buffered);
    assert_eq!(reorder.discard_all(), RX_REORDER_BACKING_SLOT_COUNT as u8);
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
}
