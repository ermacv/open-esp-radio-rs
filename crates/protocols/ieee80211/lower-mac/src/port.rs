//! The lower-MAC port trait and its events.

use crate::{Ieee80211ClockSample, Ieee80211Radio};
use oer_radio_port::{ClockError, ClockInfo, LifecycleEvent, Poisoned, PortResult, RadioPort};

use crate::{
    capabilities::LowerMacCapabilities,
    control::{KeyHandle, KeyInstall, LowerMacSetting, SettingError},
    rx::{RxBuffer, RxMeta},
    tx::{Refused, TxAttempt, TxBody, TxBuffer, TxCompletion, TxId, TxPayload},
};

/// The portable view of one owned event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacEvent<'a> {
    /// One received MPDU, from its header to the end of its body, without
    /// the FCS.
    Received { frame: &'a [u8], meta: RxMeta },
    /// The terminal event of one attempt. Its buffer is released; the event
    /// owns the attempt's bodies, which
    /// [`Ieee80211LowerMacPort::into_completed`] takes out.
    TxCompleted(TxCompletion),
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
    /// An event of an extension trait, read through that trait's view,
    /// such as [`LowerMacBeaconTiming::tbtt`](crate::LowerMacBeaconTiming::tbtt).
    Extension,
}

/// A single attempt of the base port: one MPDU, its header in a backend
/// buffer `B` and its body, if any, the owner `O`.
pub type MpduAttempt<B, O> = TxAttempt<TxPayload<B, O>>;

/// The result of a submission: admitted, or refused with the attempt, its
/// buffer and body handed back. Every refusal is the inner one; the outer
/// error is only
/// [`Poisoned`] with the backend's cause `F`,
/// after which the backend keeps the attempt until its reset.
pub type SubmitResult<A, F> = PortResult<(), Refused<A>, F>;

/// An IEEE 802.11 lower-MAC backend as portable MAC logic drives it.
///
/// The port has the five parts every radio port has; the event stream, the
/// radio clock, cancellation and the lifecycle are the shared
/// [`RadioPort`] base:
///
/// - **Submission**: [`Self::submit`] admits one [`TxAttempt`], one hardware
///   transmission attempt, or refuses it as a value. The frame's header is
///   written into a buffer of [`Self::tx_buffer`]; its body travels by
///   ownership and comes back with the attempt's completion event
///   ([`Self::into_completed`]).
/// - **Events**: [`RadioPort::next_event`] yields owned events, read through
///   [`Self::view`]: reception, attempt completions, lifecycle terminals and
///   extension events. Loss is reported as
///   [`EventsLost`](oer_radio_port::EventsLost).
/// - **Capabilities**: [`Self::capabilities`], the parametric limits, read
///   before submission.
/// - **Lifecycle**: [`RadioPort::lifecycle`] (enable, disable, quiesce) and
///   [`RadioPort::cancel`] of one attempt, each with a terminal event.
/// - **Clock**: [`RadioPort::now`], the radio time of receive timestamps,
///   with the resolution and epoch relation of [`Self::clock_info`];
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
/// Every method of this trait is synchronous: a backend decides admission
/// from its own state without waiting for hardware, as the ESP32-S31 queue
/// owner does when it prepares a bound transmission
/// (`hardware/esp32s31/driver/ieee80211/mac/src/tx.rs`, `TxHardware`). The
/// base calls return ready futures.
///
/// # Events
///
/// The port has exactly one consumer of its events: one task owns
/// [`RadioPort::next_event`]. Several exchanges share the port through a
/// router that owns the stream and hands each completion to the exchange
/// whose [`TxId`] it carries (`oer-ieee80211-upper-mac-service`'s
/// `EventRouter`). Taking an event only dequeues it. Timed work a backend
/// performs in software (a publication watchdog, a retune) runs in the
/// backend's own runner, which the composition polls beside the consumer;
/// the port never depends on its consumer to make progress.
///
/// Each transmit queue ([`LowerMacCapabilities::tx_queues`]) holds at most
/// one attempt in flight; a second attempt for the same queue is refused as
/// [`SubmitError::Busy`](crate::SubmitError::Busy). Completions correlate
/// by [`TxId`], not by order. The backend reserves an admitted attempt's
/// completion and a lifecycle command's terminal event when it admits
/// them, so neither is ever lost.
///
/// # Lifecycle and cancellation
///
/// - `Enable` starts receiving and admitting attempts.
/// - `Disable` stops admitting attempts, aborts those in flight and stops
///   receiving; it ends after the completion of every admitted attempt.
/// - `Quiesce` stops admitting attempts and lets those in flight complete;
///   `Enable` resumes admission.
/// - `cancel(id)` ends one admitted attempt. The terminal event is the
///   attempt's own completion: [`TxStatus::Aborted`](crate::TxStatus::Aborted)
///   for an attempt the backend has not yet published, and for a published
///   one whatever it ends with, which may be its natural completion.
///   Ending a published attempt on the air is
///   [`LowerMacCancelPublished`](crate::LowerMacCancelPublished).
///
/// # Refusals and poisoning
///
/// Every call returns a [`PortResult`]. Its inner `Err` is a refusal and
/// nothing changed; the port exists only while its backend is installed,
/// so no call refuses as not installed. A submission is refused only through its inner `Err`, which hands the
/// attempt back. Admitted work that ends without its result ends as
/// [`TxStatus::Aborted`](crate::TxStatus::Aborted) or
/// [`TxStatus::Fault`](crate::TxStatus::Fault), and a lifecycle command
/// as [`LifecycleEvent::Failed`]; the port stays usable. The outer
/// [`Poisoned`] means the backend's state is
/// unknown: [`RadioPort::next_event`] reports it after every earlier event,
/// every later call returns it, and only a reset restores the port.
pub trait Ieee80211LowerMacPort: RadioPort<Id = TxId, Domain = Ieee80211Radio> {
    /// Memory for one MPDU of an attempt.
    type TxBuffer: TxBuffer;
    /// The owner of an MPDU's body, which the composition chooses: the
    /// network's frame type.
    type TxBody: TxBody;
    /// The memory of one received MPDU, lent with its event.
    type RxBuffer: RxBuffer;
    /// The bodies an ended attempt's completion event carries, each with its
    /// subframe index (0 for a single MPDU).
    type TxBodies: IntoIterator<Item = (usize, Self::TxBody)>;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> LowerMacEvent<'_>;

