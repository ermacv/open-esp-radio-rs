use core::cell::{Cell, RefCell};
use std::vec::Vec;

use core::{convert::Infallible, future::Future};
use embassy_futures::{block_on, join::join};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel};
use oer_bluetooth_controller::{LeController, LeControllerConfig};
use oer_bluetooth_hci::bt_hci::{
    ControllerToHostPacket,
    cmd::{
        controller_baseband::Reset,
        le::{LeRand, LeSetAdvEnable, LeSetAdvParams},
    },
    param::{AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration as HciDuration},
    transport::Transport,
};
use oer_bluetooth_hci::{BluetoothPublicDeviceAddress, LeControllerBootstrapConfig};
use oer_bluetooth_hci_transport::{
    InProcessHciHostTransport, LeControllerHciEndpoints, LeControllerHciResources,
};

use oer_bluetooth_radio::{
    CancelError, ClockError, ClockInfo, ConnectionAllowances, EventId, EventResult, EventsLost,
    LeInstant, LePhys, LeRadio, LeRadioCapabilities, LeRadioPort, LifecycleCommand, LifecycleError,
    LifecycleEvent, NoRadio, Poisoned, PortResult, RadioActivity, RadioDuration, RadioOutcome,
    RadioPort, RadioRequest, RadioTiming, RequestError,
};

use oer_time::Duration;
use oer_time_virtual::VirtualClock;

use crate::{ServeExit, serve};

type Resources = LeControllerHciResources<NoopRawMutex, 4, 4, 258>;
type Host<'c> = InProcessHciHostTransport<'c, NoopRawMutex, 4, 4, 258>;

fn config() -> LeControllerBootstrapConfig {
    LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap()
}

fn controller_config() -> LeControllerConfig {
    LeControllerConfig {
        bootstrap: config(),
        version: None,
    }
}

fn resources() -> Resources {
    Resources::new(config()).unwrap()
}

/// The status of the next Command Complete.
async fn status(host: &Host<'_>) -> u8 {
    let mut buffer = [0; 258];
    let ControllerToHostPacket::Event(event) = host.read(&mut buffer).await.unwrap() else {
        panic!("event expected");
    };
    assert_eq!(event.kind.0, 0x0e, "Command Complete expected");
    event.data[3]
}

