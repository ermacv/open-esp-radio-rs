//! The radio port a Controller service loop drives.

use core::future::{Future, pending, ready};

use crate::{
    ConnectionAllowances, LeRadioCapabilities, RadioActivity, RadioDuration, RadioInstant,
    RadioOutcome, RadioRequest, RadioTiming, RequestError,
};

/// The radio queue overflowed and dropped outcomes; the roles can no longer
/// account their events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutcomesLost;

/// A Bluetooth LE radio backend as a Controller service loop drives it.
///
/// The loop reads [`Self::capabilities`] and [`Self::clock`], submits one
/// [`RadioRequest`] at a time, takes owned outcomes from
/// [`Self::next_outcome`] and reads each through [`Self::view`].
///
/// Failures come in three classes:
///
/// - `Ok(Err(RequestError))` from [`Self::submit`]: the backend refused the
///   request and nothing changed; the caller may try again later.
/// - `Err(Self::Error)`: the port cannot serve any more; the service ends.
/// - [`OutcomesLost`] from [`Self::next_outcome`]: outcomes were dropped, so
///   the roles cannot account their events; the service ends.
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
    /// Why the port cannot serve at all.
    type Error;

    /// What the backend serves. It does not change while the port exists.
    fn capabilities(&self) -> LeRadioCapabilities;

    /// A fresh radio time and the radio's admission timing.
    fn clock(&self) -> impl Future<Output = Result<(RadioInstant, RadioTiming), Self::Error>> + '_;

    /// Submit one request: `Ok(Err(_))` when the radio refused it. A request
    /// the capabilities exclude is refused as [`RequestError::Unsupported`].
    fn submit(
        &self,
        request: RadioRequest<'_>,
    ) -> impl Future<Output = Result<Result<(), RequestError>, Self::Error>>;

    /// The next outcome. Dropping the future loses no outcome.
    fn next_outcome(&self) -> impl Future<Output = Result<Self::Outcome, OutcomesLost>> + '_;

    /// The portable view of an owned outcome.
    fn view(outcome: &Self::Outcome) -> RadioOutcome<'_>;

    /// The roles active now. The loop reports every change, starting from
    /// [`RadioActivity::IDLE`], so a radio that shares the antenna can
    /// publish them to its coexistence arbiter.
    fn activity(&self, activity: RadioActivity) -> Result<(), Self::Error>;
}

/// A port without a radio: time stands still and every request is refused
/// as unavailable. Radio commands then complete with a failure status.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRadio;

/// [`NoRadio`] never fails and never produces an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Never {}

const ZERO: RadioDuration = RadioDuration::from_micros(0);

impl LeRadioPort for NoRadio {
    type Outcome = Never;
    type Error = Never;

    fn capabilities(&self) -> LeRadioCapabilities {
        LeRadioCapabilities::NONE
    }

    fn clock(&self) -> impl Future<Output = Result<(RadioInstant, RadioTiming), Never>> + '_ {
        ready(Ok((
            RadioInstant::from_micros(0),
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
        ready(Ok(Err(RequestError::Unavailable)))
    }

    fn next_outcome(&self) -> impl Future<Output = Result<Never, OutcomesLost>> + '_ {
        pending()
    }

    fn view(outcome: &Never) -> RadioOutcome<'_> {
        match *outcome {}
    }

    fn activity(&self, _: RadioActivity) -> Result<(), Never> {
        Ok(())
    }
}
