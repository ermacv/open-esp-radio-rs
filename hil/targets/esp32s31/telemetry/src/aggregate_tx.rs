//! Lock-free ESP32-S31 HIL observations of connected aggregate TX.
//!
//! These counters do not participate in scheduling, retry policy or DMA
//! ownership. The production adapter publishes value-only events; this module
//! selects the histogram, atomics and interval snapshot used by qualification.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_esp32s31_wifi_embassy::aggregate_tx_observer::{
    AggregateBuildStop, AggregateTxObservation, AggregateTxObserver, NetworkSingleMpduReason,
};

/// Number of histogram entries required for all currently legal A-MPDU sizes.
///
/// Index zero is deliberately unused; indexes `1..=32` are the number of
/// original MPDUs prepared for one aggregate exchange.
pub const AGGREGATE_TX_HISTOGRAM_BUCKETS: usize = 33;

/// Lock-free HIL observations of the production connected TX owner.
///
/// The counters are diagnostic only and never participate in scheduling.
/// Relaxed atomics keep a HIL observer from adding synchronization to the
/// radio path it is measuring.
pub struct AggregateTxCounters {
    network_single_mpdu_started: AtomicU32,
    network_single_legacy_rate: AtomicU32,
    network_single_block_ack_unavailable: AtomicU32,
    network_single_ht_needs_pair: AtomicU32,
    network_single_fresh_aggregate_capacity: AtomicU32,
    network_single_fresh_capacity_lifetime_max_ethernet_length: AtomicU32,
    aggregates_prepared: AtomicU32,
    aggregate_publications: AtomicU32,
    aggregates_completed: AtomicU32,
    subframes_acknowledged: AtomicU32,
    individual_retries: AtomicU32,
    hardware_timeouts: AtomicU32,
    collisions: AtomicU32,
    preparation_micros: AtomicU32,
    preparation_lifetime_max_micros: AtomicU32,
    publication_program_micros: AtomicU32,
    publication_program_lifetime_max_micros: AtomicU32,
    last_publication_micros: AtomicU32,
    exchange_micros: AtomicU32,
    exchange_lifetime_max_micros: AtomicU32,
    single_publication_exchanges: AtomicU32,
    single_publication_exchange_micros: AtomicU32,
    single_publication_exchange_lifetime_max_micros: AtomicU32,
    retried_exchanges: AtomicU32,
    retried_exchange_publications: AtomicU32,
    retried_exchange_micros: AtomicU32,
    retried_exchange_lifetime_max_micros: AtomicU32,
    tx_irq_epochs: AtomicU32,
    tx_irq_service_samples: AtomicU32,
    tx_irq_clock_skew_samples: AtomicU32,
    tx_irq_to_service_micros: AtomicU32,
    tx_irq_to_service_lifetime_max_micros: AtomicU32,
    tx_publication_to_irq_samples: AtomicU32,
    tx_publication_to_irq_micros: AtomicU32,
    tx_publication_to_irq_lifetime_max_micros: AtomicU32,
    pending_tx_irq_micros: AtomicU32,
    stopped_at_frame_limit: AtomicU32,
    stopped_at_capacity_limit: AtomicU32,
    stopped_on_empty_queue: AtomicU32,
    prepared_subframes: [AtomicU32; AGGREGATE_TX_HISTOGRAM_BUCKETS],
}

impl AggregateTxCounters {
    const IRQ_SAMPLE_MASK: u32 = 0x3f;
    const IRQ_TIME_VALID: u32 = 1 << 31;
    const IRQ_TIME_MASK: u32 = Self::IRQ_TIME_VALID - 1;