fn nonconnectable() -> LeSetAdvParams {
    LeSetAdvParams::new(
        HciDuration::from_millis(100),
        HciDuration::from_millis(100),
        AdvKind::AdvNonconnInd,
        AddrKind::PUBLIC,
        AddrKind::PUBLIC,
        BdAddr::default(),
        AdvChannelMap::ALL,
        AdvFilterPolicy::default(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Recorded {
    ConfigureAdvertising,
    Advertise(EventId),
    Cancel(EventId),
    Other,
}

/// An owned event of the model radio.
#[derive(Clone, Copy, Debug)]
enum ModelEvent {
    /// The event ended, with a failed capture when `true`.
    Ended(EventId, bool),
    Lifecycle(LifecycleEvent),
}

/// A radio that takes every request, unless told to refuse, and reports the
/// outcomes the test pushes.
struct ModelRadio {
    /// The radio epoch's origin.
    started: std::time::Instant,
    now: Cell<Option<LeInstant>>,
    capture_failure: Cell<bool>,
    requests: RefCell<Vec<Recorded>>,
    outcomes: Channel<NoopRawMutex, Result<EventId, EventsLost>, 4>,
    lifecycle: Channel<NoopRawMutex, LifecycleEvent, 2>,
    refuse: RefCell<Option<RequestError>>,
    submitted: RefCell<usize>,
    activity: RefCell<Vec<RadioActivity>>,
}

impl ModelRadio {
    fn new() -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            started: std::time::Instant::now(),
            now: Cell::new(None),
            capture_failure: Cell::new(false),
            outcomes: Channel::new(),
            lifecycle: Channel::new(),
            refuse: RefCell::new(None),
            submitted: RefCell::new(0),
            activity: RefCell::new(Vec::new()),
        }
    }

    fn advertised(&self) -> Vec<EventId> {
        self.requests
            .borrow()
            .iter()
            .filter_map(|request| match request {
                Recorded::Advertise(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    async fn until_submitted(&self, count: usize) {
        while *self.submitted.borrow() < count {
            embassy_futures::yield_now().await;
        }
    }

    async fn until(&self, count: usize) {
        while self.advertised().len() < count {
            embassy_futures::yield_now().await;
        }
    }
}

/// The timing the model admits events with.
const TIMING: RadioTiming = RadioTiming {
    preparation_lead: RadioDuration::from_micros(300),
    admission_guard: RadioDuration::from_micros(200),
    connection: ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: RadioDuration::from_micros(1),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5047),
        first_event_length: RadioDuration::from_micros(5155),
    },
};

/// The model never poisons.
impl RadioPort for ModelRadio {
    type Event = ModelEvent;
    type Id = EventId;
    type Domain = LeRadio;
    type Fault = Infallible;

    async fn next_event(&self) -> PortResult<ModelEvent, EventsLost, Infallible> {
        if let Ok(event) = self.lifecycle.try_receive() {
            return Ok(Ok(ModelEvent::Lifecycle(event)));
        }
        Ok(self
            .outcomes
            .receive()
            .await
            .map(|id| ModelEvent::Ended(id, self.capture_failure.get())))
    }

    async fn now(&self) -> PortResult<LeInstant, ClockError, Infallible> {
        Ok(Ok(self.now.get().unwrap_or_else(|| {
            LeInstant::from_micros(self.started.elapsed().as_micros() as u64)
        })))
    }

    /// The event's end follows from the test.
    async fn cancel(&self, id: EventId) -> PortResult<(), CancelError, Infallible> {
        self.requests.borrow_mut().push(Recorded::Cancel(id));
        Ok(Ok(()))
    }

    async fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> PortResult<(), LifecycleError, Infallible> {
        let terminal = match command {
            LifecycleCommand::Enable => LifecycleEvent::Enabled,
            LifecycleCommand::Disable => LifecycleEvent::Disabled,
            LifecycleCommand::Quiesce => LifecycleEvent::Quiesced,
        };
        self.lifecycle.send(terminal).await;
        Ok(Ok(()))
    }
}

impl LeRadioPort for ModelRadio {
    fn clock_info(&self) -> ClockInfo {
        ClockInfo::MONOTONIC_MICROS
    }

    /// The model serves the advertising the tests drive.
    fn capabilities(&self) -> LeRadioCapabilities {
        LeRadioCapabilities {
            legacy_advertising: true,
            phys: LePhys::LE_1M,
            timing: TIMING,
            ..LeRadioCapabilities::NONE
        }
    }

    async fn submit(&self, request: RadioRequest<'_>) -> PortResult<(), RequestError, Infallible> {
        *self.submitted.borrow_mut() += 1;
        if let Some(error) = *self.refuse.borrow() {
            return Ok(Err(error));
        }
        self.requests.borrow_mut().push(match request {
            RadioRequest::ConfigureAdvertising(_) => Recorded::ConfigureAdvertising,
            RadioRequest::Advertise(event) => Recorded::Advertise(event.id),
            _ => Recorded::Other,
        });
        Ok(Ok(()))
    }

    fn view(event: &ModelEvent) -> RadioOutcome<'_> {
        use oer_bluetooth_radio::{CaptureError, TimingError};
        match *event {
            ModelEvent::Lifecycle(event) => RadioOutcome::Lifecycle(event),
            ModelEvent::Ended(id, failed) => RadioOutcome::EventEnded {
                id,
                result: if failed {
                    EventResult::TimingFailed {
                        cause: CaptureError::PacketStartCorrection(TimingError::BeforeEpoch),
                        executed: true,
                        anchor: Ok(None),
                    }
                } else {
                    EventResult::NotExecuted
                },
            },
        }
    }

    fn activity(&self, activity: RadioActivity) -> Result<(), Poisoned<Infallible>> {
        self.activity.borrow_mut().push(activity);
        Ok(())
    }
}

#[test]
fn serves_bootstrap_and_ends_when_the_transport_closes() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let (exit, ()) = block_on(join(
        serve(&controller, &mut core, &NoRadio, &clock),
        async {
            host.write(&Reset::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            // No random source: LE Rand is unknown.
            host.write(&LeRand::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x01);
            controller.close();
        },
    ));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn a_radio_without_hardware_fails_advertising_enable() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let (exit, ()) = block_on(join(
        serve(&controller, &mut core, &NoRadio, &clock),
        async {
            host.write(&Reset::new()).await.unwrap();
            status(&host).await;
            host.write(&nonconnectable()).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            host.write(&LeSetAdvEnable::new(true)).await.unwrap();
            assert_eq!(status(&host).await, 0x03);
            controller.close();
        },
    ));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn advertising_events_follow_their_outcomes() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio, &clock), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        assert_eq!(status(&host).await, 0x00);
        assert_eq!(radio.requests.borrow()[0], Recorded::ConfigureAdvertising);

        radio.until(1).await;
        let first = radio.advertised()[0];
        // The next event waits for the first one's end.
        for _ in 0..16 {
            embassy_futures::yield_now().await;
        }
        assert_eq!(radio.advertised().len(), 1);
        radio.outcomes.send(Ok(first)).await;
        radio.until(2).await;
        assert_ne!(radio.advertised()[1], first);

        // A loss holds received PDUs only: the service goes on, and the
        // second event's end still plans the third.
        radio.outcomes.send(Err(EventsLost)).await;
        let second = radio.advertised()[1];
        radio.outcomes.send(Ok(second)).await;
        radio.until(3).await;
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn refused_requests_are_retried_after_a_delay() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio, &clock), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        status(&host).await;
        radio.until(1).await;
        let first = radio.advertised()[0];
        *radio.refuse.borrow_mut() = Some(RequestError::Busy);
        let before = *radio.submitted.borrow();
        radio.outcomes.send(Ok(first)).await;
        radio.until_submitted(before + 1).await;
        // Refused: no retry before the delay.
        for _ in 0..8 {
            embassy_futures::yield_now().await;
        }
        assert_eq!(*radio.submitted.borrow(), before + 1);
        *radio.refuse.borrow_mut() = None;
        clock.advance(Duration::from_millis(1)).unwrap();
        radio.until(2).await;
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn an_unsupported_request_is_not_retried_on_a_timer() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio, &clock), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        status(&host).await;
        radio.until(1).await;
        let first = radio.advertised()[0];
        *radio.refuse.borrow_mut() = Some(RequestError::Unsupported);
        let before = *radio.submitted.borrow();
        radio.outcomes.send(Ok(first)).await;
        radio.until_submitted(before + 1).await;
        clock.advance(Duration::from_millis(5)).unwrap();
        for _ in 0..8 {
            embassy_futures::yield_now().await;
        }
        // One refusal after the outcome, and no timed retry.
        assert_eq!(*radio.submitted.borrow(), before + 1);
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

