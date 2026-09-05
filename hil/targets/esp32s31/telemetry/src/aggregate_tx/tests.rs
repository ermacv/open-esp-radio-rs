use super::*;

#[test]
fn block_ack_readiness_is_current_state_while_transitions_are_interval_evidence() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    assert!(!before.block_ack_operational(0));

    counters.observe(AggregateTxObservation::BlockAckOperational {
        tid: 0,
        operational: true,
    });
    let operational = counters.snapshot();
    assert!(operational.block_ack_operational(0));
    assert!(!operational.block_ack_operational(8));
    let delta = operational.wrapping_delta_since(before);
    assert!(delta.block_ack_operational(0));
    assert_eq!(delta.block_ack_operational_transitions, 1);

    counters.set_block_ack_operational(0, true);
    assert_eq!(
        counters
            .snapshot()
            .wrapping_delta_since(operational)
            .block_ack_operational_transitions,
        0
    );

    counters.observe(AggregateTxObservation::BlockAckOperational {
        tid: 0,
        operational: false,
    });
    let stopped = counters.snapshot();
    assert!(!stopped.block_ack_operational(0));
    let delta = stopped.wrapping_delta_since(operational);
    assert!(!delta.block_ack_operational(0));
    assert_eq!(delta.block_ack_operational_transitions, 1);
}

#[test]
fn counters_preserve_distribution_and_timing_deltas() {
    let counters = AggregateTxCounters::new();
    let before = counters.snapshot();
    counters.observe(AggregateTxObservation::NetworkSingleMpdu {
        reason: NetworkSingleMpduReason::BlockAckUnavailable,
        ethernet_length: 42,
    });
    counters.observe(AggregateTxObservation::RateSelected {
        bandwidth_mhz: 40,
        nominal_kbps: 150_000,
    });
    counters.observe(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::QueueEmpty,
    });
    counters.observe(AggregateTxObservation::Prepared {
        subframes: 32,
        stop: AggregateBuildStop::FrameLimit,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 80,
        program_micros: 3,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 95,
        program_micros: 5,
    });
    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 31,
        individual_retry: true,
    });
    counters.observe(AggregateTxObservation::HardwareTimeout);
    counters.observe(AggregateTxObservation::ExchangeCompleted {
        micros: 41,
        publications: 1,
    });
    counters.observe(AggregateTxObservation::ExchangeCompleted {
        micros: 59,
        publications: 3,
    });
    counters.observe(AggregateTxObservation::ExchangeCompleted {
        micros: 71,
        publications: 4,
    });
    counters.observe(AggregateTxObservation::PreparationCompleted { micros: 7 });
    counters.observe(AggregateTxObservation::PreparationCompleted { micros: 11 });
    counters.record_tx_irq_epoch(|| 100);
    counters.observe(AggregateTxObservation::InterruptServiceStarted { at_micros: 109 });

    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.network_single_mpdu_started, 1);
    assert_eq!(delta.network_single_legacy_rate, 0);
    assert_eq!(delta.network_single_block_ack_unavailable, 1);
    assert_eq!(delta.network_single_ht_needs_pair, 0);
    assert_eq!(delta.network_single_fresh_aggregate_capacity, 0);
    assert_eq!(
        delta.network_single_fresh_capacity_lifetime_max_ethernet_length,
        0
    );
    assert_eq!(delta.rate_selections, 1);
    assert_eq!(delta.last_bandwidth_mhz, 40);
    assert_eq!(delta.last_nominal_rate_kbps, 150_000);
    assert_eq!(delta.aggregates_prepared, 2);
    assert_eq!(delta.aggregate_publications, 2);
    assert_eq!(delta.aggregates_completed, 1);
    assert_eq!(delta.subframes_acknowledged, 31);
    assert_eq!(delta.individual_retries, 1);
    assert_eq!(delta.hardware_timeouts, 1);
    assert_eq!(delta.collisions, 0);
    assert_eq!(delta.prepared_subframe_total(), 34);
    assert_eq!(delta.prepared_in_range(1, 1), 0);
    assert_eq!(delta.prepared_in_range(2, 3), 1);
    assert_eq!(delta.prepared_in_range(32, 32), 1);
    assert_eq!(delta.minimum_prepared_subframes(), Some(2));
    assert_eq!(delta.maximum_prepared_subframes(), Some(32));
    assert_eq!(delta.preparation_micros, 18);
    assert_eq!(delta.preparation_lifetime_max_micros, 11);
    assert_eq!(delta.publication_program_micros, 8);
    assert_eq!(delta.publication_program_lifetime_max_micros, 5);
    assert_eq!(delta.exchange_micros, 171);
    assert_eq!(delta.exchange_lifetime_max_micros, 71);
    assert_eq!(delta.single_publication_exchanges, 1);
    assert_eq!(delta.single_publication_exchange_micros, 41);
    assert_eq!(delta.single_publication_exchange_lifetime_max_micros, 41);
    assert_eq!(delta.retried_exchanges, 2);
    assert_eq!(delta.retried_exchange_publications, 7);
    assert_eq!(delta.retried_exchange_micros, 130);
    assert_eq!(delta.retried_exchange_lifetime_max_micros, 71);
    assert_eq!(delta.exchanges_by_publications[1], 1);
    assert_eq!(delta.exchange_micros_by_publications[1], 41);
    assert_eq!(delta.exchange_lifetime_max_micros_by_publications[1], 41);
    assert_eq!(delta.exchanges_by_publications[2], 0);
    assert_eq!(delta.exchanges_by_publications[3], 1);
    assert_eq!(delta.exchange_micros_by_publications[3], 59);
    assert_eq!(delta.exchange_lifetime_max_micros_by_publications[3], 59);
    assert_eq!(delta.exchanges_by_publications[4], 1);
    assert_eq!(delta.exchange_micros_by_publications[4], 71);
    assert_eq!(delta.exchange_lifetime_max_micros_by_publications[4], 71);
    assert_eq!(delta.tx_irq_epochs, 1);
    assert_eq!(delta.tx_irq_service_samples, 1);
    assert_eq!(delta.tx_irq_clock_skew_samples, 0);
    assert_eq!(delta.tx_irq_to_service_micros, 9);
    assert_eq!(delta.tx_irq_to_service_lifetime_max_micros, 9);
    assert_eq!(delta.tx_publication_to_irq_samples, 1);
    assert_eq!(delta.tx_publication_to_irq_micros, 5);
    assert_eq!(delta.tx_publication_to_irq_lifetime_max_micros, 5);
    assert_eq!(delta.stopped_at_frame_limit, 1);
    assert_eq!(delta.stopped_at_capacity_limit, 0);
    assert_eq!(delta.stopped_on_empty_queue, 1);
}

