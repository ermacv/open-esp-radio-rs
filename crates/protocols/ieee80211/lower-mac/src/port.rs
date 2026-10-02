//! The lower-MAC port trait and its events.

use core::future::Future;

use oer_radio_port::{
    CancelError, ClockInfo, EventsLost, LifecycleCommand, LifecycleError, LifecycleEvent, Poisoned,
    PortError,
};
use oer_time::RadioInstant;

use crate::{
    capabilities::LowerMacCapabilities,
    control::{KeyHandle, KeyInstall, LowerMacSetting, SettingError},
    rx::RxMeta,
    tx::{Refused, TxAttempt, TxBuffer, TxCompletion, TxId, TxPayload},
};

/// The portable view of one owned event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacEvent<'a> {
    /// One received MPDU, from its header to the end of its body, without
    /// the FCS.
    Received { frame: &'a [u8], meta: RxMeta },
    /// A received MPDU of `length` bytes was longer than the backend's
    /// receive buffer and was dropped. It is not a queue loss: no other
    /// event is missing.
    RxTooLong { length: usize },
    /// The terminal event of one attempt. Its buffer is released.
    TxCompleted(TxCompletion),
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
    /// An event of an extension trait, read through that trait's view,
    /// such as [`LowerMacBeaconTiming::tbtt`](crate::LowerMacBeaconTiming::tbtt).
    Extension,
    /// The backend's state is unknown: the terminal event of the port,
    /// reported after every earlier event and again at every later
    /// [`Ieee80211LowerMacPort::next_event`].
    Poisoned(Poisoned),
}

/// A single attempt of the base port: one MPDU in a backend buffer.
pub type MpduAttempt<B> = TxAttempt<TxPayload<B>>;

/// The result of a submission: admitted, refused with the attempt handed
/// back, or the port's error.
pub type SubmitResult<A, E> = Result<Result<(), Refused<A>>, E>;

/// An IEEE 802.11 lower-MAC backend as portable MAC logic drives it.
///
/// The port has the five parts every radio port has:
///
/// - **Submission**: [`Self::submit`] admits one [`TxAttempt`], one hardware
///   transmission attempt, or refuses it as a value. The frame is written
///   into a buffer of [`Self::tx_buffer`].
/// - **Events**: [`Self::next_event`] yields owned events, read through
///   [`Self::view`]; reception, attempt completions, lifecycle terminals and
///   the terminal [`LowerMacEvent::Poisoned`]. Loss is reported as
///   [`EventsLost`].
/// - **Capabilities**: [`Self::capabilities`], the parametric limits, read
///   before submission.
/// - **Lifecycle**: [`Self::lifecycle`] (enable, disable, quiesce) and
///   [`Self::cancel`] of one attempt, each with a terminal event.
/// - **Clock**: [`Self::now`], the radio time of receive timestamps, with
///   the resolution and epoch relation of [`Self::clock_info`].
///
/// Optional operations are extension traits over this one:
/// [`LowerMacAmpdu`](crate::LowerMacAmpdu),
/// [`LowerMacBeaconTiming`](crate::LowerMacBeaconTiming),
/// [`LowerMacMonitor`](crate::LowerMacMonitor) and
/// [`LowerMacCancelPublished`](crate::LowerMacCancelPublished). An upper
/// layer that needs one requires its trait bound; a backend that lacks it
/// does not implement it.
///
/// Every method but [`Self::next_event`] is synchronous: a backend decides
/// admission from its own state without waiting for hardware, as the
/// ESP32-S31 queue owner does when it prepares a bound transmission
/// (`hardware/esp32s31/driver/ieee80211/mac/src/tx.rs`, `TxHardware`).
///
/// # Events
///
/// The port has exactly one consumer of its events: one task owns
/// [`Self::next_event`]. Several exchanges share the port through a router
/// that owns the stream and hands each completion to the exchange whose
/// [`TxId`] it carries (`oer-ieee80211-upper-mac-service`'s `EventRouter`).
/// Taking an event only dequeues it. Timed work a backend performs in
/// software (a publication watchdog, a retune) runs in the backend's own
/// runner, which the composition polls beside the consumer; the port never
/// depends on its consumer to make progress.
///
/// Each transmit queue ([`LowerMacCapabilities::tx_queues`]) holds at most
/// one attempt in flight; a second attempt for the same queue is refused as
/// [`SubmitError::Busy`](crate::SubmitError::Busy). Completions correlate
/// by [`TxId`], not by order.
///
/// # Failures
///
/// Failures come in the three classes of every radio port
/// ([`FailureClass`](oer_radio_port::FailureClass)):
///
/// - `Rejected`: the inner `Err` of a submission, setting or command, and a
///   [`PortError`] of that class (a backend that is not installed); nothing
///   changed.
/// - `Recoverable`: an admitted attempt ended as
///   [`TxStatus::Aborted`](crate::TxStatus::Aborted) or
///   [`TxStatus::Fault`](crate::TxStatus::Fault), or a lifecycle command
///   ended with a `Recoverable` [`LifecycleEvent::Failed`]; the port stays
///   usable.
/// - `Poisoned`: the backend's state is unknown. The port reports
///   [`LowerMacEvent::Poisoned`] after every earlier event and returns an
///   error of that class from every later call; only a reset restores it.
pub trait Ieee80211LowerMacPort {
    /// One owned event.
    type Event;
    /// Why the port cannot serve; its class says whether it ever will again.
    type Error: PortError;
    /// Memory for one MPDU of an attempt.
    type TxBuffer: TxBuffer;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> LowerMacEvent<'_>;

