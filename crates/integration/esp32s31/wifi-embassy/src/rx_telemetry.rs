//! Optional lock-free observations of the staged connected RX pipeline.
//!
//! These counters never participate in ownership or scheduling. Production
//! users that do not attach them pay only one predictable `Option` branch at
//! each instrumented phase; HIL attaches one shared instance in internal SRAM
//! and prints interval deltas only after the traffic sample has ended.

use core::sync::atomic::{AtomicU32, Ordering};
use open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RxServiceObservation {
    pub frontier: usize,
    pub pool_credits: usize,
    pub queue_credits: usize,
    pub admitted: usize,
    pub staged_bytes: usize,
    pub micros: u64,
    pub hardware_buffer_full: Option<u16>,
}

/// Diagnostic observations spanning DMA staging, protocol dispatch and the
/// final `embassy-net` publication copy.
pub struct RxPipelineCounters {
    now_micros: fn() -> u64,
    service_calls: AtomicU32,
    frontier_zero_services: AtomicU32,
    frontier_one_services: AtomicU32,
    frontier_two_three_services: AtomicU32,
    frontier_four_seven_services: AtomicU32,
    frontier_eight_fifteen_services: AtomicU32,
    frontier_sixteen_thirty_one_services: AtomicU32,
    frontier_thirty_two_plus_services: AtomicU32,
    completion_frontier_frames: AtomicU32,
    admitted_frames: AtomicU32,
    staged_bytes: AtomicU32,
    stage_empty_discards: AtomicU32,
    stage_too_long_discards: AtomicU32,
    backpressured_services: AtomicU32,
    pool_credit_limited_services: AtomicU32,
    queue_credit_limited_services: AtomicU32,
    maximum_deferred_frames: AtomicU32,
    minimum_backpressured_pool_credits: AtomicU32,
    minimum_backpressured_queue_credits: AtomicU32,
    maximum_frontier: AtomicU32,
    maximum_admitted: AtomicU32,
    service_micros: AtomicU32,
    service_lifetime_max_micros: AtomicU32,
    dma_buffer_full_last_observation: AtomicU32,
    dma_buffer_full_increments: AtomicU32,
    dma_buffer_full_service_samples: AtomicU32,
    dma_buffer_full_last_service: AtomicU32,
    dma_buffer_full_last_context: AtomicU32,
    dma_buffer_full_last_service_micros: AtomicU32,
    protocol_frames: AtomicU32,
    protocol_data_frames: AtomicU32,
    protocol_amsdu_mpdus: AtomicU32,
    protocol_amsdu_subframes: AtomicU32,
    protocol_units_le_1700: AtomicU32,
    protocol_units_1701_3400: AtomicU32,
    protocol_units_over_3400: AtomicU32,
    protocol_unit_lifetime_max_bytes: AtomicU32,
    reorder_starts: AtomicU32,
    reorder_stops: AtomicU32,
    reorder_last_start: AtomicU32,
    reorder_first_samples: AtomicU32,
    reorder_last_first: AtomicU32,
    reorder_last_first_distance: AtomicU32,
    reorder_buffered: AtomicU32,
    reorder_released: AtomicU32,
    reorder_missing: AtomicU32,
    reorder_stale: AtomicU32,
    reorder_gap_expiries: AtomicU32,
    reorder_current_occupied: AtomicU32,
    reorder_maximum_occupied: AtomicU32,
    network_ready_waits: AtomicU32,
    network_ready_wait_micros: AtomicU32,
    network_ready_wait_lifetime_max_micros: AtomicU32,
    dispatch_micros: AtomicU32,
    dispatch_lifetime_max_micros: AtomicU32,
    network_publications: AtomicU32,
    network_published_bytes: AtomicU32,
    network_publish_micros: AtomicU32,
    network_publish_lifetime_max_micros: AtomicU32,
    rx_irq_epochs: AtomicU32,
    rx_irq_service_samples: AtomicU32,
    rx_irq_clock_skew_samples: AtomicU32,
    rx_irq_to_service_micros: AtomicU32,
    rx_irq_to_service_lifetime_max_micros: AtomicU32,
    pending_rx_irq_micros: AtomicU32,
}

impl RxPipelineCounters {
    const IRQ_TIME_VALID: u32 = 1 << 31;
    const IRQ_TIME_MASK: u32 = Self::IRQ_TIME_VALID - 1;
    const IRQ_SAMPLE_MASK: u32 = 63;

