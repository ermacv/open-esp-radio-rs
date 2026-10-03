#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Hardware-independent Bluetooth LE radio event contract.
//!
//! A Link Layer controller core plans radio events and hands each one to a
//! backend as a [`RadioRequest`]; the backend reports what happened as
//! [`RadioOutcome`] values. The contract carries physical values only:
//! monotonic microseconds, channels, air-interface identities and complete
//! Link Layer PDUs. Descriptor images, register fields and controller ticks
//! stay in the backend.
//!
//! Events never overlap: conflicts between roles are resolved before a
//! request reaches the backend, and a backend refuses an overlapping window
//! instead of moving it. Long-lived role state — an advertising set, a
//! scanner, a connection — is configured once and named by a small
//! caller-chosen identifier in later events.
//!
//! [`LeRadioPort`] is the port a Controller service loop drives: it states
//! the backend's [`LeRadioCapabilities`] (roles, PHYs, data PDU payload and
//! who runs the [`LinkAcknowledgement`]), reads the radio clock, submits
//! requests and yields owned outcomes. [`NoRadio`] is the port of a host
//! without a radio. Failure classes, the outcome loss marker, the terminal
//! poisoned outcome, correlation identities and the clock relation are the
//! shared ones of `oer-radio-port`.

#[cfg(test)]
extern crate std;

mod activity;
mod capabilities;
mod channel;
mod outcome;
mod pdu;
mod port;
mod request;

pub use activity::RadioActivity;
pub use capabilities::{
    LeConnectionCapabilities, LePhy, LePhys, LeRadioCapabilities, LinkAcknowledgement,
};
pub use channel::{
    AdvertisingChannel, AdvertisingChannels, ChannelError, DataChannel, TestChannel,
};
pub use oer_radio_coex::CoexPriority;
pub use oer_radio_port::{
    ClockInfo, Correlation, CorrelationIds, EventsLost, FailureClass, Poisoned, PortError,
    RadioEpoch,
};
pub use oer_time::{RadioDuration, WindowError};

/// The clock domain of the LE radio port: its instants never mix with
/// another port's.
pub enum LeRadio {}

/// An instant on the LE radio port's clock.
pub type LeInstant = oer_time::RadioInstant<LeRadio>;

/// A window reserved on the LE radio port's clock.
pub type LeWindow = oer_time::RadioWindow<LeRadio>;
pub use outcome::{EventResult, RadioFault, RadioOutcome, ReceivedPdu, TestReport};
pub use pdu::{AdvertisingPdu, DataPdu, DataPduKind, PduError, TestPayloadType};
pub use port::{LeRadioPort, Never, NoRadio};
pub use request::{
    AcceptListChange, AcceptListDevice, AccessAddress, AdvertisingConfiguration, AdvertisingEvent,
    AdvertisingReception, AdvertisingSetId, CoexistenceLevel, ConnectionAllowances,
    ConnectionConfiguration, ConnectionEvent, ConnectionEventTiming, ConnectionId, CrcInit,
    EventId, IdlePriority, RadioRequest, RadioTiming, RequestError, ScanFilterPolicy, ScanType,
    ScanWindow, ScannerConfiguration, ScannerId, TestPhy, TestReceive, TestTransmit, TxPower,
};
