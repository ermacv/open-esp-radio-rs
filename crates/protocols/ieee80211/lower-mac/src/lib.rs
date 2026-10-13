#![no_std]
#![forbid(unsafe_code)]

//! The IEEE 802.11 lower-MAC radio port.
//!
//! The port is the contract between portable Wi-Fi MAC logic and the
//! backend that puts frames on the air, whether a chip, a family driver or
//! a host model, on the owner shape of `oer-ieee80211-port`: a
//! [`LowerMacBackend`] runs under its owner, which lends each running phase
//! its event consumer ([`LowerMacEvents`]), control ([`LowerMacControl`])
//! and one client per interface ([`LowerMacClient`], [`StationClient`],
//! [`AccessPointClient`]); its disabled-only settings are a
//! [`LowerMacSetup`]. Every rule a caller controls is a type
//! ([`owner`]). One submission is one hardware transmission attempt: the
//! backend contends for the medium with the caller's backoff, sends the
//! PPDU at the frame's rate and reports how that single attempt ended.
//! Retry policy, rate selection, the backoff draw, sequence and packet
//! numbers, reordering and every other decision software takes stay above
//! the port, in portable code; the services a backend performs
//! autonomously are its [`LowerMacCapabilities::services`].
//!
//! The values its backends' cores take (attempts, settings, key installs,
//! A-MPDU and beacon timing values) live beside it ([`tx`], [`control`],
//! [`extensions`]).
//!
//! The PHY and channel values the port carries ([`Channel`], [`PhyRate`])
//! are the portable ones of `oer-ieee80211-mac`. This package only declares
//! the port and its values; it never waits on it.

pub mod capabilities;
pub mod control;
pub mod extensions;
pub mod owner;
pub mod phase;
pub mod rx;
pub mod tx;

pub use oer_ieee80211_mac::channel::{Band, Channel, ChannelError, ChannelWidth};
pub use oer_ieee80211_mac::phy::PhyRate;
pub use oer_radio_coex::CoexPriority;

/// The clock domain of the IEEE 802.11 lower-MAC port: its instants never
/// mix with another port's.
pub enum Ieee80211Radio {}

/// The installation's unique slot for the station's connection-epoch source.
/// A backend mints it once; Station consumes it to issue branded epochs.
/// The backend does not construct or interpret those protocol identities.
pub enum ConnectionEpochKind {}

/// An instant on the IEEE 802.11 lower-MAC port's clock.
pub type Ieee80211Instant = oer_time::RadioInstant<Ieee80211Radio>;

/// A span of the port's radio clock.
pub type Ieee80211Duration = oer_time::RadioDuration<Ieee80211Radio>;

/// An instant on the IEEE 802.11 lower-MAC port's clock with the generation
/// of its relation to the monotonic clock.
pub type Ieee80211Stamp = oer_radio_port::RadioStamp<Ieee80211Radio>;

/// A generation of the port's radio-clock relation to monotonic time.
pub type Ieee80211ClockGeneration = oer_radio_port::Generation<Ieee80211Instant, oer_time::Instant>;

/// A paired reading of the IEEE 802.11 lower-MAC port's clock and the
/// monotonic clock.
pub type Ieee80211ClockSample = oer_radio_port::ClockSample<Ieee80211Radio>;

pub use capabilities::{
    BandSet, CoexPrioritySet, HardwareServices, LowerMacCapabilities, PhyFormatSet, RateSupport,
    WidthSet,
};
pub use control::{MacAddress, ReceiveFilter, VifRole, VifRoleSet};
pub use extensions::{
    AmpduCapabilities, BeaconTimingCapabilities, MonitorCapabilities, TSF_DRIFT_PPM, TSF_RELATION,
    TbttEvent, TsfArithmeticError, TsfGeneration, TsfProjectionError, TsfRelation, TsfSample,
    TsfSetKind, TsfTimingError, TsfVifMismatch, VifTsf,
};
pub use oer_ieee80211_port::ClockError;
pub use oer_radio_port::{
    ClockInfo, ClockRelation, EventsLost, Generation, GenerationSource, Poisoned, PortResult,
    Projected, ProjectionError, RelationRate,
};
pub use owner::{
    AccessPoint, AckHeader, AddressSetup, BssColor, Ccmp128Key, FrameControl, FrameHeader,
    GroupAddress, HeaderSpec, Ieee80211Capabilities, IndividualAddress, InterfaceRole,
    LegacyPhaseRate, LowerMacBackend, LowerMacSetup, MpduLength, NoAckHeader, PhaseRate,
    QosControl, ReceiverAddress, SizedFrame, Station, StationReceive, SupportedBackoff,
    SupportedBlockAck, SupportedChannel, SupportedCoex, SupportedEdca, SupportedFilter,
    SupportedNoAck, SupportedPower, Tid, TxFrame,
};
pub use owner::{
    AggregateAttempt, AggregateKey, AirReservationPhase, AmpduPhase, AmpduRate, BeaconTimingPhase,
    BeaconWindow, InterfaceAggregateAttempt, MAX_RESERVATION_MICROS, MonitorPhase,
    MpduStartSpacing, PhaseAggregate, TbttPlan,
};
pub use phase::{
    AccessPointPhase, Attempt, AttemptKey, ClassFrame, GateClosed, GateClosedKind, GateOpen,
    GateOpenKind, InterfaceAttempt, InterfaceKeys, InterfacePhase, KeyTypes, LowerMacEvent,
    LowerMacPhase, PhaseTypes, PoolMpdu, PoolPreparation, PoolSource, PreparationError,
    PreparationRefused, PreparedTx, RETRY_STAGES, Removed, RemovedKey, RetransmissionPlan,
    RetryPlan, RetryStage, ReturnedMpdu, RoleKeys, StationPhase, SubmitError, TxBufferClass,
    TxBufferMemory, TxBufferPool, TxSource,
};
pub use rx::{
    LinkMetric, RxBuffer, RxCryptoStatus, RxEvidence, RxMeta, RxMetadata, SignalMetadata,
};
pub use tx::{
    Backoff, BlockAckReport, Protection, TxBody, TxCompletion, TxFault, TxPower, TxResponse,
    TxStatus,
};

#[cfg(test)]
extern crate std;
