//! Connected Embassy composition for the standalone station application.
//!
//! This module chooses board allocation and application network policy. The
//! reusable driver owns PAC/DMA/IRQ and 802.11 protocol transitions; no HIL
//! command, benchmark or qualification telemetry is part of this graph.

use core::future::Future;

use embassy_executor::Spawner;
use embassy_net::{
    Config, Stack, StackResources,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use esp_hal::system::software_reset;
use open_esp_radio::esp32s31::wifi::device::register_arena::Esp32s31RadioRegistersArena;
use open_esp_radio::esp32s31::wifi::sta::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio::{
    WifiPlan,
    adapters::network::embassy_net::{
        LinkState, PinnedTxPool, SplitPinnedDevice, SplitPinnedRadioRunner, SplitPinnedResources,
    },
    esp32s31::wifi::sta::peer::Esp32s31ConnectedStaPeer,
    esp32s31::{
        hal::RadioRegisters,
        phy::phy_cold::PhyColdState,
        registers::MacInterruptSetup,
        wifi::dma::tx_ampdu_storage::AmpduDmaStorage,
        wifi::mac::{
            crypto::{StaGroupCcmpSlot, StaPairwiseCcmpSlot},
            init::MAC_COLD_RX_INTERRUPT_MASK,
            rx::RxIngressConfig,
            rx_pool::RxStagePool,
            tx::{HeEdcaTxopLimit, HtGuardInterval, HtMcs, LegacyRate},
            tx_ampdu::{HtAmpduTxResources, HtAmpduTxStorage},
        },
    },
    wifi::{ieee80211::station::StaTxSequenceCounters, wpa2::Pmk},
};
use open_esp_radio_esp32s31_wifi_embassy::{
    aggregate_tx::AggregateTxResources,
    connected_runner::ConnectedRunner,
    connected_rx_protocol::{
        ConnectedRxProtocolStopped, Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue,
    },
    connected_sta_port::{
        Esp32s31ConnectedStaBlockAckPolicy, Esp32s31ConnectedStaConfig,
        Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaDriverParts,
        Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaPort,
        Esp32s31ConnectedStaRateConfig, Esp32s31ConnectedStaRxPolicy,
        Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxPolicy,
        Esp32s31ConnectedStaTxResources,
    },
    connected_sta_teardown::{
        Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTeardownPort,
    },
    control_mailbox::{ConnectedControlPublisher, ConnectedControlResources},
    embassy_irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch},
    embassy_rx::EmbassyEsp32s31RxReloadDelay,
    network_rx::EmbassyNetConnectedRxSink,
    preconnected_rx::{EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRx},
    rx_dma_service::{Esp32s31RxEpochResources, Esp32s31StoppedRx},
    rx_reorder::{RxReorderCommandResources, RxReorderFrameStorage},
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        Esp32s31ConnectedStationExit, Esp32s31ConnectedTaskGroup, Esp32s31ConnectedTaskStopOutcome,
        Esp32s31StationCommand, Esp32s31StationCommandReceiver,
        run_esp32s31_connected_station_epoch, stop_esp32s31_connected_task_group,
    },
    station_epoch::{
        Esp32s31DisconnectedStaEpoch, Esp32s31ReconnectedStaEpoch, Esp32s31ReconnectedStaEpochParts,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral,
    mac_interrupt_epoch::{
        EspHalMacInterruptRoute, service_mac_interrupt, service_power_interrupt,
    },
};
use static_cell::StaticCell;

use crate::station::{RX_BUFFER_SIZE, RX_DESCRIPTOR_COUNT, RxStorage, TxStorage};

const RX_STAGE_CAPACITY: usize = 1_700;
const RX_STAGE_SLOT_COUNT: usize = 16;
const RX_REORDER_WINDOW: usize = 8;
const CONTROL_QUEUE_DEPTH: usize = 16;
const NETWORK_FRAME_CAPACITY: usize = 1_600;
const NETWORK_RX_QUEUE_DEPTH: usize = 8;
const NETWORK_TX_QUEUE_DEPTH: usize = 8;
const NETWORK_TX_HEADROOM: usize =
    8 + open_esp_radio::wifi::ieee80211::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;
