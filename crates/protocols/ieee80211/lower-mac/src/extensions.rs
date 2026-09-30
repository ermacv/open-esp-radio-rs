//! Optional operations of the lower-MAC port.
//!
//! A structural feature a backend may lack is an extension trait over
//! [`Ieee80211LowerMacPort`], not a capability flag: an upper layer that
//! needs the feature requires the trait bound, and a backend without it
//! does not implement the trait, so the missing feature cannot be requested
//! at all. Each extension states its own parametric limits. Why a backend
//! lacks a feature is recorded in its qualification catalog, not in code.

use crate::{
    control::{LifecycleError, SettingError, VifId, VifRoleSet},
    port::{Ieee80211LowerMacPort, SubmitResult},
    tx::TxId,
};

/// Memory for the MPDUs of one A-MPDU, lent by the backend.
///
/// The caller appends the encoded MPDUs in transmission order, each without
/// FCS; the backend adds delimiters and padding. It is released when the
/// attempt's completion is reported, as a [`TxBuffer`](crate::TxBuffer) is.
pub trait AmpduBuffer {
    /// Append one MPDU of `len` bytes and return its bytes for writing;
    /// `None` when the buffer cannot hold another MPDU of that length.
    fn push_mpdu(&mut self, len: usize) -> Option<&mut [u8]>;

    /// The MPDUs appended so far.
    fn subframes(&self) -> usize;
}

/// The payload of one A-MPDU attempt, answered by a BlockAck.
#[derive(Debug, Eq, PartialEq)]
pub struct AmpduPayload<A> {
    pub subframes: A,
    /// The traffic identifier all subframes share.
    pub tid: u8,
    /// The recipient's Minimum MPDU Start Spacing, the IEEE encoding 0-7 of
    /// its HT Capabilities A-MPDU Parameters.
    pub min_mpdu_start_spacing: u8,
}

/// The limits of A-MPDU attempts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AmpduCapabilities {
    /// Subframes of one A-MPDU attempt.
    pub max_subframes: u16,
}

/// One A-MPDU attempt.
pub type AmpduAttempt<A> = crate::TxAttempt<AmpduPayload<A>>;

/// A-MPDU transmission: one attempt sends one aggregate and reports the
/// recipient's BlockAck in its completion's
/// [`block_ack`](crate::TxCompletion::block_ack).
///
/// The attempt occupies its access category's queue like a single MPDU;
/// selecting which MPDUs a later aggregate retransmits stays above the port
/// unless the backend reports
/// [`HardwareServices::AMPDU_RETRY_SELECTION`](crate::HardwareServices::AMPDU_RETRY_SELECTION).
pub trait LowerMacAmpdu: Ieee80211LowerMacPort {
    type AmpduBuffer: AmpduBuffer;

    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// Lend an aggregate buffer; `None` when every one is in use.
    fn ampdu_buffer(&self) -> Option<Self::AmpduBuffer>;

    /// Take back an aggregate buffer the caller will not submit.
    fn release_ampdu_buffer(&self, buffer: Self::AmpduBuffer);

    /// Admit one A-MPDU attempt, with the base port's admission rules. An
    /// empty aggregate is `InvalidLength`, one above
    /// [`AmpduCapabilities::max_subframes`] `Unsupported`.
    fn submit_ampdu(
        &self,
        attempt: AmpduAttempt<Self::AmpduBuffer>,
    ) -> SubmitResult<AmpduAttempt<Self::AmpduBuffer>, Self::Error>;
}

/// A value of an interface's Timing Synchronization Function in
/// microseconds (IEEE 802.11-2020 11.1.3).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Tsf(pub u64);

/// When the backend reports target beacon transmission times.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TbttSchedule {
    /// The beacon interval in time units of 1024 µs.
    pub beacon_interval_tu: u16,
    /// A target beacon transmission time of the schedule.
    pub next: Tsf,
    /// How long before each TBTT its event is reported, in microseconds.
    pub lead_micros: u16,
}

/// One target beacon transmission time of an interface's schedule.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TbttEvent {
    pub vif: VifId,
    /// The TBTT the event announces, in the interface's TSF.
    pub tsf: Tsf,
}

/// Which roles the beacon-timing operations serve.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BeaconTimingCapabilities {
    /// Roles whose TSF [`LowerMacBeaconTiming::tsf`] reads.
    pub tsf_read: VifRoleSet,
    /// Roles whose TSF [`LowerMacBeaconTiming::set_tsf`] sets to any value.
    pub tsf_set: VifRoleSet,
    /// Roles whose TSF [`LowerMacBeaconTiming::set_tsf`] restarts from zero.
    pub tsf_restart: VifRoleSet,
    /// Roles whose TBTT schedule [`LowerMacBeaconTiming::set_tbtt`]
    /// programs.
    pub tbtt: VifRoleSet,
}

/// The TSF and TBTT schedule of each interface.
///
/// An operation on an interface whose role the
/// [`BeaconTimingCapabilities`] exclude is refused as
/// [`SettingError::Unsupported`].
pub trait LowerMacBeaconTiming: Ieee80211LowerMacPort {
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities;

    /// The TSF of a configured interface.
    fn tsf(&self, vif: VifId) -> Result<Result<Tsf, SettingError>, Self::Error>;

    /// Set an interface's TSF.
    fn set_tsf(&self, vif: VifId, tsf: Tsf) -> Result<Result<(), SettingError>, Self::Error>;

    /// Report target beacon transmission times of an interface as events,
    /// or stop (`None`).
    fn set_tbtt(
        &self,
        vif: VifId,
        schedule: Option<TbttSchedule>,
    ) -> Result<Result<(), SettingError>, Self::Error>;

    /// The TBTT an event reports; `None` for another event. Its base view
    /// is [`LowerMacEvent::Extension`](crate::LowerMacEvent::Extension).
    fn tbtt(event: &Self::Event) -> Option<TbttEvent>;
}

/// The limits of monitor reception.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MonitorCapabilities {
    /// Whether monitor reception runs while an interface has a receive
    /// filter other than `ReceiveFilter::NONE`.
    pub with_receiving_interfaces: bool,
}

/// Monitor reception: every frame the PHY decodes with a valid FCS is
/// delivered as [`LowerMacEvent::Received`](crate::LowerMacEvent::Received),
/// whatever its addresses and the interfaces' filters.
pub trait LowerMacMonitor: Ieee80211LowerMacPort {
    fn monitor_capabilities(&self) -> MonitorCapabilities;

    /// Start or stop monitor reception.
    fn set_monitor(&self, enabled: bool) -> Result<Result<(), SettingError>, Self::Error>;
}

/// Ending a published attempt on the air.
pub trait LowerMacCancelPublished: Ieee80211LowerMacPort {
    /// Withdraw an admitted attempt from the hardware, published or not.
    /// Its terminal event is its completion,
    /// [`TxStatus::Aborted`](crate::TxStatus::Aborted) unless it had already
    /// completed; the medium may have carried part of it.
    fn cancel_published(&self, id: TxId) -> Result<Result<(), LifecycleError>, Self::Error>;
}
