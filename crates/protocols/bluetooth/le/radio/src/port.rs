//! The radio port a Controller service loop drives.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

use oer_radio_port::{
    CancelError, ClockError, ClockInfo, EventsLost, LifecycleCommand, LifecycleError, Poisoned,
    PortResult, RadioPort,
};

use crate::{
    EventId, LeCapabilities, LeInstant, LeRadio, NoCapabilities, RadioActivity, RadioOutcome,
    RadioRequest, RequestError,
};

/// A Bluetooth LE radio backend as a Controller service loop drives it.
///
/// The event stream, the radio clock, cancellation and the lifecycle are
/// the shared [`RadioPort`] base. Its capabilities are a type
/// ([`Self::Caps`]), known when the image is built, with the
/// [`RadioTiming`](crate::RadioTiming) a planner keeps reservations apart
/// by. The loop reads the radio time with [`RadioPort::now`],
/// submits one [`RadioRequest`] at a time, takes owned events from
/// [`RadioPort::next_event`] and reads each through [`Self::view`].
///
/// Every call but [`Self::activity`], which cannot be refused, returns a
/// [`PortResult`]. Its inner `Err` is a refusal and
/// nothing changed: [`RequestError::Disabled`] while the port is disabled or
/// quiesced, and [`RequestError::Late`] when time passed between placing a
/// request and its admission; other refusals may succeed later. A request
/// the capabilities exclude cannot be built ([`crate::Supported`]). The
/// outer [`Poisoned`](oer_radio_port::Poisoned) means the backend's state is
/// unknown, with its cause: every later call returns it and only a reset
/// restores the port.
///
/// # Events
///
/// The port has exactly one consumer of its events. Taking an event only
/// dequeues it; the backend's scheduler work runs in its own runner (the
/// ESP32-S31 `BluetoothRuntime::run`), which the composition polls beside
/// the consumer. The backend reserves the slots of an admitted event's end
/// ([`RadioOutcome::EventEnded`], with the
/// [`RadioOutcome::TransmitAcknowledged`] or [`RadioOutcome::TestReport`]
/// before it), of the data PDUs a connection event receives (the Link Layer
/// promises the peer reliable delivery of what the hardware acknowledged)
/// and of a lifecycle terminal when it admits them, so they are never lost;
/// [`EventsLost`] stands for advertising and scan reports only.
///
/// # Lifecycle and cancellation
///
/// The port value exists only while its backend is installed: the
/// backend's install returns it and its uninstall consumes it, moving the
/// memory and hardware owners, so no call is refused as not installed.
/// `Enable` starts admitting events; `Disable` ends every
/// admitted event with its end and stops admitting; `Quiesce` stops
/// admitting and lets the admitted events end, then reports `Quiesced`.
/// [`RadioPort::cancel`] withdraws one scheduled event, whose end still
/// follows.
///
/// Submission and the clock are asynchronous because a backend may have to
/// wait for hardware to admit a request: the ESP32-S31 takes a fresh
/// controller-time latch for every request and every clock reading, which
/// completes only after the hardware latched the time, and shares its radio
/// with a runner that holds it across scheduler stop sequences. Neither wait
/// depends on the caller; both end within the backend's own bounds.
///
/// Who acknowledges data PDUs is part of the capabilities: either way
/// the port carries only LLID and payload, reports
/// [`RadioOutcome::TransmitAcknowledged`] once the peer acknowledged the
/// connection's queued PDU and reports only new, non-empty receptions.
pub trait LeRadioPort: RadioPort<Id = EventId, Domain = LeRadio> {
    /// What the backend serves, known when the image is built.
    type Caps: LeCapabilities;

    /// The resolution of the radio time [`RadioPort::now`] reads and how its
    /// epoch relates to the image's monotonic time.
    fn clock_info(&self) -> ClockInfo<LeRadio>;

    /// Submit one request: `Ok(Err(_))` when the radio refused it.
    fn submit(
        &self,
        request: RadioRequest<'_, Self::Caps>,
    ) -> impl Future<Output = PortResult<(), RequestError, Self::Fault>>;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> RadioOutcome<'_>;

    /// The roles active now. The loop reports every change, starting from
    /// [`RadioActivity::IDLE`], so a radio that shares the antenna can
    /// publish them to its coexistence arbiter.
    fn activity(&self, activity: RadioActivity) -> Result<(), Poisoned<Self::Fault>>;
}

/// A port without a radio: time stands still and its capabilities serve
/// nothing ([`NoCapabilities`]), so no request but a release can be built,
/// and a release names nothing configured ([`RequestError::Unknown`]).
/// Radio commands then complete with a failure status. It admits nothing,
/// so enabling or disabling it changes nothing (`AlreadyInState`), and it
/// runs nothing to cancel or quiesce.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRadio;

impl RadioPort for NoRadio {
    // It never poisons and never produces an event.
    type Event = Infallible;
    type Id = EventId;
    type Domain = LeRadio;
    type Fault = Infallible;

    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<Infallible, EventsLost, Infallible>> + '_ {
        pending()
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, Infallible>> + '_ {
        ready(Ok(Ok(LeInstant::from_micros(0))))
    }

    fn cancel(
        &self,
        _: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, Infallible>> + '_ {
        ready(Ok(Err(CancelError::NotRunning)))
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Infallible>> + '_ {
        ready(Ok(Err(match command {
            LifecycleCommand::Enable | LifecycleCommand::Disable => LifecycleError::AlreadyInState,
            LifecycleCommand::Quiesce => LifecycleError::InvalidState,
        })))
    }
}

impl LeRadioPort for NoRadio {
    type Caps = NoCapabilities;

    fn clock_info(&self) -> ClockInfo<LeRadio> {
        ClockInfo {
            resolution: oer_time::Duration::from_micros(1),
            monotonic: None,
        }
    }

    fn submit(
        &self,
        _: RadioRequest<'_, NoCapabilities>,
    ) -> impl Future<Output = PortResult<(), RequestError, Infallible>> {
        ready(Ok(Err(RequestError::Unknown)))
    }

    fn view(event: &Infallible) -> RadioOutcome<'_> {
        match *event {}
    }

    fn activity(&self, _: RadioActivity) -> Result<(), Poisoned<Infallible>> {
        Ok(())
    }
}
