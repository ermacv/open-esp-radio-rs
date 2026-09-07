use super::*;

macro_rules! rx_request {
    ($interface:expr, $peer:expr, $token:expr, $tid:expr, $immediate:expr, $window:expr, $timeout:expr, $start:expr) => {
        RxBlockAckRequest {
            interface: $interface,
            peer: $peer,
            dialog_token: $token,
            tid: $tid,
            immediate: $immediate,
            requested_window: $window,
            timeout_tu: $timeout,
            starting_sequence: $start,
        }
    };
}

fn frame(sequence: u16, slot: u8) -> RxAmpduMpdu {
    RxAmpduMpdu { sequence, slot }
}

#[test]
fn station_and_access_point_share_one_public_reorder_classifier() {
    let local = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut raw = [0_u8; PUBLIC_HEADER_SIZE + 32];
    let frame = &mut raw[PUBLIC_HEADER_SIZE..];
    frame[..2].copy_from_slice(&0x4188_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&local);
    frame[10..16].copy_from_slice(&peer);
    frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
    frame[24] = 5;
    let expected = RxBlockAckMpduKey {
        peer,
        tid: 5,
        sequence: 0x123,
        retry: false,
    };

    assert_eq!(
        rx_block_ack_mpdu_key(&raw, local, Some(peer)),
        Some(expected)
    );
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), Some(expected));
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, Some([3; 6])), None);

    raw[PUBLIC_HEADER_SIZE + 22] |= 1;
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), None);

    raw[PUBLIC_HEADER_SIZE + 22] &= !1;
    raw[PUBLIC_HEADER_SIZE + 1] |= 0x04;
    assert_eq!(rx_block_ack_mpdu_key(&raw, local, None), None);
}

#[test]
fn reset_clears_sessions_without_widening_the_integration_limit() {
    let peer = [2, 0, 0, 0, 0, 1];
    let mut sessions = RxBlockAckSessions::<1>::with_maximum_window(16).unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            peer,
            7,
            0,
            true,
            64,
            0,
            10
        ))
        .unwrap();
    let activation = sessions.begin_pending().unwrap().unwrap();
    sessions.commit(activation).unwrap();
    assert!(sessions.snapshots().iter().any(Option::is_some));

    sessions.reset_after_hardware_reset();

    assert_eq!(sessions.maximum_window(), 16);
    assert!(sessions.snapshots().iter().all(Option::is_none));
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            peer,
            8,
            0,
            true,
            64,
            0,
            20
        ))
        .unwrap();
    let activation = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(activation.negotiated().window, 16);
    assert_eq!(activation.hardware().window, RX_BLOCK_ACK_MAX_WINDOW);
}

#[test]
fn in_order_frames_are_released_immediately() {
    let mut reorder = RxBlockAckReorder::new(10, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
    assert_eq!(reorder.retains_on_ingest(10), Ok(false));
    let release = reorder.ingest(frame(10, 0)).unwrap();
    assert_eq!(release.iter().collect::<std::vec::Vec<_>>(), [frame(10, 0)]);
    assert_eq!(reorder.next_sequence(), 11);
    assert_eq!(reorder.occupied(), 0);
}

#[test]
fn immediate_ingest_avoids_a_release_list_only_without_a_buffered_successor() {
    let mut reorder = RxBlockAckReorderState::<65>::new(100, 16).unwrap();
    assert_eq!(
        reorder.try_ingest_immediate(frame(100, 64)),
        Ok(Some(frame(100, 64)))
    );
    assert_eq!(reorder.next_sequence(), 101);
    assert_eq!(reorder.occupied(), 0);

    assert_eq!(reorder.try_ingest_immediate(frame(103, 64)), Ok(None));
    assert_eq!(reorder.next_sequence(), 101);

    assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
    assert_eq!(reorder.try_ingest_immediate(frame(101, 64)), Ok(None));
    let release = reorder.ingest(frame(101, 64)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(101, 64), frame(102, 2)]
    );
}

