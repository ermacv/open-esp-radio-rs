#![expect(
    clippy::manual_async_fn,
    reason = "owner-graph test doubles implement the production borrowed Future contracts"
)]

use core::{
    future::{Future, pending, ready},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::{Context, Poll},
};

use open_esp_radio_embassy_net::{
    Driver as _, DualPinnedNetworkRunner, NetworkInterfaceId, NoopRawMutex,
    PinnedEndpointResources, PinnedNetworkRunner, PinnedTxPool, PinnedTxResources, RxEnqueueError,
    SplitPinnedDevice, TxToken as _,
};
use open_esp_radio_esp32s31_wifi_mac::irq::{EVENT_RX_SUCCESS, EVENT_TX_COMPLETE};
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use std::boxed::Box;

use super::network::{DatapathNetworkRx, DatapathNetworkRxEndpoints, DatapathNetworkRxSet};
use super::*;

const FRAME_CAPACITY: usize = 64;
const HEADROOM: usize = 32;
const TRAILER: usize = 8;
const QUEUE_DEPTH: usize = 1;

static RX_UNMASK_CALLS: AtomicU32 = AtomicU32::new(0);

fn record_rx_unmask() {
    RX_UNMASK_CALLS.fetch_add(1, Ordering::Relaxed);
}

type Resources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, QUEUE_DEPTH>;
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

