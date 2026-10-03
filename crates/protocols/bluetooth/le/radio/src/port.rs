//! The radio port a Controller service loop drives.

use core::future::{Future, pending, ready};

use oer_radio_port::{ClockInfo, EventsLost, FailureClass, PortError, RadioEpoch};

use crate::{
    ConnectionAllowances, LeInstant, LeRadioCapabilities, RadioActivity, RadioDuration,
    RadioOutcome, RadioRequest, RadioTiming, RequestError,
};

/// A Bluetooth LE radio backend as a Controller service loop drives it.
///
/// The loop reads [`Self::capabilities`] and [`Self::clock`], submits one
/// [`RadioRequest`] at a time, takes owned outcomes from
/// [`Self::next_outcome`] and reads each through [`Self::view`].
///
/// Failures come in the three classes of every radio port
/// ([`FailureClass`]):
///
/// - `Rejected`: `Ok(Err(RequestError))` from [`Self::submit`], or an error
///   of that class (no radio installed); nothing changed. A refusal as
///   [`RequestError::Unsupported`] is final for that request; other
///   refusals may succeed later.
/// - `Recoverable`: admitted work ended without its result (an event that
///   ended [`EventResult::NotExecuted`](crate::EventResult::NotExecuted), a
///   time sample the backend could not take).
/// - `Poisoned`: the backend's state is unknown. It reports
///   [`RadioOutcome::Fault`] with the cause and then the terminal
///   [`RadioOutcome::Poisoned`] at every [`Self::next_outcome`], and every
///   other call returns an error of that class; only a reset restores it.
///
/// [`EventsLost`] from [`Self::next_outcome`] takes the place of the first
/// dropped outcome. The Controller's roles account every event by its end,
/// so a service loop that cannot recover them ends there.
///
/// # Events
///
/// The port has exactly one consumer of its outcomes. Taking an outcome
/// only dequeues it; the backend's scheduler work runs in its own runner
/// (the ESP32-S31 `BluetoothRuntime::run`), which the composition polls
/// beside the consumer.
///
/// # Lifecycle
///
/// The port has no lifecycle commands: a backend is enabled by installing
/// it with its memory and hardware owners and disabled by uninstalling it,
/// and a maintenance pause hands a quiescence proof to a closure. Those
/// owner transfers are not portable values, so they stay the backend's
/// own operations (the ESP32-S31 `install`, `quiesce` and `uninstall`).
///
/// Submission and the clock are asynchronous because a backend may have to
/// wait for hardware to admit a request: the ESP32-S31 takes a fresh
/// controller-time latch for every request and every clock reading, which
/// completes only after the hardware latched the time, and shares its radio
/// with a runner that holds it across scheduler stop sequences. Neither wait
/// depends on the caller; both end within the backend's own bounds.
///
/// Who acknowledges data PDUs is part of [`LeRadioCapabilities`]: either way
/// the port carries only LLID and payload, reports
/// [`RadioOutcome::TransmitAcknowledged`] once the peer acknowledged the
/// connection's queued PDU and reports only new, non-empty receptions.
pub trait LeRadioPort {
    /// One owned outcome.
    type Outcome;
    /// Why the port cannot serve; its class says whether it will again.
    type Error: PortError;

    /// What the backend serves. It does not change while the port exists.
    fn capabilities(&self) -> LeRadioCapabilities;

    /// The resolution of the radio time [`Self::clock`] reads and how its
    /// epoch relates to the image's monotonic time.
    fn clock_info(&self) -> ClockInfo;

    /// A fresh radio time and the radio's admission timing.
    fn clock(&self) -> impl Future<Output = Result<(LeInstant, RadioTiming), Self::Error>> + '_;

    /// Submit one request: `Ok(Err(_))` when the radio refused it. A request
    /// the capabilities exclude is refused as [`RequestError::Unsupported`].
    fn submit(
        &self,
        request: RadioRequest<'_>,
    ) -> impl Future<Output = Result<Result<(), RequestError>, Self::Error>>;

    /// The next outcome. Taking it only dequeues it; dropping the future
    /// loses no outcome. A queue overflow is reported as [`EventsLost`] in
    /// place of the first dropped outcome.
    fn next_outcome(&self) -> impl Future<Output = Result<Self::Outcome, EventsLost>> + '_;

    /// The portable view of an owned outcome.
    fn view(outcome: &Self::Outcome) -> RadioOutcome<'_>;

    /// The roles active now. The loop reports every change, starting from
    /// [`RadioActivity::IDLE`], so a radio that shares the antenna can
    /// publish them to its coexistence arbiter.
    fn activity(&self, activity: RadioActivity) -> Result<(), Self::Error>;
}

/// A port without a radio: time stands still and every request lies
/// outside its empty capabilities, so it is refused as
/// [`RequestError::Unsupported`]. Radio commands then complete with a
/// failure status.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRadio;

/// [`NoRadio`] never fails and never produces an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Never {}

impl PortError for Never {
    fn class(&self) -> FailureClass {
        match *self {}
    }
}

const ZERO: RadioDuration = RadioDuration::from_micros(0);

impl LeRadioPort for NoRadio {
    type Outcome = Never;
    type Error = Never;

    fn capabilities(&self) -> LeRadioCapabilities {
        LeRadioCapabilities::NONE
    }

    fn clock_info(&self) -> ClockInfo {
        ClockInfo {
            resolution: oer_time::Duration::from_micros(1),
            epoch: RadioEpoch::Unrelated,
        }
    }

    fn clock(&self) -> impl Future<Output = Result<(LeInstant, RadioTiming), Never>> + '_ {
        ready(Ok((
            LeInstant::from_micros(0),
            RadioTiming {
                preparation_lead: ZERO,
                admission_guard: ZERO,
                connection: ConnectionAllowances {
                    local_sleep_clock_ppm: 0,
                    widening_jitter: ZERO,
                    receive_guard: ZERO,
                    receive_tail: ZERO,
                    boundary_guard: ZERO,
                    first_event_guard: ZERO,
                    event_length: ZERO,
                    first_event_length: ZERO,
                },
            },
        )))
    }

    fn submit(
        &self,
        _: RadioRequest<'_>,
    ) -> impl Future<Output = Result<Result<(), RequestError>, Never>> {
        ready(Ok(Err(RequestError::Unsupported)))
    }

    fn next_outcome(&self) -> impl Future<Output = Result<Never, EventsLost>> + '_ {
        pending()
    }

    fn view(outcome: &Never) -> RadioOutcome<'_> {
        match *outcome {}
    }

    fn activity(&self, _: RadioActivity) -> Result<(), Never> {
        Ok(())
    }
}
