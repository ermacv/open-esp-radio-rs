//! Station connected HT/HE TX owner for the production Wi-Fi runner.
//!
//! The owner retains every physical frame owner referenced by DMA until
//! completion, queue detach, BlockAck processing and any retained aggregate
//! retry. It shares the ordinary descriptor, key token, sequence spaces,
//! contention state, power profile and clock through
//! [`Esp32s31SingleMpduTx`]; no parallel HIL TX state exists.
//!
//! This module is intentionally station-specific: HE/A-MSDU policy, station
//! rate control and individual-retry fallback do not belong to the AP owner.
//! Role-neutral completion, retry and retained-DMA mechanics live below this
//! adapter in the MAC and common ESP32-S31 Wi-Fi crates.

#[cfg(not(any(feature = "diagnostics", test)))]
use core::marker::PhantomData;
use core::{
    future::Future,
    mem,
    ops::{Deref, DerefMut},
};

use crate::datapath::{MaterializedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame};
use open_esp_radio_esp32s31_hal::types::MacInterface;
use open_esp_radio_esp32s31_wifi::ampdu_tx::{
    AmpduTxRoleAdapter, HtAmpduPublicationInputs, HtAmpduTxRolePolicy, HtAmpduTxRolePolicyError,
    ht_ampdu_publication_config,
};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
#[cfg(test)]
#[cfg(all(test, feature = "owned-network"))]
use open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE;
use open_esp_radio_esp32s31_wifi_mac::{
    rate_control::{AmpduRateObservationError, StaRateControlAssociation, StaTxRatePolicy},
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeEdcaTxopLimit, HeTriggerBasedTxConfig, LegacyTxQueue,
        TxCookie, TxPhyRate, TxSlotState,
    },
    tx_ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
        HtAmpduHardware, HtAmpduTxError, RetainedAmpduRetryCompletionError, RetainedDmaAmpduTx,
    },
    tx_protection::{TxProtectionAdmissionError, TxProtectionReceiver},
    tx_runtime::{
        AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState, WifiTxTraffic,
        WifiTxTrafficError, WmmTxopUnsupported,
    },
};
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::{
    ActionTxConfig, ConnectedTxHandoff, Esp32s31SingleMpduTx, Esp32s31SingleMpduTxParked,
    SingleMpduTxError, SingleMpduTxOutcome, WifiTxResources,
};
use open_esp_radio_ieee80211::{
    data::DataHeControl,
    station::{
        STA_PROTECTED_QOS_ETHERNET_HEADROOM, STA_PROTECTED_QOS_ETHERNET_OVERHEAD,
        StaTxSequenceCounters, StationFrameError, sta_protected_amsdu_pair_frame_length,
    },
    station_power_save::{StaAssociationId, StaPowerManagement},
};
use open_esp_radio_wifi_datapath::PhysicalTxSource;
use open_esp_radio_wifi_softmac::{
    MacAmpduTxResult, MacAmpduTxStatus, MacTxQueueState, MacTxResult,
};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::{
    AggregateTxObservation, AggregateTxObserver, PreparedTxSchedulerPhase,
};
use crate::{
    datapath::services::DatapathNetworkTxService,
    datapath::tx::resources::{AggregateTxArenaPair, AggregateTxResources},
    datapath::{DatapathControlProgress, WifiTxProgress, WifiTxWake},
    diagnostics::aggregate_tx::{AggregateBuildStop, NetworkSingleMpduReason},
    roles::station::control::{
        ConnectedControlTimer, ConnectedControlTx, ConnectedHeControlRuntimeRejection,
        HeNdpaRuntimeRequest, HeTriggerRuntimeRequest,
    },
};
use open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;

const AMPDU_ABORT_SETTLE_US: u64 = 16;
const HE_TRIGGER_DATA_TID: u8 = 0;

/// Control-plane notification for the current station TX BlockAck state.
///
/// Unlike [`AggregateTxObserver`], this is application-visible link state,
/// not diagnostic telemetry. It runs only when a negotiated agreement changes
/// state and therefore never observes aggregate publication or completion.
pub type StationTxBlockAckStatusSink = fn(tid: u8, operational: bool);

/// Peer-independent TX resources returned at the connected-to-disconnected
/// station boundary.
pub struct Esp32s31ConnectedTxTeardownParts<R, A> {
    pub resources: R,
    pub security: open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::ConnectedTxSecurity,
    pub sequences: StaTxSequenceCounters,
    pub aggregate: A,
}