macro_rules! split_network {
    ($resources:expr, $pool:expr) => {{
        let tx_resources = std::boxed::Box::leak(std::boxed::Box::new(PinnedTxResources::new()));
        let (provider, consumer) = tx_resources.split($pool);
        let (device, rx) =
            $resources.split(provider, NetworkInterfaceId::new(0), [2, 3, 4, 5, 6, 7]);
        (
            device,
            PinnedNetworkRunner::new(NetworkInterfaceId::new(0), rx, consumer),
        )
    }};
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestError {
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestExit {
    PeerLost,
}

#[derive(Default)]
struct EndpointRx {
    frames: std::vec::Vec<std::vec::Vec<u8>>,
}

impl DatapathNetworkRx for EndpointRx {
    fn queue_len(&self) -> usize {
        self.frames.len()
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.frames.push(frame.to_vec());
        Ok(())
    }

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        let mut storage = std::vec![0; frame.length()];
        frame
            .copy_to(&mut storage)
            .expect("test endpoint frame fits");
        self.frames.push(storage);
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send(frame)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send_parts(frame)
    }
}

#[test]
fn addressed_rx_endpoints_never_guess_an_unknown_vif() {
    let station = NetworkInterfaceId::new(0);
    let access_point = NetworkInterfaceId::new(1);
    let mut endpoints = DatapathNetworkRxEndpoints::new(
        station,
        EndpointRx::default(),
        access_point,
        EndpointRx::default(),
    );

    endpoints
        .get_mut(access_point)
        .expect("AP endpoint exists")
        .try_send(&[1, 2, 3])
        .expect("test endpoint accepts frame");
    assert!(endpoints.get_mut(NetworkInterfaceId::new(2)).is_none());

    let (station_rx, access_point_rx) = endpoints.into_parts();
    assert!(station_rx.frames.is_empty());
    assert_eq!(access_point_rx.frames, [std::vec![1, 2, 3]]);
}

#[test]
fn runner_returns_both_addressed_rx_owners_after_combined_service() {
    let resources = Box::leak(Box::new(Resources::new()));
    let pool = Pool::pin_static(Box::leak(Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
    let irq = Box::leak(Box::new(EmbassyMacIrqRuntime::new()));
    irq.publish(EVENT_RX_SUCCESS);
    let endpoints = DatapathNetworkRxEndpoints::new(
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        EndpointRx::default(),
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        EndpointRx::default(),
    );
    let mut runner = DatapathRunner::new_with_rx_set(
        irq,
        network,
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        endpoints,
        AddressedRxBackend,
    );

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    let (_network, endpoints, _services) = runner.into_complete_parts();
    let (station_rx, access_point_rx) = endpoints.into_parts();
    assert!(station_rx.frames.is_empty());
    assert_eq!(access_point_rx.frames, [std::vec![0x41, 0x50]]);
}

#[test]
fn concurrent_owner_graph_contract_has_one_physical_tx_owner_and_two_vifs() {
    type PairEndpoint = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type PairTxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, 2>;
    type PairPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, 2>;

    let station_resources = Box::leak(Box::new(PairEndpoint::new()));
    let access_point_resources = Box::leak(Box::new(PairEndpoint::new()));
    let tx_resources = Box::leak(Box::new(PairTxResources::new()));
    let tx_pool = PairPool::pin_static(Box::leak(Box::new(PairPool::new())));
    let (provider, consumer) = tx_resources.split(tx_pool);
    let (mut station_device, station_rx) = station_resources.split(
        provider,
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        [2, 0, 0, 0, 0, 1],
    );
    let (mut access_point_device, access_point_rx) = access_point_resources.split(
        provider,
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        [2, 0, 0, 0, 0, 2],
    );
    let network = DualPinnedNetworkRunner::new(
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        station_rx,
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        access_point_rx,
        consumer,
    );
    let mut context = Context::from_waker(core::task::Waker::noop());
    access_point_device
        .transmit(&mut context)
        .expect("AP owns one TX credit")
        .consume(14, |frame| frame.fill(0xa0));
    station_device
        .transmit(&mut context)
        .expect("STA owns the second TX credit")
        .consume(14, |frame| frame.fill(0x50));

    let irq = Box::leak(Box::new(EmbassyMacIrqRuntime::new()));
    let order: &'static std::sync::Mutex<std::vec::Vec<NetworkInterfaceId>> =
        Box::leak(Box::new(std::sync::Mutex::new(std::vec::Vec::new())));
    let services = crate::datapath::paired::ConcurrentRoleServices::new(
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        (),
        PairedPhysicalTx::new(PairedOrdinaryTx::default(), PairedAggregateTx),
        PairedRx { starts_tx: false },
        PairedRoleTx {
            interface: crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
            started: 0,
            order,
            active: None,
        },
        PairedRoleTx {
            interface: crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
            started: 0,
            order,
            active: None,
        },
        PairedControl,
    );
    let mut runner =
        crate::roles::concurrent::compose_sta_ap_datapath_runner(irq, network, services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Ok(DatapathRunnerExit::Role(TestExit::PeerLost))
    );
    let (_network, _endpoints, services) = runner.into_complete_parts();
    let (_hardware, physical_tx, _rx, station_tx, access_point_tx, _control) =
        services.into_parts();
    assert_eq!(station_tx.started, 1);
    assert_eq!(access_point_tx.started, 1);
    let (ordinary, _aggregate) = match physical_tx.try_into_resources() {
        Ok(resources) => resources,
        Err(_) => panic!("both roles returned the physical pair"),
    };
    assert_eq!(ordinary.lends, *order.lock().unwrap());
    assert_eq!(
        *order.lock().unwrap(),
        std::vec![
            crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
            crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        ]
    );
}

#[test]
fn paired_rx_generated_tx_is_driven_for_the_reported_ap_role() {
    type PairEndpoint = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type PairTxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, 2>;
    type PairPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, 2>;

    let station_resources = Box::leak(Box::new(PairEndpoint::new()));
    let access_point_resources = Box::leak(Box::new(PairEndpoint::new()));
    let tx_resources = Box::leak(Box::new(PairTxResources::new()));
    let tx_pool = PairPool::pin_static(Box::leak(Box::new(PairPool::new())));
    let (provider, consumer) = tx_resources.split(tx_pool);
    let (_station_device, station_rx) = station_resources.split(
        provider,
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        [2, 0, 0, 0, 0, 1],
    );
    let (_access_point_device, access_point_rx) = access_point_resources.split(
        provider,
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        [2, 0, 0, 0, 0, 2],
    );
    let network = DualPinnedNetworkRunner::new(
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        station_rx,
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        access_point_rx,
        consumer,
    );
    let irq = Box::leak(Box::new(EmbassyMacIrqRuntime::new()));
    irq.publish(EVENT_RX_SUCCESS);
    let order = Box::leak(Box::new(std::sync::Mutex::new(std::vec::Vec::new())));
    let services = crate::datapath::paired::ConcurrentRoleServices::new(
        crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
        (),
        PairedPhysicalTx::new(PairedOrdinaryTx::default(), PairedAggregateTx),
        PairedRx { starts_tx: true },
        PairedRoleTx {
            interface: crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
            started: 0,
            order,
            active: None,
        },
        PairedRoleTx {
            interface: crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
            started: 0,
            order,
            active: None,
        },
        PairedControl,
    );
    let mut runner =
        crate::roles::concurrent::compose_sta_ap_datapath_runner(irq, network, services);

    embassy_futures::block_on(runner.service_rx()).unwrap();

    assert_eq!(runner.active_tx_interface, None);
    let (_network, _endpoints, services) = runner.into_complete_parts();
    let (_hardware, physical, _rx, _station, access_point, _control) = services.into_parts();
    assert!(access_point.active.is_none());
    assert!(physical.try_into_resources().is_ok());
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

struct NonPollingControlBackend {
    irq: &'static EmbassyMacIrqRuntime<NoopRawMutex>,
    control_calls: usize,
    control_tx_on_first: bool,
    finish_on_second: bool,
}

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for NonPollingControlBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        ready(Ok(DatapathRxProgress::Drained))
    }

    fn service_control<'a>(
        &'a mut self,
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        self.control_calls += 1;
        let progress = if self.control_tx_on_first && self.control_calls == 1 {
            self.irq.publish(EVENT_TX_COMPLETE);
            Ok(DatapathControlProgress::TxPending)
        } else if self.finish_on_second && self.control_calls == 2 {
            Err(TestError::Finished)
        } else {
            Ok(DatapathControlProgress::Idle)
        };
        ready(progress)
    }

    fn control_ready(&self, _now_micros: u64) -> bool {
        false
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
        _network: &'a PinnedTxInterfaceConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.irq.publish(EVENT_TX_COMPLETE);
        ready(Ok(WifiTxProgress::Pending))
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        pending()
    }

    fn service_tx<'a>(
        &'a mut self,
        _wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        ready(Ok(WifiTxProgress::Complete))
    }
}

