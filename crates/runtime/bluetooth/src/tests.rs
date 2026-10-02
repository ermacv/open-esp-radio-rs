use core::cell::RefCell;
use std::vec::Vec;

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
    ClockInfo, ConnectionAllowances, EventId, EventResult, EventsLost, FailureClass, LePhys,
    LeRadioCapabilities, LeRadioPort, NoRadio, PortError, RadioActivity, RadioDuration,
    RadioInstant, RadioOutcome, RadioRequest, RadioTiming, RequestError,
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
    Other,
}

/// A radio that takes every request, unless told to refuse, and reports the
/// outcomes the test pushes.
struct ModelRadio {
    /// The radio epoch's origin.
    started: std::time::Instant,
    requests: RefCell<Vec<Recorded>>,
    outcomes: Channel<NoopRawMutex, Result<EventId, EventsLost>, 4>,
    refuse: RefCell<Option<RequestError>>,
    submitted: RefCell<usize>,
    activity: RefCell<Vec<RadioActivity>>,
}

impl ModelRadio {
    fn new() -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            started: std::time::Instant::now(),
            outcomes: Channel::new(),
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

/// The model never fails as a whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelError;

impl PortError for ModelError {
    fn class(&self) -> FailureClass {
        FailureClass::Rejected
    }
}

impl LeRadioPort for ModelRadio {
    type Outcome = EventId;
    type Error = ModelError;

    fn clock_info(&self) -> ClockInfo {
        ClockInfo::MONOTONIC_MICROS
    }

    /// The model serves the advertising the tests drive.
    fn capabilities(&self) -> LeRadioCapabilities {
        LeRadioCapabilities {
            legacy_advertising: true,
            phys: LePhys::LE_1M,
            ..LeRadioCapabilities::NONE
        }
    }

    async fn clock(&self) -> Result<(RadioInstant, RadioTiming), ModelError> {
        Ok((
            RadioInstant::from_micros(self.started.elapsed().as_micros() as u64),
            RadioTiming {
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
            },
        ))
    }

    async fn submit(
        &self,
        request: RadioRequest<'_>,
    ) -> Result<Result<(), RequestError>, ModelError> {
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

    async fn next_outcome(&self) -> Result<EventId, EventsLost> {
        self.outcomes.receive().await
    }

    fn view(id: &EventId) -> RadioOutcome<'_> {
        RadioOutcome::EventEnded {
            id: *id,
            result: EventResult::NotExecuted,
        }
    }

    fn activity(&self, activity: RadioActivity) -> Result<(), ModelError> {
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

        radio.outcomes.send(Err(EventsLost)).await;
    }));
    assert_eq!(exit, ServeExit::EventsLost(EventsLost));
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

/// A model whose outcome stream reports the terminal poisoned outcome.
struct PoisonedRadio;

impl LeRadioPort for PoisonedRadio {
    type Outcome = ();
    type Error = ModelError;

    fn capabilities(&self) -> LeRadioCapabilities {
        LeRadioCapabilities::NONE
    }

    fn clock_info(&self) -> ClockInfo {
        NoRadio.clock_info()
    }

    async fn clock(&self) -> Result<(RadioInstant, RadioTiming), ModelError> {
        Err(ModelError)
    }

    async fn submit(&self, _: RadioRequest<'_>) -> Result<Result<(), RequestError>, ModelError> {
        Err(ModelError)
    }

    async fn next_outcome(&self) -> Result<(), EventsLost> {
        Ok(())
    }

    fn view((): &()) -> RadioOutcome<'_> {
        RadioOutcome::Poisoned(oer_bluetooth_radio::Poisoned)
    }

    fn activity(&self, _: RadioActivity) -> Result<(), ModelError> {
        Ok(())
    }
}

#[test]
fn the_terminal_poisoned_outcome_ends_the_service() {
    let mut resources = resources();
    let LeControllerHciEndpoints { controller, .. } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    assert_eq!(
        block_on(serve(&controller, &mut core, &PoisonedRadio, &clock)),
        ServeExit::Poisoned
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

        // The disable cancels the event in flight and completes once it
        // has ended; advertising stays active until then.
        host.write(&LeSetAdvEnable::new(false)).await.unwrap();
        while !radio.requests.borrow().contains(&Recorded::Other) {
            embassy_futures::yield_now().await;
        }
        assert_eq!(*radio.activity.borrow(), [advertising]);
        let last = *radio.advertised().last().expect("an event was requested");
        radio.outcomes.send(Ok(last)).await;
        assert_eq!(status(&host).await, 0x00);
        assert_eq!(*radio.activity.borrow(), [advertising, RadioActivity::IDLE]);
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}
