//! Connected HT/HE aggregate TX owner for the one production Wi-Fi runner.
//!
//! The owner retains every pinned `embassy-net` lease referenced by DMA until
//! completion, queue detach, BlockAck processing and any retained aggregate
//! retry. It shares the ordinary descriptor, key token, sequence spaces,
//! contention state, power profile and clock through
//! [`Esp32s31SingleMpduTx`]; no parallel HIL TX state exists.

use core::{
    future::Future,
    mem,
    pin::Pin,
    sync::atomic::{AtomicU32, Ordering},
};

use open_esp_radio_embassy_net::{PinnedRadioRunner, PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi_mac::{
    irq::{MAC_INT_COLLISION, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT},
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeEdcaTxopLimit, HtAmpduTxConfig, LegacyTxQueue, TxCookie,
        TxPhyRate, TxSlotState,
    },
    tx_ampdu::{HtAmpduHardware, HtAmpduTxError, HtAmpduTxStorage},
    tx_runtime::{AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState},
};
use open_esp_radio_ieee80211::{
    data::DataHeControl,
    station::{STA_PROTECTED_QOS_ETHERNET_OVERHEAD, StationFrameError},
    station_power_save::StaPowerManagement,
};

use crate::{
    backend::Esp32s31NetworkTxService,
    connected_control::ConnectedControlTx,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    runner::{WifiControlProgress, WifiTxProgress, WifiTxWake},
    single_mpdu_tx::{
        ActionTxConfig, Esp32s31SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome,
    },
};

const AMPDU_ABORT_SETTLE_US: u64 = 16;
const DATA_TID: u8 = 0;

