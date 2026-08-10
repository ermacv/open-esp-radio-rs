use core::{
    future::{Future, pending},
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use open_esp_radio_embassy_net::{
    Driver as _, NoopRawMutex, PinnedDevice, PinnedResources, PinnedTxPool, TxToken as _,
};
use open_esp_radio_esp32s31_wifi_mac::irq::{MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE};

use super::*;

const FRAME_CAPACITY: usize = 64;
const HEADROOM: usize = 32;
const TRAILER: usize = 8;
const QUEUE_DEPTH: usize = 1;

type Resources = PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
type Pool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
type Device = PinnedDevice<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestError {
    Finished,
}

struct Backend {
    irq: &'static EmbassyMacIrqRuntime<NoopRawMutex>,
    order: [u8; 3],
    count: usize,
    publish_irq: bool,
    deadline_ready: bool,
    tx_wake: Option<WifiTxWake>,
    queue_control_on_rx: bool,
    control_pending: bool,
    complete_tx_before_control: bool,
    disconnect: bool,
    network_pending_seen: bool,
    backpressure_once: bool,
    repost_rx_when_backpressured: bool,
    stop_after_tx: Option<&'static AtomicBool>,
}

impl Backend {
    fn push(&mut self, event: u8) {
        self.order[self.count] = event;
        self.count += 1;
    }
}

impl ConnectedRunnerServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Backend
{
    type Error = TestError;

    fn service_rx(&mut self) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + '_ {
        async move {
            self.push(1);
            if self.backpressure_once {
                self.backpressure_once = false;
                if self.repost_rx_when_backpressured {
                    self.irq.publish(MAC_INT_RX_SUCCESS);
                }
                return Ok(WifiRxProgress::Backpressured);
            }
            if self.queue_control_on_rx {
                self.control_pending = true;
            }
            Ok(WifiRxProgress::Drained)
        }
    }

    fn service_control<'a>(
        &'a mut self,
        context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        async move {
            self.network_pending_seen |= context.network_tx_pending;
            if self.disconnect {
                return Ok(WifiControlProgress::Disconnected(
                    ConnectedDisconnectReason::BeaconLoss,
                ));
            }
            if !self.control_pending {
                return Ok(WifiControlProgress::Idle);
            }
            self.control_pending = false;
            self.push(3);
            Err(TestError::Finished)
        }
    }

    fn start_tx<'a>(
        &'a mut self,
        _frame: PinnedTxFrame<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
        _network: &'a PinnedTxConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            if self.publish_irq {
                self.irq.publish(MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS);
            }
            Ok(WifiTxProgress::Pending)
        }
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        let ready = self.deadline_ready;
        async move {
            if !ready {
                pending::<()>().await;
            }
        }
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.tx_wake = Some(wake);
            self.push(2);
            if let Some(stop) = self.stop_after_tx {
                stop.store(true, Ordering::Release);
            }
            if self.complete_tx_before_control {
                Ok(WifiTxProgress::Complete)
            } else {
                Err(TestError::Finished)
            }
        }
    }
}

struct PreparedBackend {
    order: [u8; 2],
    count: usize,
    control_pending: bool,
    prepared: bool,
    cancelled: bool,
}

impl ConnectedRunnerServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for PreparedBackend
{
    type Error = TestError;

    fn service_rx(&mut self) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + '_ {
        pending()
    }

    fn service_control<'a>(
        &'a mut self,
        _context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        async move {
            if self.control_pending {
                self.control_pending = false;
                self.order[self.count] = 1;
                self.count += 1;
                Ok(WifiControlProgress::More)
            } else {
                Ok(WifiControlProgress::Idle)
            }
        }
    }

    fn start_tx<'a>(
        &'a mut self,
        _frame: PinnedTxFrame<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
        _network: &'a PinnedTxConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        pending()
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        pending()
    }

    fn service_tx<'a>(
        &'a mut self,
        _wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        pending()
    }

    fn has_prepared_tx(&self) -> bool {
        self.prepared
    }

    fn start_prepared_tx<'a>(
        &'a mut self,
        _network: &'a PinnedTxConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.prepared = false;
            self.order[self.count] = 2;
            self.count += 1;
            Err(TestError::Finished)
        }
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        self.prepared = false;
        self.cancelled = true;
        Ok(())
    }
}