#[test]
fn gap_is_buffered_and_then_released_in_sequence_order() {
    let mut reorder = RxBlockAckReorder::new(100, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
    assert_eq!(reorder.retains_on_ingest(102), Ok(true));
    assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
    assert_eq!(reorder.retains_on_ingest(101), Ok(true));
    assert!(reorder.ingest(frame(101, 1)).unwrap().buffered);
    assert_eq!(reorder.retains_on_ingest(100), Ok(false));
    let release = reorder.ingest(frame(100, 0)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(100, 0), frame(101, 1), frame(102, 2)]
    );
    assert_eq!(reorder.next_sequence(), 103);
}

#[test]
#[cfg(not(feature = "rx-ba-window-8"))]
fn window_advance_releases_owned_frames_and_counts_missing_without_long_loop() {
    let mut reorder = RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
    reorder.ingest(frame(2, 2)).unwrap();
    reorder.ingest(frame(31, 31)).unwrap();
    let release = reorder.ingest(frame(1000, 1)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(2, 2), frame(31, 31)]
    );
    let expected_advance = 1000 - RX_BLOCK_ACK_MAX_WINDOW + 1;
    assert_eq!(release.missing, expected_advance - 2);
    assert!(release.buffered);
    assert_eq!(reorder.next_sequence(), expected_advance);
}

#[test]
fn async_expiry_skips_only_the_current_gap() {
    let mut reorder = RxBlockAckReorder::new(20, 8).unwrap();
    reorder.ingest(frame(22, 2)).unwrap();
    reorder.ingest(frame(23, 3)).unwrap();
    let release = reorder.expire_gap();
    assert_eq!(release.missing, 2);
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(22, 2), frame(23, 3)]
    );
    assert_eq!(reorder.next_sequence(), 24);
}

#[test]
fn sequence_wrap_and_stale_rejection_are_unambiguous() {
    let mut reorder = RxBlockAckReorder::new(0x0fff, 8).unwrap();
    assert_eq!(reorder.retains_on_ingest(0), Ok(true));
    reorder.ingest(frame(0, 1)).unwrap();
    let release = reorder.ingest(frame(0x0fff, 0)).unwrap();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(0x0fff, 0), frame(0, 1)]
    );
    let stale = reorder.ingest(frame(0x0fff, 2)).unwrap();
    assert_eq!(stale.rejected, Some(frame(0x0fff, 2)));
    assert_eq!(reorder.retains_on_ingest(0x0fff), Ok(false));
}

#[test]
fn retention_prediction_matches_window_advance_and_duplicate_edges() {
    let mut reorder = RxBlockAckReorder::new(10, 8).unwrap();
    assert_eq!(reorder.retains_on_ingest(20), Ok(true));
    reorder.ingest(frame(20, 0)).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(20),
        Err(RxAmpduError::DuplicateSequence(20))
    );

    let mut singleton = RxBlockAckReorderState::<1>::new(10, 1).unwrap();
    assert_eq!(singleton.retains_on_ingest(20), Ok(false));
    assert!(!singleton.ingest(frame(20, 0)).unwrap().buffered);
}

#[test]
fn a_slot_index_cannot_be_owned_twice() {
    let mut reorder = RxBlockAckReorder::new(1, 8).unwrap();
    reorder.ingest(frame(2, 4)).unwrap();
    assert_eq!(
        reorder.ingest(frame(3, 4)),
        Err(RxAmpduError::SlotAlreadyOwned(4))
    );
}

#[test]
fn esf_slot_id_is_independent_of_reorder_window_index() {
    let mut reorder = RxBlockAckReorder::new(1, RX_BLOCK_ACK_MAX_WINDOW).unwrap();
    let highest_valid = (RX_REORDER_SLOT_ID_CAPACITY - 1) as u8;
    let first_invalid = RX_REORDER_SLOT_ID_CAPACITY as u8;
    assert_eq!(
        reorder
            .ingest(frame(1, highest_valid))
            .unwrap()
            .iter()
            .collect::<std::vec::Vec<_>>(),
        [frame(1, highest_valid)]
    );
    assert_eq!(
        reorder.ingest(frame(2, first_invalid)),
        Err(RxAmpduError::InvalidSlot(first_invalid))
    );
}

#[test]
fn integration_can_bind_the_reorder_to_its_exact_slot_domain() {
    let mut reorder = RxBlockAckReorderState::<40>::new(1, 8).unwrap();
    assert_eq!(
        reorder
            .ingest(frame(1, 39))
            .unwrap()
            .iter()
            .collect::<std::vec::Vec<_>>(),
        [frame(1, 39)]
    );
    assert_eq!(
        reorder.ingest(frame(2, 40)),
        Err(RxAmpduError::InvalidSlot(40))
    );
}