impl Backend {
    fn push(&mut self, event: u8) {
        self.order[self.count] = event;
        self.count += 1;
    }
}

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Backend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.push(1);
            self.software_rx_work = false;
            if self.backpressure_once {
                self.backpressure_once = false;
                if self.repost_rx_when_backpressured {
                    self.irq.publish(EVENT_RX_SUCCESS);
                }
                return Ok(DatapathRxProgress::StageCapacityBlocked);
            }
            if self.queue_control_on_rx {
                self.control_pending = true;
            }
            Ok(DatapathRxProgress::Drained)
        }
    }

    fn has_rx_work(&self) -> bool {
        self.software_rx_work
    }

    fn service_control<'a>(
        &'a mut self,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        async move {
            self.network_pending_seen |= context.network_tx_pending;
            if self.disconnect {
                return Ok(DatapathControlProgress::Exit(TestExit::PeerLost));
            }
            if !self.control_pending {
                return Ok(DatapathControlProgress::Idle);
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
        _network: &'a PinnedTxInterfaceConsumer<
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
                self.irq.publish(EVENT_TX_COMPLETE | EVENT_RX_SUCCESS);
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

struct AddressedRxBackend;

struct PairedRx {
    starts_tx: bool,
}

#[derive(Debug, Default)]
struct PairedOrdinaryTx {
    lends: std::vec::Vec<NetworkInterfaceId>,
}

#[derive(Debug)]
struct PairedAggregateTx;

type PairedPhysicalTx =
    crate::datapath::paired::DatapathPairedPhysicalTx<PairedOrdinaryTx, PairedAggregateTx>;

impl<H>
    crate::datapath::paired::DatapathPairedRxService<
        H,
        PairedPhysicalTx,
        PairedRoleTx,
        PairedRoleTx,
    > for PairedRx
{
    type Error = TestError;

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        physical_tx: &'a mut PairedPhysicalTx,
        _first_role: &'a mut PairedRoleTx,
        second_role: &'a mut PairedRoleTx,
        _first: &'a mut dyn DatapathNetworkRx,
        _second: &'a mut dyn DatapathNetworkRx,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<crate::datapath::paired::DatapathPairedRxProgress, Self::Error>> + 'a
    {
        async move {
            if !self.starts_tx {
                return pending().await;
            }
            second_role.active = Some(
                physical_tx
                    .try_lend(crate::datapath::paired::DatapathPairRole::Second)
                    .unwrap(),
            );
            Ok(
                crate::datapath::paired::DatapathPairedRxProgress::TxPending(
                    crate::datapath::paired::DatapathPairRole::Second,
                ),
            )
        }
    }

    fn service_during_tx<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _physical_tx: &'a mut PairedPhysicalTx,
        _first_role: &'a mut PairedRoleTx,
        _second_role: &'a mut PairedRoleTx,
        _first: &'a mut dyn DatapathNetworkRx,
        _second: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            if self.starts_tx {
                Ok(DatapathRxProgress::Drained)
            } else {
                pending().await
            }
        }
    }

    fn work_counters(&self) -> crate::datapath::DatapathRxWorkCounters {
        crate::datapath::DatapathRxWorkCounters::default()
    }
}