    pub const fn new() -> Self {
        Self {
            network_single_mpdu_started: AtomicU32::new(0),
            network_single_legacy_rate: AtomicU32::new(0),
            network_single_block_ack_unavailable: AtomicU32::new(0),
            network_single_ht_needs_pair: AtomicU32::new(0),
            network_single_fresh_aggregate_capacity: AtomicU32::new(0),
            network_single_fresh_capacity_lifetime_max_ethernet_length: AtomicU32::new(0),
            aggregates_prepared: AtomicU32::new(0),
            aggregate_publications: AtomicU32::new(0),
            aggregates_completed: AtomicU32::new(0),
            subframes_acknowledged: AtomicU32::new(0),
            individual_retries: AtomicU32::new(0),
            hardware_timeouts: AtomicU32::new(0),
            collisions: AtomicU32::new(0),
            preparation_micros: AtomicU32::new(0),
            preparation_lifetime_max_micros: AtomicU32::new(0),
            publication_program_micros: AtomicU32::new(0),
            publication_program_lifetime_max_micros: AtomicU32::new(0),
            last_publication_micros: AtomicU32::new(0),
            exchange_micros: AtomicU32::new(0),
            exchange_lifetime_max_micros: AtomicU32::new(0),
            single_publication_exchanges: AtomicU32::new(0),
            single_publication_exchange_micros: AtomicU32::new(0),
            single_publication_exchange_lifetime_max_micros: AtomicU32::new(0),
            retried_exchanges: AtomicU32::new(0),
            retried_exchange_publications: AtomicU32::new(0),
            retried_exchange_micros: AtomicU32::new(0),
            retried_exchange_lifetime_max_micros: AtomicU32::new(0),
            tx_irq_epochs: AtomicU32::new(0),
            tx_irq_service_samples: AtomicU32::new(0),
            tx_irq_clock_skew_samples: AtomicU32::new(0),
            tx_irq_to_service_micros: AtomicU32::new(0),
            tx_irq_to_service_lifetime_max_micros: AtomicU32::new(0),
            tx_publication_to_irq_samples: AtomicU32::new(0),
            tx_publication_to_irq_micros: AtomicU32::new(0),
            tx_publication_to_irq_lifetime_max_micros: AtomicU32::new(0),
            pending_tx_irq_micros: AtomicU32::new(0),
            stopped_at_frame_limit: AtomicU32::new(0),
            stopped_at_capacity_limit: AtomicU32::new(0),
            stopped_on_empty_queue: AtomicU32::new(0),
            prepared_subframes: [const { AtomicU32::new(0) }; AGGREGATE_TX_HISTOGRAM_BUCKETS],
        }
    }

