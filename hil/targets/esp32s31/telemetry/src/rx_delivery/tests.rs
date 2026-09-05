use super::*;

#[test]
fn sequence_tracker_separates_gap_recovery_duplicate_and_terminal_tail() {
    let mut tracker = SequenceTracker::default();
    for sequence in [0, 2, 1, 1, -1, 3] {
        tracker.observe(sequence);
    }
    assert_eq!(tracker.evidence.data_units, 5);
    assert_eq!(tracker.evidence.gap_events, 1);
    assert_eq!(tracker.evidence.forward_missing, 1);
    assert_eq!(tracker.evidence.late_recovered, 1);
    assert_eq!(tracker.evidence.duplicates, 1);
    assert_eq!(tracker.evidence.control_markers, 1);
    assert_eq!(tracker.evidence.data_after_terminal, 1);
}

#[test]
fn ledger_reports_the_first_exact_enqueue_consumer_divergence() {
    let mut ledger = SequenceLedger::<8>::default();
    for sequence in [10, 11, 12] {
        ledger.push(sequence);
    }
    ledger.consume(10);
    ledger.consume(12);
    ledger.consume(11);
    let evidence = ledger.finish();
    assert_eq!(evidence.matched, 2);
    assert_eq!(evidence.skipped_before_observed, 1);
    assert_eq!(evidence.unexpected_consumer, 1);
    assert_eq!(evidence.first_expected, Some(11));
    assert_eq!(evidence.first_observed, Some(12));
}

#[test]
fn session_tracker_accounts_network_drop_without_entering_ledger() {
    let mut tracker = RxDeliveryTracker::<8>::new();
    tracker.begin(7);
    tracker.admitted(0, Some((0, 100)));
    tracker.dropped(1, Some((0, 101)), NetworkDropReason::QueueFull);
    tracker.consumed(7, 0);
    tracker.consumed(8, 1);
    let evidence = tracker
        .finish(7, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(evidence.post_reorder.data_units, 2);
    assert_eq!(evidence.network_enqueued.data_units, 1);
    assert_eq!(evidence.udp_consumer.data_units, 1);
    assert_eq!(evidence.network_queue_full, 1);
    assert_eq!(evidence.consumer_ledger.matched, 1);
}

#[test]
fn session_preserves_drop_causes_and_resets_them_between_runs() {
    let mut tracker = RxDeliveryTracker::<8>::new();
    // Events outside a session must not leak into the next run.
    tracker.dropped(0, None, NetworkDropReason::PoolExhausted);
    tracker.begin(7);
    tracker.admitted(0, None);
    for (sequence, reason) in [
        (1, NetworkDropReason::QueueFull),
        (2, NetworkDropReason::InvalidLength),
        (3, NetworkDropReason::PoolExhausted),
        (4, NetworkDropReason::PoolExhausted),
        (5, NetworkDropReason::LinkDown),
    ] {
        tracker.dropped(sequence, None, reason);
    }
    tracker.consumed(7, 0);
    let evidence = tracker
        .finish(7, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(evidence.post_reorder.data_units, 6);
    assert_eq!(evidence.network_enqueued.data_units, 1);
    assert_eq!(evidence.udp_consumer.data_units, 1);
    assert_eq!(evidence.network_queue_full, 1);
    assert_eq!(evidence.network_invalid_length, 1);
    assert_eq!(evidence.network_pool_exhausted, 2);
    assert_eq!(evidence.network_link_down, 1);
    assert_eq!(evidence.consumer_ledger.matched, 1);
    assert_eq!(evidence.consumer_ledger.enqueued_not_consumed, 0);

    tracker.begin(8);
    let next = tracker
        .finish(8, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(next, RxDeliveryEvidence::default());
}