struct PairedRoleTx {
    interface: NetworkInterfaceId,
    started: usize,
    order: &'static std::sync::Mutex<std::vec::Vec<NetworkInterfaceId>>,
    active: Option<(PairedOrdinaryTx, PairedAggregateTx)>,
}

impl
    crate::datapath::paired::DatapathPairedNetworkTxService<
        'static,
        NoopRawMutex,
        (),
        PairedPhysicalTx,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        2,
    > for PairedRoleTx
{
    type Error = TestError;

    fn start<'a>(
        &'a mut self,
        _hardware: &'a mut (),
        physical_tx: &'a mut PairedPhysicalTx,
        frame: PinnedTxFrame<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, 2>,
        network: &'a PinnedTxInterfaceConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            2,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            assert_eq!(*frame.tag(), self.interface);
            assert_eq!(network.interface(), self.interface);
            self.started += 1;
            let role = if self.interface == crate::roles::concurrent::STA_NETWORK_INTERFACE_ID {
                crate::datapath::paired::DatapathPairRole::First
            } else {
                crate::datapath::paired::DatapathPairRole::Second
            };
            let (mut ordinary, aggregate) = physical_tx.try_lend(role).unwrap();
            ordinary.lends.push(self.interface);
            physical_tx.restore(role, ordinary, aggregate).unwrap();
            self.order.lock().unwrap().push(self.interface);
            Ok(WifiTxProgress::Complete)
        }
    }

    fn wait_deadline<'a>(
        &'a mut self,
        _physical_tx: &'a mut PairedPhysicalTx,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            if self.active.is_none() {
                pending().await
            }
        }
    }

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut (),
        physical_tx: &'a mut PairedPhysicalTx,
        _wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let Some((ordinary, aggregate)) = self.active.take() else {
                return pending().await;
            };
            let role = if self.interface == crate::roles::concurrent::STA_NETWORK_INTERFACE_ID {
                crate::datapath::paired::DatapathPairRole::First
            } else {
                crate::datapath::paired::DatapathPairRole::Second
            };
            physical_tx.restore(role, ordinary, aggregate).unwrap();
            Ok(WifiTxProgress::Complete)
        }
    }
}

struct PairedControl;

