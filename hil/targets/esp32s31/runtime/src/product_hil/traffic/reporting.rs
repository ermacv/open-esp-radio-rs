#![forbid(unsafe_code)]

use core::future::Future;

use embassy_futures::yield_now;
use embassy_time::Instant;
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::{AggregateTxCounterSnapshot, AggregateTxCounters},
    mac_irq::MacIrqClassificationSnapshot,
    rx_pipeline::{RxPipelineCounterSnapshot, RxPipelineCounters},
    task_poll::{TaskPollCounters, TaskPollSet, TaskPollSetSnapshot, TaskPollSnapshot},
};

use crate::console::runtime_log;

pub(in crate::product_hil) async fn log_open_radio_ampdu_interval(
    earlier: AggregateTxCounterSnapshot,
    counters: &AggregateTxCounters,
) {
    let aggregate = counters.snapshot().wrapping_delta_since(earlier);
    let aggregate_min = aggregate.minimum_prepared_subframes().unwrap_or(0);
    let aggregate_max = aggregate.maximum_prepared_subframes().unwrap_or(0);
    runtime_log(format_args!(
        "OAMP aggregates={} publications={} completed={} subframes={} \
         acknowledged={} single={} single_rate={} single_ba={} single_pair={} \
         single_capacity={} single_capacity_max_len={} individual_retry={} timeout={} collision={} \
         min={} max={} stop_frame={} stop_capacity={} stop_empty={}",
        aggregate.aggregates_prepared,
        aggregate.aggregate_publications,
        aggregate.aggregates_completed,
        aggregate.prepared_subframe_total(),
        aggregate.subframes_acknowledged,
        aggregate.network_single_mpdu_started,
        aggregate.network_single_legacy_rate,
        aggregate.network_single_block_ack_unavailable,
        aggregate.network_single_ht_needs_pair,
        aggregate.network_single_fresh_aggregate_capacity,
        aggregate.network_single_fresh_capacity_lifetime_max_ethernet_length,
        aggregate.individual_retries,
        aggregate.hardware_timeouts,
        aggregate.collisions,
        aggregate_min,
        aggregate_max,
        aggregate.stopped_at_frame_limit,
        aggregate.stopped_at_capacity_limit,
        aggregate.stopped_on_empty_queue,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "OAMPH one={} two_three={} four_seven={} eight_fifteen={} \
         sixteen_twentythree={} twentyfour_thirty={} thirtyone={} full32={}",
        aggregate.prepared_in_range(1, 1),
        aggregate.prepared_in_range(2, 3),
        aggregate.prepared_in_range(4, 7),
        aggregate.prepared_in_range(8, 15),
        aggregate.prepared_in_range(16, 23),
        aggregate.prepared_in_range(24, 30),
        aggregate.prepared_in_range(31, 31),
        aggregate.prepared_in_range(32, 32),
    ));
    yield_now().await;
    runtime_log(format_args!(
        "OAMPT preparation_us={} preparation_max_us={} publication_us={} \
         publication_max_us={} exchange_us={} exchange_max_us={} \
         first_exchanges={} first_exchange_us={} first_exchange_max_us={} \
         retried_exchanges={} retry_publications={} retry_exchange_us={} retry_exchange_max_us={} \
         r2={}/{} r3={}/{} r4={}/{}",
        aggregate.preparation_micros,
        aggregate.preparation_lifetime_max_micros,
        aggregate.publication_program_micros,
        aggregate.publication_program_lifetime_max_micros,
        aggregate.exchange_micros,
        aggregate.exchange_lifetime_max_micros,
        aggregate.single_publication_exchanges,
        aggregate.single_publication_exchange_micros,
        aggregate.single_publication_exchange_lifetime_max_micros,
        aggregate.retried_exchanges,
        aggregate.retried_exchange_publications,
        aggregate.retried_exchange_micros,
        aggregate.retried_exchange_lifetime_max_micros,
        aggregate.exchanges_by_publications[2],
        aggregate.exchange_lifetime_max_micros_by_publications[2],
        aggregate.exchanges_by_publications[3],
        aggregate.exchange_lifetime_max_micros_by_publications[3],
        aggregate.exchanges_by_publications[4],
        aggregate.exchange_lifetime_max_micros_by_publications[4],
    ));
    yield_now().await;
    runtime_log(format_args!(
        "OAMPB operational_tids={:#04x} operational_transitions={} samples={} received={} \
         success_without={} nonzero_control={} start_outside={} start_lag_max={} \
         full={} partial={} empty={}",
        aggregate.block_ack_operational_tids,
        aggregate.block_ack_operational_transitions,
        aggregate.block_ack_samples,
        aggregate.block_ack_received,
        aggregate.success_without_block_ack,
        aggregate.nonzero_block_ack_control,
        aggregate.block_ack_start_outside_window,
        aggregate.block_ack_start_lag_max,
        aggregate.full_block_ack,
        aggregate.partial_block_ack,
        aggregate.empty_block_ack,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "OAMPI tx_irq_epochs={} tx_irq_samples={} tx_irq_skew={} \
         tx_irq_service_us={} tx_irq_service_max_us={} tx_flight_samples={} \
         tx_flight_us={} tx_flight_max_us={}",
        aggregate.tx_irq_epochs,
        aggregate.tx_irq_service_samples,
        aggregate.tx_irq_clock_skew_samples,
        aggregate.tx_irq_to_service_micros,
        aggregate.tx_irq_to_service_lifetime_max_micros,
        aggregate.tx_publication_to_irq_samples,
        aggregate.tx_publication_to_irq_micros,
        aggregate.tx_publication_to_irq_lifetime_max_micros,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "OAMPP standby_prepared={} standby_published={} standby_cancelled={}",
        aggregate.standby_prepared, aggregate.standby_published, aggregate.standby_cancelled,
    ));
    yield_now().await;
}

