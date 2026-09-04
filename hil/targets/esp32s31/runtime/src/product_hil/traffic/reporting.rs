#![forbid(unsafe_code)]

use core::future::Future;

use embassy_futures::yield_now;
use embassy_time::Instant;
#[cfg(feature = "tx-architecture-probes")]
use open_esp_radio_embassy_net::{TX_CORE1_MATERIALIZER_COUNTERS, TxCore1MaterializerSnapshot};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use open_esp_radio_embassy_net::{
    EGRESS_GRANT_TIMELINE, EgressControlSnapshot, EgressGrantTimelinePhaseSnapshot,
    EgressGrantTimelineSnapshot, TX_PERFORMANCE, TxPerformanceSnapshot,
};
#[cfg(feature = "core0-rx-cycle-telemetry")]
use open_esp_radio_esp32s31_embassy_wifi::{
    CORE0_AP_RX_CYCLES, CORE0_REORDER_CYCLES, CORE0_RX_CYCLES, CORE0_RX_SERVICE_HISTOGRAM,
    Core0ApRxCycleSnapshot, Core0ReorderSnapshot, Core0RxCycleSnapshot,
    Core0RxServiceHistogramSnapshot,
};
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use open_esp_radio_esp32s31_embassy_wifi::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0PerformanceSnapshot,
    EgressPolicyShadowSnapshot,
};
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::{AggregateTxCounterSnapshot, AggregateTxCounters},
    mac_irq::MacIrqClassificationSnapshot,
    rx_pipeline::{RxPipelineCounterSnapshot, RxPipelineCounters},
    task_poll::{TaskPollCounters, TaskPollSet, TaskPollSetSnapshot, TaskPollSnapshot},
};
use open_esp_radio_hil_protocol::{RadioEvidence, TxAggregateTimingEvidence, TxRadioEvidence};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use open_esp_radio_hil_protocol::{WifiEgressPolicyEvidence, WifiEgressVifEvidence};

use crate::console::runtime_log_reliably;

#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use super::cache_performance::L1CachePerformanceSnapshot;

pub(in crate::product_hil) fn aggregate_tx_evidence(
    aggregate: AggregateTxCounterSnapshot,
) -> (RadioEvidence, TxAggregateTimingEvidence) {
    let radio = RadioEvidence {
        rx: None,
        tx: Some(TxRadioEvidence {
            bandwidth_mhz: u16::try_from(aggregate.last_bandwidth_mhz).unwrap_or(u16::MAX),
            aggregate_rate_kbps: aggregate.last_nominal_rate_kbps,
            aggregates_prepared: aggregate.aggregates_prepared,
            aggregate_publications: aggregate.aggregate_publications,
            aggregates_completed: aggregate.aggregates_completed,
            subframes_prepared: aggregate.prepared_subframe_total(),
            subframes_acknowledged: aggregate.subframes_acknowledged,
            individual_retries: aggregate.individual_retries,
            hardware_timeouts: aggregate.hardware_timeouts,
            collisions: aggregate.collisions,
            minimum_subframes: aggregate.minimum_prepared_subframes().unwrap_or(0),
            maximum_subframes: aggregate.maximum_prepared_subframes().unwrap_or(0),
            prepared_histogram: [
                aggregate.prepared_in_range(1, 1),
                aggregate.prepared_in_range(2, 3),
                aggregate.prepared_in_range(4, 7),
                aggregate.prepared_in_range(8, 15),
                aggregate.prepared_in_range(16, 23),
                aggregate.prepared_in_range(24, 30),
                aggregate.prepared_in_range(31, 31),
                aggregate.prepared_in_range(32, 32),
            ],
            stopped_at_frame_limit: aggregate.stopped_at_frame_limit,
            stopped_at_capacity_limit: aggregate.stopped_at_capacity_limit,
            stopped_on_empty_queue: aggregate.stopped_on_empty_queue,
            block_ack_samples: aggregate.block_ack_samples,
            block_ack_received: aggregate.block_ack_received,
            success_without_block_ack: aggregate.success_without_block_ack,
            nonzero_block_ack_control: aggregate.nonzero_block_ack_control,
            full_block_ack: aggregate.full_block_ack,
            partial_block_ack: aggregate.partial_block_ack,
            empty_block_ack: aggregate.empty_block_ack,
            tx_irq_epochs: aggregate.tx_irq_epochs,
            tx_irq_service_samples: aggregate.tx_irq_service_samples,
            tx_irq_clock_skew_samples: aggregate.tx_irq_clock_skew_samples,
            tx_publication_to_irq_samples: aggregate.tx_publication_to_irq_samples,
        }),
    };
    let timing = TxAggregateTimingEvidence {
        preparation_micros: aggregate.preparation_micros,
        preparation_max_micros: aggregate.preparation_lifetime_max_micros,
        publication_micros: aggregate.publication_program_micros,
        publication_max_micros: aggregate.publication_program_lifetime_max_micros,
        exchange_micros: aggregate.exchange_micros,
        exchange_max_micros: aggregate.exchange_lifetime_max_micros,
        first_exchanges: aggregate.single_publication_exchanges,
        first_exchange_micros: aggregate.single_publication_exchange_micros,
        first_exchange_max_micros: aggregate.single_publication_exchange_lifetime_max_micros,
        retried_exchanges: aggregate.retried_exchanges,
        retry_publications: aggregate.retried_exchange_publications,
        retry_exchange_micros: aggregate.retried_exchange_micros,
        retry_exchange_max_micros: aggregate.retried_exchange_lifetime_max_micros,
        tx_irq_epochs: aggregate.tx_irq_epochs,
        tx_irq_service_samples: aggregate.tx_irq_service_samples,
        tx_irq_clock_skew_samples: aggregate.tx_irq_clock_skew_samples,
        tx_irq_service_micros: aggregate.tx_irq_to_service_micros,
        tx_irq_service_max_micros: aggregate.tx_irq_to_service_lifetime_max_micros,
        tx_publication_to_irq_samples: aggregate.tx_publication_to_irq_samples,
        tx_publication_to_irq_micros: aggregate.tx_publication_to_irq_micros,
        tx_publication_to_irq_max_micros: aggregate.tx_publication_to_irq_lifetime_max_micros,
        standby_prepared: aggregate.standby_prepared,
        standby_published: aggregate.standby_published,
        standby_cancelled: aggregate.standby_cancelled,
    };
    (radio, timing)
}

