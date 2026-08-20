use core::{
    future::{Future, pending, ready},
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use open_esp_radio_embassy_net::{
    Driver as _, NoopRawMutex, PinnedTxPool, SplitPinnedDevice, SplitPinnedResources, TxToken as _,
};
use open_esp_radio_esp32s31_wifi_mac::irq::{MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE};

use super::*;

const FRAME_CAPACITY: usize = 64;
const HEADROOM: usize = 32;
const TRAILER: usize = 8;
const QUEUE_DEPTH: usize = 1;

type Resources =
    SplitPinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, QUEUE_DEPTH>;
type Pool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
type Device = SplitPinnedDevice<
    'static,
    NoopRawMutex,
    FRAME_CAPACITY,
    HEADROOM,
    TRAILER,
    QUEUE_DEPTH,
    QUEUE_DEPTH,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestError {
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestExit {
    PeerLost,
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
    software_rx_work: bool,
    stop_after_tx: Option<&'static AtomicBool>,
}

impl Backend {
    fn push(&mut self, event: u8) {
        self.order[self.count] = event;
        self.count += 1;
    }
}

impl WdevServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Backend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn WdevNetworkRx,
        _context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            self.push(1);
            self.software_rx_work = false;
            if self.backpressure_once {
                self.backpressure_once = false;
                if self.repost_rx_when_backpressured {
                    self.irq.publish(MAC_INT_RX_SUCCESS);
                }
                return Ok(WdevRxProgress::StagingBackpressured);
            }
            if self.queue_control_on_rx {
                self.control_pending = true;
            }
            Ok(WdevRxProgress::Drained)
        }
    }

    fn has_rx_work(&self) -> bool {
        self.software_rx_work
    }

    fn service_control<'a>(
        &'a mut self,
        context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        async move {
            self.network_pending_seen |= context.network_tx_pending;
            if self.disconnect {
                return Ok(WdevControlProgress::Exit(TestExit::PeerLost));
            }
            if !self.control_pending {
                return Ok(WdevControlProgress::Idle);
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

struct RxProbeBackend {
    service_calls: u8,
}

struct NetworkBackpressureBackend {
    service_calls: u8,
}

impl WdevServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for NetworkBackpressureBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRx,
        _context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            self.service_calls = self.service_calls.saturating_add(1);
            if self.service_calls == 1 {
                network_rx.try_send(&[0; 14]).unwrap();
                Ok(WdevRxProgress::NetworkBackpressured)
            } else {
                Err(TestError::Finished)
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
}

impl WdevServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for RxProbeBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn WdevNetworkRx,
        _context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            self.service_calls = self.service_calls.saturating_add(1);
            if self.service_calls == 1 {
                Ok(WdevRxProgress::ProbePending)
            } else {
                Err(TestError::Finished)
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
}

struct CompletionFillBackend {
    order: [u8; 2],
    count: usize,
}

struct ShutdownBackend {
    stop_calls: u8,
    tx_services: u8,
}

impl WdevServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for ShutdownBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn WdevNetworkRx,
        _context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        pending()
    }

    fn service_stop(&mut self) -> Result<WdevStopProgress, Self::Error> {
        self.stop_calls = self.stop_calls.saturating_add(1);
        if self.stop_calls == 1 {
            Ok(WdevStopProgress::TxPending)
        } else {
            Ok(WdevStopProgress::Stopped)
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
        ready(())
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            assert_eq!(wake, WifiTxWake::Deadline);
            self.tx_services = self.tx_services.saturating_add(1);
            Ok(WifiTxProgress::Complete)
        }
    }
}

impl WdevServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for CompletionFillBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn WdevNetworkRx,
        _context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        pending()
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
        async { Ok(WifiTxProgress::Pending) }
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        pending()
    }

    fn service_tx<'a>(
        &'a mut self,
        _wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.order[self.count] = 2;
            self.count += 1;
            Err(TestError::Finished)
        }
    }

    fn can_prepare_tx(&self) -> bool {
        true
    }

    fn prepare_tx<'a>(
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
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            self.order[self.count] = 1;
            self.count += 1;
            Ok(())
        }
    }
}