pub(in crate::product_hil) async fn log_open_radio_rx_pipeline_interval(
    earlier: RxPipelineCounterSnapshot,
    rx_irq_posts: u32,
    mac_irq_entries: u32,
    irq_classification: MacIrqClassificationSnapshot,
    irq_auxiliary_status_or: u32,
    irq_unknown_status_or: u32,
    counters: &RxPipelineCounters,
) {
    let pipeline = counters.snapshot().wrapping_delta_since(earlier);
    runtime_log(format_args!(
        "ORXS calls={} frontier={} admitted={} bytes={} discard_empty={} discard_long={} \
         back={} pool={} queue={} deferred_max={} pool_min={} queue_min={} \
         fmax={} amax={} service_us={} service_boot_max_us={}",
        pipeline.service_calls,
        pipeline.completion_frontier_frames,
        pipeline.admitted_frames,
        pipeline.staged_bytes,
        pipeline.stage_empty_discards,
        pipeline.stage_too_long_discards,
        pipeline.backpressured_services,
        pipeline.pool_credit_limited_services,
        pipeline.queue_credit_limited_services,
        pipeline.maximum_deferred_frames,
        pipeline.minimum_backpressured_pool_credits,
        pipeline.minimum_backpressured_queue_credits,
        pipeline.maximum_frontier,
        pipeline.maximum_admitted,
        pipeline.service_micros,
        pipeline.service_lifetime_max_micros,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "ORXB increments={} samples={} last_service={} last_counter={} \
         last_frontier={} last_admitted={} last_pool={} last_queue={} last_service_us={}",
        pipeline.dma_buffer_full_increments,
        pipeline.dma_buffer_full_service_samples,
        pipeline.dma_buffer_full_last_service,
        pipeline.dma_buffer_full_last_counter,
        pipeline.dma_buffer_full_last_frontier,
        pipeline.dma_buffer_full_last_admitted,
        pipeline.dma_buffer_full_last_pool_credits,
        pipeline.dma_buffer_full_last_queue_credits,
        pipeline.dma_buffer_full_last_service_micros,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "ORXD frames={} data={} amsdu={} amsdu_subframes={} unit_le1700={} \
         unit_1701_3400={} unit_over3400={} unit_boot_max_bytes={} \
         waits={} wait_us={} wait_boot_max_us={} dispatch_us={} \
         dispatch_boot_max_us={} publications={} enqueued={} dropped={} bytes={} \
         publish_us={} publish_boot_max_us={}",
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
        pipeline.protocol_amsdu_mpdus,
        pipeline.protocol_amsdu_subframes,
        pipeline.protocol_units_le_1700,
        pipeline.protocol_units_1701_3400,
        pipeline.protocol_units_over_3400,
        pipeline.protocol_unit_lifetime_max_bytes,
        pipeline.network_ready_waits,
        pipeline.network_ready_wait_micros,
        pipeline.network_ready_wait_lifetime_max_micros,
        pipeline.dispatch_micros,
        pipeline.dispatch_lifetime_max_micros,
        pipeline.network_publications,
        pipeline.network_enqueued,
        pipeline.network_dropped,
        pipeline.network_published_bytes,
        pipeline.network_publish_micros,
        pipeline.network_publish_lifetime_max_micros,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "ORXR starts={} stops={} start_tid={} start_seq={} window={} first_samples={} \
         first_tid={} first_start={} first_seq={} first_distance={} buffered={} released={} \
         missing={} stale={} expiries={} occupied={} occupied_max={}",
        pipeline.reorder_starts,
        pipeline.reorder_stops,
        pipeline.reorder_last_start >> 26 & 0x07,
        pipeline.reorder_last_start & 0x0fff,
        pipeline.reorder_last_start >> 16 & 0x03ff,
        pipeline.reorder_first_samples,
        pipeline.reorder_last_first >> 24 & 0x0f,
        pipeline.reorder_last_first >> 12 & 0x0fff,
        pipeline.reorder_last_first & 0x0fff,
        pipeline.reorder_last_first_distance,
        pipeline.reorder_buffered,
        pipeline.reorder_released,
        pipeline.reorder_missing,
        pipeline.reorder_stale,
        pipeline.reorder_gap_expiries,
        pipeline.reorder_current_occupied,
        pipeline.reorder_maximum_occupied,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "ORXF zero={} one={} two_three={} four_seven={} eight_fifteen={} \
         sixteen_thirty_one={} thirty_two_plus={} irq_posts={} irq_epochs={} \
         irq_entries={} irq_coalesced={} irq_samples={} irq_skew={} \
         irq_service_us={} irq_service_boot_max_us={}",
        pipeline.frontier_zero_services,
        pipeline.frontier_one_services,
        pipeline.frontier_two_three_services,
        pipeline.frontier_four_seven_services,
        pipeline.frontier_eight_fifteen_services,
        pipeline.frontier_sixteen_thirty_one_services,
        pipeline.frontier_thirty_two_plus_services,
        rx_irq_posts,
        pipeline.rx_irq_epochs,
        mac_irq_entries,
        rx_irq_posts.saturating_sub(pipeline.rx_irq_epochs),
        pipeline.rx_irq_service_samples,
        pipeline.rx_irq_clock_skew_samples,
        pipeline.rx_irq_to_service_micros,
        pipeline.rx_irq_to_service_lifetime_max_micros,
    ));
    yield_now().await;
    runtime_log(format_args!(
        "ORXI spurious={} rx_only={} rx_mixed={} tx_only={} tx_mixed={} other_only={} \
         extra={} saturated={} aux_or={} unknown_or={}",
        irq_classification.spurious_entries,
        irq_classification.rx_only_entries,
        irq_classification.rx_mixed_entries,
        irq_classification.tx_only_entries,
        irq_classification.tx_mixed_entries,
        irq_classification.other_only_entries,
        irq_classification.extra_nonzero_snapshots,
        irq_classification.saturated_entries,
        irq_auxiliary_status_or,
        irq_unknown_status_or,
    ));
    yield_now().await;
}