#[test]
fn tx_irq_clock_is_read_only_for_sampled_epochs() {
    use core::cell::Cell;

    let counters = AggregateTxCounters::new();
    let reads = Cell::new(0_u32);
    for _ in 0..64 {
        counters.record_tx_irq_epoch(|| {
            reads.set(reads.get() + 1);
            100
        });
    }

    assert_eq!(reads.get(), 1);
    assert_eq!(counters.snapshot().tx_irq_epochs, 64);
}

#[test]
fn terminal_completion_is_correlated_with_the_next_publication() {
    let counters = AggregateTxCounters::with_clock(|| 100);
    let before = counters.snapshot();

    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 16,
        individual_retry: false,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::ActiveServiceReturned,
        at_micros: 110,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::SchedulerLoopResumed,
        at_micros: 125,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::StopPollCompleted,
        at_micros: 130,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::ControlReadinessChecked { ready: false },
        at_micros: 145,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::PreparedReadinessChecked,
        at_micros: 150,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::PreparedBatchChecked,
        at_micros: 155,
    });
    counters.observe(AggregateTxObservation::PreparedSchedulerPhase {
        phase: PreparedTxSchedulerPhase::PreparedEntry,
        at_micros: 160,
    });
    counters.observe(AggregateTxObservation::Published {
        at_micros: 175,
        program_micros: 4,
    });

    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.completion_to_publication_samples, 1);
    assert_eq!(delta.completion_to_publication_micros, 75);
    assert_eq!(delta.completion_to_publication_lifetime_max_micros, 75);
    assert_eq!(delta.completion_to_prepared_entry_samples, 1);
    assert_eq!(delta.completion_to_prepared_entry_micros, 60);
    assert_eq!(delta.prepared_entry_to_publication_samples, 1);
    assert_eq!(delta.prepared_entry_to_publication_micros, 15);
    assert_eq!(delta.prepared_scheduler_timing.samples, 1);
    assert_eq!(delta.prepared_scheduler_timing.scheduler_passes, 1);
    assert_eq!(delta.prepared_scheduler_timing.control_ready_passes, 0);
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .completion_to_active_service_return
            .micros,
        10
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .active_service_return_to_scheduler_loop
            .micros,
        15
    );
    assert_eq!(delta.prepared_scheduler_timing.stop_poll.micros, 5);
    assert_eq!(delta.prepared_scheduler_timing.control_readiness.micros, 15);
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .control_check_to_prepared_entry
            .micros,
        15
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .control_check_to_prepared_readiness
            .micros,
        5
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .prepared_readiness_to_batch
            .micros,
        5
    );
    assert_eq!(
        delta
            .prepared_scheduler_timing
            .prepared_batch_to_entry
            .micros,
        5
    );
}