    /// Take the frame out of a [`LowerMacEvent::Received`] event, with its
    /// metadata; any other event comes back unchanged.
    fn into_received(event: Self::Event) -> Result<(Self::RxBuffer, RxMeta), Self::Event>;

    /// Take the completion out of a [`LowerMacEvent::TxCompleted`] event,
    /// with the bodies of its attempt; any other event comes back unchanged.
    /// The event owns the bodies: dropping it, or them, ends them, so a body
    /// comes back exactly once and never outlives its attempt's terminal.
    fn into_completed(event: Self::Event) -> Result<(TxCompletion, Self::TxBodies), Self::Event>;

    /// What the backend accepts; it does not change while the port exists.
    fn capabilities(&self) -> LowerMacCapabilities;

    /// The resolution of [`RadioPort::now`] and how its epoch relates to the
    /// image's monotonic time.
    fn clock_info(&self) -> ClockInfo;

    /// Lend a buffer for an MPDU of `len` bytes. `Ok(None)` when every
    /// buffer is in use or `len` exceeds
    /// [`LowerMacCapabilities::max_mpdu_length`].
    fn tx_buffer(&self, len: usize) -> Result<Option<Self::TxBuffer>, Poisoned<Self::Fault>>;

    /// Take back a buffer the caller will not submit. A poisoned backend
    /// keeps it until the reset.
    fn release_tx_buffer(&self, buffer: Self::TxBuffer);

    /// Admit one attempt. `Ok(Err(_))` when the backend refused it, for any
    /// reason: nothing was sent and the attempt comes back with its buffer and body.
    /// An admitted attempt reports exactly one [`LowerMacEvent::TxCompleted`]
    /// with its identity, and its buffer is released then; its body stays
    /// with the backend until that event carries it back
    /// ([`Self::into_completed`]). A poisoned backend keeps it until the
    /// reset.
    fn submit(
        &self,
        attempt: MpduAttempt<Self::TxBuffer, Self::TxBody>,
    ) -> SubmitResult<MpduAttempt<Self::TxBuffer, Self::TxBody>, Self::Fault>;

    /// Apply one setting. `Ok(Err(_))` when the backend refused it.
    fn apply(&self, setting: LowerMacSetting) -> PortResult<(), SettingError, Self::Fault>;

    /// Install a key and return the handle attempts select it with.
    fn install_key(&self, key: KeyInstall<'_>) -> PortResult<KeyHandle, SettingError, Self::Fault>;

    /// The radio clock and the monotonic clock read back to back, in the
    /// current generation of their relation. A caller converts a receive
    /// stamp with it ([`ClockInfo::to_monotonic_with`]); a stamp of an
    /// earlier generation does not convert.
    fn clock_sample(&self) -> PortResult<Ieee80211ClockSample, ClockError, Self::Fault>;
}