/// Station-local TX policy retained while the one physical ordinary/A-MPDU
/// owner is lent to the AP role.
///
/// This value contains no descriptor, DMA backing or hardware publication
/// capability. Resuming it requires both exact physical owners returned by
/// the other role.
pub struct Esp32s31ConnectedTxParked<'observer, const SLOTS: usize> {
    ordinary: Esp32s31SingleMpduTxParked,
    block_ack_windows: [u8; 8],
    block_ack_generations: [u32; 8],
    block_ack_generation_exhausted: u8,
    config: AggregateTxConfig,
    rate_control: StaRateControlAssociation,
    aggregate_rate_policy: StaTxRatePolicy,
    he_trigger_based: Option<HeTriggerBasedTxConfig>,
    last_aggregate_status: Option<MacAmpduTxStatus<TxPhyRate>>,
    pending_ordinary_retry: Option<MacAmpduTxStatus<TxPhyRate>>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'observer dyn crate::diagnostics::aggregate_tx::AggregateTxObserver>,
    #[cfg(not(any(feature = "diagnostics", test)))]
    observer_lifetime: PhantomData<&'observer ()>,
    block_ack_status_sink: Option<StationTxBlockAckStatusSink>,
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
    /// Control replaced or stopped the TX BlockAck agreement after a
    /// software-owned aggregate consumed sequence numbers and PNs but before
    /// that aggregate reached hardware.
    BlockAckAgreementChanged {
        tid: u8,
    },
    Encode(StationFrameError),
    Aggregate(HtAmpduTxError),
    Retry(AmpduRetryError),
    Traffic(WifiTxTrafficError),
    Unsupported(WmmTxopUnsupported),
    Protection(TxProtectionAdmissionError),
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

impl From<WifiTxTrafficError> for AggregateTxError {
    fn from(error: WifiTxTrafficError) -> Self {
        Self::Traffic(error)
    }
}

impl From<WmmTxopUnsupported> for AggregateTxError {
    fn from(error: WmmTxopUnsupported) -> Self {
        Self::Unsupported(error)
    }
}

impl From<TxProtectionAdmissionError> for AggregateTxError {
    fn from(error: TxProtectionAdmissionError) -> Self {
        Self::Protection(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AggregateTraffic {
    selected: WifiTxTraffic,
    he_txop_limit: HeEdcaTxopLimit,
}

impl AggregateTraffic {
    const fn tid(self) -> u8 {
        self.selected.tid()
    }

    const fn queue(self) -> LegacyTxQueue {
        self.selected.queue()
    }
}

struct AggregateActive<const SLOTS: usize> {
    traffic: AggregateTraffic,
    config: AmpduTxConfig,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    /// Next deadline for the enclosing published/abort-settling phase.
    deadline_micros: u64,
    #[cfg(any(feature = "diagnostics", test))]
    first_publication_micros: Option<u64>,
}

struct AggregatePrepared<const SLOTS: usize> {
    traffic: AggregateTraffic,
    block_ack_generation: u32,
    aggregate_length: u16,
    retry: AmpduRetryState<SLOTS>,
    original_subframes: u8,
    first_sequence: u16,
    build_stop: AggregateBuildStop,
    #[cfg(any(feature = "diagnostics", test))]
    preparation_micros: u64,
}

enum ConnectedTxActive<const SLOTS: usize> {
    Idle,
    Ordinary,
    Aggregate(AggregateActive<SLOTS>),
    AbortSettling(AggregateActive<SLOTS>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AggregateFrameAdmission {
    /// The exact frame geometry was checked before the aggregate reservation.
    FreshExact,
    /// HT admission used `B::MAX_ETHERNET_LENGTH`, which is an upper bound for every
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
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> where
    B: 'ampdu,
{
    ordinary: TeardownResource<Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>>,
    ampdu: TeardownResource<
        AggregateTxArenaPair<RetainedDmaAmpduTx<'ampdu, B, SLOTS, AMPDU_BUFFER_SIZE>>,
    >,
    cookie: Option<TxCookie>,
    standby_cookie: Option<TxCookie>,
    standby_prepared: Option<AggregatePrepared<SLOTS>>,
    standby_error: Option<AggregateTxError>,
    /// One FIFO successor retained when its WMM TID differs from the current
    /// aggregate. It remains a network lease and is published before another
    /// queue entry can overtake it.
    deferred_network: Option<B>,
    /// Peer-negotiated BlockAck window per QoS TID; zero means inactive.
    block_ack_windows: [u8; 8],
    /// Generation of each agreement retained by software-prepared A-MPDUs.
    /// A DELBA/re-negotiation cannot therefore leave an old arena eligible
    /// merely because a later agreement happens to have the same window.
    block_ack_generations: [u32; 8],
    /// Sticky per-TID exhaustion for the monotonic agreement generation.
    /// Once set, no later agreement in this connected epoch may admit an
    /// aggregate whose prepared identity could alias an earlier generation.
    block_ack_generation_exhausted: u8,
    config: AggregateTxConfig,
    rate_control: TeardownResource<StaRateControlAssociation>,
    aggregate_rate_policy: StaTxRatePolicy,
    he_trigger_based: Option<HeTriggerBasedTxConfig>,
    active: ConnectedTxActive<SLOTS>,
    last_aggregate_status: Option<MacAmpduTxStatus<TxPhyRate>>,
    pending_ordinary_retry: Option<MacAmpduTxStatus<TxPhyRate>>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'ampdu dyn crate::diagnostics::aggregate_tx::AggregateTxObserver>,
    block_ack_status_sink: Option<StationTxBlockAckStatusSink>,
}

mod adapters;
mod completion;
mod owner;
mod publication;
mod resources;
use crate::datapath::tx::aggregate::AggregateTxServiceEvent;

#[cfg(all(test, feature = "owned-network"))]
mod tests;
