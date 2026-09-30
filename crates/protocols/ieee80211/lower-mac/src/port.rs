//! The lower-MAC port trait and its events.

use core::future::Future;

use oer_time::RadioInstant;

use crate::{
    capabilities::LowerMacCapabilities,
    control::{
        KeyHandle, KeyInstall, LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacSetting,
        SettingError, Tsf, VifId,
    },
    rx::RxMeta,
    tx::{SubmitError, TxAttempt, TxCompletion},
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
    /// The terminal event of one attempt.
    TxCompleted(TxCompletion),
    /// A target beacon transmission time of an interface's schedule.
    Tbtt { vif: VifId, tsf: Tsf },
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
}

/// An IEEE 802.11 lower-MAC backend as portable MAC logic drives it.
///
/// The port has the five parts every radio port has:
///
/// - **Submission**: [`Self::submit`] admits one [`TxAttempt`], one hardware
///   transmission attempt, or refuses it as a value.
/// - **Events**: [`Self::next_event`] yields owned events, read through
///   [`Self::view`]; reception, attempt completions, TBTTs and lifecycle
///   terminals. Loss is reported as [`EventsLost`].
/// - **Capabilities**: [`Self::capabilities`], read before submission.
/// - **Lifecycle**: [`Self::lifecycle`], each command with a terminal event.
/// - **Clock**: [`Self::now`], the radio time of receive timestamps, and
///   [`Self::tsf`], an interface's TSF.
///
/// Every method but [`Self::next_event`] is synchronous: a backend decides
/// admission from its own state without waiting for hardware, as the
/// ESP32-S31 queue owner does when it prepares a bound transmission
/// (`hardware/esp32s31/driver/ieee80211/mac/src/tx.rs`, `TxHardware`).
///
/// Failures come in the three classes of every radio port:
///
/// - `Rejected`: the inner `Err` of a submission, setting or command;
///   nothing changed.
/// - `Recoverable`: an admitted attempt ended as
///   [`TxStatus::Aborted`](crate::TxStatus::Aborted) or
///   [`TxStatus::Fault`](crate::TxStatus::Fault); the port stays usable.
/// - `Poisoned`: `Err(Self::Error)`; the backend's state is unknown and only
///   a reset restores the port.
pub trait Ieee80211LowerMacPort {
    /// One owned event.
    type Event;
    /// Why the port cannot serve at all.
    type Error;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> LowerMacEvent<'_>;

    /// What the backend supports; it does not change while the port exists.
    fn capabilities(&self) -> LowerMacCapabilities;

    /// Admit one attempt. `Ok(Err(_))` when the backend refused it: nothing
    /// was sent. An admitted attempt reports exactly one
    /// [`LowerMacEvent::TxCompleted`] with its identity.
    fn submit(&self, attempt: TxAttempt<'_>) -> Result<Result<(), SubmitError>, Self::Error>;

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

    /// The TSF of a configured interface.
    fn tsf(&self, vif: VifId) -> Result<Result<Tsf, SettingError>, Self::Error>;
}