pub(in crate::product_hil) async fn log_open_radio_ampdu_interval(
    earlier: AggregateTxCounterSnapshot,
    counters: &AggregateTxCounters,
) {
    let aggregate = counters.snapshot().wrapping_delta_since(earlier);
    let aggregate_min = aggregate.minimum_prepared_subframes().unwrap_or(0);
    let aggregate_max = aggregate.maximum_prepared_subframes().unwrap_or(0);
    runtime_log_reliably(format_args!(
        "OAMP width_mhz={} rate_kbps={} aggregates={} publications={} completed={} subframes={} \
         acknowledged={} single={} single_rate={} single_ba={} single_pair={} \
         single_capacity={} single_capacity_max_len={} individual_retry={} timeout={} collision={} \
         min={} max={} stop_frame={} stop_capacity={} stop_empty={}",
        aggregate.last_bandwidth_mhz,
        aggregate.last_nominal_rate_kbps,
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPT preparation_us={} preparation_max_us={} publication_us={} \
         publication_max_us={} exchange_us={} exchange_max_us={} \
         first_exchanges={} first_exchange_us={} first_exchange_max_us={} \
         retried_exchanges={} retry_publications={} retry_exchange_us={} retry_exchange_max_us={}",
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPTD completion_to_publication={}/{} completion_to_publication_max_us={} \
         completion_core_us={} completion_core_max_us={} release_us={} release_max_us={}",
        aggregate.completion_to_publication_samples,
        aggregate.completion_to_publication_micros,
        aggregate.completion_to_publication_lifetime_max_micros,
        aggregate.completion_core_micros,
        aggregate.completion_core_lifetime_max_micros,
        aggregate.backing_release_micros,
        aggregate.backing_release_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPTR r2={}/{} r3={}/{} r4={}/{}",
        aggregate.exchanges_by_publications[2],
        aggregate.exchange_lifetime_max_micros_by_publications[2],
        aggregate.exchanges_by_publications[3],
        aggregate.exchange_lifetime_max_micros_by_publications[3],
        aggregate.exchanges_by_publications[4],
        aggregate.exchange_lifetime_max_micros_by_publications[4],
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPS completion_to_entry={}/{} completion_to_entry_max_us={} \
         entry_to_publication={}/{} entry_to_publication_max_us={}",
        aggregate.completion_to_prepared_entry_samples,
        aggregate.completion_to_prepared_entry_micros,
        aggregate.completion_to_prepared_entry_lifetime_max_micros,
        aggregate.prepared_entry_to_publication_samples,
        aggregate.prepared_entry_to_publication_micros,
        aggregate.prepared_entry_to_publication_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    let scheduler = aggregate.prepared_scheduler_timing;
    runtime_log_reliably(format_args!(
        "OAMPSP samples={} passes={} passes_max={} control_ready_passes={} \
         completion_to_return_us={} completion_to_return_max_us={} \
         return_to_loop_us={} return_to_loop_max_us={} \
         stop_poll_us={} stop_poll_max_us={} readiness_us={} readiness_max_us={}",
        scheduler.samples,
        scheduler.scheduler_passes,
        scheduler.scheduler_passes_lifetime_max,
        scheduler.control_ready_passes,
        scheduler.completion_to_active_service_return.micros,
        scheduler
            .completion_to_active_service_return
            .lifetime_max_micros,
        scheduler.active_service_return_to_scheduler_loop.micros,
        scheduler
            .active_service_return_to_scheduler_loop
            .lifetime_max_micros,
        scheduler.stop_poll.micros,
        scheduler.stop_poll.lifetime_max_micros,
        scheduler.control_readiness.micros,
        scheduler.control_readiness.lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPSR prepared_readiness_us={} prepared_readiness_max_us={} \
         batch_policy_us={} batch_policy_max_us={} batch_to_entry_us={} batch_to_entry_max_us={} \
         readiness_to_entry_us={} readiness_to_entry_max_us={}",
        scheduler.control_check_to_prepared_readiness.micros,
        scheduler
            .control_check_to_prepared_readiness
            .lifetime_max_micros,
        scheduler.prepared_readiness_to_batch.micros,
        scheduler.prepared_readiness_to_batch.lifetime_max_micros,
        scheduler.prepared_batch_to_entry.micros,
        scheduler.prepared_batch_to_entry.lifetime_max_micros,
        scheduler.control_check_to_prepared_entry.micros,
        scheduler
            .control_check_to_prepared_entry
            .lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPP standby_prepared={} standby_published={} standby_cancelled={}",
        aggregate.standby_prepared, aggregate.standby_published, aggregate.standby_cancelled,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OAMPA ap_udp_claimed={} backward={} first={} after={} maximum_distance={}",
        aggregate.ap_udp_claimed,
        aggregate.ap_udp_claim_backward,
        aggregate.ap_udp_claim_first_sequence,
        aggregate.ap_udp_claim_first_previous,
        aggregate.ap_udp_claim_maximum_distance,
    ))
    .await;
    yield_now().await;
}

pub(in crate::product_hil) async fn log_open_radio_rx_pipeline_interval(
    earlier: RxPipelineCounterSnapshot,
    rx_irq_posts: u32,
    mac_irq_entries: u32,
    irq_classification: MacIrqClassificationSnapshot,
    irq_auxiliary_entries: u32,
    irq_unhandled_entries: u32,
    counters: &RxPipelineCounters,
) {
    let pipeline = counters.snapshot().wrapping_delta_since(earlier);
    runtime_log_reliably(format_args!(
        "ORXS calls={} frontier={} completed={} descriptors={} admitted={} staged={} bytes={} \
         discarded={} discard_empty={} discard_long={} discard_chained={} discard_overload={} \
         overload_discarded={} recycled={} overload_recycled={} \
         fmax={} amax={} service_us={} service_boot_max_us={}",
        pipeline.service_calls,
        pipeline.completion_frontier_frames,
        pipeline.completed_units,
        pipeline.completed_descriptors,
        pipeline.admitted_frames,
        pipeline.staged_units,
        pipeline.staged_bytes,
        pipeline.discarded_units,
        pipeline.stage_empty_discards,
        pipeline.stage_too_long_discards,
        pipeline.stage_chained_discards,
        pipeline.stage_overload_bulk_discards,
        pipeline.overload_discarded_units,
        pipeline.recycled_descriptors,
        pipeline.overload_recycled_descriptors,
        pipeline.maximum_frontier,
        pipeline.maximum_admitted,
        pipeline.service_micros,
        pipeline.service_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXSC samples={} back={} bulk_blocked={} pool={} queue={} deferred_max={} \
         pool_min={} queue_min={} pool_floor={} queue_floor={}",
        pipeline.service_calls,
        pipeline.backpressured_services,
        pipeline.bulk_capacity_blocked_services,
        pipeline.pool_credit_limited_services,
        pipeline.queue_credit_limited_services,
        pipeline.maximum_deferred_frames,
        pipeline.minimum_backpressured_pool_credits,
        pipeline.minimum_backpressured_queue_credits,
        pipeline.minimum_pool_credits,
        pipeline.minimum_queue_credits,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXL transactions={} reload_us={} reload_boot_max_us={}",
        pipeline.reload_transactions, pipeline.reload_micros, pipeline.reload_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXB increments={} samples={} between={} during={} between_samples={} \
         during_samples={} last_service={} last_phase={} last_counter={} \
         last_frontier={} last_admitted={} last_pool={} last_queue={} last_service_us={}",
        pipeline.dma_buffer_full_increments,
        pipeline.dma_buffer_full_service_samples,
        pipeline.dma_buffer_full_between_services,
        pipeline.dma_buffer_full_during_services,
        pipeline.dma_buffer_full_between_service_samples,
        pipeline.dma_buffer_full_during_service_samples,
        pipeline.dma_buffer_full_last_service,
        pipeline.dma_buffer_full_last_phase,
        pipeline.dma_buffer_full_last_counter,
        pipeline.dma_buffer_full_last_frontier,
        pipeline.dma_buffer_full_last_admitted,
        pipeline.dma_buffer_full_last_pool_credits,
        pipeline.dma_buffer_full_last_queue_credits,
        pipeline.dma_buffer_full_last_service_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXP frames={} frame_us={} frame_boot_max_us={} reorder={} reorder_us={} \
         reorder_boot_max_us={} preflight={} preflight_us={} preflight_boot_max_us={}",
        pipeline.protocol_frame_transactions,
        pipeline.protocol_frame_micros,
        pipeline.protocol_frame_lifetime_max_micros,
        pipeline.reorder_preflights,
        pipeline.reorder_preflight_micros,
        pipeline.reorder_preflight_lifetime_max_micros,
        pipeline.protocol_preflights,
        pipeline.protocol_preflight_micros,
        pipeline.protocol_preflight_lifetime_max_micros,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
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
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORXI spurious={} rx_only={} rx_mixed={} tx_only={} tx_mixed={} other_only={} \
         extra={} saturated={} aux_entries={} unhandled_entries={}",
        irq_classification.spurious_entries,
        irq_classification.rx_only_entries,
        irq_classification.rx_mixed_entries,
        irq_classification.tx_only_entries,
        irq_classification.tx_mixed_entries,
        irq_classification.other_only_entries,
        irq_classification.extra_nonzero_snapshots,
        irq_classification.saturated_entries,
        irq_auxiliary_entries,
        irq_unhandled_entries,
    ))
    .await;
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
    log_open_radio_task_poll("radio", current.radio.wrapping_delta_since(earlier.radio)).await;
    log_open_radio_task_poll(
        "udp_rx",
        current.udp_rx.wrapping_delta_since(earlier.udp_rx),
    )
    .await;
    log_open_radio_task_poll(
        "udp_tx",
        current.udp_tx.wrapping_delta_since(earlier.udp_tx),
    )
    .await;
    log_open_radio_task_poll("tcp", current.tcp.wrapping_delta_since(earlier.tcp)).await;
}

#[cfg(feature = "core0-rx-cycle-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core0_rx_cycles(
    earlier: Core0RxCycleSnapshot,
    ap_earlier: Core0ApRxCycleSnapshot,
    performance_earlier: Core0PerformanceSnapshot,
    reorder_earlier: Core0ReorderSnapshot,
    cache: L1CachePerformanceSnapshot,
) {
    let cycles = CORE0_RX_CYCLES.snapshot().wrapping_delta_since(earlier);
    let ap = CORE0_AP_RX_CYCLES
        .snapshot()
        .wrapping_delta_since(ap_earlier);
    let performance = CORE0_PERFORMANCE
        .snapshot()
        .wrapping_delta_since(performance_earlier);
    let reorder = CORE0_REORDER_CYCLES
        .snapshot()
        .wrapping_delta_since(reorder_earlier);
    runtime_log_reliably(format_args!(
        "ORC0A calls={} total={} view={} dispatch={} dispatch_leaf={} reorder_key={} leaf_peer={} leaf_publish_check={} leaf_body={} leaf_admission={} leaf_observe={} publication={} activity_tail={} telemetry={}",
        ap.calls,
        ap.total,
        ap.view,
        ap.dispatch,
        ap.dispatch_leaf,
        ap.reorder_key,
        ap.leaf_peer,
        ap.leaf_publish_check,
        ap.leaf_body,
        ap.leaf_admission,
        ap.leaf_observe,
        ap.publication,
        ap.activity_tail,
        ap.telemetry,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0AP in_place_eligible={} in_place_published={} deferred_published={} reorder_buffered={} turn_calls={} turn_frames={} initial_batch={} initial_reorder={} mailbox_blocked={} tx_blocked={} batch_pending={} reorder_pending={} drained={} budget={}",
        ap.in_place_eligible,
        ap.in_place_published,
        ap.deferred_published,
        ap.reorder_buffered,
        ap.turn_calls,
        ap.turn_frames,
        ap.turn_initial_batch,
        ap.turn_initial_reorder,
        ap.turn_mailbox_blocked,
        ap.turn_tx_blocked,
        ap.turn_batch_pending,
        ap.turn_reorder_pending,
        ap.turn_drained,
        ap.turn_budget_exhausted,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0 services={} units={} total={} setup={} frontier={} admission={} \
         stage_total={} stage_take={} stage_pool={} recycle={} reload={} publish={} tail={}",
        cycles.services,
        cycles.units,
        cycles.total,
        cycles.setup,
        cycles.frontier,
        cycles.admission,
        cycles.stage_total,
        cycles.stage_take,
        cycles.stage_pool,
        cycles.recycle,
        cycles.reload,
        cycles.publish,
        cycles.tail,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0R runner_calls={} runner_total={} runner_pre={} runner_driver={} runner_post={} \
         mac_irq_entries={} mac_irq_cycles={} control_calls={} control_cycles={} \
         control_idle={} control_more={} control_tx={}",
        cycles.runner_rx_calls,
        cycles.runner_rx_total,
        cycles.runner_rx_pre,
        cycles.runner_rx_driver,
        cycles.runner_rx_post,
        cycles.mac_irq_entries,
        cycles.mac_irq_cycles,
        cycles.control_calls,
        cycles.control_cycles,
        cycles.control_idle,
        cycles.control_more,
        cycles.control_tx_pending,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0P polls={} poll_cycles={} polls_with_rx={} poll_to_runner={} runner_to_exit={}",
        cycles.radio_polls,
        cycles.radio_poll_cycles,
        cycles.radio_polls_with_rx,
        cycles.poll_to_runner_cycles,
        cycles.runner_to_poll_exit_cycles,
    ))
    .await;
    yield_now().await;
    // Accepted-list exhaustion distinguishes a physical descriptor-list stop
    // from the similarly named MAC RX statistics counter. Keep this line
    // ahead of the optional fine-grained phase dump so diagnostic UART loss
    // cannot remove the primary BUFFER_FULL correlation.
    runtime_log_reliably(format_args!(
        "ORC0R exhausted={} remaining1_8={} remaining9_16={} remaining17_32={} remaining33_48={} remaining49_plus={} unknown={} episodes={} resolved_le64us={} resolved_le256us={} resolved_le1024us={} resolved_gt1024us={}",
        performance.dma_entry_remaining_exhausted,
        performance.dma_entry_remaining_1_8,
        performance.dma_entry_remaining_9_16,
        performance.dma_entry_remaining_17_32,
        performance.dma_entry_remaining_33_48,
        performance.dma_entry_remaining_49_plus,
        performance.dma_entry_remaining_unknown,
        performance.dma_exhaustion_episodes,
        performance.dma_exhaustion_resolved_le_64us,
        performance.dma_exhaustion_resolved_le_256us,
        performance.dma_exhaustion_resolved_le_1024us,
        performance.dma_exhaustion_resolved_gt_1024us,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0S calls={} software={} irq={} select={} reentry={} stop={} housekeeping={} tx_checks={} rx_checks={}",
        cycles.scheduler_rx_calls,
        cycles.scheduler_software_rx_calls,
        cycles.scheduler_irq_rx_calls,
        cycles.scheduler_select_rx_calls,
        cycles.scheduler_reentry_cycles,
        cycles.scheduler_stop_cycles,
        cycles.scheduler_housekeeping_cycles,
        cycles.scheduler_tx_checks_cycles,
        cycles.scheduler_rx_checks_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0H discard={} queue={} control_ready={} prepared={} pending={} idle={}",
        cycles.scheduler_discard_wakes_cycles,
        cycles.scheduler_first_network_queue_cycles,
        cycles.scheduler_control_ready_cycles,
        cycles.scheduler_prepared_cycles,
        cycles.scheduler_network_pending_cycles,
        cycles.scheduler_idle_accounting_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0T polls={} poll_cycles={} polls_with_frame={} poll_to_frame={} frame_to_exit={}",
        cycles.protocol_polls,
        cycles.protocol_poll_cycles,
        cycles.protocol_polls_with_frame,
        cycles.protocol_poll_to_first_frame_cycles,
        cycles.protocol_last_frame_to_poll_exit_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0Q dequeues={} poll_to_dequeue={} between_to_dequeue={} dequeue_to_frame={}",
        cycles.protocol_dequeues,
        cycles.protocol_poll_to_first_dequeue_cycles,
        cycles.protocol_between_frame_to_dequeue_cycles,
        cycles.protocol_dequeue_to_frame_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0D calls={} ordinary={} scratch={} total={} preflight={} wait={} dispatch={} publish_tail={}",
        cycles.protocol_frame_calls,
        cycles.protocol_frame_ordinary,
        cycles.protocol_frame_scratch,
        cycles.protocol_frame_total,
        cycles.protocol_frame_preflight,
        cycles.protocol_frame_wait,
        cycles.protocol_frame_dispatch,
        cycles.protocol_frame_publish_tail,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0E dispatch_pre={} capture={} dispatch_post={} observer={} in_place={} shared={} protected_calls={} protected_cycles={}",
        cycles.protocol_dispatch_pre_publish,
        cycles.protocol_dispatch_capture,
        cycles.protocol_dispatch_post_publish,
        cycles.protocol_publication_observer,
        cycles.protocol_publication_in_place,
        cycles.protocol_publication_shared,
        cycles.protocol_protected_view_calls,
        cycles.protocol_protected_view_cycles,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0F calls={} completed={} total={} view={} fragment_guard={} decapsulate={} replay={} duplicate={} publish={}",
        cycles.data_calls,
        cycles.data_completed,
        cycles.data_total,
        cycles.data_view,
        cycles.data_fragment_guard,
        cycles.data_decapsulate,
        cycles.data_replay,
        cycles.data_duplicate,
        cycles.data_publish,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0M calls={} total={} admission={} layout={} projection={} validate={} telemetry={}",
        cycles.ccmp_view_calls,
        cycles.ccmp_view_total,
        cycles.ccmp_view_admission,
        cycles.ccmp_view_layout,
        cycles.ccmp_view_projection,
        cycles.ccmp_view_validate,
        cycles.ccmp_view_telemetry,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0I dma={} runner={} scheduler={} protocol_dequeue={} protocol_entry={} protocol_frame={} data={} publication={}",
        cycles.telemetry_dma_record,
        cycles.telemetry_runner_record,
        cycles.telemetry_scheduler_record,
        cycles.telemetry_protocol_dequeue_record,
        cycles.telemetry_protocol_entry_record,
        cycles.telemetry_protocol_frame_record,
        cycles.telemetry_data_record,
        cycles.telemetry_publication_record,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0J radio_polls={} radio_cycles={} radio_instret={} poll_to_runner_instret={} runner_to_exit_instret={} runner_calls={} runner_cycles={} runner_instret={} protocol_polls={} protocol_cycles={} protocol_instret={}",
        performance.radio_polls,
        performance.radio_cycles,
        performance.radio_instructions,
        performance.poll_to_runner_instructions,
        performance.runner_to_poll_exit_instructions,
        performance.runner_calls,
        performance.runner_cycles,
        performance.runner_instructions,
        performance.protocol_polls,
        performance.protocol_cycles,
        performance.protocol_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0K dma_calls={} dma_units={} dma_cycles={} dma_instret={} protocol_frames={} protocol_frame_cycles={} protocol_frame_instret={}",
        performance.dma_calls,
        performance.dma_units,
        performance.dma_cycles,
        performance.dma_instructions,
        performance.protocol_frames,
        performance.protocol_frame_cycles,
        performance.protocol_frame_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0G calls={} no_key={} inactive={} immediate={} slow={} total={} key={} bank={} ingress_observer={} first={} ingest={} deadline={} release_observer={} occupied_observer={} prepared_observer={} tail={} telemetry={}",
        reorder.calls,
        reorder.no_key,
        reorder.inactive,
        reorder.immediate,
        reorder.slow,
        reorder.total,
        reorder.key,
        reorder.bank,
        reorder.ingress_observer,
        reorder.first,
        reorder.ingest,
        reorder.deadline,
        reorder.release_observer,
        reorder.occupied_observer,
        reorder.prepared_observer,
        reorder.tail,
        reorder.telemetry_record,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0L calls={} accepted={} preflight_rejected={} key_rejected={} bank_rejected={} reorder_rejected={} duplicate_or_ignored={} total={} preflight={} key={} bank={} ingest={} deadline={} dispatch={} tail={} telemetry={}",
        reorder.direct_calls,
        reorder.direct_accepted,
        reorder.direct_preflight_rejected,
        reorder.direct_key_rejected,
        reorder.direct_bank_rejected,
        reorder.direct_reorder_rejected,
        reorder.direct_duplicate_or_ignored,
        reorder.direct_total,
        reorder.direct_preflight,
        reorder.direct_key,
        reorder.direct_bank,
        reorder.direct_ingest,
        reorder.direct_deadline,
        reorder.direct_dispatch,
        reorder.direct_tail,
        reorder.direct_telemetry_record,
    ))
    .await;
    yield_now().await;
    log_open_radio_l1_cache_interval(cache).await;
}

#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
pub(in crate::product_hil) async fn log_open_radio_l1_cache_interval(
    cache: L1CachePerformanceSnapshot,
) {
    runtime_log_reliably(format_args!(
        "OCACHEI trace={} bus0_enabled={} bus1_enabled={} bus0_hit={} bus0_miss={} bus0_conflict={} bus0_next={} bus1_hit={} bus1_miss={} bus1_conflict={} bus1_next={}",
        u8::from(cache.trace_enabled),
        u8::from(cache.counter_enable.ibus0),
        u8::from(cache.counter_enable.ibus1),
        cache.ibus0.hit,
        cache.ibus0.miss,
        cache.ibus0.conflict,
        cache.ibus0.next_level_read,
        cache.ibus1.hit,
        cache.ibus1.miss,
        cache.ibus1.conflict,
        cache.ibus1.next_level_read,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "OCACHED bus0_enabled={} bus1_enabled={} bus0_hit={} bus0_miss={} bus0_conflict={} bus0_next_read={} bus0_next_write={} bus1_hit={} bus1_miss={} bus1_conflict={} bus1_next_read={} bus1_next_write={}",
        u8::from(cache.counter_enable.dbus0),
        u8::from(cache.counter_enable.dbus1),
        cache.dbus0.hit,
        cache.dbus0.miss,
        cache.dbus0.conflict,
        cache.dbus0.next_level_read,
        cache.dbus0.next_level_write,
        cache.dbus1.hit,
        cache.dbus1.miss,
        cache.dbus1.conflict,
        cache.dbus1.next_level_read,
        cache.dbus1.next_level_write,
    ))
    .await;
    yield_now().await;
}

/// Emit only batch-level Core0 cost evidence.
///
/// This is intentionally distinct from `log_open_radio_core0_rx_cycles`: the
/// coarse image never links its per-frame phase counters or driver observers.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core0_rx_coarse(
    earlier: Core0PerformanceSnapshot,
) {
    let performance = CORE0_PERFORMANCE.snapshot().wrapping_delta_since(earlier);
    runtime_log_reliably(format_args!(
        "ORC0C rx_irq_posts={} radio_polls={} radio_cycles={} radio_instret={} poll_to_runner_cycles={} poll_to_runner_instret={} runner_to_exit_cycles={} runner_to_exit_instret={} runner_calls={} runner_cycles={} runner_instret={}",
        performance.rx_interrupt_posts,
        performance.radio_polls,
        performance.radio_cycles,
        performance.radio_instructions,
        performance.poll_to_runner_cycles,
        performance.poll_to_runner_instructions,
        performance.runner_to_poll_exit_cycles,
        performance.runner_to_poll_exit_instructions,
        performance.runner_calls,
        performance.runner_cycles,
        performance.runner_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0B protocol_polls={} protocol_cycles={} protocol_instret={} direct_frames={} async_frames={} dma_calls={} dma_empty={} dma_units={} dma_cycles={} dma_instret={}",
        performance.protocol_polls,
        performance.protocol_cycles,
        performance.protocol_instructions,
        performance.direct_protocol_frames,
        performance.asynchronous_protocol_frames,
        performance.dma_calls,
        performance.dma_empty_calls,
        performance.dma_units,
        performance.dma_cycles,
        performance.dma_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TX start_calls={} start_cycles={} start_instret={} prepare_calls={} prepare_cycles={} prepare_instret={} publish_calls={} publish_cycles={} publish_instret={} service_calls={} service_cycles={} service_instret={}",
        performance.tx_start_calls,
        performance.tx_start_cycles,
        performance.tx_start_instructions,
        performance.tx_prepare_calls,
        performance.tx_prepare_cycles,
        performance.tx_prepare_instructions,
        performance.tx_publish_calls,
        performance.tx_publish_cycles,
        performance.tx_publish_instructions,
        performance.tx_service_calls,
        performance.tx_service_cycles,
        performance.tx_service_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXN encode_calls={} encode_cycles={} encode_instret={} commit_calls={} commit_cycles={} commit_instret={}",
        performance.tx_encode_calls,
        performance.tx_encode_cycles,
        performance.tx_encode_instructions,
        performance.tx_commit_calls,
        performance.tx_commit_cycles,
        performance.tx_commit_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXG samples={} cycles={} instret={} le64={} le256={} le512={} le1024={} gt1024={} completions={} prepared={} full={} partial={} frames={} queued={} empty={}",
        performance.tx_prepared_gap_samples,
        performance.tx_prepared_gap_cycles,
        performance.tx_prepared_gap_instructions,
        performance.tx_prepared_gap_le_64us,
        performance.tx_prepared_gap_le_256us,
        performance.tx_prepared_gap_le_512us,
        performance.tx_prepared_gap_le_1024us,
        performance.tx_prepared_gap_gt_1024us,
        performance.tx_network_completions,
        performance.tx_completion_prepared,
        performance.tx_completion_prepared_full,
        performance.tx_completion_prepared_partial,
        performance.tx_completion_prepared_frames,
        performance.tx_completion_queued,
        performance.tx_completion_empty,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXF initial_frames={} partial_samples={} matching_retained={} other_retained={} network_ready={} mismatch_claims={}",
        performance.tx_initial_network_frames,
        performance.tx_ap_partial_frontiers,
        performance.tx_ap_partial_matching_retained,
        performance.tx_ap_partial_other_retained,
        performance.tx_ap_partial_network_ready,
        performance.tx_ap_partial_mismatch_claims,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXO ownership_samples={} admitted={} pool_free={} ready_same={} ready_other={} ingress_reserved={} application_reserved={} tokens_in_flight={} radio_owned={} unattributed_radio_owned={}",
        performance.tx_ap_partial_publications,
        performance.tx_ap_publication_admitted,
        performance.tx_ap_publication_pool_free,
        performance.tx_ap_publication_ready_same,
        performance.tx_ap_publication_ready_other,
        performance.tx_ap_publication_ingress_reserved,
        performance.tx_ap_publication_application_reserved,
        performance.tx_ap_publication_tokens_in_flight,
        performance.tx_ap_publication_radio_owned,
        performance.tx_ap_publication_unattributed_radio_owned,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXI exact={} unclassified={} non_associated={} role_unbound={} interface_mismatch={} peer_slot_mismatch={} peer_generation_mismatch={} traffic_class_mismatch={} terminal_current_aggregates={} terminal_current_frames={} terminal_stale_aggregates={} terminal_stale_frames={}",
        performance.tx_ap_identity_exact,
        performance.tx_ap_identity_unclassified,
        performance.tx_ap_identity_non_associated,
        performance.tx_ap_identity_role_unbound,
        performance.tx_ap_identity_interface_mismatch,
        performance.tx_ap_identity_peer_slot_mismatch,
        performance.tx_ap_identity_peer_generation_mismatch,
        performance.tx_ap_identity_traffic_class_mismatch,
        performance.tx_ap_terminal_identity_current_aggregates,
        performance.tx_ap_terminal_identity_current_frames,
        performance.tx_ap_terminal_identity_stale_aggregates,
        performance.tx_ap_terminal_identity_stale_frames,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0TXA modeled_aggregates={} identity_bound={} terminal_mismatch={} publications={} modeled_hundred_ns={} hardware_measurement=unavailable",
        performance.tx_ap_airtime_aggregates,
        performance.tx_ap_airtime_identity_bound,
        performance.tx_ap_airtime_terminal_mismatch,
        performance.tx_ap_airtime_publications,
        performance.tx_ap_airtime_modeled_hundred_ns,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0P drained={} probe={} protocol_tx_blocked={} recycled_append={} budget={} stage_blocked={} network_blocked={} droppable={}",
        performance.rx_progress_drained,
        performance.rx_progress_probe_pending,
        performance.rx_progress_protocol_tx_blocked,
        performance.rx_progress_recycled_append_pending,
        performance.rx_progress_budget_exhausted,
        performance.rx_progress_stage_blocked,
        performance.rx_progress_network_blocked,
        performance.rx_progress_droppable,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0Q empty={} one={} two={} three_seven={} eight_plus={} probe_recycled={} probe_frontier={} probe_terminal={} probe_republication={}",
        performance.dma_empty_calls,
        performance.dma_single_unit_calls,
        performance.dma_two_unit_calls,
        performance.dma_three_to_seven_unit_calls,
        performance.dma_eight_plus_unit_calls,
        performance.dma_probe_recycled,
        performance.dma_probe_completed_frontier,
        performance.dma_probe_terminal_writeback,
        performance.dma_probe_republication,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ORC0A delay64={} delay128={} delay256={} delay512={} delay_other={} empty_work={} work_units={} staged_bytes={}",
        performance.adaptive_probe_delay_64,
        performance.adaptive_probe_delay_128,
        performance.adaptive_probe_delay_256,
        performance.adaptive_probe_delay_512,
        performance.adaptive_probe_delay_other,
        performance.adaptive_probe_empty_work,
        performance.adaptive_probe_work_units,
        performance.adaptive_probe_staged_bytes,
    ))
    .await;
    yield_now().await;
}