/// Finite aggregate publication policy installed after Association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateTxConfig {
    /// Independent A-MPDU schedule rate selected by rate control.
    pub rate: TxPhyRate,
    /// Maximum descriptors claimed for one aggregate.
    pub frame_limit: u8,
    /// Maximum aggregate publications, including the first one.
    pub attempt_limit: u8,
    /// Executor watchdog for each hardware publication.
    pub completion_timeout_us: u64,
    /// HE duration/APEP ceiling selected from negotiated EDCA policy.
    pub he_txop_limit: HeEdcaTxopLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxResetReason {
    CompletionInterruptWithoutState,
    TimeoutInterruptWithoutState,
    CollisionInterruptWithoutState,
    ConflictingInterruptEvents(u32),
    ExecutorDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxError {
    Busy,
    PeerDoesNotSupportQos,
    UnsupportedRate,
    InvalidFrameLimit {
        limit: u8,
        capacity: usize,
    },
    MissingQosSequence(u8),
    BufferSizeOverflow,
    DeadlineOverflow,
    DmaPrefixGeometry {
        encoded_offset: usize,
        metadata_size: usize,
    },
    Encode(StationFrameError),
    Aggregate(HtAmpduTxError),
    Retry(AmpduRetryError),
    Ordinary(SingleMpduTxError),
    RadioResetRequired(AggregateTxResetReason),
}

impl From<HtAmpduTxError> for AggregateTxError {
    fn from(error: HtAmpduTxError) -> Self {
        Self::Aggregate(error)
    }
}

impl From<AmpduRetryError> for AggregateTxError {
    fn from(error: AmpduRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<SingleMpduTxError> for AggregateTxError {
    fn from(error: SingleMpduTxError) -> Self {
        Self::Ordinary(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxOutcome {
    Complete {
        original_subframes: u8,
        aggregate_attempts: u8,
        acknowledged: u8,
        individual_retry: bool,
    },
    HardwareTimeout,
    Collision,
}

/// Number of histogram entries required for all legal A-MPDU sizes.
///
/// Index zero is deliberately unused; indexes `1..=32` are the number of
/// original MPDUs prepared for one aggregate exchange.
pub const AGGREGATE_TX_HISTOGRAM_BUCKETS: usize = 33;

/// Lock-free observations of the production connected TX owner.
///
/// The counters are diagnostic only and never participate in scheduling.
/// Relaxed atomics keep a HIL observer from adding synchronization to the
/// radio path it is measuring.
pub struct AggregateTxCounters {
    network_single_mpdu_started: AtomicU32,
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
    exchange_micros: AtomicU32,
    exchange_lifetime_max_micros: AtomicU32,
    stopped_at_frame_limit: AtomicU32,
    stopped_at_capacity_limit: AtomicU32,
    stopped_on_empty_queue: AtomicU32,
    prepared_subframes: [AtomicU32; AGGREGATE_TX_HISTOGRAM_BUCKETS],
}

impl AggregateTxCounters {
    pub const fn new() -> Self {
        Self {
            network_single_mpdu_started: AtomicU32::new(0),
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
            exchange_micros: AtomicU32::new(0),
            exchange_lifetime_max_micros: AtomicU32::new(0),
            stopped_at_frame_limit: AtomicU32::new(0),
            stopped_at_capacity_limit: AtomicU32::new(0),
            stopped_on_empty_queue: AtomicU32::new(0),
            prepared_subframes: [const { AtomicU32::new(0) }; AGGREGATE_TX_HISTOGRAM_BUCKETS],
        }
    }

    pub fn snapshot(&self) -> AggregateTxCounterSnapshot {
        AggregateTxCounterSnapshot {
            network_single_mpdu_started: self.network_single_mpdu_started.load(Ordering::Relaxed),
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
            stopped_at_frame_limit: self.stopped_at_frame_limit.load(Ordering::Relaxed),
            stopped_at_capacity_limit: self.stopped_at_capacity_limit.load(Ordering::Relaxed),
            stopped_on_empty_queue: self.stopped_on_empty_queue.load(Ordering::Relaxed),
            prepared_subframes: core::array::from_fn(|index| {
                self.prepared_subframes[index].load(Ordering::Relaxed)
            }),
        }
    }

    fn record_network_single_mpdu(&self) {
        self.network_single_mpdu_started
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_prepared(&self, subframes: u8, stop: AggregateBuildStop) {
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

    fn record_preparation_time(&self, micros: u64) {
        Self::record_time(
            &self.preparation_micros,
            &self.preparation_lifetime_max_micros,
            micros,
        );
    }

    fn record_publication(&self, program_micros: u64) {
        self.aggregate_publications.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.publication_program_micros,
            &self.publication_program_lifetime_max_micros,
            program_micros,
        );
    }

    fn record_complete(&self, acknowledged: u8, individual_retry: bool) {
        self.aggregates_completed.fetch_add(1, Ordering::Relaxed);
        self.subframes_acknowledged
            .fetch_add(u32::from(acknowledged), Ordering::Relaxed);
        if individual_retry {
            self.individual_retries.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_hardware_timeout(&self) {
        self.hardware_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    fn record_collision(&self) {
        self.collisions.fetch_add(1, Ordering::Relaxed);
    }

    fn record_exchange_time(&self, micros: u64) {
        Self::record_time(
            &self.exchange_micros,
            &self.exchange_lifetime_max_micros,
            micros,
        );
    }

    fn record_time(total: &AtomicU32, maximum: &AtomicU32, micros: u64) {
        let micros = u32::try_from(micros).unwrap_or(u32::MAX);
        total.fetch_add(micros, Ordering::Relaxed);
        maximum.fetch_max(micros, Ordering::Relaxed);
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

struct AggregateActive<const SLOTS: usize> {
    config: AmpduTxConfig,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    deadline_micros: u64,
    first_publication_micros: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AggregateBuildStop {
    FrameLimit,
    CapacityLimit,
    QueueEmpty,
}

enum ConnectedTxActive<const SLOTS: usize> {
    Idle,
    Ordinary,
    Aggregate(AggregateActive<SLOTS>),
}

/// Unique connected TX owner for ordinary frames and referenced A-MPDU.
pub struct Esp32s31ConnectedTx<
    'slot,
    'ampdu,
    'resources,
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> {
    ordinary: Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    ampdu: Pin<&'ampdu mut HtAmpduTxStorage<SLOTS, AMPDU_BUFFER_SIZE>>,
    frames: [Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>;
        SLOTS],
    held_frames: usize,
    cookie: Option<TxCookie>,
    block_ack_operational: [bool; 8],
    config: AggregateTxConfig,
    active: ConnectedTxActive<SLOTS>,
    last_aggregate_outcome: Option<AggregateTxOutcome>,
    counters: Option<&'ampdu AggregateTxCounters>,
}

impl<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        ordinary: Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
        mut ampdu: Pin<&'ampdu mut HtAmpduTxStorage<SLOTS, AMPDU_BUFFER_SIZE>>,
        config: AggregateTxConfig,
    ) -> Result<Self, AggregateTxError> {
        if SLOTS == 0
            || SLOTS > 32
            || config.frame_limit == 0
            || usize::from(config.frame_limit) > SLOTS
        {
            return Err(AggregateTxError::InvalidFrameLimit {
                limit: config.frame_limit,
                capacity: SLOTS,
            });
        }
        if !ordinary.config().peer_qos {
            return Err(AggregateTxError::PeerDoesNotSupportQos);
        }
        if config.attempt_limit == 0 {
            return Err(AmpduRetryError::ZeroAttemptLimit.into());
        }
        ampdu.as_mut().configure_max_aggregate_bytes(
            ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        )?;
        Ok(Self {
            ordinary,
            ampdu,
            frames: [const { None }; SLOTS],
            held_frames: 0,
            cookie: None,
            block_ack_operational: [false; 8],
            config,
            active: ConnectedTxActive::Idle,
            last_aggregate_outcome: None,
            counters: None,
        })
    }

    /// Attach optional production/HIL observations without changing TX
    /// scheduling or completion ownership.
    pub fn with_counters(mut self, counters: &'ampdu AggregateTxCounters) -> Self {
        self.counters = Some(counters);
        self
    }

    pub fn ordinary(&self) -> &Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE> {
        &self.ordinary
    }

    pub fn ordinary_mut(
        &mut self,
    ) -> &mut Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE> {
        &mut self.ordinary
    }

    pub fn set_block_ack_operational(&mut self, tid: u8, operational: bool) {
        if let Some(entry) = self.block_ack_operational.get_mut(usize::from(tid)) {
            *entry = operational;
        }
    }

    pub fn block_ack_operational(&self, tid: u8) -> bool {
        self.block_ack_operational
            .get(usize::from(tid))
            .copied()
            .unwrap_or(false)
    }

    pub fn take_last_aggregate_outcome(&mut self) -> Option<AggregateTxOutcome> {
        self.last_aggregate_outcome.take()
    }

    pub fn take_last_ordinary_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub fn active(&self) -> bool {
        !matches!(self.active, ConnectedTxActive::Idle)
    }

    pub async fn wait_deadline(&mut self) {
        match &self.active {
            ConnectedTxActive::Aggregate(active) => {
                self.ordinary
                    .wait_until_micros(active.deadline_micros)
                    .await;
            }
            ConnectedTxActive::Idle | ConnectedTxActive::Ordinary => {
                self.ordinary.wait_deadline().await;
            }
        }
    }

    pub fn start_network<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if self.active() {
            return Err(AggregateTxError::Busy);
        }
        let aggregate_rate = !matches!(self.config.rate, TxPhyRate::Legacy(_));
        let ht_requires_pair = matches!(self.config.rate, TxPhyRate::Ht(_));
        if !aggregate_rate
            || !self.block_ack_operational(DATA_TID)
            || (ht_requires_pair && network.tx_queue_len() == 0)
        {
            let progress = self.ordinary.start(hardware, first.ethernet())?;
            drop(first);
            if let Some(counters) = self.counters {
                counters.record_network_single_mpdu();
            }
            self.active = ConnectedTxActive::Ordinary;
            return Ok(progress);
        }

        let preparation_started = self.counters.map(|_| self.ordinary.now_micros());
        self.prepare_aggregate(first, network)?;
        if let (Some(counters), Some(started)) = (self.counters, preparation_started) {
            counters.record_preparation_time(self.ordinary.now_micros().wrapping_sub(started));
        }
        self.publish_initial(hardware)
    }

    pub async fn service<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        match active {
            ConnectedTxActive::Idle => Err(AggregateTxError::Busy),
            ConnectedTxActive::Ordinary => {
                let progress = self.ordinary.service(hardware, wake).await?;
                if progress == WifiTxProgress::Pending {
                    self.active = ConnectedTxActive::Ordinary;
                }
                Ok(progress)
            }
            ConnectedTxActive::Aggregate(active) => {
                self.service_aggregate(hardware, wake, active).await
            }
        }
    }

    fn prepare_aggregate(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<(), AggregateTxError> {
        let first_sequence = self
            .ordinary
            .peek_qos_sequence(DATA_TID)
            .ok_or(AggregateTxError::MissingQosSequence(DATA_TID))?;
        let cookie = self.ampdu.as_mut().begin()?;
        self.cookie = Some(cookie);

        let result = self.prepare_reserved(first, network, first_sequence, cookie);
        if result.is_err() {
            self.cancel_prepared();
        }
        result
    }

    fn prepare_reserved(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        first_sequence: u16,
        cookie: TxCookie,
    ) -> Result<(), AggregateTxError> {
        self.push_frame(first)?;

        let build_stop = loop {
            if self.held_frames >= usize::from(self.config.frame_limit) {
                break AggregateBuildStop::FrameLimit;
            }
            if !self.can_push(FRAME_CAPACITY)? {
                break AggregateBuildStop::CapacityLimit;
            }
            let Some(frame) = network.try_receive_tx() else {
                break AggregateBuildStop::QueueEmpty;
            };
            self.push_frame(frame)?;
        };

        let aggregate = self.ampdu.prepared_aggregate(cookie)?;
        let retry = AmpduRetryState::<SLOTS>::new(
            first_sequence,
            aggregate.subframes,
            AmpduRetryPolicy {
                attempt_limit: self.config.attempt_limit,
                retain_single_mpdu: matches!(self.config.rate, TxPhyRate::He(_)),
            },
        )?;
        let config = self.publication_config(aggregate.bytes, aggregate.subframes)?;
        self.active = ConnectedTxActive::Aggregate(AggregateActive {
            config,
            retry,
            original_subframes: aggregate.subframes,
            deadline_micros: 0,
            first_publication_micros: None,
        });
        if let Some(counters) = self.counters {
            counters.record_prepared(aggregate.subframes, build_stop);
        }
        Ok(())
    }

    fn can_push(&self, ethernet_length: usize) -> Result<bool, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = HEADROOM + FRAME_CAPACITY + TRAILER;
        let hardware_mic_length = crate::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.can_commit_referenced_he_frame_with_txop(
                cookie,
                frame_length,
                hardware_mic_length,
                rate,
                self.ordinary.policy().ht_ampdu().density(),
                self.config.he_txop_limit,
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn push_frame(
        &mut self,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<(), AggregateTxError> {
        if self.held_frames >= SLOTS || !self.can_push(frame.ethernet_length())? {
            return Err(HtAmpduTxError::AggregateFull.into());
        }
        let metadata = self
            .ordinary
            .take_protected_metadata(DATA_TID)
            .ok_or(AggregateTxError::MissingQosSequence(DATA_TID))?;
        let ethernet_offset = frame.ethernet_offset();
        let ethernet_length = frame.ethernet_length();
        let encoded = metadata
            .encode_in_place(
                frame.storage_mut(),
                ethernet_offset,
                ethernet_length,
                DataHeControl::Disabled,
            )
            .map_err(AggregateTxError::Encode)?;
        let metadata_size = open_esp_radio_esp32s31_wifi_mac::tx_ampdu::TX_AMPDU_METADATA_SIZE;
        let dma_offset = encoded.offset.checked_sub(metadata_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        if dma_offset & 3 != 0 {
            return Err(AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            });
        }
        let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
        let hardware_mic_length = crate::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        // SAFETY: `frame` moves into `self.frames` immediately after the
        // commit and stays there through every exposed hardware state.
        unsafe {
            match self.config.rate {
                TxPhyRate::Ht(rate) => self.ampdu.as_mut().commit_referenced_ht_frame(
                    cookie,
                    &mut frame.storage_mut()[dma_offset..],
                    encoded.length,
                    hardware_mic_length,
                    0,
                    rate,
                )?,
                TxPhyRate::He(rate) => self.ampdu.as_mut().commit_referenced_he_frame_with_txop(
                    cookie,
                    &mut frame.storage_mut()[dma_offset..],
                    encoded.length,
                    hardware_mic_length,
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    self.config.he_txop_limit,
                )?,
                TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
            }
        }
        self.frames[self.held_frames] = Some(frame);
        self.held_frames += 1;
        Ok(())
    }

    fn publication_config(
        &mut self,
        aggregate_length: u16,
        subframes: u8,
    ) -> Result<AmpduTxConfig, AggregateTxError> {
        let queue = LegacyTxQueue::BestEffort;
        let key = self.ordinary.hardware_key_selector();
        let (contention, contention_window) =
            self.ordinary.ordinary_mut().contention_publication(queue);
        match self.config.rate {
            TxPhyRate::Ht(rate) => {
                let mut config = HtAmpduTxConfig::new(rate, aggregate_length, subframes)
                    .ok_or(AggregateTxError::BufferSizeOverflow)?;
                let data_power = self
                    .ordinary
                    .ordinary()
                    .power()
                    .power_pair(rate.power_lookup_code());
                let rts_power = self
                    .ordinary
                    .ordinary()
                    .power()
                    .power_pair(rate.vendor_rts_rate().code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.rts_power_primary = rts_power.primary as u8;
                config.rts_power_alternate = rts_power.alternate as u8;
                config.protection_spacing = self.ordinary.policy().ht_ampdu().protection_spacing();
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = queue.vendor_data_scheduler_priority();
                config.pti = queue.vendor_data_packet_priority();
                config.pti_count = 1;
                config.hardware_key_selector = key;
                Ok(AmpduTxConfig::Ht(config))
            }
            TxPhyRate::He(rate) => {
                let mut config = HeAmpduTxConfig::new_with_txop(
                    rate,
                    self.ordinary.policy().he_bss_color(),
                    aggregate_length,
                    subframes,
                    self.ordinary.policy().ht_ampdu().density(),
                    self.config.he_txop_limit,
                )
                .ok_or(AggregateTxError::BufferSizeOverflow)?;
                let data_power = self
                    .ordinary
                    .ordinary()
                    .power()
                    .power_pair(rate.power_lookup_code());
                let rts_power = self
                    .ordinary
                    .ordinary()
                    .power()
                    .power_pair(rate.vendor_rts_rate().code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.rts_power_primary = rts_power.primary as u8;
                config.rts_power_alternate = rts_power.alternate as u8;
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = queue.vendor_data_scheduler_priority();
                config.pti = queue.vendor_data_packet_priority();
                config.pti_count = 1;
                config.hardware_key_selector = key;
                Ok(AmpduTxConfig::He(config))
            }
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn publish_initial<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        let ConnectedTxActive::Aggregate(mut active) = active else {
            return Err(AggregateTxError::Busy);
        };
        if let Err(error) = self.publish_attempt(hardware, &mut active) {
            self.cancel_prepared();
            return Err(error);
        }
        self.last_aggregate_outcome = None;
        self.active = ConnectedTxActive::Aggregate(active);
        Ok(WifiTxProgress::Pending)
    }

    fn publish_attempt<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        active: &mut AggregateActive<SLOTS>,
    ) -> Result<(), AggregateTxError> {
        let publication_started = self.ordinary.now_micros();
        let deadline = publication_started
            .checked_add(self.config.completion_timeout_us)
            .ok_or(AggregateTxError::DeadlineOverflow)?;
        match active.config {
            AmpduTxConfig::Ht(config) => self.ampdu.as_mut().submit(
                hardware,
                self.cookie.ok_or(AggregateTxError::Busy)?,
                LegacyTxQueue::BestEffort,
                config,
            )?,
            AmpduTxConfig::He(config) => self.ampdu.as_mut().submit_he(
                hardware,
                self.cookie.ok_or(AggregateTxError::Busy)?,
                LegacyTxQueue::BestEffort,
                config,
            )?,
        }
        if let Some(counters) = self.counters {
            let publication_finished = self.ordinary.now_micros();
            if active.first_publication_micros.is_none() {
                active.first_publication_micros = Some(publication_started);
            }
            counters.record_publication(publication_finished.wrapping_sub(publication_started));
        }
        active.deadline_micros = deadline;
        Ok(())
    }

    async fn service_aggregate<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        mut active: AggregateActive<SLOTS>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let interrupt_events = match wake {
            WifiTxWake::Interrupt { events } => events,
            WifiTxWake::Deadline => 0,
        };
        let tx_events =
            interrupt_events & (MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION);
        if tx_events.count_ones() > 1 {
            return self.reset_required(AggregateTxResetReason::ConflictingInterruptEvents(
                tx_events,
            ));
        }

        if let Some(completion) = self.ampdu.as_mut().acknowledge_completion(hardware)? {
            let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
            self.ampdu.as_mut().detach_completed(hardware, cookie)?;
            let current_subframes = self.ampdu.frame_count();
            let decision = active.retry.observe(completion, current_subframes)?;
            if let AmpduRetryDecision::RetainAggregate { retry_mask } = decision {
                let aggregate = self
                    .ampdu
                    .as_mut()
                    .retain_for_ampdu_retry(cookie, retry_mask)?;
                self.ordinary
                    .ordinary_mut()
                    .record_retry_failure(LegacyTxQueue::BestEffort);
                let (_, contention_window) = self
                    .ordinary
                    .ordinary_mut()
                    .contention_publication(LegacyTxQueue::BestEffort);
                active.config.update_retained_retry(
                    aggregate.bytes,
                    aggregate.subframes,
                    contention_window,
                );
                if let Err(error) = self.publish_attempt(hardware, &mut active) {
                    self.cancel_prepared();
                    return Err(error);
                }
                self.active = ConnectedTxActive::Aggregate(active);
                return Ok(WifiTxProgress::Pending);
            }

            let retry_mask = decision.retry_mask();
            let missing = decision.missing();
            if missing == 0 {
                self.ordinary
                    .ordinary_mut()
                    .record_success(LegacyTxQueue::BestEffort);
            } else {
                self.ordinary
                    .ordinary_mut()
                    .reset_terminal_exchange(LegacyTxQueue::BestEffort);
            }

            let individual_retry = matches!(active.config, AmpduTxConfig::Ht(_))
                && missing == 1
                && active.retry.aggregate_attempts() < self.config.attempt_limit;
            if individual_retry {
                let index = retry_mask.trailing_zeros() as u8;
                let (frame_length, hardware_mic_length) = {
                    let (encoded, mic) = self.ampdu.completed_frame(cookie, index)?;
                    (self.ordinary.copy_encoded_retry(encoded)?, usize::from(mic))
                };
                self.release_completed()?;
                let progress = self.ordinary.start_prepared_encoded_retry(
                    hardware,
                    frame_length,
                    hardware_mic_length,
                    active.config.rate(),
                )?;
                self.last_aggregate_outcome = Some(AggregateTxOutcome::Complete {
                    original_subframes: active.original_subframes,
                    aggregate_attempts: active.retry.aggregate_attempts(),
                    acknowledged: active.retry.acknowledged(),
                    individual_retry: true,
                });
                if let Some(counters) = self.counters {
                    counters.record_complete(active.retry.acknowledged(), true);
                    Self::record_exchange_time(counters, &active, self.ordinary.now_micros());
                }
                self.active = ConnectedTxActive::Ordinary;
                return Ok(progress);
            }

            self.release_completed()?;
            self.last_aggregate_outcome = Some(AggregateTxOutcome::Complete {
                original_subframes: active.original_subframes,
                aggregate_attempts: active.retry.aggregate_attempts(),
                acknowledged: active.retry.acknowledged(),
                individual_retry: false,
            });
            if let Some(counters) = self.counters {
                counters.record_complete(active.retry.acknowledged(), false);
                Self::record_exchange_time(counters, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        if tx_events == MAC_INT_TX_COMPLETE {
            return self.reset_required(AggregateTxResetReason::CompletionInterruptWithoutState);
        }
        if tx_events == MAC_INT_TX_TIMEOUT || matches!(wake, WifiTxWake::Deadline) {
            let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
            if !self.ampdu.as_mut().begin_timeout_abort(hardware, cookie)? {
                return self.reset_required(if matches!(wake, WifiTxWake::Deadline) {
                    AggregateTxResetReason::ExecutorDeadline
                } else {
                    AggregateTxResetReason::TimeoutInterruptWithoutState
                });
            }
            self.ordinary
                .ordinary_mut()
                .after_micros(AMPDU_ABORT_SETTLE_US)
                .await;
            self.ampdu.as_mut().finish_timeout_abort(hardware, cookie)?;
            self.release_frames();
            self.cookie = None;
            self.ordinary
                .ordinary_mut()
                .reset_terminal_exchange(LegacyTxQueue::BestEffort);
            self.last_aggregate_outcome = Some(AggregateTxOutcome::HardwareTimeout);
            if let Some(counters) = self.counters {
                counters.record_hardware_timeout();
                Self::record_exchange_time(counters, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }
        if tx_events == MAC_INT_COLLISION {
            let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
            if !self.ampdu.as_mut().abort_collision(hardware, cookie)? {
                return self.reset_required(AggregateTxResetReason::CollisionInterruptWithoutState);
            }
            self.release_frames();
            self.cookie = None;
            self.ordinary
                .ordinary_mut()
                .reset_terminal_exchange(LegacyTxQueue::BestEffort);
            self.last_aggregate_outcome = Some(AggregateTxOutcome::Collision);
            if let Some(counters) = self.counters {
                counters.record_collision();
                Self::record_exchange_time(counters, &active, self.ordinary.now_micros());
            }
            return Ok(WifiTxProgress::Complete);
        }

        self.active = ConnectedTxActive::Aggregate(active);
        Ok(WifiTxProgress::Pending)
    }

    fn record_exchange_time(
        counters: &AggregateTxCounters,
        active: &AggregateActive<SLOTS>,
        finished_micros: u64,
    ) {
        if let Some(started_micros) = active.first_publication_micros {
            counters.record_exchange_time(finished_micros.wrapping_sub(started_micros));
        }
    }

    fn cancel_prepared(&mut self) {
        if let Some(cookie) = self.cookie.take() {
            let _ = self.ampdu.as_mut().cancel(cookie);
        }
        self.release_frames();
        self.active = ConnectedTxActive::Idle;
    }

    fn release_completed(&mut self) -> Result<(), AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
        self.ampdu.as_mut().release_completed(cookie)?;
        self.cookie = None;
        self.release_frames();
        Ok(())
    }

    fn release_frames(&mut self) {
        for frame in &mut self.frames[..self.held_frames] {
            drop(frame.take());
        }
        self.held_frames = 0;
    }

    fn forget_frames(&mut self) {
        for frame in &mut self.frames[..self.held_frames] {
            if let Some(frame) = frame.take() {
                mem::forget(frame);
            }
        }
        self.held_frames = 0;
    }

    fn reset_required(
        &mut self,
        reason: AggregateTxResetReason,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::Busy)?;
        self.ampdu.as_mut().require_reset(cookie)?;
        self.forget_frames();
        Err(AggregateTxError::RadioResetRequired(reason))
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
        task::{Context, Waker},
    };

    use open_esp_radio_embassy_net::{
        Driver as _, NoopRawMutex, PinnedDevice, PinnedResources, PinnedTxPool, TxToken as _,
    };
    use open_esp_radio_esp32s31_pac::{
        MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
        MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionRegisters, MacHtTxProgram,
        MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionRegisters,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
        tx::{HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate, TxSlot},
        tx_runtime::StaTxRuntimePolicy,
    };
    use open_esp_radio_ieee80211::station::{
        STA_PROTECTED_QOS_ETHERNET_HEADROOM, StaTxSequenceCounters,
    };

    use crate::{
        ordinary_tx::{WifiTxPowerPair, WifiTxResources},
        single_mpdu_tx::{ConnectedTxHandoff, SingleMpduTxConfig},
    };

    use super::*;

    const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
    const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
    const TEST_FRAME_CAPACITY: usize = 64;
    const TEST_HEADROOM: usize = open_esp_radio_esp32s31_wifi_mac::tx_ampdu::TX_AMPDU_METADATA_SIZE
        + STA_PROTECTED_QOS_ETHERNET_HEADROOM;
    const TEST_TRAILER: usize = 12;
    const TEST_QUEUE_DEPTH: usize = 3;
    const TEST_SLOTS: usize = 3;
    const TEST_BUFFER_SIZE: usize = 256;
    const TEST_RATE: HtRate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz20,
    );

    type Resources = PinnedResources<
        NoopRawMutex,
        TEST_FRAME_CAPACITY,
        TEST_HEADROOM,
        TEST_TRAILER,
        TEST_QUEUE_DEPTH,
    >;
    type Pool = PinnedTxPool<TEST_FRAME_CAPACITY, TEST_HEADROOM, TEST_TRAILER, TEST_QUEUE_DEPTH>;
    type Device = PinnedDevice<
        'static,
        NoopRawMutex,
        TEST_FRAME_CAPACITY,
        TEST_HEADROOM,
        TEST_TRAILER,
        TEST_QUEUE_DEPTH,
    >;

    #[derive(Default)]
    struct Hardware {
        ht_publications: usize,
        ordinary_completion: Option<MacTxCompletionRegisters>,
        aggregate_completion: Option<MacHtAmpduCompletionRegisters>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, _index: u8) {}
    }

    impl open_esp_radio_esp32s31_wifi_mac::tx::TxHardware for Hardware {
        fn tx_descriptor_address(&self, cpu_address: u32) -> u32 {
            0x2f00_1000 | (cpu_address & 0x0ffc)
        }

        fn prepare_legacy_tx(&mut self, _queue: u8, _program: MacLegacyTxProgram) -> bool {
            true
        }

        fn start_legacy_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn prepare_ht_tx(&mut self, _queue: u8, _program: MacHtTxProgram) -> bool {
            self.ht_publications += 1;
            true
        }

        fn start_ht_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn prepare_he_tx(&mut self, _queue: u8, _program: MacHeTxProgram) -> bool {
            false
        }

        fn start_he_tx(&mut self, _queue: u8, _plcp0: u32) {}

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
            self.ordinary_completion.take()
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            true
        }

        fn finish_tx_timeout_abort(&mut self, _queue: u8) -> Option<bool> {
            Some(false)
        }

        fn abort_tx_collision(&mut self, _queue: u8) -> bool {
            true
        }

        fn detach_completed_tx(&mut self, _queue: u8) -> bool {
            true
        }
    }

    impl HtAmpduHardware for Hardware {
        fn take_ht_ampdu_completion(
            &mut self,
            _queue: u8,
        ) -> Option<MacHtAmpduCompletionRegisters> {
            self.aggregate_completion.take()
        }

        fn prepare_he_trigger_based_queue(
            &mut self,
            _policy: MacHeTbTidLimit,
            _reservation: MacHeTbLinkReservation,
            _tid: MacHeTid,
            _mpdu_lengths: &[u16],
            _queued_msdu_bytes: u32,
        ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
            unreachable!("HT tests never publish a trigger-based HE queue")
        }

        fn clear_he_trigger_based_queue(&mut self, _reservation: MacHeTbLinkReservation) {}
    }

    struct Power;

    impl WifiTxPowerProfile for Power {
        fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
            WifiTxPowerPair {
                primary: 5,
                alternate: 6,
            }
        }
    }

    #[derive(Default)]
    struct Timer {
        now: u64,
    }

    impl WifiTxTimer for Timer {
        fn now_micros(&self) -> u64 {
            self.now
        }

        fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
            self.now = deadline_micros;
            ready(())
        }

        fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
            self.now += micros;
            ready(())
        }
    }

    fn context() -> Context<'static> {
        Context::from_waker(Waker::noop())
    }

    fn send_frame(device: &mut Device, marker: u8) {
        device
            .transmit(&mut context())
            .expect("free pinned network slot")
            .consume(17, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }

    fn aggregate_completion(starting_sequence: u16, bitmap: u64) -> MacHtAmpduCompletionRegisters {
        MacHtAmpduCompletionRegisters {
            tx: MacTxCompletionRegisters {
                aux_a: 0,
                aux_b: 0,
                aux_c: 0,
                primary: 0,
                alternate: 0,
                trigger_flow: false,
            },
            block_ack_control_and_sequence: u32::from(starting_sequence & 0x0fff) << 4,
            block_ack_bitmap_low: bitmap as u32,
            block_ack_bitmap_high: (bitmap >> 32) as u32,
        }
    }

    fn make_ordinary<'a>(
        slot: Pin<&'a mut TxSlot<TEST_BUFFER_SIZE>>,
        hardware: &mut Hardware,
    ) -> Esp32s31SingleMpduTx<'a, Power, fn() -> u32, Timer, TEST_BUFFER_SIZE> {
        fn entropy() -> u32 {
            0x1234_5678
        }

        let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
        Esp32s31SingleMpduTx::new(
            WifiTxResources {
                slot,
                policy: StaTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy,
                timer: Timer::default(),
            },
            ConnectedTxHandoff {
                key,
                sequences: StaTxSequenceCounters::new(7),
                config: SingleMpduTxConfig {
                    station_address: STATION,
                    bssid: BSSID,
                    peer_qos: true,
                    rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    attempt_limit: 2,
                    completion_timeout_us: 250_000,
                },
            },
        )
    }

    fn make_network() -> (
        Device,
        open_esp_radio_embassy_net::PinnedRadioRunner<
            'static,
            NoopRawMutex,
            TEST_FRAME_CAPACITY,
            TEST_HEADROOM,
            TEST_TRAILER,
            TEST_QUEUE_DEPTH,
        >,
    ) {
        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
        resources.split(pool, STATION)
    }

    #[test]
    fn aggregate_counters_preserve_distribution_and_timing_deltas() {
        let counters = AggregateTxCounters::new();
        let before = counters.snapshot();
        counters.record_network_single_mpdu();
        counters.record_prepared(2, AggregateBuildStop::QueueEmpty);
        counters.record_prepared(32, AggregateBuildStop::FrameLimit);
        counters.record_publication(3);
        counters.record_publication(5);
        counters.record_complete(31, true);
        counters.record_hardware_timeout();
        counters.record_exchange_time(41);
        counters.record_exchange_time(59);
        counters.record_preparation_time(7);
        counters.record_preparation_time(11);

        let delta = counters.snapshot().wrapping_delta_since(before);
        assert_eq!(delta.network_single_mpdu_started, 1);
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
        assert_eq!(delta.stopped_at_frame_limit, 1);
        assert_eq!(delta.stopped_at_capacity_limit, 0);
        assert_eq!(delta.stopped_on_empty_queue, 1);
    }

    #[test]
    fn block_ack_completion_releases_all_referenced_network_leases() {
        let (mut device, network) = make_network();
        send_frame(&mut device, 1);
        send_frame(&mut device, 2);
        let first = network.try_receive_tx().unwrap();
        let mut hardware = Hardware::default();
        let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new());
        let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
        let mut ampdu_backing = HtAmpduTxStorage::<TEST_SLOTS, 0>::new();
        // SAFETY: the stack allocation is not moved while `tx` owns the pin.
        let ampdu = unsafe { Pin::new_unchecked(&mut ampdu_backing) };
        let mut tx = Esp32s31ConnectedTx::new(
            ordinary,
            ampdu,
            AggregateTxConfig {
                rate: TxPhyRate::Ht(TEST_RATE),
                frame_limit: TEST_SLOTS as u8,
                attempt_limit: 2,
                completion_timeout_us: 250_000,
                he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            },
        )
        .unwrap();
        tx.set_block_ack_operational(0, true);

        assert_eq!(
            tx.start_network(&mut hardware, first, &network),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(hardware.ht_publications, 1);
        // Only the unused third slot can return to the producer while the two
        // submitted frames remain radio-owned.
        send_frame(&mut device, 3);
        assert!(device.transmit(&mut context()).is_none());

        hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
        assert_eq!(
            embassy_futures::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert_eq!(
            tx.take_last_aggregate_outcome(),
            Some(AggregateTxOutcome::Complete {
                original_subframes: 2,
                aggregate_attempts: 1,
                acknowledged: 2,
                individual_retry: false,
            })
        );
        send_frame(&mut device, 4);
        send_frame(&mut device, 5);
        assert!(device.transmit(&mut context()).is_none());
        assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
        for _ in 0..TEST_QUEUE_DEPTH {
            drop(network.try_receive_tx().unwrap());
        }
    }

    #[test]
    fn partial_block_ack_retains_missing_frames_across_one_republication() {
        let (mut device, network) = make_network();
        for marker in 1..=3 {
            send_frame(&mut device, marker);
        }
        let first = network.try_receive_tx().unwrap();
        let mut hardware = Hardware::default();
        let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new());
        let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
        let mut ampdu_backing = HtAmpduTxStorage::<TEST_SLOTS, 0>::new();
        // SAFETY: the stack allocation is not moved while `tx` owns the pin.
        let ampdu = unsafe { Pin::new_unchecked(&mut ampdu_backing) };
        let mut tx = Esp32s31ConnectedTx::new(
            ordinary,
            ampdu,
            AggregateTxConfig {
                rate: TxPhyRate::Ht(TEST_RATE),
                frame_limit: TEST_SLOTS as u8,
                attempt_limit: 2,
                completion_timeout_us: 250_000,
                he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            },
        )
        .unwrap();
        tx.set_block_ack_operational(0, true);
        assert_eq!(
            tx.start_network(&mut hardware, first, &network),
            Ok(WifiTxProgress::Pending)
        );

        hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
        assert_eq!(
            embassy_futures::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(hardware.ht_publications, 2);
        assert!(device.transmit(&mut context()).is_none());

        hardware.aggregate_completion = Some(aggregate_completion(8, 0b11));
        assert_eq!(
            embassy_futures::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert_eq!(
            tx.take_last_aggregate_outcome(),
            Some(AggregateTxOutcome::Complete {
                original_subframes: 3,
                aggregate_attempts: 2,
                acknowledged: 3,
                individual_retry: false,
            })
        );
        send_frame(&mut device, 4);
        send_frame(&mut device, 5);
        send_frame(&mut device, 6);
        assert!(device.transmit(&mut context()).is_none());
        assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
        for _ in 0..TEST_QUEUE_DEPTH {
            drop(network.try_receive_tx().unwrap());
        }
    }

    #[test]
    fn one_missing_ht_mpdu_moves_to_ordinary_retry_without_new_sequence_or_pn() {
        let (mut device, network) = make_network();
        send_frame(&mut device, 1);
        send_frame(&mut device, 2);
        let first = network.try_receive_tx().unwrap();
        let mut hardware = Hardware::default();
        let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new());
        let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
        let mut ampdu_backing = HtAmpduTxStorage::<TEST_SLOTS, 0>::new();
        // SAFETY: the stack allocation is not moved while `tx` owns the pin.
        let ampdu = unsafe { Pin::new_unchecked(&mut ampdu_backing) };
        let mut tx = Esp32s31ConnectedTx::new(
            ordinary,
            ampdu,
            AggregateTxConfig {
                rate: TxPhyRate::Ht(TEST_RATE),
                frame_limit: TEST_SLOTS as u8,
                attempt_limit: 2,
                completion_timeout_us: 250_000,
                he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            },
        )
        .unwrap();
        tx.set_block_ack_operational(0, true);
        assert_eq!(
            tx.start_network(&mut hardware, first, &network),
            Ok(WifiTxProgress::Pending)
        );

        hardware.aggregate_completion = Some(aggregate_completion(7, 0b01));
        assert_eq!(
            embassy_futures::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(
            tx.take_last_aggregate_outcome(),
            Some(AggregateTxOutcome::Complete {
                original_subframes: 2,
                aggregate_attempts: 1,
                acknowledged: 1,
                individual_retry: true,
            })
        );
        assert_eq!(tx.ordinary().peek_qos_sequence(0), Some(9));

        // The individual retry uses the private ordinary descriptor. Both
        // referenced network allocations have already crossed the safe
        // detach/release edge and can be filled again while it is in flight.
        send_frame(&mut device, 3);
        send_frame(&mut device, 4);
        send_frame(&mut device, 5);
        assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);

        hardware.ordinary_completion = Some(MacTxCompletionRegisters {
            aux_a: 0,
            aux_b: 0,
            aux_c: 0,
            primary: 0,
            alternate: 0,
            trigger_flow: false,
        });
        assert_eq!(
            embassy_futures::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert!(matches!(
            tx.take_last_ordinary_outcome(),
            Some(SingleMpduTxOutcome::Success(_))
        ));
        for _ in 0..TEST_QUEUE_DEPTH {
            drop(network.try_receive_tx().unwrap());
        }
    }
}

impl<
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedControlTx
    for Esp32s31ConnectedTx<
        '_,
        '_,
        '_,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.take_last_ordinary_outcome()
    }

    fn now_micros(&self) -> u64 {
        self.ordinary.now_micros()
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.ordinary.wait_until_micros(deadline_micros)
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        self.ordinary.peek_qos_sequence(tid)
    }

    fn start_action<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiControlProgress, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_action(hardware, body, config)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(WifiControlProgress::TxPending)
    }

    fn start_power_management_null<H: open_esp_radio_esp32s31_wifi_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiControlProgress, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary
            .start_power_management_null(hardware, power_management)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(WifiControlProgress::TxPending)
    }

    fn set_tx_block_ack_operational(&mut self, tid: u8, operational: bool) {
        self.set_block_ack_operational(tid, operational);
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    M,
    H,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> Esp32s31NetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    H: HtAmpduHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    type Error = AggregateTxError;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move { self.start_network(hardware, frame, network) }
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        Esp32s31ConnectedTx::wait_deadline(self)
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        Esp32s31ConnectedTx::service(self, hardware, wake)
    }
}

impl<
    M: RawMutex,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> Drop
    for Esp32s31ConnectedTx<
        '_,
        '_,
        '_,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn drop(&mut self) {
        match self.ampdu.state() {
            TxSlotState::Free => self.release_frames(),
            TxSlotState::Reserved => {
                if self
                    .cookie
                    .is_some_and(|cookie| self.ampdu.as_mut().cancel(cookie).is_ok())
                {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::Completed => {
                if self
                    .cookie
                    .is_some_and(|cookie| self.ampdu.as_mut().release_completed(cookie).is_ok())
                {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::HardwareOwned | TxSlotState::ResetRequired => self.forget_frames(),
        }
    }
}
