//! The IEEE 802.15.4 radio port: the contract between portable protocol
//! logic (an upper stack's radio adapter, a MAC service) and the backend
//! that executes radio operations, whether a chip, a family driver or a
//! host model.
//!
//! [`Ieee802154RadioPort`] has the five parts every radio port has:
//! synchronous submission of a [`RadioCommand`] whose refusal is a value,
//! an asynchronous stream of owned events viewed as [`RadioEvent`] with
//! reported loss ([`EventsLost`]), the backend's [`RadioCapabilities`], the
//! lifecycle commands (`Enable`, `Disable`, `Sleep`, and `Cancel`, which
//! ends a running operation through its terminal event), and the radio
//! clock in the [`RadioInstant`] epoch. Its
//! [`RadioSetting`] values carry the state the backend reads from its own
//! interrupt context (MAC keys and frame counters, the CSL receiver, the
//! enhanced-ACK generator and armed transmit security); a backend changes
//! them atomically with respect to that context and in any radio state.
//!
//! This module only declares the port; it never waits on it.

use core::future::Future;

use crate::mac::link_metrics::ProbingInitiator;
use crate::radio::{
    RadioInstant,
    capabilities::RadioCapabilities,
    command::RadioCommand,
    event::RadioEvent,
    interface::Interface,
    state::{AcceptedCommand, CommandError, RadioState},
};

/// The backend's bounded event queue overflowed and dropped events it
/// could not hold; reported once, before the events after the loss.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventsLost;

/// How a frame counter changes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FrameCounterUpdate {
    /// Replace the counter (`otPlatRadioSetMacFrameCounter`).
    Set(u32),
    /// Raise the counter to at least this value
    /// (`otPlatRadioSetMacFrameCounterIfLarger`).
    SetIfLarger(u32),
}

/// The CSL receiver state of the radio (`otPlatRadioEnableCsl`,
/// `otPlatRadioUpdateCslSampleTime`).
///
/// With a period, generated enhanced ACKs carry a CSL IE, every frame the
/// radio sends gets the period and the phase to the next sample time
/// written into its CSL IE when its SFD goes out, and retransmissions take
/// a new frame counter.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CslReceiver {
    /// The CSL period in units of ten symbols; zero disables CSL.
    pub period: u16,
    /// The next sample time, in the low 32 bits of the radio clock.
    pub sample_time: u32,
}

/// How the radio generates enhanced acknowledgements of IEEE 802.15.4-2015
/// frames.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EnhancedAckGeneration {
    /// The noise floor in dBm Link Metrics probing measures link margins
    /// from.
    pub noise_floor_dbm: i8,
}

/// The security the next transmission is sent with, armed by the upper
/// layer (`esp_ieee802154_set_transmit_security`): the transmission and its
/// later attempts are secured with `key` and the nonce source `address`,
/// taking precedence over the radio's MAC keys.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TransmitSecurityArming<'frame> {
    /// The secured frame as a `[PHR, PSDU...]` image.
    pub frame: &'frame [u8],
    /// The key.
    pub key: [u8; 16],
    /// The extended address of the nonce, in frame byte order.
    pub address: [u8; 8],
}