    pub const fn new(now_micros: fn() -> u64) -> Self {
        Self {
            now_micros,
            service_calls: AtomicU32::new(0),
            frontier_zero_services: AtomicU32::new(0),
            frontier_one_services: AtomicU32::new(0),
            frontier_two_three_services: AtomicU32::new(0),
            frontier_four_seven_services: AtomicU32::new(0),
            frontier_eight_fifteen_services: AtomicU32::new(0),
            frontier_sixteen_thirty_one_services: AtomicU32::new(0),
            frontier_thirty_two_plus_services: AtomicU32::new(0),
            completion_frontier_frames: AtomicU32::new(0),
            admitted_frames: AtomicU32::new(0),
            staged_bytes: AtomicU32::new(0),
            stage_empty_discards: AtomicU32::new(0),
            stage_too_long_discards: AtomicU32::new(0),
            backpressured_services: AtomicU32::new(0),
            pool_credit_limited_services: AtomicU32::new(0),
            queue_credit_limited_services: AtomicU32::new(0),
            maximum_deferred_frames: AtomicU32::new(0),
            minimum_backpressured_pool_credits: AtomicU32::new(u32::MAX),
            minimum_backpressured_queue_credits: AtomicU32::new(u32::MAX),
            maximum_frontier: AtomicU32::new(0),
            maximum_admitted: AtomicU32::new(0),
            service_micros: AtomicU32::new(0),
            service_lifetime_max_micros: AtomicU32::new(0),
            dma_buffer_full_last_observation: AtomicU32::new(0),
            dma_buffer_full_increments: AtomicU32::new(0),
            dma_buffer_full_service_samples: AtomicU32::new(0),
            dma_buffer_full_last_service: AtomicU32::new(0),
            dma_buffer_full_last_context: AtomicU32::new(0),
            dma_buffer_full_last_service_micros: AtomicU32::new(0),
            protocol_frames: AtomicU32::new(0),
            protocol_data_frames: AtomicU32::new(0),
            protocol_amsdu_mpdus: AtomicU32::new(0),
            protocol_amsdu_subframes: AtomicU32::new(0),
            protocol_units_le_1700: AtomicU32::new(0),
            protocol_units_1701_3400: AtomicU32::new(0),
            protocol_units_over_3400: AtomicU32::new(0),
            protocol_unit_lifetime_max_bytes: AtomicU32::new(0),
            reorder_starts: AtomicU32::new(0),
            reorder_stops: AtomicU32::new(0),
            reorder_last_start: AtomicU32::new(0),
            reorder_first_samples: AtomicU32::new(0),
            reorder_last_first: AtomicU32::new(0),
            reorder_last_first_distance: AtomicU32::new(0),
            reorder_buffered: AtomicU32::new(0),
            reorder_released: AtomicU32::new(0),
            reorder_missing: AtomicU32::new(0),
            reorder_stale: AtomicU32::new(0),
            reorder_gap_expiries: AtomicU32::new(0),
            reorder_current_occupied: AtomicU32::new(0),
            reorder_maximum_occupied: AtomicU32::new(0),
            network_ready_waits: AtomicU32::new(0),
            network_ready_wait_micros: AtomicU32::new(0),
            network_ready_wait_lifetime_max_micros: AtomicU32::new(0),
            dispatch_micros: AtomicU32::new(0),
            dispatch_lifetime_max_micros: AtomicU32::new(0),
            network_publications: AtomicU32::new(0),
            network_published_bytes: AtomicU32::new(0),
            network_publish_micros: AtomicU32::new(0),
            network_publish_lifetime_max_micros: AtomicU32::new(0),
            rx_irq_epochs: AtomicU32::new(0),
            rx_irq_service_samples: AtomicU32::new(0),
            rx_irq_clock_skew_samples: AtomicU32::new(0),
            rx_irq_to_service_micros: AtomicU32::new(0),
            rx_irq_to_service_lifetime_max_micros: AtomicU32::new(0),
            pending_rx_irq_micros: AtomicU32::new(0),
        }
    }

    pub(crate) fn now_micros(&self) -> u64 {
        (self.now_micros)()
    }

    pub(crate) fn elapsed_micros_since(&self, started: u64) -> u64 {
        self.now_micros().wrapping_sub(started)
    }