impl
    crate::datapath::paired::DatapathPairedControlService<
        (),
        PairedPhysicalTx,
        PairedRoleTx,
        PairedRoleTx,
    > for PairedControl
{
    type Error = TestError;
    type Exit = TestExit;

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut (),
        _physical_tx: &'a mut PairedPhysicalTx,
        first_tx: &'a mut PairedRoleTx,
        second_tx: &'a mut PairedRoleTx,
        _context: DatapathControlContext,
        _retained_tx: Option<crate::datapath::paired::DatapathPairRole>,
    ) -> impl Future<
        Output = Result<
            crate::datapath::paired::DatapathPairedControlProgress<Self::Exit>,
            Self::Error,
        >,
    > + 'a {
        async move {
            if first_tx.started == 1 && second_tx.started == 1 {
                Ok(
                    crate::datapath::paired::DatapathPairedControlProgress::Exit(
                        TestExit::PeerLost,
                    ),
                )
            } else {
                Ok(crate::datapath::paired::DatapathPairedControlProgress::Idle)
            }
        }
    }

    fn stop(
        &mut self,
        _hardware: &mut (),
        _physical_tx: &mut PairedPhysicalTx,
        _first_tx: &mut PairedRoleTx,
        _second_tx: &mut PairedRoleTx,
    ) -> Result<crate::datapath::paired::DatapathPairedStopProgress, Self::Error> {
        Ok(crate::datapath::paired::DatapathPairedStopProgress::Stopped)
    }

    fn wait_ready<'a>(
        &'a mut self,
        _physical_tx: &'a mut PairedPhysicalTx,
        _first_tx: &'a mut PairedRoleTx,
        _second_tx: &'a mut PairedRoleTx,
    ) -> impl Future<Output = ()> + 'a {
        pending()
    }
}

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for AddressedRxBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            network_rx
                .get_mut(crate::roles::concurrent::AP_NETWORK_INTERFACE_ID)
                .expect("combined owner contains the AP endpoint")
                .try_send(&[0x41, 0x50])
                .expect("test endpoint accepts one frame");
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
        _network: &'a PinnedTxInterfaceConsumer<
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

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for NetworkBackpressureBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.service_calls = self.service_calls.saturating_add(1);
            if self.service_calls == 1 {
                network_rx.primary_mut().try_send(&[0; 14]).unwrap();
                Ok(DatapathRxProgress::NetworkBackpressured)
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
        _network: &'a PinnedTxInterfaceConsumer<
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

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for RxProbeBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.service_calls = self.service_calls.saturating_add(1);
            if self.service_calls == 1 {
                Ok(DatapathRxProgress::ProbePending)
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
        _network: &'a PinnedTxInterfaceConsumer<
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

struct RxRepostDeadlineBackend {
    irq: &'static EmbassyMacIrqRuntime<NoopRawMutex>,
    rx_services: u8,
    tx_wake: Option<WifiTxWake>,
    tx_services: usize,
}

struct RxStartedTxBackend {
    irq: &'static EmbassyMacIrqRuntime<NoopRawMutex>,
    active: bool,
    serviced_wake: Option<WifiTxWake>,
}

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for RxStartedTxBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.active = true;
        self.irq.publish(EVENT_TX_COMPLETE);
        ready(Ok(DatapathRxProgress::Drained))
    }

    fn has_active_tx(&self) -> bool {
        self.active
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
        _network: &'a PinnedTxInterfaceConsumer<
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
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.active = false;
        self.serviced_wake = Some(wake);
        ready(Err(TestError::Finished))
    }
}

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for RxRepostDeadlineBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.rx_services = self.rx_services.saturating_add(1);
            self.irq.publish(EVENT_RX_SUCCESS);
            if self.rx_services > 4 {
                return Err(TestError::Finished);
            }
            Ok(DatapathRxProgress::Drained)
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
        _network: &'a PinnedTxInterfaceConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.irq.publish(EVENT_RX_SUCCESS);
        ready(Ok(WifiTxProgress::Pending))
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        let ready = self.rx_services != 0;
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
            self.tx_services += 1;
            Err(TestError::Finished)
        }
    }
}

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for ShutdownBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        pending()
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, Self::Error> {
        self.stop_calls = self.stop_calls.saturating_add(1);
        if self.stop_calls == 1 {
            Ok(DatapathStopProgress::TxPending)
        } else {
            Ok(DatapathStopProgress::Stopped)
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
        _network: &'a PinnedTxInterfaceConsumer<
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

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for CompletionFillBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
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
        _network: &'a PinnedTxInterfaceConsumer<
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
        _network: &'a PinnedTxInterfaceConsumer<
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

impl DatapathServices<'static, NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for PreparedBackend
{
    type Error = TestError;
    type Exit = TestExit;

    fn service_rx<'a>(
        &'a mut self,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        _context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        pending()
    }

    fn service_control<'a>(
        &'a mut self,
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'static: 'a,
        NoopRawMutex: 'a,
    {
        async move {
            if self.control_pending {
                self.control_pending = false;
                self.order[self.count] = 1;
                self.count += 1;
                Ok(DatapathControlProgress::More)
            } else {
                Ok(DatapathControlProgress::Idle)
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
        _network: &'a PinnedTxInterfaceConsumer<
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

    fn start_prepared_tx(
        &mut self,
        _network: &PinnedTxInterfaceConsumer<
            'static,
            NoopRawMutex,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        self.prepared = false;
        self.order[self.count] = 2;
        self.count += 1;
        Err(TestError::Finished)
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
fn adaptive_batching_sends_isolated_frames_and_collects_proven_bursts() {
    let start = Instant::from_millis(100);
    let mut state = TxBatchState::new();

    assert_eq!(state.collection_deadline(1, 1, start), None);
    assert_eq!(state.collection_deadline(32, 1, start), None);

    state.note_started(1);
    // A prepared standby proves continuation even when the active hardware
    // transaction itself took longer than the time used for an empty gap.
    let second = start + Duration::from_millis(10);
    let deadline = state
        .collection_deadline(32, 1, second)
        .expect("a standby without an observed empty boundary proves a burst");
    assert_eq!(deadline, second + TX_BATCH_MAX_WAIT);
    assert_eq!(state.collection_deadline(32, 2, second), Some(deadline));
    assert_eq!(state.collection_deadline(32, 32, second), None);

    state.note_started(16);
    state.note_idle(second);
    let warm = second + Duration::from_millis(3);
    assert!(state.collection_deadline(32, 1, warm).is_some());

    state.note_started(1);
    state.note_idle(warm);
    let quiet = warm + TX_BURST_QUIET_TIMEOUT + Duration::from_millis(1);
    assert_eq!(state.collection_deadline(32, 1, quiet), None);
    assert!(state.collection_deadline(32, 2, quiet).is_some());
}

#[test]
fn only_single_interface_network_tx_may_prepare_a_standby() {
    let first = NetworkInterfaceId::new(0);
    let second = NetworkInterfaceId::new(1);

    assert!(tx_lookahead_allowed(
        true,
        DatapathInterfaceScope::Single(first),
        DatapathTxOrigin::Network,
    ));
    assert!(!tx_lookahead_allowed(
        false,
        DatapathInterfaceScope::Single(first),
        DatapathTxOrigin::Network,
    ));
    assert!(!tx_lookahead_allowed(
        true,
        DatapathInterfaceScope::Pair { first, second },
        DatapathTxOrigin::Network,
    ));
    assert!(!tx_lookahead_allowed(
        true,
        DatapathInterfaceScope::Single(first),
        DatapathTxOrigin::Control,
    ));
}

#[test]
fn drained_rx_is_unmasked_before_the_cooperative_yield() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
    RX_UNMASK_CALLS.store(0, Ordering::Relaxed);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new_with_rx_moderation(record_rx_unmask),
    ));
    irq.begin_rx_moderation();
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
    let mut service = std::boxed::Box::pin(runner.service_rx());
    let mut context = Context::from_waker(core::task::Waker::noop());

    assert_eq!(service.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(RX_UNMASK_CALLS.load(Ordering::Relaxed), 1);
}

#[test]
fn control_boundary_precedes_prepared_network_publication() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order, [1, 2]);
}

#[test]
fn due_recycled_rx_probe_blocks_the_saturated_prepared_tx_chain() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
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
    let interface = NetworkInterfaceId::new(0);
    let mut runner = DatapathRunner::new(irq, network, interface, services);

    runner.recycled_rx_probe_deadline = Some(Instant::now());
    assert_eq!(runner.prepared_network_tx_candidate(), None);
    runner.clear_recycled_rx_probe_deadline();
    assert_eq!(runner.prepared_network_tx_candidate(), Some((interface, 1)));
}

#[test]
fn network_data_completion_does_not_repoll_idle_control() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = NonPollingControlBackend {
        irq,
        control_calls: 0,
        control_tx_on_first: false,
        finish_on_second: false,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    drop(run);
    assert_eq!(runner.services().control_calls, 1);
    assert_eq!(runner.active_tx_origin, None);
    assert!(!runner.control_ready_latched);
}

#[test]
fn ordinary_rx_completion_does_not_repoll_idle_control() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(EVENT_RX_SUCCESS);
    let services = NonPollingControlBackend {
        irq,
        control_calls: 0,
        control_tx_on_first: false,
        finish_on_second: false,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    // The first poll consumes initial control readiness and the RX frontier,
    // ending at the RX cooperative yield. The resumed poll must wait for a
    // real control wake instead of manufacturing another idle control pass.
    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    drop(run);
    assert_eq!(runner.services().control_calls, 1);
    assert!(!runner.control_ready_latched);
}

#[test]
fn control_tx_completion_rearms_exactly_one_control_step() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = NonPollingControlBackend {
        irq,
        control_calls: 0,
        control_tx_on_first: true,
        finish_on_second: true,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().control_calls, 2);
    assert_eq!(runner.active_tx_origin, None);
}