fn enqueue_frame(device: &mut Device) {
    let mut context = Context::from_waker(core::task::Waker::noop());
    device
        .transmit(&mut context)
        .unwrap()
        .consume(14, |frame| frame.fill(0x5a));
}

#[test]
fn control_boundary_precedes_prepared_network_publication() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = PreparedBackend {
        order: [0; 2],
        count: 0,
        control_pending: true,
        prepared: true,
        cancelled: false,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order, [1, 2]);
}

#[test]
fn caller_stop_cancels_software_owned_prepared_network_tx() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = PreparedBackend {
        order: [0; 2],
        count: 0,
        control_pending: false,
        prepared: true,
        cancelled: false,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(ConnectedRunnerExit::Stopped)
    );
    assert!(runner.services().cancelled);
    assert!(!runner.services().prepared);
}

#[test]
fn frame_arriving_inside_select_rechecks_control_as_network_pending() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: true,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    enqueue_frame(&mut device);
    assert_eq!(
        embassy_futures::block_on(run.as_mut()),
        Err(TestError::Finished)
    );
    drop(run);
    assert!(runner.services().network_pending_seen);
}

#[test]
fn rx_is_serviced_before_tx_when_both_irqs_are_ready() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: true,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[..2], [1, 2]);
    assert_eq!(
        runner.services().tx_wake,
        Some(WifiTxWake::Interrupt {
            events: MAC_INT_TX_COMPLETE,
        })
    );
}

#[test]
fn staging_backpressure_gates_new_rx_edges_but_not_tx_completion() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: true,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: true,
        repost_rx_when_backpressured: true,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[..2], [1, 2]);
    assert_eq!(
        runner.services().tx_wake,
        Some(WifiTxWake::Interrupt {
            events: MAC_INT_TX_COMPLETE,
        })
    );
    assert!(irq.rx_signaled());
}

#[test]
fn executor_deadline_services_tx_without_an_interrupt() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: false,
        deadline_ready: true,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[0], 2);
    assert_eq!(runner.services().tx_wake, Some(WifiTxWake::Deadline));
}

#[test]
fn rx_control_waits_for_the_active_network_tx_then_precedes_another_lease() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: true,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: true,
        control_pending: false,
        complete_tx_before_control: true,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order, [1, 2, 3]);
}

#[test]
fn caller_stop_publishes_link_down_and_returns_distinct_outcome() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: false,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(ConnectedRunnerExit::Stopped)
    );
    let mut context = Context::from_waker(core::task::Waker::noop());
    assert!(matches!(
        device.link_state(&mut context),
        open_esp_radio_embassy_net::LinkState::Down
    ));
    let (_network, services) = runner.into_parts();
    assert!(!services.disconnect);
}

#[test]
fn caller_stop_waits_for_active_tx_to_release_hardware() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    enqueue_frame(&mut device);
    network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let stop = std::boxed::Box::leak(std::boxed::Box::new(AtomicBool::new(false)));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: true,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: true,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: Some(stop),
    };
    let stop_future = core::future::poll_fn(|context| {
        if stop.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            context.waker().wake_by_ref();
            Poll::Pending
        }
    });
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(stop_future)),
        Ok(ConnectedRunnerExit::Stopped)
    );
    assert_eq!(runner.services().order[..2], [1, 2]);
    assert_eq!(
        runner.services().tx_wake,
        Some(WifiTxWake::Interrupt {
            events: MAC_INT_TX_COMPLETE,
        })
    );
    let mut context = Context::from_waker(core::task::Waker::noop());
    assert!(matches!(
        device.link_state(&mut context),
        open_esp_radio_embassy_net::LinkState::Down
    ));
}

#[test]
fn disconnected_control_edge_publishes_link_down_and_returns() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    network.set_link_state(open_esp_radio_embassy_net::LinkState::Up);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = Backend {
        irq,
        order: [0; 3],
        count: 0,
        publish_irq: false,
        deadline_ready: false,
        tx_wake: None,
        queue_control_on_rx: false,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: true,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        stop_after_tx: None,
    };
    let mut runner = ConnectedRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Ok(ConnectedRunnerExit::Disconnected(
            ConnectedDisconnectReason::BeaconLoss
        ))
    );
    let mut context = Context::from_waker(core::task::Waker::noop());
    assert!(matches!(
        device.link_state(&mut context),
        open_esp_radio_embassy_net::LinkState::Down
    ));
    let (_network, services) = runner.into_parts();
    assert!(services.disconnect);
}