#[test]
fn window_cannot_exceed_the_owned_reorder_slot_pool() {
    assert!(RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW).is_ok());
    assert!(matches!(
        RxBlockAckReorder::new(0, RX_BLOCK_ACK_MAX_WINDOW + 1),
        Err(RxAmpduError::InvalidWindow(_))
    ));
}

#[test]
fn stop_releases_every_owned_slot_in_sequence_order() {
    let mut reorder = RxBlockAckReorder::new(4094, 8).unwrap();
    reorder.ingest(frame(1, 3)).unwrap();
    reorder.ingest(frame(4095, 1)).unwrap();
    let release = reorder.stop();
    assert_eq!(
        release.iter().collect::<std::vec::Vec<_>>(),
        [frame(4095, 1), frame(1, 3)]
    );
    assert_eq!(reorder.occupied(), 0);
}

#[test]
fn successful_response_narrows_the_window_and_disables_amsdu() {
    let mut body = [0xff; 9];
    write_successful_addba_response(&mut body, 137, 0, 16).unwrap();
    assert_eq!(body, [3, 1, 137, 0, 0, 0x02, 0x04, 0, 0]);
    assert_eq!(
        crate::tx::ampdu::parse_block_ack_action(&body),
        Some(crate::tx::ampdu::BlockAckAction::AddbaResponse {
            dialog_token: 137,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: false,
            window: 16,
            timeout_tu: 0,
        })
    );
}

#[test]
fn declined_response_preserves_request_identity_without_claiming_success() {
    let mut body = [0xff; 9];
    write_declined_addba_response(&mut body, 23, 6, 64).unwrap();
    assert_eq!(
        crate::tx::ampdu::parse_block_ack_action(&body),
        Some(crate::tx::ampdu::BlockAckAction::AddbaResponse {
            dialog_token: 23,
            status: ADDBA_STATUS_REQUEST_DECLINED,
            tid: 6,
            immediate: true,
            amsdu: false,
            window: 64,
            timeout_tu: 0,
        })
    );
}

#[test]
fn station_rx_sessions_bind_protocol_window_hardware_bank_and_response() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            17,
            7,
            true,
            1023,
            0,
            0x0abc
        ))
        .unwrap();
    let activation = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(
        activation.negotiated(),
        RxBlockAckSnapshot {
            hardware_index: 0,
            interface: MacInterface::Station,
            peer,
            tid: 7,
            window: RX_BLOCK_ACK_MAX_WINDOW,
            starting_sequence: 0x0abc,
        }
    );
    assert_eq!(
        activation.hardware(),
        S31RxBlockAckAgreement {
            hardware_index: 0,
            interface: MacInterface::Station,
            peer,
            tid: 7,
            starting_sequence: 0x0abc,
            window: RX_BLOCK_ACK_MAX_WINDOW,
        }
    );
    assert_eq!(
        crate::tx::ampdu::parse_block_ack_action(activation.response_body()),
        Some(crate::tx::ampdu::BlockAckAction::AddbaResponse {
            dialog_token: 17,
            status: 0,
            tid: 7,
            immediate: true,
            amsdu: false,
            window: RX_BLOCK_ACK_MAX_WINDOW,
            timeout_tu: 0,
        })
    );
    assert!(matches!(
        sessions.begin_pending(),
        Err(RxBlockAckSessionsError::ActivationBusy)
    ));
    let snapshot = sessions.commit(activation).unwrap();
    assert_eq!(sessions.snapshots()[0], Some(snapshot));
    assert_eq!(
        sessions.stop(MacInterface::Station, peer, 7),
        Some(snapshot)
    );
    assert_eq!(sessions.snapshots(), [None; RX_BLOCK_ACK_BANK_COUNT]);
}