    pub fn snapshot(&self) -> AggregateTxCounterSnapshot {
        AggregateTxCounterSnapshot {
            network_single_mpdu_started: self.network_single_mpdu_started.load(Ordering::Relaxed),
            network_single_legacy_rate: self.network_single_legacy_rate.load(Ordering::Relaxed),
            network_single_block_ack_unavailable: self
                .network_single_block_ack_unavailable
                .load(Ordering::Relaxed),
            network_single_ht_needs_pair: self.network_single_ht_needs_pair.load(Ordering::Relaxed),
            network_single_fresh_aggregate_capacity: self
                .network_single_fresh_aggregate_capacity
                .load(Ordering::Relaxed),
            network_single_fresh_capacity_lifetime_max_ethernet_length: self
                .network_single_fresh_capacity_lifetime_max_ethernet_length
                .load(Ordering::Relaxed),
            aggregates_prepared: self.aggregates_prepared.load(Ordering::Relaxed),
            aggregate_publications: self.aggregate_publications.load(Ordering::Relaxed),
            aggregates_completed: self.aggregates_completed.load(Ordering::Relaxed),
            subframes_acknowledged: self.subframes_acknowledged.load(Ordering::Relaxed),
            individual_retries: self.individual_retries.load(Ordering::Relaxed),
            hardware_timeouts: self.hardware_timeouts.load(Ordering::Relaxed),
            collisions: self.collisions.load(Ordering::Relaxed),
            preparation_micros: self.preparation_micros.load(Ordering::Relaxed),
            preparation_lifetime_max_micros: self
                .preparation_lifetime_max_micros
                .load(Ordering::Relaxed),
            publication_program_micros: self.publication_program_micros.load(Ordering::Relaxed),
            publication_program_lifetime_max_micros: self
                .publication_program_lifetime_max_micros
                .load(Ordering::Relaxed),
            exchange_micros: self.exchange_micros.load(Ordering::Relaxed),
            exchange_lifetime_max_micros: self.exchange_lifetime_max_micros.load(Ordering::Relaxed),
            single_publication_exchanges: self.single_publication_exchanges.load(Ordering::Relaxed),
            single_publication_exchange_micros: self
                .single_publication_exchange_micros
                .load(Ordering::Relaxed),
            single_publication_exchange_lifetime_max_micros: self
                .single_publication_exchange_lifetime_max_micros
                .load(Ordering::Relaxed),
            retried_exchanges: self.retried_exchanges.load(Ordering::Relaxed),
            retried_exchange_publications: self
                .retried_exchange_publications
                .load(Ordering::Relaxed),
            retried_exchange_micros: self.retried_exchange_micros.load(Ordering::Relaxed),
            retried_exchange_lifetime_max_micros: self
                .retried_exchange_lifetime_max_micros
                .load(Ordering::Relaxed),
            tx_irq_epochs: self.tx_irq_epochs.load(Ordering::Relaxed),
            tx_irq_service_samples: self.tx_irq_service_samples.load(Ordering::Relaxed),
            tx_irq_clock_skew_samples: self.tx_irq_clock_skew_samples.load(Ordering::Relaxed),
            tx_irq_to_service_micros: self.tx_irq_to_service_micros.load(Ordering::Relaxed),
            tx_irq_to_service_lifetime_max_micros: self
                .tx_irq_to_service_lifetime_max_micros
                .load(Ordering::Relaxed),
            tx_publication_to_irq_samples: self
                .tx_publication_to_irq_samples
                .load(Ordering::Relaxed),
            tx_publication_to_irq_micros: self
                .tx_publication_to_irq_micros
                .load(Ordering::Relaxed),
            tx_publication_to_irq_lifetime_max_micros: self
                .tx_publication_to_irq_lifetime_max_micros
                .load(Ordering::Relaxed),
            stopped_at_frame_limit: self.stopped_at_frame_limit.load(Ordering::Relaxed),
            stopped_at_capacity_limit: self.stopped_at_capacity_limit.load(Ordering::Relaxed),
            stopped_on_empty_queue: self.stopped_on_empty_queue.load(Ordering::Relaxed),
            prepared_subframes: core::array::from_fn(|index| {
                self.prepared_subframes[index].load(Ordering::Relaxed)
            }),
        }
    }