    /// Sample one newly published RX wake epoch at the ISR-to-executor handoff.
    ///
    /// The ISR adapter calls this only before publishing a wake when no older
    /// RX wake is pending. Timestamp reads and cross-core CAS operations in the
    /// hard ISR are not free, so only one in 64 wake epochs is timed; the epoch
    /// count itself remains exact. The 31-bit microsecond image wraps every 35
    /// minutes, and modular subtraction remains exact for the bounded
    /// ISR-to-service interval.
    #[inline]
    pub fn record_rx_irq_epoch(&self) {
        let epoch = self.rx_irq_epochs.fetch_add(1, Ordering::Relaxed);
        if epoch & Self::IRQ_SAMPLE_MASK != 0 {
            return;
        }
        let timestamp = Self::IRQ_TIME_VALID | (self.now_micros() as u32 & Self::IRQ_TIME_MASK);
        let _ = self.pending_rx_irq_micros.compare_exchange(
            0,
            timestamp,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn begin_service(&self) -> u64 {
        let started = self.now_micros();
        let pending = if self.pending_rx_irq_micros.load(Ordering::Acquire) == 0 {
            0
        } else {
            self.pending_rx_irq_micros.swap(0, Ordering::AcqRel)
        };
        if pending != 0 {
            let started_modulo = started as u32 & Self::IRQ_TIME_MASK;
            let posted_modulo = pending & Self::IRQ_TIME_MASK;
            let elapsed = started_modulo.wrapping_sub(posted_modulo) & Self::IRQ_TIME_MASK;
            if elapsed <= Self::IRQ_TIME_MASK / 2 {
                self.rx_irq_service_samples.fetch_add(1, Ordering::Relaxed);
                Self::record_time(
                    &self.rx_irq_to_service_micros,
                    &self.rx_irq_to_service_lifetime_max_micros,
                    u64::from(elapsed),
                );
            } else {
                // The platform clock can differ by a few microseconds across
                // cores. A modular value in the upper half is therefore a
                // negative cross-core skew, not a multi-minute service delay.
                self.rx_irq_clock_skew_samples
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        started
    }

    pub fn snapshot(&self) -> RxPipelineCounterSnapshot {
        let buffer_full_observation = self
            .dma_buffer_full_last_observation
            .load(Ordering::Relaxed);
        let buffer_full_context = self.dma_buffer_full_last_context.load(Ordering::Relaxed);
        RxPipelineCounterSnapshot {
            service_calls: self.service_calls.load(Ordering::Relaxed),
            frontier_zero_services: self.frontier_zero_services.load(Ordering::Relaxed),
            frontier_one_services: self.frontier_one_services.load(Ordering::Relaxed),
            frontier_two_three_services: self.frontier_two_three_services.load(Ordering::Relaxed),
            frontier_four_seven_services: self.frontier_four_seven_services.load(Ordering::Relaxed),
            frontier_eight_fifteen_services: self
                .frontier_eight_fifteen_services
                .load(Ordering::Relaxed),
            frontier_sixteen_thirty_one_services: self
                .frontier_sixteen_thirty_one_services
                .load(Ordering::Relaxed),
            frontier_thirty_two_plus_services: self
                .frontier_thirty_two_plus_services
                .load(Ordering::Relaxed),
            completion_frontier_frames: self.completion_frontier_frames.load(Ordering::Relaxed),
            admitted_frames: self.admitted_frames.load(Ordering::Relaxed),
            staged_bytes: self.staged_bytes.load(Ordering::Relaxed),
            stage_empty_discards: self.stage_empty_discards.load(Ordering::Relaxed),
            stage_too_long_discards: self.stage_too_long_discards.load(Ordering::Relaxed),
            backpressured_services: self.backpressured_services.load(Ordering::Relaxed),
            pool_credit_limited_services: self.pool_credit_limited_services.load(Ordering::Relaxed),
            queue_credit_limited_services: self
                .queue_credit_limited_services
                .load(Ordering::Relaxed),
            maximum_deferred_frames: self.maximum_deferred_frames.load(Ordering::Relaxed),
            minimum_backpressured_pool_credits: self
                .minimum_backpressured_pool_credits
                .load(Ordering::Relaxed),
            minimum_backpressured_queue_credits: self
                .minimum_backpressured_queue_credits
                .load(Ordering::Relaxed),
            maximum_frontier: self.maximum_frontier.load(Ordering::Relaxed),
            maximum_admitted: self.maximum_admitted.load(Ordering::Relaxed),
            service_micros: self.service_micros.load(Ordering::Relaxed),
            service_lifetime_max_micros: self.service_lifetime_max_micros.load(Ordering::Relaxed),
            dma_buffer_full_increments: self.dma_buffer_full_increments.load(Ordering::Relaxed),
            dma_buffer_full_service_samples: self
                .dma_buffer_full_service_samples
                .load(Ordering::Relaxed),
            dma_buffer_full_last_service: self.dma_buffer_full_last_service.load(Ordering::Relaxed),
            dma_buffer_full_last_counter: buffer_full_observation & u32::from(u16::MAX),
            dma_buffer_full_last_frontier: buffer_full_context & 0xff,
            dma_buffer_full_last_admitted: buffer_full_context >> 8 & 0xff,
            dma_buffer_full_last_pool_credits: buffer_full_context >> 16 & 0xff,
            dma_buffer_full_last_queue_credits: buffer_full_context >> 24,
            dma_buffer_full_last_service_micros: self
                .dma_buffer_full_last_service_micros
                .load(Ordering::Relaxed),
            protocol_frames: self.protocol_frames.load(Ordering::Relaxed),
            protocol_data_frames: self.protocol_data_frames.load(Ordering::Relaxed),
            protocol_amsdu_mpdus: self.protocol_amsdu_mpdus.load(Ordering::Relaxed),
            protocol_amsdu_subframes: self.protocol_amsdu_subframes.load(Ordering::Relaxed),
            protocol_units_le_1700: self.protocol_units_le_1700.load(Ordering::Relaxed),
            protocol_units_1701_3400: self.protocol_units_1701_3400.load(Ordering::Relaxed),
            protocol_units_over_3400: self.protocol_units_over_3400.load(Ordering::Relaxed),
            protocol_unit_lifetime_max_bytes: self
                .protocol_unit_lifetime_max_bytes
                .load(Ordering::Relaxed),
            reorder_starts: self.reorder_starts.load(Ordering::Relaxed),
            reorder_stops: self.reorder_stops.load(Ordering::Relaxed),
            reorder_last_start: self.reorder_last_start.load(Ordering::Relaxed),
            reorder_first_samples: self.reorder_first_samples.load(Ordering::Relaxed),
            reorder_last_first: self.reorder_last_first.load(Ordering::Relaxed),
            reorder_last_first_distance: self.reorder_last_first_distance.load(Ordering::Relaxed),
            reorder_buffered: self.reorder_buffered.load(Ordering::Relaxed),
            reorder_released: self.reorder_released.load(Ordering::Relaxed),
            reorder_missing: self.reorder_missing.load(Ordering::Relaxed),
            reorder_stale: self.reorder_stale.load(Ordering::Relaxed),
            reorder_gap_expiries: self.reorder_gap_expiries.load(Ordering::Relaxed),
            reorder_current_occupied: self.reorder_current_occupied.load(Ordering::Relaxed),
            reorder_maximum_occupied: self.reorder_maximum_occupied.load(Ordering::Relaxed),
            network_ready_waits: self.network_ready_waits.load(Ordering::Relaxed),
            network_ready_wait_micros: self.network_ready_wait_micros.load(Ordering::Relaxed),
            network_ready_wait_lifetime_max_micros: self
                .network_ready_wait_lifetime_max_micros
                .load(Ordering::Relaxed),
            dispatch_micros: self.dispatch_micros.load(Ordering::Relaxed),
            dispatch_lifetime_max_micros: self.dispatch_lifetime_max_micros.load(Ordering::Relaxed),
            network_publications: self.network_publications.load(Ordering::Relaxed),
            network_published_bytes: self.network_published_bytes.load(Ordering::Relaxed),
            network_publish_micros: self.network_publish_micros.load(Ordering::Relaxed),
            network_publish_lifetime_max_micros: self
                .network_publish_lifetime_max_micros
                .load(Ordering::Relaxed),
            rx_irq_epochs: self.rx_irq_epochs.load(Ordering::Relaxed),
            rx_irq_service_samples: self.rx_irq_service_samples.load(Ordering::Relaxed),
            rx_irq_clock_skew_samples: self.rx_irq_clock_skew_samples.load(Ordering::Relaxed),
            rx_irq_to_service_micros: self.rx_irq_to_service_micros.load(Ordering::Relaxed),
            rx_irq_to_service_lifetime_max_micros: self
                .rx_irq_to_service_lifetime_max_micros
                .load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_service(&self, observation: RxServiceObservation) {
        let RxServiceObservation {
            frontier,
            pool_credits,
            queue_credits,
            admitted,
            staged_bytes,
            micros,
            hardware_buffer_full: _,
        } = observation;
        let service = self.service_calls.fetch_add(1, Ordering::Relaxed) + 1;
        match frontier {
            0 => &self.frontier_zero_services,
            1 => &self.frontier_one_services,
            2..=3 => &self.frontier_two_three_services,
            4..=7 => &self.frontier_four_seven_services,
            8..=15 => &self.frontier_eight_fifteen_services,
            16..=31 => &self.frontier_sixteen_thirty_one_services,
            _ => &self.frontier_thirty_two_plus_services,
        }
        .fetch_add(1, Ordering::Relaxed);
        Self::add_usize(&self.completion_frontier_frames, frontier);
        Self::add_usize(&self.admitted_frames, admitted);
        Self::add_usize(&self.staged_bytes, staged_bytes);
        self.maximum_frontier.fetch_max(
            u32::try_from(frontier).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.maximum_admitted.fetch_max(
            u32::try_from(admitted).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        if admitted < frontier {
            self.backpressured_services.fetch_add(1, Ordering::Relaxed);
            self.maximum_deferred_frames.fetch_max(
                u32::try_from(frontier - admitted).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            self.minimum_backpressured_pool_credits.fetch_min(
                u32::try_from(pool_credits).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            self.minimum_backpressured_queue_credits.fetch_min(
                u32::try_from(queue_credits).unwrap_or(u32::MAX),
                Ordering::Relaxed,
            );
            if pool_credits <= queue_credits {
                self.pool_credit_limited_services
                    .fetch_add(1, Ordering::Relaxed);
            }
            if queue_credits <= pool_credits {
                self.queue_credit_limited_services
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Self::record_time(
            &self.service_micros,
            &self.service_lifetime_max_micros,
            micros,
        );
        self.record_dma_buffer_full(service, observation);
    }

    fn record_dma_buffer_full(&self, service: u32, observation: RxServiceObservation) {
        const VALID: u32 = 1 << 16;

        let Some(current) = observation.hardware_buffer_full else {
            return;
        };
        let encoded = VALID | u32::from(current);
        let previous = self
            .dma_buffer_full_last_observation
            .swap(encoded, Ordering::Relaxed);
        if previous & VALID == 0 {
            return;
        }
        let increments = current.wrapping_sub(previous as u16);
        if increments == 0 {
            return;
        }

        self.dma_buffer_full_increments
            .fetch_add(u32::from(increments), Ordering::Relaxed);
        self.dma_buffer_full_service_samples
            .fetch_add(1, Ordering::Relaxed);
        self.dma_buffer_full_last_service
            .store(service, Ordering::Relaxed);
        let byte = |value: usize| u32::try_from(value.min(0xff)).unwrap_or(0xff);
        self.dma_buffer_full_last_context.store(
            byte(observation.frontier)
                | byte(observation.admitted) << 8
                | byte(observation.pool_credits) << 16
                | byte(observation.queue_credits) << 24,
            Ordering::Relaxed,
        );
        self.dma_buffer_full_last_service_micros.store(
            u32::try_from(observation.micros).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_network_ready_wait(&self, micros: u64) {
        self.network_ready_waits.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.network_ready_wait_micros,
            &self.network_ready_wait_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_stage_discard(&self, error: RxStageError) {
        match error {
            RxStageError::Empty => &self.stage_empty_discards,
            RxStageError::TooLong => &self.stage_too_long_discards,
            _ => return,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_dispatch(
        &self,
        data: bool,
        amsdu: bool,
        amsdu_subframes: u8,
        unit_bytes: usize,
        micros: u64,
    ) {
        self.protocol_frames.fetch_add(1, Ordering::Relaxed);
        if data {
            self.protocol_data_frames.fetch_add(1, Ordering::Relaxed);
        }
        if amsdu {
            self.protocol_amsdu_mpdus.fetch_add(1, Ordering::Relaxed);
            self.protocol_amsdu_subframes
                .fetch_add(u32::from(amsdu_subframes), Ordering::Relaxed);
        }
        match unit_bytes {
            0..=1700 => &self.protocol_units_le_1700,
            1701..=3400 => &self.protocol_units_1701_3400,
            _ => &self.protocol_units_over_3400,
        }
        .fetch_add(1, Ordering::Relaxed);
        self.protocol_unit_lifetime_max_bytes.fetch_max(
            u32::try_from(unit_bytes).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        Self::record_time(
            &self.dispatch_micros,
            &self.dispatch_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_reorder_start(&self, tid: u8, starting_sequence: u16, window: u16) {
        self.reorder_starts.fetch_add(1, Ordering::Relaxed);
        self.reorder_last_start.store(
            u32::from(starting_sequence) | (u32::from(window) << 16) | (u32::from(tid) << 26),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_reorder_stop(&self) {
        self.reorder_stops.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reorder_first(&self, tid: u8, start: u16, sequence: u16) {
        self.reorder_first_samples.fetch_add(1, Ordering::Relaxed);
        self.reorder_last_first.store(
            u32::from(sequence) | (u32::from(start) << 12) | (u32::from(tid) << 24),
            Ordering::Relaxed,
        );
        self.reorder_last_first_distance.store(
            u32::from(sequence.wrapping_sub(start) & 0x0fff),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_reorder_release(
        &self,
        buffered: bool,
        released: u8,
        missing: u16,
        stale: bool,
    ) {
        if buffered {
            self.reorder_buffered.fetch_add(1, Ordering::Relaxed);
        }
        self.reorder_released
            .fetch_add(u32::from(released), Ordering::Relaxed);
        self.reorder_missing
            .fetch_add(u32::from(missing), Ordering::Relaxed);
        if stale {
            self.reorder_stale.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_reorder_gap_expiry(&self) {
        self.reorder_gap_expiries.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reorder_occupied(&self, occupied: u32) {
        self.reorder_current_occupied
            .store(occupied, Ordering::Relaxed);
        self.reorder_maximum_occupied
            .fetch_max(occupied, Ordering::Relaxed);
    }

    pub(crate) fn record_network_publish(&self, bytes: usize, micros: u64) {
        self.network_publications.fetch_add(1, Ordering::Relaxed);
        Self::add_usize(&self.network_published_bytes, bytes);
        Self::record_time(
            &self.network_publish_micros,
            &self.network_publish_lifetime_max_micros,
            micros,
        );
    }

    fn add_usize(counter: &AtomicU32, value: usize) {
        counter.fetch_add(u32::try_from(value).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    fn record_time(total: &AtomicU32, maximum: &AtomicU32, micros: u64) {
        let micros = u32::try_from(micros).unwrap_or(u32::MAX);
        total.fetch_add(micros, Ordering::Relaxed);
        maximum.fetch_max(micros, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxPipelineCounterSnapshot {
    pub service_calls: u32,
    pub frontier_zero_services: u32,
    pub frontier_one_services: u32,
    pub frontier_two_three_services: u32,
    pub frontier_four_seven_services: u32,
    pub frontier_eight_fifteen_services: u32,
    pub frontier_sixteen_thirty_one_services: u32,
    pub frontier_thirty_two_plus_services: u32,
    pub completion_frontier_frames: u32,
    pub admitted_frames: u32,
    pub staged_bytes: u32,
    pub stage_empty_discards: u32,
    pub stage_too_long_discards: u32,
    pub backpressured_services: u32,
    pub pool_credit_limited_services: u32,
    pub queue_credit_limited_services: u32,
    /// Largest completed frontier suffix deferred by one service since boot.
    pub maximum_deferred_frames: u32,
    /// Smallest staging-pool credit observation at a backpressured service.
    pub minimum_backpressured_pool_credits: u32,
    /// Smallest staging-queue credit observation at a backpressured service.
    pub minimum_backpressured_queue_credits: u32,
    /// Maximum observed since boot, not an interval delta.
    pub maximum_frontier: u32,
    /// Maximum observed since boot, not an interval delta.
    pub maximum_admitted: u32,
    pub service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub service_lifetime_max_micros: u32,
    /// Hardware `BUFFER_FULL` increments observed at RX service boundaries.
    pub dma_buffer_full_increments: u32,
    /// Service boundaries at which one or more new increments were observed.
    pub dma_buffer_full_service_samples: u32,
    /// Boot-lifetime service ordinal carrying the most recent observation.
    pub dma_buffer_full_last_service: u32,
    pub dma_buffer_full_last_counter: u32,
    pub dma_buffer_full_last_frontier: u32,
    pub dma_buffer_full_last_admitted: u32,
    pub dma_buffer_full_last_pool_credits: u32,
    pub dma_buffer_full_last_queue_credits: u32,
    pub dma_buffer_full_last_service_micros: u32,
    pub protocol_frames: u32,
    pub protocol_data_frames: u32,
    pub protocol_amsdu_mpdus: u32,
    pub protocol_amsdu_subframes: u32,
    pub protocol_units_le_1700: u32,
    pub protocol_units_1701_3400: u32,
    pub protocol_units_over_3400: u32,
    /// Maximum raw staged unit size observed since boot, not an interval delta.
    pub protocol_unit_lifetime_max_bytes: u32,
    pub reorder_starts: u32,
    pub reorder_stops: u32,
    /// Last packed `tid[28:26] | window[21:16] | starting_sequence[11:0]`.
    pub reorder_last_start: u32,
    pub reorder_first_samples: u32,
    /// Last packed `tid[27:24] | start[23:12] | first_sequence[11:0]`.
    pub reorder_last_first: u32,
    pub reorder_last_first_distance: u32,
    pub reorder_buffered: u32,
    pub reorder_released: u32,
    pub reorder_missing: u32,
    pub reorder_stale: u32,
    pub reorder_gap_expiries: u32,
    pub reorder_current_occupied: u32,
    pub reorder_maximum_occupied: u32,
    pub network_ready_waits: u32,
    pub network_ready_wait_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub network_ready_wait_lifetime_max_micros: u32,
    pub dispatch_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub dispatch_lifetime_max_micros: u32,
    pub network_publications: u32,
    pub network_published_bytes: u32,
    pub network_publish_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub network_publish_lifetime_max_micros: u32,
    pub rx_irq_epochs: u32,
    pub rx_irq_service_samples: u32,
    pub rx_irq_clock_skew_samples: u32,
    pub rx_irq_to_service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub rx_irq_to_service_lifetime_max_micros: u32,
}

impl RxPipelineCounterSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            service_calls: self.service_calls.wrapping_sub(earlier.service_calls),
            frontier_zero_services: self
                .frontier_zero_services
                .wrapping_sub(earlier.frontier_zero_services),
            frontier_one_services: self
                .frontier_one_services
                .wrapping_sub(earlier.frontier_one_services),
            frontier_two_three_services: self
                .frontier_two_three_services
                .wrapping_sub(earlier.frontier_two_three_services),
            frontier_four_seven_services: self
                .frontier_four_seven_services
                .wrapping_sub(earlier.frontier_four_seven_services),
            frontier_eight_fifteen_services: self
                .frontier_eight_fifteen_services
                .wrapping_sub(earlier.frontier_eight_fifteen_services),
            frontier_sixteen_thirty_one_services: self
                .frontier_sixteen_thirty_one_services
                .wrapping_sub(earlier.frontier_sixteen_thirty_one_services),
            frontier_thirty_two_plus_services: self
                .frontier_thirty_two_plus_services
                .wrapping_sub(earlier.frontier_thirty_two_plus_services),
            completion_frontier_frames: self
                .completion_frontier_frames
                .wrapping_sub(earlier.completion_frontier_frames),
            admitted_frames: self.admitted_frames.wrapping_sub(earlier.admitted_frames),
            staged_bytes: self.staged_bytes.wrapping_sub(earlier.staged_bytes),
            stage_empty_discards: self
                .stage_empty_discards
                .wrapping_sub(earlier.stage_empty_discards),
            stage_too_long_discards: self
                .stage_too_long_discards
                .wrapping_sub(earlier.stage_too_long_discards),
            backpressured_services: self
                .backpressured_services
                .wrapping_sub(earlier.backpressured_services),
            pool_credit_limited_services: self
                .pool_credit_limited_services
                .wrapping_sub(earlier.pool_credit_limited_services),
            queue_credit_limited_services: self
                .queue_credit_limited_services
                .wrapping_sub(earlier.queue_credit_limited_services),
            maximum_deferred_frames: self.maximum_deferred_frames,
            minimum_backpressured_pool_credits: self.minimum_backpressured_pool_credits,
            minimum_backpressured_queue_credits: self.minimum_backpressured_queue_credits,
            maximum_frontier: self.maximum_frontier,
            maximum_admitted: self.maximum_admitted,
            service_micros: self.service_micros.wrapping_sub(earlier.service_micros),
            service_lifetime_max_micros: self.service_lifetime_max_micros,
            dma_buffer_full_increments: self
                .dma_buffer_full_increments
                .wrapping_sub(earlier.dma_buffer_full_increments),
            dma_buffer_full_service_samples: self
                .dma_buffer_full_service_samples
                .wrapping_sub(earlier.dma_buffer_full_service_samples),
            dma_buffer_full_last_service: self.dma_buffer_full_last_service,
            dma_buffer_full_last_counter: self.dma_buffer_full_last_counter,
            dma_buffer_full_last_frontier: self.dma_buffer_full_last_frontier,
            dma_buffer_full_last_admitted: self.dma_buffer_full_last_admitted,
            dma_buffer_full_last_pool_credits: self.dma_buffer_full_last_pool_credits,
            dma_buffer_full_last_queue_credits: self.dma_buffer_full_last_queue_credits,
            dma_buffer_full_last_service_micros: self.dma_buffer_full_last_service_micros,
            protocol_frames: self.protocol_frames.wrapping_sub(earlier.protocol_frames),
            protocol_data_frames: self
                .protocol_data_frames
                .wrapping_sub(earlier.protocol_data_frames),
            protocol_amsdu_mpdus: self
                .protocol_amsdu_mpdus
                .wrapping_sub(earlier.protocol_amsdu_mpdus),
            protocol_amsdu_subframes: self
                .protocol_amsdu_subframes
                .wrapping_sub(earlier.protocol_amsdu_subframes),
            protocol_units_le_1700: self
                .protocol_units_le_1700
                .wrapping_sub(earlier.protocol_units_le_1700),
            protocol_units_1701_3400: self
                .protocol_units_1701_3400
                .wrapping_sub(earlier.protocol_units_1701_3400),
            protocol_units_over_3400: self
                .protocol_units_over_3400
                .wrapping_sub(earlier.protocol_units_over_3400),
            protocol_unit_lifetime_max_bytes: self.protocol_unit_lifetime_max_bytes,
            reorder_starts: self.reorder_starts.wrapping_sub(earlier.reorder_starts),
            reorder_stops: self.reorder_stops.wrapping_sub(earlier.reorder_stops),
            reorder_last_start: self.reorder_last_start,
            reorder_first_samples: self
                .reorder_first_samples
                .wrapping_sub(earlier.reorder_first_samples),
            reorder_last_first: self.reorder_last_first,
            reorder_last_first_distance: self.reorder_last_first_distance,
            reorder_buffered: self.reorder_buffered.wrapping_sub(earlier.reorder_buffered),
            reorder_released: self.reorder_released.wrapping_sub(earlier.reorder_released),
            reorder_missing: self.reorder_missing.wrapping_sub(earlier.reorder_missing),
            reorder_stale: self.reorder_stale.wrapping_sub(earlier.reorder_stale),
            reorder_gap_expiries: self
                .reorder_gap_expiries
                .wrapping_sub(earlier.reorder_gap_expiries),
            reorder_current_occupied: self.reorder_current_occupied,
            reorder_maximum_occupied: self.reorder_maximum_occupied,
            network_ready_waits: self
                .network_ready_waits
                .wrapping_sub(earlier.network_ready_waits),
            network_ready_wait_micros: self
                .network_ready_wait_micros
                .wrapping_sub(earlier.network_ready_wait_micros),
            network_ready_wait_lifetime_max_micros: self.network_ready_wait_lifetime_max_micros,
            dispatch_micros: self.dispatch_micros.wrapping_sub(earlier.dispatch_micros),
            dispatch_lifetime_max_micros: self.dispatch_lifetime_max_micros,
            network_publications: self
                .network_publications
                .wrapping_sub(earlier.network_publications),
            network_published_bytes: self
                .network_published_bytes
                .wrapping_sub(earlier.network_published_bytes),
            network_publish_micros: self
                .network_publish_micros
                .wrapping_sub(earlier.network_publish_micros),
            network_publish_lifetime_max_micros: self.network_publish_lifetime_max_micros,
            rx_irq_epochs: self.rx_irq_epochs.wrapping_sub(earlier.rx_irq_epochs),
            rx_irq_service_samples: self
                .rx_irq_service_samples
                .wrapping_sub(earlier.rx_irq_service_samples),
            rx_irq_clock_skew_samples: self
                .rx_irq_clock_skew_samples
                .wrapping_sub(earlier.rx_irq_clock_skew_samples),
            rx_irq_to_service_micros: self
                .rx_irq_to_service_micros
                .wrapping_sub(earlier.rx_irq_to_service_micros),
            rx_irq_to_service_lifetime_max_micros: self.rx_irq_to_service_lifetime_max_micros,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU64, Ordering};
    use open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageError;

    use super::{RxPipelineCounters, RxServiceObservation};

    static IRQ_CLOCK: AtomicU64 = AtomicU64::new(0);
    static IRQ_SKEW_CLOCK: AtomicU64 = AtomicU64::new(0);

    fn test_clock() -> u64 {
        0
    }

    fn irq_clock() -> u64 {
        IRQ_CLOCK.load(Ordering::Relaxed)
    }

    fn irq_skew_clock() -> u64 {
        IRQ_SKEW_CLOCK.load(Ordering::Relaxed)
    }

    #[test]
    fn interval_delta_retains_totals_limits_and_phase_times() {
        let counters = RxPipelineCounters::new(test_clock);
        let before = counters.snapshot();
        counters.record_service(RxServiceObservation {
            frontier: 4,
            pool_credits: 3,
            queue_credits: 5,
            admitted: 3,
            staged_bytes: 4_500,
            micros: 70,
            hardware_buffer_full: None,
        });
        counters.record_stage_discard(RxStageError::Empty);
        counters.record_stage_discard(RxStageError::TooLong);
        counters.record_network_ready_wait(2);
        counters.record_network_publish(1_514, 13);
        counters.record_dispatch(true, true, 3, 2_750, 31);
        let delta = counters.snapshot().wrapping_delta_since(before);

        assert_eq!(delta.service_calls, 1);
        assert_eq!(delta.frontier_four_seven_services, 1);
        assert_eq!(delta.completion_frontier_frames, 4);
        assert_eq!(delta.admitted_frames, 3);
        assert_eq!(delta.staged_bytes, 4_500);
        assert_eq!(delta.stage_empty_discards, 1);
        assert_eq!(delta.stage_too_long_discards, 1);
        assert_eq!(delta.backpressured_services, 1);
        assert_eq!(delta.pool_credit_limited_services, 1);
        assert_eq!(delta.queue_credit_limited_services, 0);
        assert_eq!(delta.maximum_deferred_frames, 1);
        assert_eq!(delta.minimum_backpressured_pool_credits, 3);
        assert_eq!(delta.minimum_backpressured_queue_credits, 5);
        assert_eq!(delta.maximum_frontier, 4);
        assert_eq!(delta.maximum_admitted, 3);
        assert_eq!(delta.service_micros, 70);
        assert_eq!(delta.network_ready_wait_micros, 2);
        assert_eq!(delta.network_published_bytes, 1_514);
        assert_eq!(delta.network_publish_micros, 13);
        assert_eq!(delta.protocol_data_frames, 1);
        assert_eq!(delta.protocol_amsdu_mpdus, 1);
        assert_eq!(delta.protocol_amsdu_subframes, 3);
        assert_eq!(delta.protocol_units_1701_3400, 1);
        assert_eq!(delta.protocol_unit_lifetime_max_bytes, 2_750);
        assert_eq!(delta.dispatch_micros, 31);
    }

    #[test]
    fn sampled_irq_latency_wraps_and_frontiers_cover_every_service() {
        IRQ_CLOCK.store((1_u64 << 31) - 3, Ordering::Relaxed);
        let counters = RxPipelineCounters::new(irq_clock);
        let before = counters.snapshot();
        counters.record_rx_irq_epoch();
        IRQ_CLOCK.store((1_u64 << 31) - 1, Ordering::Relaxed);
        counters.record_rx_irq_epoch();
        IRQ_CLOCK.store((1_u64 << 31) + 4, Ordering::Relaxed);
        let started = counters.begin_service();
        counters.record_service(RxServiceObservation {
            frontier: 32,
            pool_credits: 32,
            queue_credits: 32,
            admitted: 32,
            staged_bytes: 48_000,
            micros: 80,
            hardware_buffer_full: None,
        });
        counters.begin_service();
        counters.record_service(RxServiceObservation {
            frontier: 0,
            pool_credits: 32,
            queue_credits: 32,
            admitted: 0,
            staged_bytes: 0,
            micros: 1,
            hardware_buffer_full: None,
        });
        let delta = counters.snapshot().wrapping_delta_since(before);

        assert_eq!(started, (1_u64 << 31) + 4);
        assert_eq!(delta.rx_irq_epochs, 2);
        assert_eq!(delta.rx_irq_service_samples, 1);
        assert_eq!(delta.rx_irq_to_service_micros, 7);
        assert_eq!(delta.rx_irq_to_service_lifetime_max_micros, 7);
        assert_eq!(delta.frontier_zero_services, 1);
        assert_eq!(delta.frontier_thirty_two_plus_services, 1);
        assert_eq!(delta.service_calls, 2);
    }

    #[test]
    fn negative_cross_core_clock_skew_is_not_reported_as_latency() {
        IRQ_SKEW_CLOCK.store(100, Ordering::Relaxed);
        let counters = RxPipelineCounters::new(irq_skew_clock);
        counters.record_rx_irq_epoch();
        IRQ_SKEW_CLOCK.store(96, Ordering::Relaxed);
        counters.begin_service();
        let snapshot = counters.snapshot();

        assert_eq!(snapshot.rx_irq_service_samples, 0);
        assert_eq!(snapshot.rx_irq_clock_skew_samples, 1);
        assert_eq!(snapshot.rx_irq_to_service_micros, 0);
    }

    #[test]
    fn buffer_full_wrap_is_attributed_to_the_next_service_context() {
        let counters = RxPipelineCounters::new(test_clock);
        counters.record_service(RxServiceObservation {
            frontier: 1,
            pool_credits: 64,
            queue_credits: 64,
            admitted: 1,
            staged_bytes: 1_600,
            micros: 20,
            hardware_buffer_full: Some(0xfffe),
        });
        let before = counters.snapshot();

        counters.record_service(RxServiceObservation {
            frontier: 7,
            pool_credits: 60,
            queue_credits: 61,
            admitted: 7,
            staged_bytes: 11_200,
            micros: 73,
            hardware_buffer_full: Some(1),
        });
        let delta = counters.snapshot().wrapping_delta_since(before);

        assert_eq!(delta.dma_buffer_full_increments, 3);
        assert_eq!(delta.dma_buffer_full_service_samples, 1);
        assert_eq!(delta.dma_buffer_full_last_service, 2);
        assert_eq!(delta.dma_buffer_full_last_counter, 1);
        assert_eq!(delta.dma_buffer_full_last_frontier, 7);
        assert_eq!(delta.dma_buffer_full_last_admitted, 7);
        assert_eq!(delta.dma_buffer_full_last_pool_credits, 60);
        assert_eq!(delta.dma_buffer_full_last_queue_credits, 61);
        assert_eq!(delta.dma_buffer_full_last_service_micros, 73);
    }
}
