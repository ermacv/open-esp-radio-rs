#![no_std]
#![forbid(unsafe_code)]

//! The IEEE 802.11 lower-MAC radio port.
//!
//! [`Ieee80211LowerMacPort`] is the contract between portable Wi-Fi MAC
//! logic and the backend that puts frames on the air, whether a chip, a
//! family driver or a host model. One submission is one hardware
//! transmission attempt: the backend contends for the medium with the
//! caller's backoff, sends the PPDU at the given rate and reports how that
//! single attempt ended. Retry policy, rate selection, the backoff draw,
//! sequence and packet numbers, reordering and every other decision
//! software takes stay above the port, in portable code; the services a
//! backend performs autonomously are its [`LowerMacCapabilities::services`].
//!
//! The capability model has three parts. Structural optional features are
//! extension traits ([`LowerMacAmpdu`], [`LowerMacBeaconTiming`],
//! [`LowerMacMonitor`], [`LowerMacCancelPublished`],
//! [`LowerMacAirReservation`], [`LowerMacLiveRetune`]); parametric limits of
//! what a backend has are its capabilities, and a value outside them is
//! refused as `Unsupported`; why a backend lacks a feature is recorded in
//! its qualification catalog.
//!
//! Failure classes, event loss and poisoning, the lifecycle vocabulary,
//! correlation identities and the clock relation are the shared ones of
//! `oer-radio-port`, re-exported here.
//!
//! The PHY and channel values the port carries ([`Channel`], [`PhyRate`])
//! are the portable ones of `oer-ieee80211-mac`. This package only declares
//! the port and its values; it never waits on it.

pub mod capabilities;
pub mod control;
pub mod extensions;
pub mod port;
pub mod rx;
pub mod tx;

pub use oer_ieee80211_mac::channel::{Band, Channel, ChannelError, ChannelWidth};
pub use oer_ieee80211_mac::phy::PhyRate;
pub use oer_radio_coex::CoexPriority;

/// The clock domain of the IEEE 802.11 lower-MAC port: its instants never
/// mix with another port's.
pub enum Ieee80211Radio {}

/// An instant on the IEEE 802.11 lower-MAC port's clock.
pub type Ieee80211Instant = oer_time::RadioInstant<Ieee80211Radio>;

/// An instant on the IEEE 802.11 lower-MAC port's clock with the generation
/// of its relation to the monotonic clock.
pub type Ieee80211Stamp = oer_radio_port::RadioStamp<Ieee80211Radio>;

/// A paired reading of the IEEE 802.11 lower-MAC port's clock and the
/// monotonic clock.
pub type Ieee80211ClockSample = oer_radio_port::ClockSample<Ieee80211Radio>;

pub use capabilities::{
    BandSet, CoexPrioritySet, HardwareServices, LowerMacCapabilities, PhyFormatSet, RateSupport,
    WidthSet,
};
pub use control::{
    Cipher, KeyHandle, KeyInstall, KeyScope, LowerMacSetting, MacAddress, ReceiveFilter,
    RxBeaconPriority, RxBlockAckAgreement, SettingError, VifConfig, VifId, VifRole, VifRoleSet,
};
pub use extensions::{
    AirReservation, AirReservationAttempt, AmpduAttempt, AmpduBuffer, AmpduCapabilities,
    AmpduPayload, BeaconTimingCapabilities, LowerMacAirReservation, LowerMacAmpdu,
    LowerMacBeaconTiming, LowerMacCancelPublished, LowerMacLiveRetune, LowerMacMonitor,
    MAX_AIR_RESERVATION, MonitorCapabilities, TSF_DRIFT_PPM, TbttEvent, TbttSchedule,
    TsfGeneration, TsfProjectionError, TsfRelation, TsfSample, TsfSetKind, TsfVifMismatch, VifTsf,
};
pub use oer_radio_port::{
    CancelError, ClockInfo, Correlation, CorrelationIds, EpochError, EventsLost, FailureClass,
    LifecycleCommand, LifecycleError, LifecycleEvent, Poisoned, PortError, Projected, RadioEpoch,
};
pub use port::{Ieee80211LowerMacPort, LowerMacEvent, MpduAttempt, SubmitResult};
pub use rx::{RxBuffer, RxCryptoStatus, RxEvidence, RxMeta};
pub use tx::{
    Backoff, BlockAckReport, KeySelector, Protection, ReclaimError, Refused, SubmitError,
    TxAttempt, TxBody, TxBuffer, TxCompletion, TxFault, TxId, TxPayload, TxPower, TxResponse,
    TxStatus,
};

#[cfg(any(test, feature = "model"))]
extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(any(test, feature = "model"))]
pub mod model;

#[cfg(test)]
mod tests;
