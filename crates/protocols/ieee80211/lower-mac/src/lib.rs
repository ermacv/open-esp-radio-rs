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
//! [`LowerMacMonitor`], [`LowerMacCancelPublished`]); parametric limits of
//! what a backend has are its capabilities, and a value outside them is
//! refused as `Unsupported`; why a backend lacks a feature is recorded in
//! its qualification catalog.
//!
//! The PHY and channel values the port carries ([`Channel`], [`PhyRate`])
//! are the portable ones of `oer-ieee80211-mac`. This package only declares
//! the port and its values; it never waits on it.

pub mod capabilities;
pub mod control;
pub mod extensions;
pub mod failure;
pub mod port;
pub mod rx;
pub mod tx;

pub use oer_ieee80211_mac::channel::{Band, Channel, ChannelError, ChannelWidth};
pub use oer_ieee80211_mac::phy::PhyRate;
pub use oer_radio_coex::CoexPriority;

pub use capabilities::{
    BandSet, CoexPrioritySet, HardwareServices, LowerMacCapabilities, PhyFormatSet, RateSupport,
    WidthSet,
};
pub use control::{
    Cipher, KeyHandle, KeyInstall, KeyScope, LifecycleCommand, LifecycleError, LifecycleEvent,
    LowerMacSetting, MacAddress, ReceiveFilter, RxBlockAckAgreement, SettingError, VifConfig,
    VifId, VifRole, VifRoleSet,
};
pub use extensions::{
    AmpduAttempt, AmpduBuffer, AmpduCapabilities, AmpduPayload, BeaconTimingCapabilities,
    LowerMacAmpdu, LowerMacBeaconTiming, LowerMacCancelPublished, LowerMacMonitor,
    MonitorCapabilities, TbttEvent, TbttSchedule, Tsf,
};
pub use failure::FailureClass;
pub use port::{EventsLost, Ieee80211LowerMacPort, LowerMacEvent, MpduAttempt, SubmitResult};
pub use rx::{RxCryptoStatus, RxEvidence, RxMeta};
pub use tx::{
    Backoff, BlockAckReport, KeySelector, Protection, Refused, SubmitError, TxAttempt, TxBuffer,
    TxCompletion, TxFault, TxId, TxPayload, TxPower, TxResponse, TxStatus,
};

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
