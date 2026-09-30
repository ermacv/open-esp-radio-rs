//! The lower-MAC port trait and its events.

use core::future::Future;

use oer_time::RadioInstant;

use crate::{
    capabilities::LowerMacCapabilities,
    control::{
        KeyHandle, KeyInstall, LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacSetting,
        SettingError,
    },
    rx::RxMeta,
    tx::{Refused, TxAttempt, TxBuffer, TxCompletion, TxPayload},
};

/// The backend's bounded event queue overflowed and dropped events it could
/// not hold; reported once, before the events after the loss.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventsLost;

/// The portable view of one owned event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacEvent<'a> {
    /// One received MPDU, from its header to the end of its body, without
    /// the FCS.
    Received { frame: &'a [u8], meta: RxMeta },
    /// The terminal event of one attempt. Its buffer is released.
    TxCompleted(TxCompletion),
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
    /// An event of an extension trait, read through that trait's view,
    /// such as [`LowerMacBeaconTiming::tbtt`](crate::LowerMacBeaconTiming::tbtt).
    Extension,
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
///   [`Self::view`]; reception, attempt completions and lifecycle
///   terminals. Loss is reported as [`EventsLost`].
/// - **Capabilities**: [`Self::capabilities`], the parametric limits, read
///   before submission.
/// - **Lifecycle**: [`Self::lifecycle`], each command with a terminal event.
/// - **Clock**: [`Self::now`], the radio time of receive timestamps.
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
/// Each transmit queue ([`LowerMacCapabilities::tx_queues`]) holds at most
/// one attempt in flight; a second attempt for the same queue is refused as
/// [`SubmitError::Busy`](crate::SubmitError::Busy). Completions correlate
/// by [`TxId`](crate::TxId), not by order.
///
/// Failures come in the three classes of every radio port:
///
/// - `Rejected`: the inner `Err` of a submission, setting or command;
///   nothing changed.
/// - `Recoverable`: an admitted attempt ended as
///   [`TxStatus::Aborted`](crate::TxStatus::Aborted) or
///   [`TxStatus::Fault`](crate::TxStatus::Fault), or a lifecycle command
///   ended with a `Recoverable`
///   [`LifecycleEvent::Failed`]; the port stays usable.
/// - `Poisoned`: `Err(Self::Error)`; the backend's state is unknown and only
///   a reset restores the port.
pub trait Ieee80211LowerMacPort {
    /// One owned event.
    type Event;
    /// Why the port cannot serve at all.
    type Error;
    /// Memory for one MPDU of an attempt.
    type TxBuffer: TxBuffer;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> LowerMacEvent<'_>;

    /// What the backend accepts; it does not change while the port exists.
    fn capabilities(&self) -> LowerMacCapabilities;

    /// Lend a buffer for an MPDU of `len` bytes; `None` when every buffer is
    /// in use or `len` exceeds
    /// [`LowerMacCapabilities::max_mpdu_length`].
    fn tx_buffer(&self, len: usize) -> Option<Self::TxBuffer>;

    /// Take back a buffer the caller will not submit.
    fn release_tx_buffer(&self, buffer: Self::TxBuffer);

    /// Admit one attempt. `Ok(Err(_))` when the backend refused it: nothing
    /// was sent and the attempt comes back with its buffer. An admitted
    /// attempt reports exactly one [`LowerMacEvent::TxCompleted`] with its
    /// identity, and its buffer is released then.
    fn submit(
        &self,
        attempt: MpduAttempt<Self::TxBuffer>,
    ) -> SubmitResult<MpduAttempt<Self::TxBuffer>, Self::Error>;

    /// The next event. Dropping the future loses no event; after a queue
    /// overflow it reports [`EventsLost`] once.
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
    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> Result<Result<(), LifecycleError>, Self::Error>;

    /// The radio clock in microseconds: the epoch of receive timestamps.
    fn now(&self) -> Result<RadioInstant, Self::Error>;
}