/// The cause of the poisoned model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelFault;

const POISONED: Poisoned<ModelFault> = Poisoned { cause: ModelFault };

/// A model whose backend's state is unknown: every call is poisoned.
struct PoisonedRadio;

impl RadioPort for PoisonedRadio {
    type Event = Infallible;
    type Id = EventId;
    type Domain = LeRadio;
    type Fault = ModelFault;

    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<Infallible, EventsLost, ModelFault>> + '_ {
        core::future::ready(Err(POISONED))
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, ModelFault>> + '_ {
        core::future::ready(Err(POISONED))
    }

    fn cancel(
        &self,
        _: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, ModelFault>> + '_ {
        core::future::ready(Err(POISONED))
    }

    fn lifecycle(
        &self,
        _: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, ModelFault>> + '_ {
        core::future::ready(Err(POISONED))
    }
}

impl LeRadioPort for PoisonedRadio {
    fn capabilities(&self) -> LeRadioCapabilities {
        LeRadioCapabilities::NONE
    }

    fn clock_info(&self) -> ClockInfo {
        NoRadio.clock_info()
    }

    fn submit(
        &self,
        _: RadioRequest<'_>,
    ) -> impl Future<Output = PortResult<(), RequestError, ModelFault>> {
        core::future::ready(Err(POISONED))
    }