    /// What the backend accepts; it does not change while the port exists.
    fn capabilities(&self) -> LowerMacCapabilities;

    /// The resolution of [`Self::now`] and how its epoch relates to the
    /// image's monotonic time.
    fn clock_info(&self) -> ClockInfo;

    /// Lend a buffer for an MPDU of `len` bytes. `Ok(None)` when every
    /// buffer is in use or `len` exceeds
    /// [`LowerMacCapabilities::max_mpdu_length`]; `Err` when the port
    /// cannot serve.
    fn tx_buffer(&self, len: usize) -> Result<Option<Self::TxBuffer>, Self::Error>;

    /// Take back a buffer the caller will not submit. A poisoned backend
    /// keeps it until the reset.
    fn release_tx_buffer(&self, buffer: Self::TxBuffer);

    /// Admit one attempt. `Ok(Err(_))` when the backend refused it: nothing
    /// was sent and the attempt comes back with its buffer. An admitted
    /// attempt reports exactly one [`LowerMacEvent::TxCompleted`] with its
    /// identity, and its buffer is released then.
    fn submit(
        &self,
        attempt: MpduAttempt<Self::TxBuffer>,
    ) -> SubmitResult<MpduAttempt<Self::TxBuffer>, Self::Error>;

    /// The next event. Taking it only dequeues it. Dropping the future
    /// loses no event. A loss is reported as [`EventsLost`] in place of the
    /// first dropped event; after poisoning, every call yields the terminal
    /// [`LowerMacEvent::Poisoned`].
    fn next_event(&self) -> impl Future<Output = Result<Self::Event, EventsLost>> + '_;

    /// Apply one setting. `Ok(Err(_))` when the backend refused it.
    fn apply(&self, setting: LowerMacSetting) -> Result<Result<(), SettingError>, Self::Error>;

    /// Install a key and return the handle attempts select it with.
    fn install_key(
        &self,
        key: KeyInstall<'_>,
    ) -> Result<Result<KeyHandle, SettingError>, Self::Error>;

    /// Start one lifecycle command; its terminal event follows through
    /// [`Self::next_event`].
    ///
    /// - `Enable` starts receiving and admitting attempts.
    /// - `Disable` stops admitting attempts, aborts those in flight and
    ///   stops receiving; it ends after the completion of every admitted
    ///   attempt.
    /// - `Quiesce` stops admitting attempts and lets those in flight
    ///   complete; `Enable` resumes admission.
    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> Result<Result<(), LifecycleError>, Self::Error>;

    /// End one admitted attempt. The terminal event is the attempt's own
    /// completion: [`TxStatus::Aborted`](crate::TxStatus::Aborted) for an
    /// attempt the backend has not yet published, and for a published one
    /// whatever it ends with, which may be its natural completion. Ending a
    /// published attempt on the air is
    /// [`LowerMacCancelPublished`](crate::LowerMacCancelPublished).
    ///
    /// A refusal as [`CancelError::NotRunning`] proves that the attempt
    /// already ended, which recovers an attempt whose completion was lost.
    fn cancel(&self, id: TxId) -> Result<Result<(), CancelError>, Self::Error>;

    /// The radio clock in microseconds: the epoch of receive timestamps.
    fn now(&self) -> Result<RadioInstant, Self::Error>;
}
