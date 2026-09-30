#![no_std]
#![forbid(unsafe_code)]

//! The IEEE 802.11 lower-MAC radio port.
//!
//! [`Ieee80211LowerMacPort`] is the contract between portable Wi-Fi MAC
//! logic and the backend that puts frames on the air, whether a chip, a
//! family driver or a host model. One submission is one hardware
//! transmission attempt: the backend contends for the medium, sends the
//! PPDU at the given rate and reports how that single attempt ended. Retry
//! policy, rate selection, sequence and packet numbers, reordering and every
//! other decision software takes stay above the port, in portable code; the
//! services a backend performs autonomously are its
//! [`LowerMacCapabilities::services`].
//!
//! The PHY and channel values the port carries ([`Channel`], [`PhyRate`])
//! are the portable ones of `oer-ieee80211-mac`. This package only declares
//! the port and its values; it never waits on it.

pub mod capabilities;
pub mod control;
pub mod port;
pub mod rx;
pub mod tx;

pub use oer_ieee80211_mac::channel::{Band, Channel, ChannelError, ChannelWidth};
pub use oer_ieee80211_mac::phy::PhyRate;

pub use capabilities::{BandSet, HardwareServices, LowerMacCapabilities, RateSupport, WidthSet};
pub use control::{
    Cipher, KeyHandle, KeyInstall, KeyScope, LifecycleCommand, LifecycleError, LifecycleEvent,
    LowerMacSetting, MacAddress, ReceiveFilter, RxBlockAckAgreement, SettingError, TbttSchedule,
    Tsf, VifConfig, VifId, VifRole,
};
pub use port::{EventsLost, Ieee80211LowerMacPort, LowerMacEvent};
pub use rx::{RxCryptoStatus, RxEvidence, RxMeta};
pub use tx::{
    AmpduSubmission, BlockAckReport, KeySelector, Protection, SubmitError, TxAttempt, TxCompletion,
    TxFault, TxId, TxPayload, TxPower, TxResponse, TxStatus,
};

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