#[test]
fn interval_boundary_drops_stale_timing_and_ap_sequence_correlations() {
    fn ap_udp_frame(sequence: u32) -> [u8; 46] {
        let mut ethernet = [0_u8; 46];
        ethernet[14] = 0x45;
        ethernet[23] = 17;
        ethernet[34..36].copy_from_slice(&4_324_u16.to_be_bytes());
        ethernet[42..46].copy_from_slice(&sequence.to_be_bytes());
        ethernet
    }

    let counters = AggregateTxCounters::with_clock(|| 100);
    counters.observe(AggregateTxObservation::Completed {
        acknowledged: 32,
        individual_retry: false,
    });
    counters.observe_access_point_network_claim(&ap_udp_frame(7));

    counters.begin_interval();
    let before = counters.snapshot();
    counters.observe(AggregateTxObservation::Published {
        at_micros: 1_000_000,
        program_micros: 4,
    });
    counters.observe_access_point_network_claim(&ap_udp_frame(0));
    counters.observe_access_point_network_claim(&ap_udp_frame(1));

    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.completion_to_publication_samples, 0);
    assert_eq!(delta.completion_to_publication_micros, 0);
    assert_eq!(delta.ap_udp_claimed, 2);
    assert_eq!(delta.ap_udp_claim_backward, 0);
    assert_eq!(delta.ap_udp_claim_first_previous, u32::MAX);
    assert_eq!(delta.ap_udp_claim_first_sequence, u32::MAX);
    assert_eq!(delta.ap_udp_claim_maximum_distance, 0);
}

#[test]
fn scheduler_trace_recorder_preserves_detours_and_resets_at_terminal_service() {
    let trace = PreparedTxSchedulerTraceRecorder::new();
    trace.record(PreparedTxSchedulerPhase::ActiveServiceReturned, 10);
    trace.record(PreparedTxSchedulerPhase::SchedulerLoopResumed, 20);
    trace.record(
        PreparedTxSchedulerPhase::ControlReadinessChecked { ready: true },
        25,
    );
    trace.record(PreparedTxSchedulerPhase::ActiveServiceReturned, 30);
    trace.record(PreparedTxSchedulerPhase::SchedulerLoopResumed, 40);
    trace.record(PreparedTxSchedulerPhase::StopPollCompleted, 45);
    trace.record(
        PreparedTxSchedulerPhase::ControlReadinessChecked { ready: false },
        50,
    );
    trace.record(PreparedTxSchedulerPhase::PreparedReadinessChecked, 51);
    trace.record(PreparedTxSchedulerPhase::PreparedBatchChecked, 53);
    trace.record(PreparedTxSchedulerPhase::PreparedEntry, 55);

    assert_eq!(
        trace.take(),
        Some(PreparedTxSchedulerTrace {
            active_service_returned_micros: 30,
            scheduler_loop_resumed_micros: 40,
            stop_poll_completed_micros: 45,
            control_readiness_checked_micros: 50,
            prepared_readiness_checked_micros: 51,
            prepared_batch_checked_micros: 53,
            prepared_entry_micros: 55,
            scheduler_passes: 1,
            control_ready_passes: 0,
        })
    );
    assert_eq!(trace.take(), None);
}

#[test]
fn incomplete_scheduler_trace_cannot_become_a_timing_sample() {
    let trace = PreparedTxSchedulerTraceRecorder::new();
    trace.record(PreparedTxSchedulerPhase::ActiveServiceReturned, 10);
    trace.record(PreparedTxSchedulerPhase::PreparedEntry, 45);

    assert_eq!(trace.take(), None);
}
