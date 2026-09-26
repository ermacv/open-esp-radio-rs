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

#[cfg(test)]
extern crate std;

mod channel;
mod outcome;
mod pdu;
mod request;
mod time;

pub use channel::{
    AdvertisingChannel, AdvertisingChannels, ChannelError, DataChannel, TestChannel,
};
pub use outcome::{EventResult, RadioFault, RadioOutcome, ReceivedPdu, TestReport};
pub use pdu::{AdvertisingPdu, DataPdu, DataPduKind, PduError, TestPayloadType};
pub use request::{
    AccessAddress, AdvertisingConfiguration, AdvertisingEvent, AdvertisingSetId,
    ConnectionConfiguration, ConnectionEvent, ConnectionEventTiming, ConnectionId, CrcInit,
    EventId, RadioRequest, RadioTiming, RequestError, ScanWindow, ScannerConfiguration, ScannerId,
    TestPhy, TestReceive, TestTransmit, TxPower,
};
pub use time::{RadioDuration, RadioInstant, RadioWindow, WindowError};
