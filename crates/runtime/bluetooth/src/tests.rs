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
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, InProcessHciHostTransport, LeControllerBootstrapConfig,
    LeControllerHciEndpoints, LeControllerHciResources,
};
use oer_bluetooth_radio::{
    ConnectionAllowances, EventId, EventResult, RadioDuration, RadioInstant, RadioOutcome,
    RadioRequest, RadioTiming, RequestError,
};

use crate::{LeRadioPort, NoRadio, OutcomesLost, ServeExit, serve};

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
    requests: RefCell<Vec<Recorded>>,
    outcomes: Channel<NoopRawMutex, Result<EventId, OutcomesLost>, 4>,
    refuse: RefCell<bool>,
}

impl ModelRadio {
    fn new() -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            outcomes: Channel::new(),
            refuse: RefCell::new(false),
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

    async fn until(&self, count: usize) {
        while self.advertised().len() < count {
            embassy_futures::yield_now().await;
        }
    }
}

impl LeRadioPort for ModelRadio {
    type Outcome = EventId;
    type Error = ();

    async fn clock(&self) -> Result<(RadioInstant, RadioTiming), ()> {
        Ok((
            RadioInstant::from_micros(embassy_time::Instant::now().as_micros()),
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

    async fn request(&self, request: RadioRequest<'_>) -> Result<Result<(), RequestError>, ()> {
        if *self.refuse.borrow() {
            return Ok(Err(RequestError::Busy));
        }
        self.requests.borrow_mut().push(match request {
            RadioRequest::ConfigureAdvertising(_) => Recorded::ConfigureAdvertising,
            RadioRequest::Advertise(event) => Recorded::Advertise(event.id),
            _ => Recorded::Other,
        });
        Ok(Ok(()))
    }

    async fn next_outcome(&self) -> Result<EventId, OutcomesLost> {
        self.outcomes.receive().await
    }

    fn view(id: &EventId) -> RadioOutcome<'_> {
        RadioOutcome::EventEnded {
            id: *id,
            result: EventResult::NotExecuted,
        }
    }
}

#[test]
fn serves_bootstrap_and_ends_when_the_transport_closes() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &NoRadio), async {
        host.write(&Reset::new()).await.unwrap();
        assert_eq!(status(&host).await, 0x00);
        // No random source: LE Rand is unknown.
        host.write(&LeRand::new()).await.unwrap();
        assert_eq!(status(&host).await, 0x01);
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn a_radio_without_hardware_fails_advertising_enable() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &NoRadio), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        assert_eq!(status(&host).await, 0x00);
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        assert_eq!(status(&host).await, 0x03);
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn advertising_events_follow_their_outcomes() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let radio = ModelRadio::new();
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio), async {
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

        radio.outcomes.send(Err(OutcomesLost)).await;
    }));
    assert_eq!(exit, ServeExit::OutcomesLost);
}

#[test]
fn refused_requests_are_retried_after_a_delay() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let radio = ModelRadio::new();
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &radio), async {
        host.write(&Reset::new()).await.unwrap();
        status(&host).await;
        host.write(&nonconnectable()).await.unwrap();
        status(&host).await;
        host.write(&LeSetAdvEnable::new(true)).await.unwrap();
        status(&host).await;
        radio.until(1).await;
        let first = radio.advertised()[0];
        *radio.refuse.borrow_mut() = true;
        radio.outcomes.send(Ok(first)).await;
        embassy_time::Timer::after_millis(5).await;
        *radio.refuse.borrow_mut() = false;
        radio.until(2).await;
        controller.close();
    }));
    assert_eq!(exit, ServeExit::Closed);
}

#[test]
fn host_data_without_a_connection_does_not_block_commands() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, 12>::new(controller_config(), None);
    let (exit, ()) = block_on(join(serve(&controller, &mut core, &NoRadio), async {
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
    }));
    assert_eq!(exit, ServeExit::Closed);
}