/// A radio setting outside the command state machine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioSetting<'a> {
    /// Replace the MAC keys of `interface` (`otPlatRadioSetMacKey`),
    /// keeping its frame counter; an interface without keys starts from
    /// [`MacKeys::ZEROED`](crate::MacKeys::ZEROED).
    MacKeys {
        /// The interface whose keys change.
        interface: Interface,
        /// The key index of `current`.
        key_id: u8,
        /// The key of key index `key_id - 1`.
        previous: [u8; 16],
        /// The key of key index `key_id`.
        current: [u8; 16],
        /// The key of key index `key_id + 1`.
        next: [u8; 16],
    },
    /// Change the frame counter of `interface`; an interface without keys
    /// starts from [`MacKeys::ZEROED`](crate::MacKeys::ZEROED).
    FrameCounter {
        /// The interface whose frame counter changes.
        interface: Interface,
        /// The change.
        update: FrameCounterUpdate,
    },
    /// Forget the MAC keys of `interface`: its secured frames go out as
    /// given and secured enhanced ACKs to its frames are refused.
    RemoveMacKeys {
        /// The interface whose keys go.
        interface: Interface,
    },
    /// Replace the CSL receiver state.
    Csl(CslReceiver),
    /// Generate enhanced ACKs, starting without header IEs or probing
    /// initiators, or refuse every enhanced ACK (`None`).
    EnhancedAck(Option<EnhancedAckGeneration>),
    /// Replace the header IEs every enhanced ACK carries.
    EnhancedAckHeaderIes(&'a [u8]),
    /// Replace the Link Metrics probing initiators, most recently added
    /// first ([`EnhAckProbing::replace`](crate::EnhAckProbing::replace)).
    EnhancedAckProbing(&'a [ProbingInitiator]),
    /// Arm the security of the next transmission.
    TransmitSecurity(TransmitSecurityArming<'a>),
}

/// Why the backend refused a [`RadioSetting`]; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingError {
    /// The setting names an interface the radio does not have.
    UnknownInterface {
        /// The rejected interface.
        interface: Interface,
        /// The radio's interface count.
        interfaces: u8,
    },
    /// The setting changes the enhanced-ACK generator, which is off.
    EnhancedAckDisabled,
    /// The header IEs exceed what an enhanced ACK carries.
    HeaderIesTooLong {
        /// The IE bytes the backend holds at most.
        capacity: usize,
    },
    /// The backend does not implement the setting.
    Unsupported,
}

/// An IEEE 802.15.4 radio backend as portable protocol logic drives it.
///
/// Every method but [`Self::next_event`] is synchronous and takes the
/// backend's lock for its own duration only. `Err(Self::Error)` means the
/// port cannot serve at all (no radio installed, a poisoned backend);
/// refusal of one command or setting is the inner `Err`, and nothing
/// changed.
pub trait Ieee802154RadioPort {
    /// One owned event.
    type Event;
    /// Why the port cannot serve at all.
    type Error;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> RadioEvent<'_>;

    /// What the backend supports; read before submission.
    fn capabilities(&self) -> RadioCapabilities;

    /// Admit and start one command. `Ok(Err(_))` when the backend refused
    /// it: nothing changed. An admitted operation reports its terminal
    /// event through [`Self::next_event`].
    fn submit(
        &self,
        command: RadioCommand<'_>,
    ) -> Result<Result<AcceptedCommand, CommandError>, Self::Error>;

    /// The next event. Dropping the future loses no event; after a queue
    /// overflow it reports [`EventsLost`] once.
    fn next_event(&self) -> impl Future<Output = Result<Self::Event, EventsLost>> + '_;

    /// The radio clock in microseconds: the epoch of scheduled operations
    /// and receive timestamps.
    fn now(&self) -> Result<RadioInstant, Self::Error>;

    /// The radio clock as a function in microseconds, for callers that
    /// read it synchronously without the port (OpenThread's
    /// `otPlatRadioGetNow`).
    fn clock(&self) -> Result<fn() -> u64, Self::Error>;

    /// The portable state.
    fn state(&self) -> Result<RadioState, Self::Error>;

    /// The number of addressing interfaces.
    fn interfaces(&self) -> Result<u8, Self::Error>;

    /// Change one setting. `Ok(Err(_))` when the backend refused it.
    fn apply(&self, setting: RadioSetting<'_>) -> Result<Result<(), SettingError>, Self::Error>;

    /// The next frame counter of `interface`; `None` for an interface
    /// without keys or one the radio does not have.
    fn frame_counter(&self, interface: Interface) -> Result<Option<u32>, Self::Error>;

    /// The live RSSI in dBm of the most recent reception
    /// (`otPlatRadioGetRssi`), whatever the radio's state.
    fn recent_rssi(&self) -> Result<i8, Self::Error>;
}