    fn view(event: &Infallible) -> RadioOutcome<'_> {
        match *event {}
    }

    fn activity(&self, _: RadioActivity) -> Result<(), Poisoned<ModelFault>> {
        Err(POISONED)
    }
}

#[test]
fn a_poisoned_port_ends_the_service_with_its_cause() {
    let mut resources = resources();
    let LeControllerHciEndpoints { controller, .. } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    assert_eq!(
        block_on(serve(&controller, &mut core, &PoisonedRadio, &clock)),
        ServeExit::Poisoned(POISONED)
    );
}

#[test]
fn host_data_without_a_connection_does_not_block_commands() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let (exit, ()) = block_on(join(
        serve(&controller, &mut core, &NoRadio, &clock),
        async {
            host.write(&Reset::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            let data = [1, 2, 3];
            host.write(&oer_bluetooth_hci::bt_hci::data::AclPacket::new(
                oer_bluetooth_hci::bt_hci::param::ConnHandle::new(0),
                oer_bluetooth_hci::bt_hci::data::AclPacketBoundary::FirstNonFlushable,
                oer_bluetooth_hci::bt_hci::data::AclBroadcastFlag::PointToPoint,
                &data,
            ))
            .await
            .unwrap();
            host.write(&LeRand::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x01);
            controller.close();
        },
    ));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn advertising_enable_and_disable_report_the_active_roles() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let advertising = RadioActivity {
        advertising: true,
        ..RadioActivity::IDLE
    };
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio, &clock), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        // Nothing active: nothing reported.
        assert!(radio.activity.borrow().is_empty());
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        assert_eq!(status(&host).await, 0x00);
        radio.until(1).await;
        assert_eq!(*radio.activity.borrow(), [advertising]);

        // The disable cancels the event in flight through the port's
        // cancellation and completes once it has ended; advertising stays
        // active until then.
        host.write(&LeSetAdvEnable::new(false)).await.unwrap();
        let last = *radio.advertised().last().expect("an event was requested");
        while !radio.requests.borrow().contains(&Recorded::Cancel(last)) {
            embassy_futures::yield_now().await;
        }
        assert_eq!(*radio.activity.borrow(), [advertising]);
        radio.outcomes.send(Ok(last)).await;
        assert_eq!(status(&host).await, 0x00);
        assert_eq!(*radio.activity.borrow(), [advertising, RadioActivity::IDLE]);
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn a_capture_failure_settles_the_operation_and_the_same_service_continues() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    radio.capture_failure.set(true);
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio, &clock), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        assert_eq!(status(&host).await, 0);
        radio.until(1).await;
        let first = radio.advertised()[0];
        radio.outcomes.send(Ok(first)).await;
        radio.until(2).await;
        assert_ne!(radio.advertised()[1], first);
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn required_epoch_exhaustion_exits_with_context_and_retains_the_core() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio, &clock), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        assert_eq!(status(&host).await, 0);
        radio.until(1).await;
        radio.now.set(Some(LeInstant::from_micros(u64::MAX)));
        radio.outcomes.send(Ok(radio.advertised()[0])).await;
    }));
    let ServeExit::Planning(error) = exit else {
        panic!("required planning must end service: {exit:?}")
    };
    assert_eq!(
        error.role,
        oer_bluetooth_controller::PlanningRole::Advertising
    );
    assert_eq!(
        error.calculation,
        oer_bluetooth_controller::PlanningCalculation::Admission
    );
    assert_eq!(radio.advertised().len(), 1);
    assert_ne!(
        core.activity(),
        RadioActivity::IDLE,
        "lifecycle retains the active logical owner"
    );
}