#[test]
fn caller_stop_cancels_software_owned_prepared_network_tx() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(DatapathRunnerExit::Stopped)
    );
    assert!(runner.services().cancelled);
    assert!(!runner.services().prepared);
}

#[test]
fn single_owner_graph_contract_returns_all_owners_after_role_shutdown() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = ShutdownBackend {
        stop_calls: 0,
        tx_services: 0,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(DatapathRunnerExit::Stopped)
    );
    assert_eq!(runner.services().stop_calls, 2);
    assert_eq!(runner.services().tx_services, 1);
}

#[test]
fn frame_arriving_inside_select_rechecks_control_as_network_pending() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
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
    let (_device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

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
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
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
    let (_device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(EVENT_RX_SUCCESS);
    let services = RxProbeBackend { service_calls: 0 };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

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
    let (mut device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(EVENT_RX_SUCCESS);
    let services = NetworkBackpressureBackend { service_calls: 0 };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
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
    let (_device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(EVENT_RX_SUCCESS);
    let services = NetworkBackpressureBackend { service_calls: 0 };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    // The final network queue is still occupied, but a later DMA completion
    // must re-enter the role service instead of waiting for network capacity.
    irq.publish(EVENT_RX_SUCCESS);
    assert_eq!(
        embassy_futures::block_on(run.as_mut()),
        Err(TestError::Finished)
    );
    drop(run);
    assert_eq!(runner.services().service_calls, 2);
}

#[test]
fn ready_single_interface_prefix_is_prepared_before_terminal_service() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = CompletionFillBackend {
        order: [0; 2],
        count: 0,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);
    let mut run = std::boxed::Box::pin(runner.run());
    let mut context = Context::from_waker(core::task::Waker::noop());

    // Start the first hardware transaction, then make another frame and its
    // completion interrupt ready in the same scheduler epoch. The ordered TX
    // branch may synchronously transfer that ready single-interface prefix to
    // standby before terminal service; it must not defer it to a later outer
    // scheduler turn.
    assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
    enqueue_frame(&mut device);
    irq.publish(EVENT_TX_COMPLETE);
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
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[..2], [1, 2]);
    assert_eq!(
        runner.services().tx_wake,
        Some(WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE,
        })
    );
}