impl WdevServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for PreparedBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn WdevNetworkRx,
        _context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        pending()
    }

    fn service_control<'a>(
        &'a mut self,
        _context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        async move {
            if self.control_pending {
                self.control_pending = false;
                self.order[self.count] = 1;
                self.count += 1;
                Ok(WdevControlProgress::More)
            } else {
                Ok(WdevControlProgress::Idle)
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
fn aggregate_batch_window_includes_the_first_published_frame() {
    assert!(!should_collect_network_batch(1, 1));
    assert!(should_collect_network_batch(32, 1));
    assert!(should_collect_network_batch(32, 2));
    assert!(should_collect_network_batch(32, 31));
    assert!(!should_collect_network_batch(32, 32));
    assert!(!should_collect_network_batch(32, 0));
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
    let mut runner = WdevRunner::new(irq, network, services);

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
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(WdevRunnerExit::Stopped)
    );
    assert!(runner.services().cancelled);
    assert!(!runner.services().prepared);
}

#[test]
fn caller_stop_drives_role_shutdown_tx_before_returning_owners() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = ShutdownBackend {
        stop_calls: 0,
        tx_services: 0,
    };
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(WdevRunnerExit::Stopped)
    );
    assert_eq!(runner.services().stop_calls, 2);
    assert_eq!(runner.services().tx_services, 1);
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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);
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
fn due_software_rx_frontier_runs_without_forging_a_hardware_irq() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
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
        queue_control_on_rx: true,
        control_pending: false,
        complete_tx_before_control: false,
        disconnect: false,
        network_pending_seen: false,
        backpressure_once: false,
        repost_rx_when_backpressured: false,
        software_rx_work: true,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[..2], [1, 3]);
    assert!(!irq.rx_signaled());
}

#[test]
fn due_software_rx_frontier_yields_to_tx_after_frame_deficit() {
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
        software_rx_work: true,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);
    runner.rx_frame_deficit = i64::from(RX_TX_FAIRNESS_QUANTUM_FRAMES);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[0], 2);
    assert!(runner.services().software_rx_work);
    runner.rx_frame_deficit = i64::from(RX_TX_FAIRNESS_QUANTUM_FRAMES);
    runner.account_tx_frames(16);
    assert_eq!(runner.rx_frame_deficit, -8);
}

#[test]
fn queued_tx_exports_exact_remaining_rx_frame_credit() {
    assert_eq!(rx_protocol_frame_budget(0, false), None);
    assert_eq!(rx_protocol_frame_budget(0, true), Some(8));
    assert_eq!(rx_protocol_frame_budget(8, true), Some(1));
    assert_eq!(rx_protocol_frame_budget(-8, true), Some(16));
}

#[test]
fn exhausted_rx_republication_is_serviced_again_without_a_new_hardware_irq() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(MAC_INT_RX_SUCCESS);
    let services = RxProbeBackend { service_calls: 0 };
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().service_calls, 2);
}

#[test]
fn network_backpressure_waits_for_the_network_rx_capacity_owner() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(MAC_INT_RX_SUCCESS);
    let services = NetworkBackpressureBackend { service_calls: 0 };
    let mut runner = WdevRunner::new(irq, network, services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    let received = device.receive(&mut context).unwrap();
    drop(received);
    assert_eq!(
        embassy_futures::block_on(run.as_mut()),
        Err(TestError::Finished)
    );
    drop(run);
    assert_eq!(runner.services().service_calls, 2);
}

#[test]
fn network_backpressure_still_services_a_new_dma_frontier() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(MAC_INT_RX_SUCCESS);
    let services = NetworkBackpressureBackend { service_calls: 0 };
    let mut runner = WdevRunner::new(irq, network, services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    // The final network queue is still occupied, but a later DMA completion
    // must re-enter the role service instead of waiting for network capacity.
    irq.publish(MAC_INT_RX_SUCCESS);
    assert_eq!(
        embassy_futures::block_on(run.as_mut()),
        Err(TestError::Finished)
    );
    drop(run);
    assert_eq!(runner.services().service_calls, 2);
}

#[test]
fn ready_network_frame_extends_standby_before_active_tx_completion() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = CompletionFillBackend {
        order: [0; 2],
        count: 0,
    };
    let mut runner = WdevRunner::new(irq, network, services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    // Start the first hardware transaction, then make a standby frame and
    // its completion interrupt ready in the same scheduler epoch.
    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    enqueue_frame(&mut device);
    irq.publish(MAC_INT_TX_COMPLETE);
    assert_eq!(
        embassy_futures::block_on(run.as_mut()),
        Err(TestError::Finished)
    );
    drop(run);
    assert_eq!(runner.services().order, [1, 2]);
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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(WdevRunnerExit::Stopped)
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
        software_rx_work: false,
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
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(stop_future)),
        Ok(WdevRunnerExit::Stopped)
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
        software_rx_work: false,
        stop_after_tx: None,
    };
    let mut runner = WdevRunner::new(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Ok(WdevRunnerExit::Role(TestExit::PeerLost))
    );
    let mut context = Context::from_waker(core::task::Waker::noop());
    assert!(matches!(
        device.link_state(&mut context),
        open_esp_radio_embassy_net::LinkState::Down
    ));
    let (_network, services) = runner.into_parts();
    assert!(services.disconnect);
}
