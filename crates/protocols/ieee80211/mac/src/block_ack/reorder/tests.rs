use super::*;

const WINDOW: usize = 64;
const SLOTS: usize = 34;

type Buffer = RxReorderBuffer<WINDOW, SLOTS>;

fn seq(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

fn frame(sequence: u16, slot: u8) -> RxReorderMpdu {
    RxReorderMpdu {
        sequence: seq(sequence),
        slot,
    }
}

/// Whether `release` holds exactly `expected`, in order.
fn releases<const N: usize>(release: &RxReorderRelease<N>, expected: &[RxReorderMpdu]) -> bool {
    usize::from(release.count) == expected.len() && release.iter().eq(expected.iter().copied())
}

#[test]
fn in_order_frames_are_released_immediately() {
    let mut reorder = Buffer::new(seq(10), 64).unwrap();
    assert_eq!(reorder.retains_on_ingest(seq(10)), Ok(false));
    let release = reorder.ingest(frame(10, 0)).unwrap();
    assert!(releases(&release, &[frame(10, 0)]));
    assert_eq!(reorder.next_sequence(), seq(11));
    assert_eq!(reorder.occupied(), 0);
}

#[test]
fn immediate_ingest_avoids_a_release_list_only_without_a_buffered_successor() {
    let mut reorder = RxReorderBuffer::<WINDOW, 65>::new(seq(100), 16).unwrap();
    assert_eq!(
        reorder.try_ingest_immediate(frame(100, 64)),
        Ok(Some(frame(100, 64)))
    );
    assert_eq!(reorder.next_sequence(), seq(101));
    assert_eq!(reorder.try_ingest_immediate(frame(103, 64)), Ok(None));
    assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
    assert_eq!(reorder.try_ingest_immediate(frame(101, 64)), Ok(None));
    let release = reorder.ingest(frame(101, 64)).unwrap();
    assert!(releases(&release, &[frame(101, 64), frame(102, 2)]));
}

#[test]
fn a_gap_is_buffered_and_then_released_in_sequence_order() {
    let mut reorder = Buffer::new(seq(100), 64).unwrap();
    assert_eq!(reorder.retains_on_ingest(seq(102)), Ok(true));
    assert!(reorder.ingest(frame(102, 2)).unwrap().buffered);
    assert!(reorder.ingest(frame(101, 1)).unwrap().buffered);
    assert_eq!(reorder.retains_on_ingest(seq(100)), Ok(false));
    let release = reorder.ingest(frame(100, 0)).unwrap();
    assert!(releases(
        &release,
        &[frame(100, 0), frame(101, 1), frame(102, 2)]
    ));
    assert_eq!(reorder.next_sequence(), seq(103));
}

#[test]
fn a_window_advance_releases_buffered_frames_and_counts_the_missing() {
    let mut reorder = Buffer::new(seq(0), 64).unwrap();
    reorder.ingest(frame(2, 2)).unwrap();
    reorder.ingest(frame(31, 31)).unwrap();
    let release = reorder.ingest(frame(1000, 1)).unwrap();
    assert!(releases(&release, &[frame(2, 2), frame(31, 31)]));
    let advance = 1000 - 64 + 1;
    assert_eq!(release.missing, advance - 2);
    assert!(release.buffered);
    assert_eq!(reorder.next_sequence(), seq(advance));
}

#[test]
fn gap_expiry_skips_only_the_current_gap() {
    let mut reorder = Buffer::new(seq(20), 8).unwrap();
    reorder.ingest(frame(22, 2)).unwrap();
    reorder.ingest(frame(23, 3)).unwrap();
    let release = reorder.expire_gap();
    assert_eq!(release.missing, 2);
    assert!(releases(&release, &[frame(22, 2), frame(23, 3)]));
    assert_eq!(reorder.next_sequence(), seq(24));
}

#[test]
fn sequence_wrap_and_stale_rejection_are_unambiguous() {
    let mut reorder = Buffer::new(seq(0x0fff), 8).unwrap();
    assert_eq!(reorder.retains_on_ingest(seq(0)), Ok(true));
    reorder.ingest(frame(0, 1)).unwrap();
    let release = reorder.ingest(frame(0x0fff, 0)).unwrap();
    assert!(releases(&release, &[frame(0x0fff, 0), frame(0, 1)]));
    let stale = reorder.ingest(frame(0x0fff, 2)).unwrap();
    assert_eq!(stale.rejected, Some(frame(0x0fff, 2)));
    assert_eq!(reorder.retains_on_ingest(seq(0x0fff)), Ok(false));
}

#[test]
fn retention_prediction_matches_window_advance_and_duplicate_edges() {
    let mut reorder = Buffer::new(seq(10), 8).unwrap();
    assert_eq!(reorder.retains_on_ingest(seq(20)), Ok(true));
    reorder.ingest(frame(20, 0)).unwrap();
    assert_eq!(
        reorder.retains_on_ingest(seq(20)),
        Err(RxReorderError::DuplicateSequence(seq(20)))
    );

    let mut singleton = RxReorderBuffer::<WINDOW, 1>::new(seq(10), 1).unwrap();
    assert_eq!(singleton.retains_on_ingest(seq(20)), Ok(false));
    assert!(!singleton.ingest(frame(20, 0)).unwrap().buffered);
}

#[test]
fn a_slot_cannot_be_owned_twice_or_leave_its_domain() {
    let mut reorder = Buffer::new(seq(1), 8).unwrap();
    reorder.ingest(frame(2, 4)).unwrap();
    assert_eq!(
        reorder.ingest(frame(3, 4)),
        Err(RxReorderError::SlotAlreadyOwned(4))
    );
    let last = (SLOTS - 1) as u8;
    assert!(reorder.ingest(frame(3, last)).is_ok());
    assert_eq!(
        reorder.ingest(frame(4, SLOTS as u8)),
        Err(RxReorderError::InvalidSlot(SLOTS as u8))
    );
}

#[test]
fn the_window_is_bounded_by_the_buffer_capacity() {
    assert!(RxReorderBuffer::<8, SLOTS>::new(seq(0), 8).is_ok());
    assert!(matches!(
        RxReorderBuffer::<8, SLOTS>::new(seq(0), 9),
        Err(RxReorderError::InvalidWindow(9))
    ));
    assert!(matches!(
        Buffer::new(seq(0), 0),
        Err(RxReorderError::InvalidWindow(0))
    ));
}

#[test]
fn a_small_capacity_reorders_within_its_window() {
    let mut reorder = RxReorderBuffer::<8, SLOTS>::new(seq(4090), 8).unwrap();
    reorder.ingest(frame(4093, 3)).unwrap();
    reorder.ingest(frame(4095, 5)).unwrap();
    let release = reorder.ingest(frame(4090, 0)).unwrap();
    assert!(releases(&release, &[frame(4090, 0)]));
    // Sequence 5 lies ten past the start 4091: the window advances by three
    // to end at it, releasing 4093 and passing the missing 4091 and 4092.
    let release = reorder.ingest(frame(5, 7)).unwrap();
    assert!(releases(&release, &[frame(4093, 3)]));
    assert_eq!(release.missing, 2);
    assert!(release.buffered);
    assert_eq!(reorder.next_sequence(), seq(4094));
    // 4094 completes the run through the buffered 4095.
    let release = reorder.ingest(frame(4094, 4)).unwrap();
    assert!(releases(&release, &[frame(4094, 4), frame(4095, 5)]));
}

#[test]
fn stop_releases_every_buffered_slot_in_sequence_order() {
    let mut reorder = Buffer::new(seq(4094), 8).unwrap();
    reorder.ingest(frame(1, 3)).unwrap();
    reorder.ingest(frame(4095, 1)).unwrap();
    let release = reorder.stop();
    assert!(releases(&release, &[frame(4095, 1), frame(1, 3)]));
    assert_eq!(reorder.occupied(), 0);
}

#[test]
fn a_block_ack_request_releases_frames_before_its_start_then_the_run_from_it() {
    let mut reorder = RxReorderBuffer::<WINDOW, 40>::new(seq(10), 8).unwrap();
    reorder.ingest(frame(11, 1)).unwrap();
    reorder.ingest(frame(13, 3)).unwrap();
    reorder.ingest(frame(14, 4)).unwrap();
    let release = reorder.move_window_to(seq(13)).unwrap();
    assert!(releases(
        &release,
        &[frame(11, 1), frame(13, 3), frame(14, 4)]
    ));
    assert_eq!(release.missing, 2);
    assert_eq!(reorder.next_sequence(), seq(15));
}

#[test]
fn a_block_ack_request_at_or_behind_the_window_start_is_ignored() {
    let mut reorder = RxReorderBuffer::<WINDOW, 40>::new(seq(100), 8).unwrap();
    assert!(reorder.move_window_to(seq(100)).is_none());
    assert!(reorder.move_window_to(seq(99)).is_none());
    assert!(reorder.move_window_to(seq(100 + 2048)).is_none());
    assert_eq!(reorder.next_sequence(), seq(100));
}

#[test]
fn a_stale_first_aggregate_rebases_the_window() {
    let mut reorder = Buffer::new(seq(100), 8).unwrap();
    reorder.ingest(frame(102, 2)).unwrap();
    // Ahead of the window: nothing to rebase.
    assert!(
        reorder
            .resynchronize_stale_initial_ampdu(seq(105), false)
            .is_none()
    );
    // Behind the window and behind the negotiated start: the window ends at
    // the received sequence.
    let (released, start) = reorder
        .resynchronize_stale_initial_ampdu(seq(50), false)
        .unwrap();
    assert!(releases(&released, &[frame(102, 2)]));
    assert_eq!(start, seq(43));
    assert_eq!(reorder.next_sequence(), seq(43));

    // A newer format starts at the received sequence itself.
    let mut reorder = Buffer::new(seq(100), 8).unwrap();
    let (_, start) = reorder
        .resynchronize_stale_initial_ampdu(seq(50), true)
        .unwrap();
    assert_eq!(start, seq(50));
}