#[test]
fn critical_admission_block_gates_new_rx_edges_but_not_tx_completion() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[..2], [1, 2]);
    assert_eq!(
        runner.services().tx_wake,
        Some(WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE,
        })
    );
    assert!(irq.rx_signaled());
}

#[test]
fn executor_deadline_services_tx_without_an_interrupt() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().order[0], 2);
    assert_eq!(runner.services().tx_wake, Some(WifiTxWake::Deadline));
}

#[test]
fn reposted_rx_cannot_starve_a_due_active_tx_deadline() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
    enqueue_frame(&mut device);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    let services = RxRepostDeadlineBackend {
        irq,
        rx_services: 0,
        tx_wake: None,
        tx_services: 0,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(runner.services().rx_services, 1);
    assert_eq!(runner.services().tx_wake, Some(WifiTxWake::Deadline));
    assert_eq!(runner.services().tx_services, 1);
    assert!(irq.rx_signaled());
}

#[test]
fn single_role_tx_started_by_rx_is_adopted_before_an_idle_boundary() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (_device, network) = split_network!(resources, pool);
    let irq = std::boxed::Box::leak(std::boxed::Box::new(
        EmbassyMacIrqRuntime::<NoopRawMutex>::new(),
    ));
    irq.publish(EVENT_RX_SUCCESS);
    let services = RxStartedTxBackend {
        irq,
        active: false,
        serviced_wake: None,
    };
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Err(TestError::Finished)
    );
    assert_eq!(
        runner.services().serviced_wake,
        Some(WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE,
        })
    );
}