#[test]
fn integration_can_narrow_the_negotiated_rx_window_without_changing_hardware_geometry() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<1>::with_maximum_window(32).unwrap();
    assert_eq!(sessions.maximum_window(), 32);
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            17,
            0,
            true,
            64,
            0,
            123
        ))
        .unwrap();

    let activation = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(activation.negotiated().window, 32);
    assert_eq!(activation.hardware().window, RX_BLOCK_ACK_MAX_WINDOW);
    assert_eq!(
        crate::tx::ampdu::parse_block_ack_action(activation.response_body()),
        Some(crate::tx::ampdu::BlockAckAction::AddbaResponse {
            dialog_token: 17,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: false,
            window: 32,
            timeout_tu: 0,
        })
    );

    assert!(matches!(
        RxBlockAckSessions::<1>::with_maximum_window(0),
        Err(RxBlockAckSessionsError::InvalidWindow(0))
    ));
    assert!(matches!(
        RxBlockAckSessions::<1>::with_maximum_window(RX_BLOCK_ACK_MAX_WINDOW + 1),
        Err(RxBlockAckSessionsError::InvalidWindow(window))
            if window == RX_BLOCK_ACK_MAX_WINDOW + 1
    ));
}

#[test]
fn replacement_and_cancel_remove_the_previous_hardware_owner() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            32,
            0,
            10
        ))
        .unwrap();
    let first = sessions.begin_pending().unwrap().unwrap();
    let first_snapshot = sessions.commit(first).unwrap();

    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            2,
            0,
            true,
            16,
            0,
            20
        ))
        .unwrap();
    let replacement = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(replacement.replaced(), Some(first_snapshot));
    assert_eq!(replacement.hardware().hardware_index, 0);
    sessions.cancel(replacement).unwrap();
    assert_eq!(sessions.snapshots(), [None; RX_BLOCK_ACK_BANK_COUNT]);
}

#[test]
fn station_rx_sessions_reject_every_unsupported_request_class() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            false,
            32,
            0,
            0
        )),
        Err(RxBlockAckSessionsError::DelayedPolicyUnsupported)
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            8,
            true,
            32,
            0,
            0
        )),
        Err(RxBlockAckSessionsError::InvalidTid(8))
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            0,
            0,
            0
        )),
        Err(RxBlockAckSessionsError::InvalidWindow(0))
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            32,
            1,
            0
        )),
        Err(RxBlockAckSessionsError::NonzeroTimeout(1))
    );
    assert_eq!(
        sessions.offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            32,
            0,
            0x1000
        )),
        Err(RxBlockAckSessionsError::InvalidStartingSequence(0x1000))
    );
}

#[test]
fn access_point_peers_with_the_same_tid_receive_distinct_hardware_banks() {
    let first_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let second_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();

    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            1,
            0,
            true,
            32,
            0,
            10
        ))
        .unwrap();
    let first = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(first.hardware().interface, MacInterface::AccessPoint);
    assert_eq!(first.hardware().hardware_index, 0);
    sessions.commit(first).unwrap();

    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            second_peer,
            2,
            0,
            true,
            32,
            0,
            20
        ))
        .unwrap();
    let second = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(second.hardware().hardware_index, 1);
    sessions.commit(second).unwrap();

    assert!(
        sessions
            .stop(MacInterface::AccessPoint, first_peer, 0)
            .is_some()
    );
    assert!(
        sessions
            .stop(MacInterface::AccessPoint, second_peer, 0)
            .is_some()
    );
}

#[test]
fn station_and_access_point_with_the_same_peer_tid_use_distinct_banks() {
    let peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let mut sessions = RxBlockAckSessions::<2>::new();

    sessions
        .offer(rx_request!(
            MacInterface::Station,
            peer,
            1,
            0,
            true,
            16,
            0,
            10
        ))
        .unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            peer,
            2,
            0,
            true,
            16,
            0,
            20
        ))
        .unwrap();

    let station = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(station.negotiated().interface, MacInterface::Station);
    assert_eq!(station.negotiated().hardware_index, 0);
    sessions.commit(station).unwrap();

    let access_point = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(
        access_point.negotiated().interface,
        MacInterface::AccessPoint
    );
    assert_eq!(access_point.negotiated().hardware_index, 1);
    sessions.commit(access_point).unwrap();

    assert_eq!(
        sessions.snapshots()[0].unwrap().interface,
        MacInterface::Station
    );
    assert_eq!(
        sessions.snapshots()[1].unwrap().interface,
        MacInterface::AccessPoint
    );
}

