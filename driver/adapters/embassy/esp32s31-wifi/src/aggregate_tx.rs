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
    ops::{Deref, DerefMut},
};

use open_esp_radio_embassy_net::{PinnedTxConsumer, PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::StaPairwiseCcmpSlot,
    irq::{MAC_INT_COLLISION, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT},
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeEdcaTxopLimit, HtAmpduTxConfig, LegacyTxQueue, TxCookie,
        TxPhyRate, TxSlotState,
    },
    tx_ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
        HtAmpduHardware, HtAmpduTxError, HtAmpduTxResources, RetainedAmpduDmaStorage,
        RetainedDmaAmpduTx,
    },
    tx_runtime::{AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState},
};
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::{
    ActionTxConfig, ConnectedTxHandoff, Esp32s31SingleMpduTx, SingleMpduTxError,
    SingleMpduTxOutcome, WifiTxResources,
};
use open_esp_radio_ieee80211::{
    data::DataHeControl,
    station::{
        STA_PROTECTED_QOS_ETHERNET_OVERHEAD, StaTxSequenceCounters, StationFrameError,
        sta_protected_amsdu_pair_frame_length,
    },
    station_power_save::StaPowerManagement,
};
use open_esp_radio_wifi_softmac::{
    MacAmpduTxResult, MacAmpduTxStatus, MacTxQueueState, MacTxResult,
};

use crate::{
    aggregate_tx_observer::{
        AggregateBuildStop, AggregateTxObservation, AggregateTxObserver, NetworkSingleMpduReason,
    },
    connected_control::{ConnectedControlTimer, ConnectedControlTx},
    connected_runner::{WifiControlProgress, WifiTxProgress, WifiTxWake},
    connected_services::Esp32s31NetworkTxService,
};

const AMPDU_ABORT_SETTLE_US: u64 = 16;
const DATA_TID: u8 = 0;

/// Peer-independent TX resources returned at the connected-to-disconnected
/// station boundary.
pub struct Esp32s31ConnectedTxTeardownParts<R, A> {
    pub resources: R,
    pub pairwise_key: StaPairwiseCcmpSlot,
    pub sequences: StaTxSequenceCounters,
    pub aggregate: A,
}

/// Descriptor arenas owned by one connected aggregate-TX scheduler.
///
/// `primary` is the only arena which may become hardware-owned. `standby`,
/// when present, may be filled while `primary` is in flight but remains in the
/// software-owned `Reserved` state until the outer scheduler admits its
/// publication. The separate retention arenas hold the comparatively large
/// network-lease and descriptor-identity tables. Embedded composition roots
/// should allocate those arenas statically so this movable resource handle
/// remains small across async boundaries.
pub struct AggregateTxResources<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
{
    primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
    primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    standby: Option<HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>>,
    standby_retention: Option<&'storage mut RetainedAmpduDmaStorage<B, SLOTS>>,
}

impl<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    AggregateTxResources<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub const fn single(
        primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Self {
        Self {
            primary,
            primary_retention,
            standby: None,
            standby_retention: None,
        }
    }

    pub const fn pipelined(
        primary: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        primary_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        standby: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        standby_retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Self {
        Self {
            primary,
            primary_retention,
            standby: Some(standby),
            standby_retention: Some(standby_retention),
        }
    }

    pub const fn primary(&self) -> &HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE> {
        &self.primary
    }

    pub const fn standby(&self) -> Option<&HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>> {
        self.standby.as_ref()
    }
}

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
    /// A new network publication was requested while the shared TX owner
    /// still retained an ordinary or aggregate transaction.
    ActiveTransaction,
    /// TX service was entered without an ordinary or aggregate transaction.
    InactiveTransaction,
    /// The aggregate state lost the cookie that proves ownership of the
    /// reserved DMA arena.
    MissingCookie,
    /// A prepared aggregate did not reach the publication state expected by
    /// the initial submit edge.
    InvalidPublicationState,
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
    /// An aggregate detached one MPDU into the ordinary owner, but that owner
    /// completed without publishing its normalized terminal status.
    MissingOrdinaryRetryStatus,
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

struct AggregateActive<const SLOTS: usize> {
    config: AmpduTxConfig,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    deadline_micros: u64,
    first_publication_micros: Option<u64>,
}

struct AggregatePrepared<const SLOTS: usize> {
    aggregate_length: u16,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    first_sequence: u16,
    build_stop: AggregateBuildStop,
    preparation_micros: u64,
}

enum ConnectedTxActive<const SLOTS: usize> {
    Idle,
    Ordinary,
    Aggregate(AggregateActive<SLOTS>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AggregateFrameAdmission {
    /// The exact frame geometry was checked before the aggregate reservation.
    FreshExact,
    /// HT admission used `FRAME_CAPACITY`, which is an upper bound for every
    /// frame obtainable from this typed network queue.
    HtQueueCapacity,
    /// HE delimiter policy depends on the actual encoded length, so the
    /// dequeued frame still needs an exact check before consuming PN/sequence.
    NeedsExactCheck,
}

struct TeardownResource<T>(Option<T>);

impl<T> TeardownResource<T> {
    fn new(resource: T) -> Self {
        Self(Some(resource))
    }

    fn take(&mut self) -> T {
        self.0
            .take()
            .expect("connected TX resource exists until successful teardown")
    }

    fn is_present(&self) -> bool {
        self.0.is_some()
    }
}

impl<T> Deref for TeardownResource<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
            .as_ref()
            .expect("connected TX resource is present while the owner is usable")
    }
}

impl<T> DerefMut for TeardownResource<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .as_mut()
            .expect("connected TX resource is present while the owner is usable")
    }
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
> where
    'resources: 'ampdu,
{
    ordinary: TeardownResource<Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>>,
    ampdu: TeardownResource<
        RetainedDmaAmpduTx<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
    >,
    standby_ampdu: Option<
        RetainedDmaAmpduTx<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
    >,
    cookie: Option<TxCookie>,
    standby_cookie: Option<TxCookie>,
    standby_prepared: Option<AggregatePrepared<SLOTS>>,
    standby_error: Option<AggregateTxError>,
    /// Peer-negotiated BlockAck window per QoS TID; zero means inactive.
    block_ack_windows: [u8; 8],
    config: AggregateTxConfig,
    active: ConnectedTxActive<SLOTS>,
    last_aggregate_status: Option<MacAmpduTxStatus<TxPhyRate>>,
    pending_ordinary_retry: Option<MacAmpduTxStatus<TxPhyRate>>,
    observer: Option<&'ampdu dyn AggregateTxObserver>,
}

mod adapters;
mod completion;
mod owner;
mod publication;
mod resources;

#[cfg(test)]
mod tests;