const NETWORK_TX_TRAILER: usize = 12;
const TX_AMPDU_FRAME_COUNT: usize = 8;
const TX_AMPDU_BUFFER_SIZE: usize = 0;

type NetworkResources = SplitPinnedResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkTxPool = PinnedTxPool<
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkDevice = SplitPinnedDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkRunner = SplitPinnedRadioRunner<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type ControlResources = ConnectedControlResources<CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
type ControlPublisher =
    ConnectedControlPublisher<'static, CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
type ConnectedRxSink = EmbassyNetConnectedRxSink<
    'static,
    CriticalSectionRawMutex,
    ControlPublisher,
    NETWORK_FRAME_CAPACITY,
    NETWORK_RX_QUEUE_DEPTH,
>;
type ConnectedRxProtocol = Esp32s31ConnectedRxProtocol<
    'static,
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    ConnectedRxSink,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_WINDOW,
>;
pub type ConnectedHardware = CooperativeRadioHardware<'static>;
type ConnectedStoppedRx = Esp32s31StoppedRx<
    'static,
    'static,
    'static,
    EmbassyEsp32s31RxReloadDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
>;
type ConnectedRxEpochResources = Esp32s31RxEpochResources<
    'static,
    'static,
    'static,
    EmbassyEsp32s31RxReloadDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
>;
type ConnectedAmpduStorage =
    AggregateTxResources<'static, TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>;
pub type ConnectedReconnectedEpoch = Esp32s31ReconnectedStaEpoch<
    ConnectedHardware,
    Esp32s31PreconnectedRx<
        'static,
        EmbassyEsp32s31PreconnectedRxDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    ConnectedRxEpochResources,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
pub type ConnectedDisconnectedEpoch = Esp32s31DisconnectedStaEpoch<
    RunningNetwork,
    ConnectedHardware,
    ConnectedStoppedRx,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
pub type MacInterruptEpoch =
    Esp32s31MacInterruptEpoch<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;

static IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> = EmbassyMacIrqRuntime::new();
static POWER_IRQ_RUNTIME: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
    EmbassyPowerIrqRuntime::new();
static RX_STAGE_POOL: RxStagePool<RX_STAGE_SLOT_COUNT, RX_STAGE_CAPACITY> = RxStagePool::new();
static STAGED_RX_QUEUE: Esp32s31StagedRxQueue<
    'static,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
> = Esp32s31StagedRxQueue::new();
static RX_REORDER_COMMANDS: RxReorderCommandResources<CriticalSectionRawMutex> =
    RxReorderCommandResources::new();
static RX_REORDER_STORAGE: RxReorderFrameStorage<RX_STAGE_CAPACITY, RX_REORDER_WINDOW> =
    RxReorderFrameStorage::new();
static CONTROL_RESOURCES: StaticCell<ControlResources> = StaticCell::new();
static NETWORK_RESOURCES: StaticCell<NetworkResources> = StaticCell::new();
static NETWORK_TX_POOL: StaticCell<NetworkTxPool> = StaticCell::new();
static NETWORK_STACK_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
static TX_AMPDU_STORAGE: StaticCell<HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>> =
    StaticCell::new();
static TX_AMPDU_DMA_STORAGE: StaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    StaticCell::new();
static REGISTER_ARENA: StaticCell<Esp32s31RadioRegistersArena> = StaticCell::new();
static ETHERNET_FRAME: StaticCell<[u8; RX_STAGE_CAPACITY]> = StaticCell::new();
static UDP_RX_METADATA: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static UDP_TX_METADATA: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static UDP_RX_BUFFER: StaticCell<[u8; 2_048]> = StaticCell::new();
static UDP_TX_BUFFER: StaticCell<[u8; 2_048]> = StaticCell::new();
static RX_PROTOCOL_STOP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static RX_PROTOCOL_STOPPED: Signal<CriticalSectionRawMutex, ConnectedRxProtocolStopped<'static>> =
    Signal::new();

const CONNECTED_TASK_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// One-time versus persistent Embassy network ownership.
pub enum StationNetwork {
    Unstarted {
        device: NetworkDevice,
        runner: NetworkRunner,
    },
    Running(RunningNetwork),
}

/// Network stack and radio endpoint retained across association epochs.
pub struct RunningNetwork {
    stack: Stack<'static>,
    runner: NetworkRunner,
}