pub(in crate::product_hil) async fn log_open_radio_task_poll_interval(
    earlier: TaskPollSetSnapshot,
    enabled: bool,
    counters: &TaskPollSet,
) {
    if !enabled {
        return;
    }
    let current = counters.snapshot();
    log_open_radio_task_poll(
        "network",
        current.network.wrapping_delta_since(earlier.network),
    )
    .await;
    log_open_radio_task_poll(
        "protocol",
        current.protocol.wrapping_delta_since(earlier.protocol),
    )
    .await;
    log_open_radio_task_poll("radio", current.radio.wrapping_delta_since(earlier.radio)).await;
    log_open_radio_task_poll(
        "benchmark",
        current.benchmark.wrapping_delta_since(earlier.benchmark),
    )
    .await;
}

async fn log_open_radio_task_poll(task: &str, poll: TaskPollSnapshot) {
    runtime_log(format_args!(
        "ORTP task={task} polls={} poll_us={} poll_boot_max_us={} \
         over_100us={} over_500us={} over_1000us={} over_5000us={}",
        poll.polls,
        poll.poll_micros,
        poll.lifetime_max_micros,
        poll.over_100_micros,
        poll.over_500_micros,
        poll.over_1_000_micros,
        poll.over_5_000_micros,
    ));
    yield_now().await;
}

/// Observe continuous executor residence without changing the wrapped
/// future's wake or pending semantics. Wall time includes interrupt
/// preemption, which is intentional: a long task poll that blocks sibling
/// Embassy work is harmful regardless of whether its body or an ISR consumed
/// the interval.
pub(in crate::product_hil) async fn observe_open_radio_task_polls<F: Future>(
    future: F,
    counters: &'static TaskPollCounters,
    enabled: bool,
) -> F::Output {
    if !enabled {
        return future.await;
    }
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|context| {
        let started = Instant::now();
        let result = future.as_mut().poll(context);
        counters.record(started.elapsed().as_micros());
        result
    })
    .await
}