    pub(crate) fn record_network_single_mpdu(
        &self,
        reason: NetworkSingleMpduReason,
        ethernet_length: usize,
    ) {
        self.network_single_mpdu_started
            .fetch_add(1, Ordering::Relaxed);
        let reason = match reason {
            NetworkSingleMpduReason::LegacyRate => &self.network_single_legacy_rate,
            NetworkSingleMpduReason::BlockAckUnavailable => {
                &self.network_single_block_ack_unavailable
            }
            NetworkSingleMpduReason::HtNeedsPair => &self.network_single_ht_needs_pair,
            NetworkSingleMpduReason::FreshAggregateCapacity => {
                self.network_single_fresh_capacity_lifetime_max_ethernet_length
                    .fetch_max(
                        u32::try_from(ethernet_length).unwrap_or(u32::MAX),
                        Ordering::Relaxed,
                    );
                &self.network_single_fresh_aggregate_capacity
            }
        };
        reason.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_prepared(&self, subframes: u8, stop: AggregateBuildStop) {
        let index = usize::from(subframes);
        debug_assert!((1..AGGREGATE_TX_HISTOGRAM_BUCKETS).contains(&index));
        self.aggregates_prepared.fetch_add(1, Ordering::Relaxed);
        if let Some(bucket) = self.prepared_subframes.get(index) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
        match stop {
            AggregateBuildStop::FrameLimit => {
                self.stopped_at_frame_limit.fetch_add(1, Ordering::Relaxed);
            }
            AggregateBuildStop::CapacityLimit => {
                self.stopped_at_capacity_limit
                    .fetch_add(1, Ordering::Relaxed);
            }
            AggregateBuildStop::QueueEmpty => {
                self.stopped_on_empty_queue.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn record_preparation_time(&self, micros: u64) {
        Self::record_time(
            &self.preparation_micros,
            &self.preparation_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_publication(&self, at_micros: u64, program_micros: u64) {
        self.last_publication_micros.store(
            Self::IRQ_TIME_VALID | (at_micros as u32 & Self::IRQ_TIME_MASK),
            Ordering::Release,
        );
        self.aggregate_publications.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.publication_program_micros,
            &self.publication_program_lifetime_max_micros,
            program_micros,
        );
    }

    pub(crate) fn record_complete(&self, acknowledged: u8, individual_retry: bool) {
        self.aggregates_completed.fetch_add(1, Ordering::Relaxed);
        self.subframes_acknowledged
            .fetch_add(u32::from(acknowledged), Ordering::Relaxed);
        if individual_retry {
            self.individual_retries.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_hardware_timeout(&self) {
        self.hardware_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_collision(&self) {
        self.collisions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_exchange_time(&self, micros: u64, publications: u8) {
        Self::record_time(
            &self.exchange_micros,
            &self.exchange_lifetime_max_micros,
            micros,
        );
        if publications <= 1 {
            self.single_publication_exchanges
                .fetch_add(1, Ordering::Relaxed);
            Self::record_time(
                &self.single_publication_exchange_micros,
                &self.single_publication_exchange_lifetime_max_micros,
                micros,
            );
        } else {
            self.retried_exchanges.fetch_add(1, Ordering::Relaxed);
            self.retried_exchange_publications
                .fetch_add(u32::from(publications), Ordering::Relaxed);
            Self::record_time(
                &self.retried_exchange_micros,
                &self.retried_exchange_lifetime_max_micros,
                micros,
            );
        }
    }

    /// Sample one newly published TX wake epoch at the ISR-to-executor handoff.
    ///
    /// The caller supplies the clock lazily so the hard ISR reads it only for
    /// one in 64 wake epochs. Epoch counting remains exact, and an already
    /// pending sample is never replaced by a newer coalesced interrupt.
    #[inline]
    pub fn record_tx_irq_epoch(&self, now_micros: impl FnOnce() -> u64) {
        // Consume this correlation on every TX edge, including unsampled
        // epochs, so an ordinary completion cannot inherit an older A-MPDU
        // publication timestamp.
        let publication = self.last_publication_micros.swap(0, Ordering::AcqRel);
        let epoch = self.tx_irq_epochs.fetch_add(1, Ordering::Relaxed);
        if epoch & Self::IRQ_SAMPLE_MASK != 0 {
            return;
        }
        let now_modulo = now_micros() as u32 & Self::IRQ_TIME_MASK;
        let timestamp = Self::IRQ_TIME_VALID | now_modulo;
        if publication != 0 {
            let posted_modulo = publication & Self::IRQ_TIME_MASK;
            let elapsed = now_modulo.wrapping_sub(posted_modulo) & Self::IRQ_TIME_MASK;
            if elapsed <= Self::IRQ_TIME_MASK / 2 {
                self.tx_publication_to_irq_samples
                    .fetch_add(1, Ordering::Relaxed);
                Self::record_time(
                    &self.tx_publication_to_irq_micros,
                    &self.tx_publication_to_irq_lifetime_max_micros,
                    u64::from(elapsed),
                );
            }
        }
        let _ = self.pending_tx_irq_micros.compare_exchange(
            0,
            timestamp,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    fn record_tx_service_started(&self, started: u64) {
        let pending = if self.pending_tx_irq_micros.load(Ordering::Acquire) == 0 {
            0
        } else {
            self.pending_tx_irq_micros.swap(0, Ordering::AcqRel)
        };
        if pending == 0 {
            return;
        }
        let started_modulo = started as u32 & Self::IRQ_TIME_MASK;
        let posted_modulo = pending & Self::IRQ_TIME_MASK;
        let elapsed = started_modulo.wrapping_sub(posted_modulo) & Self::IRQ_TIME_MASK;
        if elapsed <= Self::IRQ_TIME_MASK / 2 {
            self.tx_irq_service_samples.fetch_add(1, Ordering::Relaxed);
            Self::record_time(
                &self.tx_irq_to_service_micros,
                &self.tx_irq_to_service_lifetime_max_micros,
                u64::from(elapsed),
            );
        } else {
            // The clock can differ slightly across the ISR and radio cores.
            self.tx_irq_clock_skew_samples
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_time(total: &AtomicU32, maximum: &AtomicU32, micros: u64) {
        let micros = u32::try_from(micros).unwrap_or(u32::MAX);
        total.fetch_add(micros, Ordering::Relaxed);
        maximum.fetch_max(micros, Ordering::Relaxed);
    }
}

impl AggregateTxObserver for AggregateTxCounters {
    fn observe(&self, observation: AggregateTxObservation) {
        match observation {
            AggregateTxObservation::InterruptServiceStarted { at_micros } => {
                self.record_tx_service_started(at_micros);
            }
            AggregateTxObservation::NetworkSingleMpdu {
                reason,
                ethernet_length,
            } => self.record_network_single_mpdu(reason, ethernet_length),
            AggregateTxObservation::Prepared { subframes, stop } => {
                self.record_prepared(subframes, stop);
            }
            AggregateTxObservation::PreparationCompleted { micros } => {
                self.record_preparation_time(micros);
            }
            AggregateTxObservation::Published {
                at_micros,
                program_micros,
            } => {
                self.record_publication(at_micros, program_micros);
            }
            AggregateTxObservation::Completed {
                acknowledged,
                individual_retry,
            } => self.record_complete(acknowledged, individual_retry),
            AggregateTxObservation::HardwareTimeout => self.record_hardware_timeout(),
            AggregateTxObservation::Collision => self.record_collision(),
            AggregateTxObservation::ExchangeCompleted {
                micros,
                publications,
            } => {
                self.record_exchange_time(micros, publications);
            }
        }
    }
}

impl Default for AggregateTxCounters {
    fn default() -> Self {
        Self::new()
    }
}

/// One coherent-enough diagnostic observation of [`AggregateTxCounters`].
///
/// The fields may straddle a live TX update, so interval qualification must
/// tolerate one aggregate crossing a sample boundary. Individual counters
/// remain exact monotonic observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateTxCounterSnapshot {
    pub network_single_mpdu_started: u32,
    pub network_single_legacy_rate: u32,
    pub network_single_block_ack_unavailable: u32,
    pub network_single_ht_needs_pair: u32,
    pub network_single_fresh_aggregate_capacity: u32,
    /// Maximum rejected Ethernet length since boot, not an interval delta.
    pub network_single_fresh_capacity_lifetime_max_ethernet_length: u32,
    pub aggregates_prepared: u32,
    pub aggregate_publications: u32,
    pub aggregates_completed: u32,
    pub subframes_acknowledged: u32,
    pub individual_retries: u32,
    pub hardware_timeouts: u32,
    pub collisions: u32,
    pub preparation_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub preparation_lifetime_max_micros: u32,
    pub publication_program_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub publication_program_lifetime_max_micros: u32,
    pub exchange_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub exchange_lifetime_max_micros: u32,
    pub single_publication_exchanges: u32,
    pub single_publication_exchange_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub single_publication_exchange_lifetime_max_micros: u32,
    pub retried_exchanges: u32,
    pub retried_exchange_publications: u32,
    pub retried_exchange_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub retried_exchange_lifetime_max_micros: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_irq_to_service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub tx_irq_to_service_lifetime_max_micros: u32,
    pub tx_publication_to_irq_samples: u32,
    pub tx_publication_to_irq_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub tx_publication_to_irq_lifetime_max_micros: u32,
    pub stopped_at_frame_limit: u32,
    pub stopped_at_capacity_limit: u32,
    pub stopped_on_empty_queue: u32,
    pub prepared_subframes: [u32; AGGREGATE_TX_HISTOGRAM_BUCKETS],
}

impl AggregateTxCounterSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            network_single_mpdu_started: self
                .network_single_mpdu_started
                .wrapping_sub(earlier.network_single_mpdu_started),
            network_single_legacy_rate: self
                .network_single_legacy_rate
                .wrapping_sub(earlier.network_single_legacy_rate),
            network_single_block_ack_unavailable: self
                .network_single_block_ack_unavailable
                .wrapping_sub(earlier.network_single_block_ack_unavailable),
            network_single_ht_needs_pair: self
                .network_single_ht_needs_pair
                .wrapping_sub(earlier.network_single_ht_needs_pair),
            network_single_fresh_aggregate_capacity: self
                .network_single_fresh_aggregate_capacity
                .wrapping_sub(earlier.network_single_fresh_aggregate_capacity),
            network_single_fresh_capacity_lifetime_max_ethernet_length: self
                .network_single_fresh_capacity_lifetime_max_ethernet_length,
            aggregates_prepared: self
                .aggregates_prepared
                .wrapping_sub(earlier.aggregates_prepared),
            aggregate_publications: self
                .aggregate_publications
                .wrapping_sub(earlier.aggregate_publications),
            aggregates_completed: self
                .aggregates_completed
                .wrapping_sub(earlier.aggregates_completed),
            subframes_acknowledged: self
                .subframes_acknowledged
                .wrapping_sub(earlier.subframes_acknowledged),
            individual_retries: self
                .individual_retries
                .wrapping_sub(earlier.individual_retries),
            hardware_timeouts: self
                .hardware_timeouts
                .wrapping_sub(earlier.hardware_timeouts),
            collisions: self.collisions.wrapping_sub(earlier.collisions),
            preparation_micros: self
                .preparation_micros
                .wrapping_sub(earlier.preparation_micros),
            preparation_lifetime_max_micros: self.preparation_lifetime_max_micros,
            publication_program_micros: self
                .publication_program_micros
                .wrapping_sub(earlier.publication_program_micros),
            publication_program_lifetime_max_micros: self.publication_program_lifetime_max_micros,
            exchange_micros: self.exchange_micros.wrapping_sub(earlier.exchange_micros),
            exchange_lifetime_max_micros: self.exchange_lifetime_max_micros,
            single_publication_exchanges: self
                .single_publication_exchanges
                .wrapping_sub(earlier.single_publication_exchanges),
            single_publication_exchange_micros: self
                .single_publication_exchange_micros
                .wrapping_sub(earlier.single_publication_exchange_micros),
            single_publication_exchange_lifetime_max_micros: self
                .single_publication_exchange_lifetime_max_micros,
            retried_exchanges: self
                .retried_exchanges
                .wrapping_sub(earlier.retried_exchanges),
            retried_exchange_publications: self
                .retried_exchange_publications
                .wrapping_sub(earlier.retried_exchange_publications),
            retried_exchange_micros: self
                .retried_exchange_micros
                .wrapping_sub(earlier.retried_exchange_micros),
            retried_exchange_lifetime_max_micros: self.retried_exchange_lifetime_max_micros,
            tx_irq_epochs: self.tx_irq_epochs.wrapping_sub(earlier.tx_irq_epochs),
            tx_irq_service_samples: self
                .tx_irq_service_samples
                .wrapping_sub(earlier.tx_irq_service_samples),
            tx_irq_clock_skew_samples: self
                .tx_irq_clock_skew_samples
                .wrapping_sub(earlier.tx_irq_clock_skew_samples),
            tx_irq_to_service_micros: self
                .tx_irq_to_service_micros
                .wrapping_sub(earlier.tx_irq_to_service_micros),
            tx_irq_to_service_lifetime_max_micros: self
                .tx_irq_to_service_lifetime_max_micros,
            tx_publication_to_irq_samples: self
                .tx_publication_to_irq_samples
                .wrapping_sub(earlier.tx_publication_to_irq_samples),
            tx_publication_to_irq_micros: self
                .tx_publication_to_irq_micros
                .wrapping_sub(earlier.tx_publication_to_irq_micros),
            tx_publication_to_irq_lifetime_max_micros: self
                .tx_publication_to_irq_lifetime_max_micros,
            stopped_at_frame_limit: self
                .stopped_at_frame_limit
                .wrapping_sub(earlier.stopped_at_frame_limit),
            stopped_at_capacity_limit: self
                .stopped_at_capacity_limit
                .wrapping_sub(earlier.stopped_at_capacity_limit),
            stopped_on_empty_queue: self
                .stopped_on_empty_queue
                .wrapping_sub(earlier.stopped_on_empty_queue),
            prepared_subframes: core::array::from_fn(|index| {
                self.prepared_subframes[index].wrapping_sub(earlier.prepared_subframes[index])
            }),
        }
    }

    pub fn prepared_subframe_total(&self) -> u32 {
        self.prepared_subframes
            .iter()
            .enumerate()
            .map(|(subframes, count)| (subframes as u32).saturating_mul(*count))
            .fold(0, u32::saturating_add)
    }

    pub fn prepared_in_range(&self, minimum: usize, maximum: usize) -> u32 {
        let start = minimum.max(1).min(AGGREGATE_TX_HISTOGRAM_BUCKETS);
        let end = maximum
            .saturating_add(1)
            .min(AGGREGATE_TX_HISTOGRAM_BUCKETS);
        self.prepared_subframes[start..end]
            .iter()
            .copied()
            .fold(0, u32::saturating_add)
    }

    pub fn minimum_prepared_subframes(&self) -> Option<u8> {
        self.prepared_subframes
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(subframes, count)| (*count != 0).then_some(subframes as u8))
    }

    pub fn maximum_prepared_subframes(&self) -> Option<u8> {
        self.prepared_subframes
            .iter()
            .enumerate()
            .skip(1)
            .rev()
            .find_map(|(subframes, count)| (*count != 0).then_some(subframes as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_preserve_distribution_and_timing_deltas() {
        let counters = AggregateTxCounters::new();
        let before = counters.snapshot();
        counters.observe(AggregateTxObservation::NetworkSingleMpdu {
            reason: NetworkSingleMpduReason::BlockAckUnavailable,
            ethernet_length: 42,
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
        assert_eq!(delta.exchange_micros, 100);
        assert_eq!(delta.exchange_lifetime_max_micros, 59);
        assert_eq!(delta.single_publication_exchanges, 1);
        assert_eq!(delta.single_publication_exchange_micros, 41);
        assert_eq!(delta.single_publication_exchange_lifetime_max_micros, 41);
        assert_eq!(delta.retried_exchanges, 1);
        assert_eq!(delta.retried_exchange_publications, 3);
        assert_eq!(delta.retried_exchange_micros, 59);
        assert_eq!(delta.retried_exchange_lifetime_max_micros, 59);
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
}