#[test]
fn preparing_access_point_preserves_station_banks() {
    let station_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let access_point_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let mut sessions = RxBlockAckSessions::<2>::new();
    sessions
        .offer(rx_request!(
            MacInterface::Station,
            station_peer,
            1,
            0,
            true,
            16,
            0,
            10
        ))
        .unwrap();
    let station = sessions.begin_pending().unwrap().unwrap();
    let station = sessions.commit(station).unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            access_point_peer,
            2,
            0,
            true,
            16,
            0,
            20
        ))
        .unwrap();

    sessions
        .prepare_interface(MacInterface::AccessPoint)
        .unwrap();

    assert_eq!(
        sessions.snapshots_for(MacInterface::Station)[0],
        Some(station)
    );
    assert!(
        sessions
            .snapshots_for(MacInterface::AccessPoint)
            .into_iter()
            .all(|entry| entry.is_none())
    );
    assert!(matches!(sessions.begin_pending(), Ok(None)));
    assert_eq!(
        sessions.prepare_interface(MacInterface::Station),
        Err(RxBlockAckSessionsError::InterfaceActive(
            MacInterface::Station
        ))
    );
}

#[test]
fn request_remains_pending_while_every_hardware_bank_is_owned() {
    let mut sessions = RxBlockAckSessions::<{ RX_BLOCK_ACK_BANK_COUNT + 1 }>::new();
    for index in 0..RX_BLOCK_ACK_BANK_COUNT {
        let peer = [2, 0, 0, 0, 0, index as u8];
        sessions
            .offer(rx_request!(
                MacInterface::AccessPoint,
                peer,
                index as u8,
                0,
                true,
                32,
                0,
                index as u16
            ))
            .unwrap();
        let activation = sessions.begin_pending().unwrap().unwrap();
        sessions.commit(activation).unwrap();
    }

    let waiting_peer = [2, 0, 0, 0, 1, 0];
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            waiting_peer,
            9,
            0,
            true,
            32,
            0,
            100
        ))
        .unwrap();
    assert!(matches!(
        sessions.begin_pending(),
        Err(RxBlockAckSessionsError::NoFreeHardwareBank)
    ));

    let released_peer = [2, 0, 0, 0, 0, 0];
    assert!(
        sessions
            .stop(MacInterface::AccessPoint, released_peer, 0)
            .is_some()
    );
    let activation = sessions
        .begin_pending()
        .unwrap()
        .expect("bank release must admit the retained pending request");
    assert_eq!(activation.negotiated().peer, waiting_peer);
}

#[test]
fn explicit_decline_removes_only_the_selected_pending_request() {
    let first_peer = [2, 0, 0, 0, 0, 1];
    let second_peer = [2, 0, 0, 0, 0, 2];
    let mut sessions = RxBlockAckSessions::<2>::new();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            1,
            1,
            true,
            16,
            0,
            10
        ))
        .unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            second_peer,
            2,
            1,
            true,
            16,
            0,
            20
        ))
        .unwrap();

    assert!(sessions.discard_pending(MacInterface::AccessPoint, first_peer, 1));
    assert!(!sessions.discard_pending(MacInterface::AccessPoint, first_peer, 1));
    let activation = sessions
        .begin_pending()
        .unwrap()
        .expect("other peer request remains pending");
    assert_eq!(activation.negotiated().peer, second_peer);
}

#[test]
fn peer_teardown_removes_pending_and_active_agreements_only_for_that_peer() {
    let first_peer = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let second_peer = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let mut sessions = RxBlockAckSessions::<RX_BLOCK_ACK_BANK_COUNT>::new();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            1,
            0,
            true,
            32,
            0,
            10
        ))
        .unwrap();
    let active = sessions.begin_pending().unwrap().unwrap();
    sessions.commit(active).unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            first_peer,
            2,
            1,
            true,
            32,
            0,
            20
        ))
        .unwrap();
    sessions
        .offer(rx_request!(
            MacInterface::AccessPoint,
            second_peer,
            3,
            1,
            true,
            32,
            0,
            30
        ))
        .unwrap();

    let stopped = sessions.stop_peer(MacInterface::AccessPoint, first_peer);
    assert_eq!(stopped.into_iter().flatten().count(), 1);
    let remaining = sessions.begin_pending().unwrap().unwrap();
    assert_eq!(remaining.negotiated().peer, second_peer);
}