/// Emit Core1 packet-emission and driver-publication phase costs for TX.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core1_tx_phases(
    earlier: TxPerformanceSnapshot,
    earlier_control: (EgressControlSnapshot, EgressControlSnapshot),
    earlier_policy: EgressPolicyShadowSnapshot,
    earlier_timeline: EgressGrantTimelineSnapshot,
) -> WifiEgressPolicyEvidence {
    let performance = TX_PERFORMANCE.snapshot().wrapping_delta_since(earlier);
    runtime_log_reliably(format_args!(
        "ONTX admission_attempts={} admission_successes={} admission_cycles={} admission_instret={} consume_calls={} consume_bytes={} consume_cycles={} consume_instret={} emit_cycles={} emit_instret={} publication_cycles={} publication_instret={}",
        performance.admission_attempts,
        performance.admission_successes,
        performance.admission_cycles,
        performance.admission_instructions,
        performance.consume_calls,
        performance.consume_bytes,
        performance.consume_cycles,
        performance.consume_instructions,
        performance.emit_cycles,
        performance.emit_instructions,
        performance.publication_cycles(),
        performance.publication_instructions(),
    ))
    .await;
    yield_now().await;
    log_open_radio_egress_control_interval(earlier_control).await;
    let policy = open_esp_radio_esp32s31_embassy_wifi::egress_policy_shadow_snapshot()
        .wrapping_delta_since(earlier_policy);
    runtime_log_reliably(format_args!(
        "ONTXES grants_issued={} grants_finished={} grants_used={} grants_unused={} progress_without_grant={} rejected_updates={} rejected_progress={} snapshot_queries={} snapshot_ready={} key_rejected={} identity_rejected={} traffic_class_rejected={} role_unavailable={} non_ht_rate={} no_block_ack={} invalid_geometry={}",
        policy.grants_issued,
        policy.grants_finished,
        policy.grants_used,
        policy.grants_unused,
        policy.progress_without_grant,
        policy.rejected_updates,
        policy.rejected_progress,
        policy.snapshot_queries,
        policy.snapshot_ready,
        policy.key_rejected,
        policy.identity_rejected,
        policy.traffic_class_rejected,
        policy.role_unavailable,
        policy.non_ht_rate,
        policy.no_block_ack,
        policy.invalid_geometry,
    ))
    .await;
    yield_now().await;
    for (vif, vif_policy) in ["sta", "ap"].into_iter().zip(policy.vifs) {
        runtime_log_reliably(format_args!(
            "ONTXESV vif={} grants_issued={} issued_frame_credits={} issued_airtime_100ns={} grants_finished={} used_frames={} used_airtime_100ns={} grants_unused={}",
            vif,
            vif_policy.grants_issued,
            vif_policy.issued_frame_credits,
            vif_policy.issued_modeled_airtime_100ns,
            vif_policy.grants_finished,
            vif_policy.used_frames,
            vif_policy.used_modeled_airtime_100ns,
            vif_policy.grants_unused,
        ))
        .await;
        yield_now().await;
    }
    let timeline = EGRESS_GRANT_TIMELINE
        .snapshot()
        .wrapping_delta_since(earlier_timeline);
    runtime_log_reliably(format_args!(
        "ONTXTL issued={} completed={} incomplete={} collisions={} unmatched={}",
        timeline.grants_issued,
        timeline.grants_completed,
        timeline.incomplete_completions,
        timeline.slot_collisions,
        timeline.unmatched_events,
    ))
    .await;
    yield_now().await;
    log_egress_timeline_phase("issue_receive", timeline.issue_to_receive).await;
    log_egress_timeline_phase(
        "receive_network_finish",
        timeline.receive_to_network_finish,
    )
    .await;
    log_egress_timeline_phase(
        "network_finish_progress_publish",
        timeline.network_finish_to_progress_publish,
    )
    .await;
    log_egress_timeline_phase(
        "progress_publish_radio_receive",
        timeline.progress_publish_to_radio_receive,
    )
    .await;
    log_egress_timeline_phase("issue_radio_receive", timeline.issue_to_radio_receive).await;
    log_egress_timeline_phase(
        "radio_receive_successor_issue",
        timeline.radio_receive_to_successor_issue,
    )
    .await;
    let (ba_peers, ba_min, ba_max) = crate::product_hil::access_point_tx_block_ack_geometry();
    runtime_log_reliably(format_args!(
        "ONTXQ runs={} run31={} run32={} other={} shadow_checks={} shadow_matches={} shadow_no_window={} shadow_key_mismatch={} shadow_credit_exhausted={} shadow_unclassified={} returns={} return_wakes={} free0={} free1={} free2p={} ready_le31={} ready32={} ready_ge33={} ba_peers={} ba_min={} ba_max={}",
        performance.egress_runs,
        performance.egress_run_31,
        performance.egress_run_32,
        performance.egress_run_other,
        performance.shadow_grant_checks,
        performance.shadow_grant_matches,
        performance.shadow_grant_no_window,
        performance.shadow_grant_key_mismatch,
        performance.shadow_grant_credit_exhausted,
        performance.shadow_grant_unclassified,
        performance.radio_returns,
        performance.radio_return_wakes,
        performance.publication_free_zero,
        performance.publication_free_one,
        performance.publication_free_two_plus,
        performance.publication_ready_le31,
        performance.publication_ready_32,
        performance.publication_ready_ge33,
        ba_peers,
        ba_min,
        ba_max,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ONTXP attempts={} successes={} no_credit={} bytes={} total_cycles={} total_instret={} credit_cycles={} credit_instret={} destination_claim_cycles={} destination_claim_instret={} copy_cycles={} copy_instret={}",
        performance.promotion_attempts,
        performance.promotion_successes,
        performance.promotion_no_credit,
        performance.promotion_bytes,
        performance.promotion_cycles,
        performance.promotion_instructions,
        performance.promotion_credit_cycles,
        performance.promotion_credit_instructions,
        performance.promotion_destination_claim_cycles,
        performance.promotion_destination_claim_instructions,
        performance.promotion_copy_cycles,
        performance.promotion_copy_instructions,
    ))
    .await;
    yield_now().await;
    runtime_log_reliably(format_args!(
        "ONTXPO publication_cycles={} publication_instret={} source_release_cycles={} source_release_instret={} radio_claim_cycles={} radio_claim_instret={} unattributed_cycles={} unattributed_instret={}",
        performance.promotion_publication_cycles,
        performance.promotion_publication_instructions,
        performance.promotion_source_release_cycles,
        performance.promotion_source_release_instructions,
        performance.promotion_radio_claim_cycles,
        performance.promotion_radio_claim_instructions,
        performance.promotion_unattributed_cycles(),
        performance.promotion_unattributed_instructions(),
    ))
    .await;
    yield_now().await;
    WifiEgressPolicyEvidence {
        grants_issued: policy.grants_issued,
        grants_finished: policy.grants_finished,
        grants_used: policy.grants_used,
        grants_unused: policy.grants_unused,
        progress_without_grant: policy.progress_without_grant,
        rejected_updates: policy.rejected_updates,
        rejected_progress: policy.rejected_progress,
        station: hil_egress_vif_evidence(policy.vifs[0]),
        access_point: hil_egress_vif_evidence(policy.vifs[1]),
    }
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
async fn log_egress_timeline_phase(
    phase: &str,
    evidence: EgressGrantTimelinePhaseSnapshot,
) {
    runtime_log_reliably(format_args!(
        "ONTXTLP phase={} samples={} total_us={} lifetime_max_us={}",
        phase, evidence.samples, evidence.total_micros, evidence.lifetime_max_micros,
    ))
    .await;
    yield_now().await;
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
const fn hil_egress_vif_evidence(
    snapshot: open_esp_radio_esp32s31_embassy_wifi::EgressPolicyVifShadowSnapshot,
) -> WifiEgressVifEvidence {
    WifiEgressVifEvidence {
        grants_issued: snapshot.grants_issued,
        issued_frame_credits: snapshot.issued_frame_credits,
        issued_modeled_airtime_100ns: snapshot.issued_modeled_airtime_100ns,
        grants_finished: snapshot.grants_finished,
        used_frames: snapshot.used_frames,
        used_modeled_airtime_100ns: snapshot.used_modeled_airtime_100ns,
        grants_unused: snapshot.grants_unused,
    }
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_egress_control_interval(
    earlier: (EgressControlSnapshot, EgressControlSnapshot),
) {
    let station = open_esp_radio_esp32s31_embassy_wifi::station_egress_control_snapshot()
        .wrapping_delta_since(earlier.0);
    let access_point = open_esp_radio_esp32s31_embassy_wifi::access_point_egress_control_snapshot()
        .wrapping_delta_since(earlier.1);
    log_open_radio_egress_control_vif("sta", station).await;
    log_open_radio_egress_control_vif("ap", access_point).await;
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
async fn log_open_radio_egress_control_vif(vif: &str, control: EgressControlSnapshot) {
    runtime_log_reliably(format_args!(
        "ONTXC vif={} demands={} demand_full={} grants={} grant_full={} network_grants={} grant_progress={} grant_progress_full={} radio_grants={} radio_demands={} radio_demand_rejected={} radio_wakes={} radio_service_calls={} radio_service_progressed={} radio_service_cycles={} radio_service_instret={}",
        vif,
        control.demand_publications,
        control.demand_full,
        control.grant_publications,
        control.grant_full,
        control.network_grants,
        control.grant_progress_publications,
        control.grant_progress_full,
        control.radio_grant_updates,
        control.radio_demand_updates,
        control.radio_demand_rejected,
        control.radio_wakes,
        control.radio_service_calls,
        control.radio_service_progressed,
        control.radio_service_cycles,
        control.radio_service_instructions,
    ))
    .await;
    yield_now().await;
}

/// Emit ownership counts for the selected-batch Core1 materializer.
#[cfg(feature = "tx-architecture-probes")]
pub(in crate::product_hil) async fn log_open_radio_tx_core1_materializer(
    earlier: TxCore1MaterializerSnapshot,
) {
    let materializer = TX_CORE1_MATERIALIZER_COUNTERS
        .snapshot()
        .wrapping_delta_since(earlier);
    runtime_log_reliably(format_args!(
        "ONTXM submitted={} completed={} frames={} no_credit={} cancelled={}",
        materializer.submitted_batches,
        materializer.completed_batches,
        materializer.materialized_frames,
        materializer.no_credit,
        materializer.cancelled_batches,
    ))
    .await;
    yield_now().await;
}

#[cfg(feature = "core0-rx-cycle-telemetry")]
pub(in crate::product_hil) async fn log_open_radio_core0_rx_service_histogram(
    earlier: &Core0RxServiceHistogramSnapshot,
) {
    let services = CORE0_RX_SERVICE_HISTOGRAM
        .snapshot()
        .wrapping_delta_since(*earlier);
    runtime_log_reliably(format_args!(
        "ORC0X spsc_push_calls={} spsc_push_full={} spsc_push_cycles={} \
         spsc_pop_calls={} spsc_pop_empty={} spsc_pop_cycles={} \
         service_record_cycles={} spsc_record_cycles={}",
        services.spsc_push_calls,
        services.spsc_push_full,
        services.spsc_push_cycles,
        services.spsc_pop_calls,
        services.spsc_pop_empty,
        services.spsc_pop_cycles,
        services.service_record_cycles,
        services.spsc_record_cycles,
    ))
    .await;
    yield_now().await;
    for (units, bin) in services.bins.into_iter().enumerate() {
        if bin.services == 0 {
            continue;
        }
        runtime_log_reliably(format_args!(
            "ORC0B units={} services={} total={} setup={} frontier={} admission={} \
             stage_take={} stage_pool={} publish={} tail={}",
            units,
            bin.services,
            bin.total,
            bin.setup,
            bin.frontier,
            bin.admission,
            bin.stage_take,
            bin.stage_pool,
            bin.publish,
            bin.tail,
        ))
        .await;
        yield_now().await;
    }
}

async fn log_open_radio_task_poll(task: &str, poll: TaskPollSnapshot) {
    runtime_log_reliably(format_args!(
        "ORTP task={task} polls={} poll_us={} poll_boot_max_us={} \
         over_100us={} over_500us={} over_1000us={} over_5000us={}",
        poll.polls,
        poll.poll_micros,
        poll.lifetime_max_micros,
        poll.over_100_micros,
        poll.over_500_micros,
        poll.over_1_000_micros,
        poll.over_5_000_micros,
    ))
    .await;
    yield_now().await;
}

/// Observe continuous executor residence without changing the wrapped
/// future's wake or pending semantics. Wall time includes interrupt
/// preemption, which is intentional: a long task poll that blocks sibling
/// Embassy work is harmful regardless of whether its body or an ISR consumed
/// the interval.
#[allow(
    large_assignments,
    reason = "the generic wrapper pins the already owner-rich future inside the final static Embassy task arena; linked-frame and runtime watermark audits remain authoritative"
)]
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

#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
#[allow(
    large_assignments,
    reason = "the diagnostic wrapper pins the existing radio future directly inside its reviewed static task arena"
)]
pub(in crate::product_hil) async fn observe_open_radio_core0_task_polls<F: Future>(
    future: F,
    counters: &'static TaskPollCounters,
) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|context| {
        let started = Instant::now();
        let performance_started = Core0PerformanceSample::read();
        CORE0_PERFORMANCE.begin_radio_poll(performance_started);
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        CORE0_RX_CYCLES.begin_radio_poll(performance_started.cycles);
        let result = future.as_mut().poll(context);
        let performance_ended = Core0PerformanceSample::read();
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        CORE0_RX_CYCLES.end_radio_poll(performance_ended.cycles);
        CORE0_PERFORMANCE.record_radio_poll(performance_started, performance_ended);
        counters.record(started.elapsed().as_micros());
        result
    })
    .await
}