#[test]
fn rx_control_waits_for_the_active_network_tx_then_precedes_another_lease() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

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
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(core::future::ready(()))),
        Ok(DatapathRunnerExit::Stopped)
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
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run_until(stop_future)),
        Ok(DatapathRunnerExit::Stopped)
    );
    assert_eq!(runner.services().order[..2], [1, 2]);
    assert_eq!(
        runner.services().tx_wake,
        Some(WifiTxWake::Interrupt {
            events: EVENT_TX_COMPLETE,
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
    let (mut device, network) = split_network!(resources, pool);
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
    let mut runner = DatapathRunner::new(irq, network, NetworkInterfaceId::new(0), services);

    assert_eq!(
        embassy_futures::block_on(runner.run()),
        Ok(DatapathRunnerExit::Role(TestExit::PeerLost))
    );
    let mut context = Context::from_waker(core::task::Waker::noop());
    assert!(matches!(
        device.link_state(&mut context),
        open_esp_radio_embassy_net::LinkState::Down
    ));
    let (_network, services) = runner.into_parts();
    assert!(services.disconnect);
}

#[test]
fn adaptive_rx_continuation_distinguishes_byte_rate_from_packet_rate() {
    assert_eq!(
        adaptive_recycled_rx_probe_delay(DatapathRxWorkCounters::default(), 4),
        (Duration::from_micros(1_024), 4)
    );
    assert_eq!(
        adaptive_recycled_rx_probe_delay(DatapathRxWorkCounters::default(), 0),
        (Duration::from_micros(64), 0)
    );
    assert_eq!(
        adaptive_recycled_rx_probe_delay(
            DatapathRxWorkCounters {
                completed_units: 32,
                staged_bytes: 32 * 256,
            },
            3,
        ),
        (Duration::from_micros(512), 3)
    );
    assert_eq!(
        adaptive_recycled_rx_probe_delay(
            DatapathRxWorkCounters {
                completed_units: 3,
                staged_bytes: 3 * 1_536,
            },
            0,
        ),
        (Duration::from_micros(1_024), 4)
    );
    assert_eq!(
        adaptive_recycled_rx_probe_delay(
            DatapathRxWorkCounters {
                completed_units: 4,
                staged_bytes: 4 * 1_536,
            },
            0,
        ),
        (Duration::from_micros(1_024), 4)
    );
    assert_eq!(
        adaptive_recycled_rx_probe_delay(
            DatapathRxWorkCounters {
                completed_units: 1,
                staged_bytes: 1_536,
            },
            2,
        ),
        (Duration::from_micros(1_024), 4)
    );
}

#[test]
fn rx_work_counter_delta_is_saturating_across_epoch_reset() {
    let current = DatapathRxWorkCounters {
        completed_units: 7,
        staged_bytes: 10_752,
    };
    assert_eq!(
        current.saturating_sub(DatapathRxWorkCounters {
            completed_units: 3,
            staged_bytes: 4_608,
        }),
        DatapathRxWorkCounters {
            completed_units: 4,
            staged_bytes: 6_144,
        }
    );
    assert_eq!(
        current.saturating_sub(DatapathRxWorkCounters {
            completed_units: 9,
            staged_bytes: 12_000,
        }),
        DatapathRxWorkCounters::default()
    );
}
