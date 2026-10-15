#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Hardware-independent Bluetooth LE radio event contract.
//!
//! A Link Layer controller core plans radio events and hands each one to a
//! backend as a [`RadioRequest`]; the backend reports what happened as
//! [`RadioOutcome`] values. The contract carries physical values only:
//! radio-domain microseconds, channels, air-interface identities and complete
//! Link Layer PDUs. Descriptor images, register fields and controller ticks
//! stay in the backend.
//!
//! Events never overlap: conflicts between roles are resolved before a
//! request reaches the backend, with the one reservation function both use
//! ([`RadioTiming::reservation`]), so a backend never meets an overlapping
//! window. Every value a request carries is checked against the backend's
//! capabilities when it is built ([`Supported`], branded by the
//! capabilities marker [`LeCapabilities`]), so a backend refuses nothing
//! they exclude. Long-lived role state — an advertising set, a
//! scanner, a connection — is configured once and named by a small
//! caller-chosen identifier in later events.
//!
//! [`LeRadioBackend`] is the backend a Controller service body drives
//! through the shared port owner of `oer-radio-port`
//! ([`Owner`](oer_radio_port::Owner)): its capabilities marker states the
//! backend's [`LeRadioCapabilities`] (roles, PHYs, data PDU payload, who
//! runs the [`LinkAcknowledgement`] and the [`RadioTiming`]), and an enabled
//! owner's phase submits requests, reads the radio clock, cancels events and
//! yields owned events. [`NoRadio`] is the backend of a host without a
//! radio. Poisoning, event loss, correlation identities and the clock
//! relation are the shared ones of `oer-radio-port`.

#[cfg(test)]
extern crate std;

mod activity;
mod capabilities;
mod channel;
mod outcome;
mod owned;
mod pdu;
mod port;
mod request;
mod supported;

pub use activity::RadioActivity;
pub use capabilities::{
    LeCapabilities, LeConnectionCapabilities, LePhy, LePhys, LeRadioCapabilities,
    LegacyAdvertisingTypes, LinkAcknowledgement, NoCapabilities,
};
pub use channel::{
    AdvertisingChannel, AdvertisingChannels, ChannelError, DataChannel, TestChannel,
};
pub use oer_radio_coex::CoexPriority;
pub use oer_radio_port::{CancelError, ClockError};
pub use oer_radio_port::{ClockInfo, EventsLost, Poisoned, PortResult};
pub use oer_radio_port::{Correlation, CorrelationIds};
pub use oer_time::{NonZeroRadioDuration, RadioDuration, WindowError};

/// The clock domain of the LE radio port: its instants never mix with
/// another port's.
pub enum LeRadio {}

/// An instant on the LE radio port's clock.
pub type LeInstant = oer_time::RadioInstant<LeRadio>;

/// A window reserved on the LE radio port's clock.
pub type LeWindow = oer_time::RadioWindow<LeRadio>;
pub use outcome::{EventResult, RadioOutcome, ReceivedPdu, TestReport};
pub use owned::{
    OwnedAdvertisingConfiguration, OwnedDataPdu, OwnedRadioRequest, OwnedTestTransmit, Restore,
};
pub use pdu::{AdvertisingPdu, DataPdu, DataPduKind, PduError, TestPayloadType};
pub use port::{LeRadioBackend, NoRadio, NoRadioPart};
pub use request::{
    AcceptListChange, AcceptListDevice, AccessAddress, AdvertisingConfiguration, AdvertisingEvent,
    AdvertisingReception, AdvertisingSetId, CoexistenceLevel, ConnectionAllowances,
    ConnectionConfiguration, ConnectionEvent, ConnectionEventTiming, ConnectionId, CrcInit,
    EventId, IdlePriority, OutsideEpoch, RadioRequest, RadioTiming, RequestError, ScanFilterPolicy,
    ScanType, ScanWindow, ScannerConfiguration, ScannerId, TestPhy, TestReceive, TestTransmit,
    TxPower,
};
pub use supported::{Capability, Supported, Timed, Unplaced, Untimed};
