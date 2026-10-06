//! The lower-MAC port trait and its events.

use core::future::Future;

use crate::{Ieee80211ClockSample, Ieee80211Instant};
use oer_radio_port::{
    CancelError, ClockInfo, EventsLost, LifecycleCommand, LifecycleError, LifecycleEvent, Poisoned,
    PortError,
};

use crate::{
    capabilities::LowerMacCapabilities,
    control::{KeyHandle, KeyInstall, LowerMacSetting, SettingError},
    rx::{RxBuffer, RxMeta},
    tx::{ReclaimError, Refused, TxAttempt, TxBody, TxBuffer, TxCompletion, TxId, TxPayload},
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

/// A single attempt of the base port: one MPDU, its header in a backend
/// buffer `B` and its body, if any, the owner `O`.
pub type MpduAttempt<B, O> = TxAttempt<TxPayload<B, O>>;

/// The result of a submission: admitted, refused with the attempt handed
/// back, or the port's error.
pub type SubmitResult<A, E> = Result<Result<(), Refused<A>>, E>;

/// An IEEE 802.11 lower-MAC backend as portable MAC logic drives it.
///
/// The port has the five parts every radio port has:
///
/// - **Submission**: [`Self::submit`] admits one [`TxAttempt`], one hardware
///   transmission attempt, or refuses it as a value. The frame's header is
///   written into a buffer of [`Self::tx_buffer`]; its body travels by
///   ownership and comes back through [`Self::reclaim_tx_bodies`].
/// - **Events**: [`Self::next_event`] yields owned events, read through
///   [`Self::view`]; reception, attempt completions, lifecycle terminals and
///   the terminal [`LowerMacEvent::Poisoned`]. Loss is reported as
///   [`EventsLost`].
/// - **Capabilities**: [`Self::capabilities`], the parametric limits, read
///   before submission.
/// - **Lifecycle**: [`Self::lifecycle`] (enable, disable, quiesce) and
///   [`Self::cancel`] of one attempt, each with a terminal event.
/// - **Clock**: [`Self::now`], the radio time of receive timestamps, with
///   the resolution and epoch relation of [`Self::clock_info`];
///   [`Self::clock_sample`] pairs it with the monotonic clock.
///
/// Optional operations are extension traits over this one:
/// [`LowerMacAmpdu`](crate::LowerMacAmpdu),
/// [`LowerMacBeaconTiming`](crate::LowerMacBeaconTiming),
/// [`LowerMacMonitor`](crate::LowerMacMonitor),
/// [`LowerMacCancelPublished`](crate::LowerMacCancelPublished),
/// [`LowerMacAirReservation`](crate::LowerMacAirReservation) and
/// [`LowerMacLiveRetune`](crate::LowerMacLiveRetune). An upper
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
    /// The owner of an MPDU's body, which the composition chooses: the
    /// network's frame type.
    type TxBody: TxBody;
    /// The memory of one received MPDU, lent with its event.
    type RxBuffer: RxBuffer;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> LowerMacEvent<'_>;

    /// Take the frame out of a [`LowerMacEvent::Received`] event, with its
    /// metadata; any other event comes back unchanged.
    fn into_received(event: Self::Event) -> Result<(Self::RxBuffer, RxMeta), Self::Event>;

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
    /// was sent and the attempt comes back with its buffer and body. An
    /// admitted attempt reports exactly one [`LowerMacEvent::TxCompleted`]
    /// with its identity, and its buffer is released then; its body stays
    /// with the backend until [`Self::reclaim_tx_bodies`].
    fn submit(
        &self,
        attempt: MpduAttempt<Self::TxBuffer, Self::TxBody>,
    ) -> SubmitResult<MpduAttempt<Self::TxBuffer, Self::TxBody>, Self::Error>;

    /// Hand the bodies of attempt `id` to `each`, with their subframe index
    /// (0 for a single MPDU), once the attempt ended: its completion was
    /// reported, or [`Self::cancel`] proved it over. The backend keeps no
    /// body of the attempt afterwards. A poisoned backend keeps them until
    /// the reset.
    fn reclaim_tx_bodies(
        &self,
        id: TxId,
        each: impl FnMut(usize, Self::TxBody),
    ) -> Result<Result<(), ReclaimError>, Self::Error>;

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
    fn now(&self) -> Result<Ieee80211Instant, Self::Error>;

    /// The radio clock and the monotonic clock read back to back, in the
    /// current generation of their relation. A caller converts a receive
    /// stamp with it ([`ClockInfo::to_monotonic_with`]); a stamp of an
    /// earlier generation does not convert.
    fn clock_sample(&self) -> Result<Ieee80211ClockSample, Self::Error>;
}
