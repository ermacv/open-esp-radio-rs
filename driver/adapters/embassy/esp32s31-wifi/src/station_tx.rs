//! Station connected HT/HE TX owner for the production Wi-Fi runner.
//!
//! The owner retains every pinned `embassy-net` lease referenced by DMA until
//! completion, queue detach, BlockAck processing and any retained aggregate
//! retry. It shares the ordinary descriptor, key token, sequence spaces,
//! contention state, power profile and clock through
//! [`Esp32s31SingleMpduTx`]; no parallel HIL TX state exists.
//!
//! This module is intentionally station-specific: HE/A-MSDU policy, station
//! rate control and individual-retry fallback do not belong to the AP owner.
//! Role-neutral completion, retry and retained-DMA mechanics live below this
//! adapter in the MAC and common ESP32-S31 Wi-Fi crates.

use core::{
    future::Future,
    mem,
    ops::{Deref, DerefMut},
};

use open_esp_radio_embassy_net::{PinnedTxConsumer, PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_hal::types::MacInterface;
use open_esp_radio_esp32s31_wifi::ampdu_tx::{
    AmpduTxRoleAdapter, HtAmpduPublicationInputs, HtAmpduTxRolePolicy, HtAmpduTxRolePolicyError,
    ht_ampdu_publication_config,
};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
#[cfg(test)]
use open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE;
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::StaPairwiseCcmpSlot,
    rate_control::{AmpduRateObservationError, StaRateControlAssociation, StaTxRatePolicy},
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeEdcaTxopLimit, LegacyTxQueue, TxCookie, TxPhyRate,
        TxSlotState,
    },
    tx_ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
        HtAmpduHardware, HtAmpduTxError, RetainedAmpduRetryCompletionError, RetainedDmaAmpduTx,
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
    ampdu_resources::{AggregateTxArenaPair, AggregateTxResources},
    connected_control::{ConnectedControlTimer, ConnectedControlTx},
    wdev::services::WdevNetworkTxService,
    wdev::{WdevControlProgress, WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;

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
    RolePolicy(HtAmpduTxRolePolicyError),
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

impl From<RetainedAmpduRetryCompletionError> for AggregateTxError {
    fn from(error: RetainedAmpduRetryCompletionError) -> Self {
        match error {
            RetainedAmpduRetryCompletionError::Hardware(error) => Self::Aggregate(error),
            RetainedAmpduRetryCompletionError::Retry(error) => Self::Retry(error),
        }
    }
}

impl From<HtAmpduTxRolePolicyError> for AggregateTxError {
    fn from(error: HtAmpduTxRolePolicyError) -> Self {
        Self::RolePolicy(error)
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
        AggregateTxArenaPair<
            RetainedDmaAmpduTx<
                'ampdu,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    >,
    cookie: Option<TxCookie>,
    standby_cookie: Option<TxCookie>,
    standby_prepared: Option<AggregatePrepared<SLOTS>>,
    standby_error: Option<AggregateTxError>,
    /// Peer-negotiated BlockAck window per QoS TID; zero means inactive.
    block_ack_windows: [u8; 8],
    config: AggregateTxConfig,
    rate_control: StaRateControlAssociation,
    aggregate_rate_policy: StaTxRatePolicy,
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
use crate::aggregate_tx_common::AggregateTxServiceEvent;

#[cfg(test)]
mod tests;