/// Hardware frontier accepted by one connected epoch.
pub enum ConnectedStationEpoch {
    Initial {
        hardware: RadioRegisters,
        receive: Esp32s31PreconnectedRx<
            'static,
            EmbassyEsp32s31PreconnectedRxDelay,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
        >,
    },
    Reconnected(ConnectedReconnectedEpoch),
}

/// Normal outcome returned after all connected owners are quiescent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStationOutcome {
    Disconnected,
    ReconnectRequested,
    StationStopped(Esp32s31StationCommand),
}

/// Exact input for the next finite join attempt.
pub struct ConnectedStationReturn {
    pub disconnected: ConnectedDisconnectedEpoch,
    pub frame: &'static mut [u8],
    pub ethernet: &'static mut [u8],
    pub outcome: ConnectedStationOutcome,
}

/// Exact owners transferred from a successful finite station attempt.
pub struct ConnectedStationResources<'state, 'tx, 'security> {
    pub wifi: WifiPlan,
    pub epoch: ConnectedStationEpoch,
    pub network: StationNetwork,
    pub phy: &'state mut PhyColdState,
    pub platform: &'state mut EspHalRadioPeripheral,
    pub rx_storage: &'static RxStorage,
    pub tx_storage: &'tx mut TxStorage,
    pub frame: &'static mut [u8],
    pub peer: Esp32s31ConnectedStaPeer,
    pub pairwise: StaPairwiseCcmpSlot,
    pub group: StaGroupCcmpSlot,
    pub pmk: &'security Pmk,
    pub sequences: &'security mut StaTxSequenceCounters,
    pub ethernet: Option<&'static mut [u8]>,
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn mac_interrupt() {
    let _ = service_mac_interrupt(&IRQ_RUNTIME);
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn power_interrupt() {
    let _ = service_power_interrupt(&POWER_IRQ_RUNTIME);
}

#[embassy_executor::task]
async fn network_task(mut runner: embassy_net::Runner<'static, NetworkDevice>) {
    runner.run().await
}

#[embassy_executor::task]
async fn rx_protocol_task(protocol: ConnectedRxProtocol) {
    let stopped = protocol.run_until_stopped(RX_PROTOCOL_STOP.wait()).await;
    RX_PROTOCOL_STOPPED.signal(stopped);
}

struct ConnectedTaskGroup;

impl Esp32s31ConnectedTaskGroup for ConnectedTaskGroup {
    type Stopped = ConnectedRxProtocolStopped<'static>;

    fn request_stop(&mut self) {
        RX_PROTOCOL_STOP.signal(());
    }

    fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_ {
        RX_PROTOCOL_STOPPED.wait()
    }
}

#[embassy_executor::task]
async fn network_status_task(stack: Stack<'static>) {
    loop {
        if let Some(config) = stack.config_v4() {
            esp_println::println!(
                "open-radio: DHCP address={}.{}.{}.{}",
                config.address.address().octets()[0],
                config.address.address().octets()[1],
                config.address.address().octets()[2],
                config.address.address().octets()[3],
            );
            break;
        }
        Timer::after_millis(100).await;
    }
    loop {
        Timer::after_secs(60).await;
    }
}

/// Small application-level service proving that the standalone target, rather
/// than a HIL benchmark, can consume and produce ordinary Embassy traffic.
#[embassy_executor::task]
async fn udp_echo_task(stack: Stack<'static>) {
    let rx_metadata = UDP_RX_METADATA.init([PacketMetadata::EMPTY; 4]);
    let tx_metadata = UDP_TX_METADATA.init([PacketMetadata::EMPTY; 4]);
    let rx_buffer = UDP_RX_BUFFER.init([0; 2_048]);
    let tx_buffer = UDP_TX_BUFFER.init([0; 2_048]);
    let mut socket = UdpSocket::new(stack, rx_metadata, rx_buffer, tx_metadata, tx_buffer);
    socket.bind(4_321).expect("UDP echo port must be free");
    let mut reply = [0_u8; 1_472];
    loop {
        let (length, remote) = socket
            .recv_from_with(|packet, remote| {
                let length = packet.len().min(reply.len());
                reply[..length].copy_from_slice(&packet[..length]);
                (length, remote)
            })
            .await;
        if let Err(error) = socket.send_to(&reply[..length], remote).await {
            esp_println::println!("open-radio: UDP echo send failed: {error:?}");
        }
    }
}

const fn connected_config() -> Esp32s31ConnectedStaConfig {
    Esp32s31ConnectedStaConfig {
        tx: Esp32s31ConnectedStaTxPolicy {
            rate: Esp32s31ConnectedStaRateConfig {
                high_throughput_enabled: true,
                fallback_legacy_rate: LegacyRate::Ofdm54M,
                fallback_ht_mcs: HtMcs::Mcs7,
                fallback_ht_guard_interval: HtGuardInterval::Short400Ns,
                ht_mcs_override: None,
                ht_guard_interval_override: None,
                he_mcs_override: None,
                he_guard_interval_and_ltf_override: None,
                he_dcm_override: None,
            },
            unicast_attempt_limit: 4,
            completion_timeout_us: 250_000,
            aggregate_frame_limit: TX_AMPDU_FRAME_COUNT as u8,
            aggregate_he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
        block_ack: Esp32s31ConnectedStaBlockAckPolicy {
            tx_block_ack_window: 8,
            tx_block_ack_negotiation_timeout_us: 500_000,
            tx_block_ack_negotiation_attempt_limit: 3,
            tid0_amsdu: false,
            rx_block_ack_maximum_window: RX_REORDER_WINDOW as u16,
            request_initial_tx_block_ack: true,
        },
        receive: Esp32s31ConnectedStaRxPolicy {
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            beacon_miss_limit: 10,
        },
    }
}

/// Enter the real connected PAC/Embassy owner graph.
pub async fn run_connected(
    spawner: Spawner,
    interrupt_epoch: &mut MacInterruptEpoch,
    station_control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    resources: ConnectedStationResources<'_, '_, '_>,
) -> ConnectedStationReturn {
    let ConnectedStationResources {
        wifi,
        epoch,
        network,
        phy,
        platform,
        rx_storage,
        tx_storage,
        frame,
        peer,
        pairwise,
        group,
        pmk,
        sequences,
        ethernet,
    } = resources;
    let _retained_radio_state = (phy, pmk);

    let plan = Esp32s31ConnectedStaPort::prepare_for_wifi_plan_with_storage::<
        TX_AMPDU_FRAME_COUNT,
        RX_REORDER_WINDOW,
    >(peer, connected_config(), wifi)
    .unwrap_or_else(|failure| panic!("invalid connected policy: {:?}", failure.error));
    let station_address = plan.link().station_address;

    let (stack, network_runner, stack_runner) = match network {
        StationNetwork::Unstarted { device, runner } => {
            let stack_resources = NETWORK_STACK_RESOURCES.init(StackResources::new());
            let mut seed = [0_u8; 8];
            seed[..6].copy_from_slice(&station_address);
            seed[6..].copy_from_slice(&0x31a5_u16.to_le_bytes());
            let (stack, stack_runner) = embassy_net::new(
                device,
                Config::dhcpv4(Default::default()),
                stack_resources,
                u64::from_le_bytes(seed),
            );
            (stack, runner, Some(stack_runner))
        }
        StationNetwork::Running(network) => (network.stack, network.runner, None),
    };
    network_runner.set_link_state(LinkState::Up);

    if let Err(error) = interrupt_epoch.activate(platform, MAC_COLD_RX_INTERRUPT_MASK) {
        esp_println::println!(
            "open-radio: MAC interrupt activation invariant failed: {error:?}; resetting"
        );
        software_reset();
    }

    let (staged_sender, staged_receiver) = STAGED_RX_QUEUE.split();
    let (hardware, rx, aggregate, control_resources) = match epoch {
        ConnectedStationEpoch::Initial {
            mut hardware,
            receive,
        } => {
            let live_ring = receive
                .try_into_live_with_storage(&mut hardware, rx_storage)
                .await
                .unwrap_or_else(|failure| panic!("connected RX arm failed: {:?}", failure.error));
            let rx = Esp32s31RxEpochResources::new(
                rx_storage,
                &RX_STAGE_POOL,
                staged_sender,
                EmbassyEsp32s31RxReloadDelay,
            )
            .with_live_ring(live_ring);
            let aggregate = AggregateTxResources::single(
                HtAmpduTxResources::pin_static(
                    TX_AMPDU_STORAGE.init_with(HtAmpduTxStorage::new),
                    TX_AMPDU_DMA_STORAGE.init_with(AmpduDmaStorage::new),
                )
                .expect("A-MPDU metadata and descriptor storage must be valid"),
            );
            let control_resources = CONTROL_RESOURCES.init(ConnectedControlResources::new());
            let register_arena = REGISTER_ARENA.init_with(Esp32s31RadioRegistersArena::new);
            let published = register_arena
                .publish(hardware)
                .unwrap_or_else(|_| panic!("connected register arena requires radio reset"));
            (
                CooperativeRadioHardware::new(published),
                rx,
                aggregate,
                &*control_resources,
            )
        }
        ConnectedStationEpoch::Reconnected(epoch) => {
            let Esp32s31ReconnectedStaEpochParts {
                mut hardware,
                rx: receive,
                rx_resources,
                aggregate_tx,
                control,
            } = epoch.into_parts();
            let live_ring = receive
                .try_into_live_with_storage(&mut hardware, rx_storage)
                .await
                .unwrap_or_else(|failure| panic!("reconnected RX arm failed: {:?}", failure.error));
            (
                hardware,
                rx_resources.with_live_ring(live_ring),
                aggregate_tx,
                control,
            )
        }
    };

    let network_rx = network_runner.rx_publisher();
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new(network_rx, control_publisher);
    let (reorder_sender, reorder_receiver) = RX_REORDER_COMMANDS.split();
    let ethernet =
        ethernet.unwrap_or_else(|| ETHERNET_FRAME.init([0; RX_STAGE_CAPACITY]).as_mut_slice());
    let protocol = Esp32s31ConnectedStaPort::build_rx_protocol(
        &plan,
        Esp32s31ConnectedStaRxProtocolResources {
            frames: staged_receiver,
            irq: &IRQ_RUNTIME,
            sink: rx_sink,
            mpdu: frame,
            ethernet,
            reorder_commands: reorder_receiver,
            reorder_storage: &RX_REORDER_STORAGE,
            reorder_scratch: None,
            pipeline_observer: None,
        },
    );

    let control_tx = tx_storage
        .take_control()
        .expect("station attempt returned the ordinary TX owner");
    let tx_sequences = core::mem::replace(sequences, StaTxSequenceCounters::new(0));
    let tx = Esp32s31ConnectedStaPort::build_tx(
        &plan,
        Esp32s31ConnectedStaTxResources {
            control: control_tx,
            aggregate,
            pairwise_key: pairwise,
            sequences: tx_sequences,
            aggregate_tx_observer: None,
            network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
        },
    )
    .unwrap_or_else(|_| {
        esp_println::println!("open-radio: connected TX handoff found a live owner; resetting");
        software_reset()
    });
    let control = Esp32s31ConnectedStaPort::build_control(
        &plan,
        Esp32s31ConnectedStaControlResources {
            receiver: control_receiver,
            reorder_commands: reorder_sender,
        },
    );

    let drivers = Esp32s31ConnectedStaPort::assemble(
        plan,
        Esp32s31ConnectedStaDriverParts {
            hardware,
            rx,
            tx,
            control,
            protocol,
        },
    );
    let mut radio_runner = ConnectedRunner::new(&IRQ_RUNTIME, network_runner, drivers.services);

    if let Some(stack_runner) = stack_runner {
        let task = network_task(stack_runner).expect("network task storage must be available once");
        spawner.spawn(task);
        let task =
            network_status_task(stack).expect("network status task storage must be available once");
        spawner.spawn(task);
        let task = udp_echo_task(stack).expect("UDP echo task storage must be available once");
        spawner.spawn(task);
    }
    RX_PROTOCOL_STOP.reset();
    RX_PROTOCOL_STOPPED.reset();
    let task = rx_protocol_task(drivers.protocol)
        .expect("RX protocol task storage must be available once");
    spawner.spawn(task);
    esp_println::println!(
        "open-radio: connected datapath active phy={} tx={}kbps ampdu={}kbps",
        drivers.report.link.association_phy.name(),
        drivers.report.data_tx_rate.nominal_kbps(),
        drivers.report.aggregate_tx_rate.nominal_kbps(),
    );

    let outcome =
        match run_esp32s31_connected_station_epoch(&mut radio_runner, station_control).await {
            Esp32s31ConnectedStationExit::Disconnected => ConnectedStationOutcome::Disconnected,
            Esp32s31ConnectedStationExit::ReconnectRequested { .. } => {
                ConnectedStationOutcome::ReconnectRequested
            }
            Esp32s31ConnectedStationExit::StationStopped(command) => {
                ConnectedStationOutcome::StationStopped(command)
            }
            Esp32s31ConnectedStationExit::HardwareFailure(_) => {
                esp_println::println!(
                    "open-radio: connected hardware failure; resetting retained radio owners"
                );
                software_reset()
            }
        };
    esp_println::println!("open-radio: connected runner stopped: {outcome:?}");
    if let Err(error) = interrupt_epoch.quiesce(platform) {
        esp_println::println!(
            "open-radio: MAC interrupt quiescence invariant failed: {error:?}; resetting"
        );
        software_reset();
    }
    let mut tasks = ConnectedTaskGroup;
    let stopped_protocol =
        match stop_esp32s31_connected_task_group(&mut tasks, CONNECTED_TASK_STOP_TIMEOUT).await {
            Esp32s31ConnectedTaskStopOutcome::Stopped(stopped) => stopped,
            Esp32s31ConnectedTaskStopOutcome::ResetRequired { .. } => {
                esp_println::println!(
                    "open-radio: connected RX protocol stop timed out; resetting retained owners"
                );
                software_reset()
            }
        };
    let shutdown = stopped_protocol.shutdown();
    esp_println::println!(
        "open-radio: RX protocol stopped queued={} retained={} commands={} active={}",
        shutdown.queued_frames,
        shutdown.retained_frames,
        shutdown.reorder_commands,
        shutdown.active_reorders,
    );
    let (frame, ethernet) = stopped_protocol.into_scratch();
    let (network_runner, services) = radio_runner.into_parts();
    let teardown = match Esp32s31ConnectedStaTeardownPort::try_teardown(services, group) {
        Ok(teardown) => teardown,
        Err(Esp32s31ConnectedStaTeardownFailure::Control { .. }) => {
            esp_println::println!("open-radio: connected control teardown failed; resetting");
            software_reset()
        }
        Err(Esp32s31ConnectedStaTeardownFailure::Rx { .. }) => {
            esp_println::println!("open-radio: connected RX DMA teardown failed; resetting");
            software_reset()
        }
        Err(Esp32s31ConnectedStaTeardownFailure::TxActive { .. }) => {
            esp_println::println!("open-radio: connected TX remained active; resetting");
            software_reset()
        }
    };
    *sequences = teardown.sequences;
    tx_storage
        .restore_resources(teardown.tx_resources)
        .unwrap_or_else(|_| {
            esp_println::println!("open-radio: connected TX return found a live owner; resetting");
            software_reset()
        });
    let disconnected: ConnectedDisconnectedEpoch = Esp32s31DisconnectedStaEpoch::new(
        RunningNetwork {
            stack,
            runner: network_runner,
        },
        teardown.hardware,
        teardown.stopped_rx,
        teardown.aggregate,
        control_resources,
    );
    ConnectedStationReturn {
        disconnected,
        frame,
        ethernet,
        outcome,
    }
}

/// Allocate the one-time Embassy network owner before the station lifecycle.
pub fn initialize_station_network(station_address: [u8; 6]) -> StationNetwork {
    let network_resources = NETWORK_RESOURCES.init_with(NetworkResources::new);
    let tx_pool = NetworkTxPool::pin_static(NETWORK_TX_POOL.init_with(NetworkTxPool::new));
    let (device, runner) = network_resources.split(tx_pool, station_address);
    StationNetwork::Unstarted { device, runner }
}

/// Construct the reusable interrupt epoch retained by the station backend.
pub fn mac_interrupt_epoch(setup: MacInterruptSetup) -> MacInterruptEpoch {
    Esp32s31MacInterruptEpoch::new(
        EspHalMacInterruptRoute::new(mac_interrupt, power_interrupt),
        setup,
        &IRQ_RUNTIME,
        &POWER_IRQ_RUNTIME,
    )
}
