//! Connected Embassy composition for the standalone station application.
//!
//! This module chooses board allocation and application network policy. The
//! reusable driver owns PAC/DMA/IRQ and 802.11 protocol transitions; no HIL
//! command, benchmark or qualification telemetry is part of this graph.

use embassy_executor::{SpawnError, Spawner};
use embassy_net::{Config, Stack, StackResources};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    channel::{Channel, Receiver, Sender},
};
use embassy_time::Timer;
use open_esp_radio::esp32s31::wifi::device::register_arena::Esp32s31RadioRegistersArena;
use open_esp_radio::esp32s31::wifi::sta::attempt::Esp32s31StaAttemptSecurity;
use open_esp_radio::esp32s31::wifi::sta::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio::{
    adapters::network::embassy_net::{
        PinnedTxFrame, PinnedTxPool, SplitPinnedDevice, SplitPinnedRadioRunner,
        SplitPinnedResources,
    },
    adapters::wifi::embassy::connected_tasks::{
        ConnectedTaskControlError, ConnectedTaskControlResources, ConnectedTaskEndpoint,
        ConnectedTaskReservation,
    },
    adapters::wifi::embassy::station_network::{RunningStationNetwork, StationNetworkResources},
    esp32s31::{
        hal::RadioRegisters,
        registers::MacInterruptSetup,
        wifi::dma::tx_ampdu_storage::AmpduDmaStorage,
        wifi::mac::{
            crypto::{StaCcmpClearReport, StaGroupCcmpSlot},
            rx::RxIngressConfig,
            rx::RxRingError,
            rx_pool::RxStagePool,
            tx::{HeEdcaTxopLimit, HtGuardInterval, HtMcs, LegacyRate},
            tx_ampdu::{
                HtAmpduTxError, HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage,
            },
        },
    },
    wifi::{ieee80211::station::StaTxSequenceCounters, wpa2::Pmk},
};
use open_esp_radio_esp32s31_wifi_embassy::{
    aggregate_tx::{AggregateTxResources, Esp32s31ConnectedTx},
    connected_rx_protocol::{
        Esp32s31ConnectedRxProtocol, Esp32s31ConnectedRxProtocolStopped,
        Esp32s31ConnectedRxProtocolStorage, Esp32s31StagedRxQueue,
    },
    connected_sta_port::{
        Esp32s31ConnectedStaBlockAckPolicy, Esp32s31ConnectedStaCompositionFailure,
        Esp32s31ConnectedStaConfig, Esp32s31ConnectedStaConfigError,
        Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaNetworkTxDomain,
        Esp32s31ConnectedStaRateConfig, Esp32s31ConnectedStaRxPolicy,
        Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxHandoffFailure,
        Esp32s31ConnectedStaTxPolicy, Esp32s31ConnectedStaTxResources,
    },
    connected_sta_teardown::Esp32s31ConnectedStaTeardownFailure,
    control_mailbox::{ConnectedControlPublisher, ConnectedControlResources},
    embassy_irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch},
    embassy_rx::EmbassyEsp32s31RxReloadDelay,
    network_rx::EmbassyNetConnectedRxSink,
    preconnected_rx::{
        EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRx, Esp32s31PreconnectedRxError,
    },
    resource_profile::{
        ESP32S31_DEFAULT_CONTROL_QUEUE_DEPTH as CONTROL_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY as NETWORK_FRAME_CAPACITY,
        ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH as NETWORK_RX_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH as NETWORK_TX_QUEUE_DEPTH,
        ESP32S31_DEFAULT_NETWORK_TX_TRAILER as NETWORK_TX_TRAILER,
        ESP32S31_DEFAULT_RX_REORDER_WINDOW as RX_REORDER_WINDOW,
        ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
        ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT as RX_STAGE_SLOT_COUNT,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
    },
    rx_dma_service::{Esp32s31ConnectedRx, Esp32s31RxEpochResources, Esp32s31StoppedRx},
    rx_reorder::{RxReorderCommandResources, RxReorderFrameStorage},
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        ConnectedControlShutdown as Esp32s31ConnectedControlShutdown,
        Esp32s31ConnectedDriverAssembly, Esp32s31ConnectedDriverAssemblyResources,
        Esp32s31ConnectedDriverServices, Esp32s31ConnectedDriverTeardownFailure,
        Esp32s31ConnectedEpochResources, Esp32s31ConnectedEpochStartFailure,
        Esp32s31ConnectedEpochStarted, Esp32s31ConnectedNetworkStarted,
        Esp32s31ConnectedNetworkStartedParts, Esp32s31ConnectedServiceResources,
        Esp32s31ConnectedStationExit, Esp32s31InitialConnectedEpochResources,
        Esp32s31StationCommand, Esp32s31StationCommandReceiver, NoopEsp32s31ConnectedRunObserver,
        activate_esp32s31_connected_epoch, assemble_esp32s31_connected_driver,
        prepare_esp32s31_connected_service, run_and_quiesce_esp32s31_connected_epoch,
        start_esp32s31_initial_connected_epoch, start_esp32s31_reconnected_connected_epoch,
    },
    station_epoch::{Esp32s31DisconnectedStaEpoch, Esp32s31ReconnectedStaEpoch},
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_esp32s31_wifi_esp_hal::mac_interrupt_epoch::{
    EspHalMacInterruptRoute, service_mac_interrupt, service_power_interrupt,
};
use static_cell::{ConstStaticCell, StaticCell};

use crate::station::{
    ControlTx, ProductionStationBoardResources, ProductionStationRuntime, RX_BUFFER_SIZE,
    RX_DESCRIPTOR_COUNT, RxStorage, TxStorage, production_station_runtime,
};

const NETWORK_TX_HEADROOM: usize =
    8 + open_esp_radio::wifi::ieee80211::station::STA_PROTECTED_QOS_ETHERNET_HEADROOM;
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
type ConnectedTxBacking = PinnedTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type ConnectedAmpduRetention = RetainedAmpduDmaStorage<ConnectedTxBacking, TX_AMPDU_FRAME_COUNT>;
type NetworkDevice = SplitPinnedDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkStackRunner = embassy_net::Runner<'static, NetworkDevice>;
pub(super) type NetworkRunner = SplitPinnedRadioRunner<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
pub(super) type ControlResources =
    ConnectedControlResources<CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
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
pub(super) type ConnectedRxProtocolStorage = Esp32s31ConnectedRxProtocolStorage<
    'static,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_WINDOW,
>;
type ConnectedRxProtocolStoppedOwner = Esp32s31ConnectedRxProtocolStopped<
    'static,
    'static,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_WINDOW,
>;
type ConnectedLiveRx = Esp32s31ConnectedRx<
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
pub type ConnectedHardware = CooperativeRadioHardware<'static>;
pub(super) type ConnectedStoppedRx = Esp32s31StoppedRx<
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
pub(super) type ConnectedRxEpochResources = Esp32s31RxEpochResources<
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
pub(super) type ConnectedAmpduStorage =
    AggregateTxResources<'static, ConnectedTxBacking, TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>;
type ConnectedLiveTx = Esp32s31ConnectedTx<
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    open_esp_radio::esp32s31::phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    { open_esp_radio_esp32s31_wifi_embassy::resource_profile::ESP32S31_DEFAULT_TX_BUFFER_SIZE },
>;
type ConnectedDriverServices = Esp32s31ConnectedDriverServices<
    'static,
    CriticalSectionRawMutex,
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedLiveTx,
    CONTROL_QUEUE_DEPTH,
>;
type ConnectedServicesMapper = fn(ConnectedDriverServices) -> ConnectedDriverServices;
type ConnectedProtocolAssemblyResources = Esp32s31ConnectedStaRxProtocolResources<
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
type ConnectedControlAssemblyResources =
    Esp32s31ConnectedStaControlResources<'static, CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
type ConnectedTxAssemblyFailure = Esp32s31ConnectedStaTxHandoffFailure<
    'static,
    'static,
    ConnectedTxBacking,
    open_esp_radio::esp32s31::phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    { open_esp_radio_esp32s31_wifi_embassy::resource_profile::ESP32S31_DEFAULT_TX_BUFFER_SIZE },
>;
type ConnectedAssemblyComposition = Esp32s31ConnectedStaCompositionFailure<
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedProtocolAssemblyResources,
    ConnectedControlAssemblyResources,
    ConnectedTxAssemblyFailure,
>;
type ConnectedAssemblyFailure =
    open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31ConnectedDriverAssemblyFailure<
        NetworkRunner,
        ConnectedAssemblyComposition,
        ConnectedServicesMapper,
    >;
type ConnectedTaskReservationOwner =
    ConnectedTaskReservation<'static, CriticalSectionRawMutex, ConnectedRxProtocolStoppedOwner>;
type ConnectedDriverStarted = Esp32s31ConnectedEpochStarted<
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
type ConnectedDriverTeardownFailure = Esp32s31ConnectedDriverTeardownFailure<
    'static,
    CriticalSectionRawMutex,
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedStoppedRx,
    ConnectedLiveTx,
    CONTROL_QUEUE_DEPTH,
    RxRingError,
>;
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
    RunningStationNetwork<Stack<'static>, NetworkRunner>,
    ConnectedHardware,
    ConnectedStoppedRx,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
pub(super) type ConnectedRunningNetwork = RunningStationNetwork<Stack<'static>, NetworkRunner>;
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
// Reorder machines and retained RX lease tokens are long-lived protocol
// state. Keeping their arena static prevents this multi-kilobyte owner table
// from being copied through the connected async state machine's stack frame.
static RX_PROTOCOL_RUNTIME: StaticCell<ConnectedRxProtocolStorage> = StaticCell::new();
static CONTROL_RESOURCES: ConstStaticCell<ControlResources> =
    ConstStaticCell::new(ControlResources::new());
// Network RX slots, pinned TX slots and Embassy socket state are the largest
// standalone owners. Const static initialization guarantees they are never
// returned by value through an async task stack during startup.
static NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
static NETWORK_TX_POOL: ConstStaticCell<NetworkTxPool> = ConstStaticCell::new(NetworkTxPool::new());
static NETWORK_STACK_RESOURCES: ConstStaticCell<StackResources<4>> =
    ConstStaticCell::new(StackResources::new());
static TX_AMPDU_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
static TX_AMPDU_DMA_STORAGE: ConstStaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    ConstStaticCell::new(AmpduDmaStorage::new());
// Network lease tokens and their descriptor identities live for an entire
// connected epoch. Keep this multi-slot arena static so it is borrowed by the
// movable TX handle instead of being copied through nested async stack frames.
static TX_AMPDU_RETENTION: StaticCell<ConnectedAmpduRetention> = StaticCell::new();
static REGISTER_ARENA: ConstStaticCell<Esp32s31RadioRegistersArena> =
    ConstStaticCell::new(Esp32s31RadioRegistersArena::new());
// Ethernet scratch belongs to the driver datapath. Application socket buffers
// are deliberately not allocated by this product integration.
static ETHERNET_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
static RX_PROTOCOL_TASK_CONTROL: ConnectedTaskControlResources<
    CriticalSectionRawMutex,
    ConnectedRxProtocolStoppedOwner,
> = ConnectedTaskControlResources::new();
type ConnectedProtocolStartChannel = Channel<CriticalSectionRawMutex, ConnectedProtocolStart, 1>;
static RX_PROTOCOL_START: ConnectedProtocolStartChannel = Channel::new();
// `embassy-net` deliberately keeps Stack and Runner local to one executor.
// These channels are therefore initialized once and their two endpoints are
// handed only to tasks on the station executor; they are not global
// cross-core mailboxes.
static NETWORK_RUNNER_START: StaticCell<Channel<NoopRawMutex, NetworkStackRunner, 1>> =
    StaticCell::new();
static NETWORK_APPLICATION_START: StaticCell<Channel<NoopRawMutex, Stack<'static>, 1>> =
    StaticCell::new();

struct ConnectedProtocolStart {
    protocol: ConnectedRxProtocol,
    endpoint:
        ConnectedTaskEndpoint<'static, CriticalSectionRawMutex, ConnectedRxProtocolStoppedOwner>,
}

/// Send-only handles for the one-time network services running on the same
/// executor as the station supervisor.
#[derive(Clone, Copy)]
pub(super) struct ConnectedWorkerPublishers {
    network_runner: Sender<'static, NoopRawMutex, NetworkStackRunner, 1>,
    network_application: Sender<'static, NoopRawMutex, Stack<'static>, 1>,
}

/// Application-facing readiness edge for the persistent Embassy network.
///
/// Receiving the stack does not transfer radio, DMA or IRQ ownership; those
/// remain inside the physical supervisor for the whole station epoch.
pub struct Esp32s31StationNetworkEvents {
    receiver: Receiver<'static, NoopRawMutex, Stack<'static>, 1>,
}

impl Esp32s31StationNetworkEvents {
    pub async fn wait_connected(&mut self) -> Stack<'static> {
        self.receiver.receive().await
    }
}

/// One-time versus persistent Embassy network ownership.
pub type StationNetwork = StationNetworkResources<NetworkDevice, NetworkRunner, Stack<'static>>;

/// Hardware frontier accepted by one connected epoch.
pub type ConnectedStationEpoch = Esp32s31ConnectedEpochResources<
    RadioRegisters,
    Esp32s31PreconnectedRx<
        'static,
        EmbassyEsp32s31PreconnectedRxDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    ConnectedReconnectedEpoch,
>;
type ConnectedPreconnectedRx = Esp32s31PreconnectedRx<
    'static,
    EmbassyEsp32s31PreconnectedRxDelay,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
>;
type InitialConnectedResources = Esp32s31InitialConnectedEpochResources<
    'static,
    ConnectedRxEpochResources,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
type ConnectedEpochStartFault = Esp32s31ConnectedEpochStartFailure<
    InitialConnectedResources,
    ConnectedHardware,
    ConnectedPreconnectedRx,
    ConnectedRxEpochResources,
    ConnectedAmpduStorage,
    &'static ControlResources,
    Esp32s31PreconnectedRxError,
>;

/// Initial-only connected resources materialized before the supervisor can
/// activate any station IRQ or DMA epoch.
pub(super) struct InitialConnectedStaticResources {
    registers: &'static Esp32s31RadioRegistersArena,
    aggregate: ConnectedAmpduStorage,
    control: &'static ControlResources,
}

impl InitialConnectedStaticResources {
    fn with_rx(
        self,
        rx: ConnectedRxEpochResources,
    ) -> Esp32s31InitialConnectedEpochResources<
        'static,
        ConnectedRxEpochResources,
        ConnectedAmpduStorage,
        &'static ControlResources,
    > {
        Esp32s31InitialConnectedEpochResources::new(
            self.registers,
            rx,
            self.aggregate,
            self.control,
        )
    }
}

type ConnectedNetworkStarted<'state, 'security> = Esp32s31ConnectedNetworkStarted<
    'security,
    ProductionStationRuntime<'state>,
    ConnectedStationEpoch,
    Stack<'static>,
    NetworkRunner,
    NetworkStackRunner,
>;

/// Normal outcome returned after all connected owners are quiescent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStationOutcome {
    Disconnected,
    ReconnectRequested,
    StationStopped(Esp32s31StationCommand),
    HardwareFailure,
}

/// Exact input for the next finite join attempt.
pub struct ConnectedStationReturn<'state, 'security> {
    pub disconnected: ConnectedDisconnectedEpoch,
    pub runtime: ProductionStationRuntime<'state>,
    pub security: Esp32s31StaAttemptSecurity<'security>,
    pub outcome: ConnectedStationOutcome,
}

pub(super) struct ConnectedDriverAssemblyFault {
    _role: open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRoleOwner<
        EspHalRadioPeripheral,
    >,
    _interrupt: MacInterruptEpoch,
    _dma: open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationDmaResources<
        'static,
        RxStorage,
        RX_DESCRIPTOR_COUNT,
    >,
    _tx_storage: &'static mut TxStorage,
    _scan_table: &'static mut open_esp_radio::wifi::ieee80211::scan::ScanTable,
    _stack: Stack<'static>,
    _initial_network_task: Option<NetworkStackRunner>,
    _control_resources: &'static ControlResources,
    _group: StaGroupCcmpSlot,
    _pmk: Pmk,
    _supplicant_nonce: [u8; 32],
    _message4_protection: open_esp_radio::esp32s31::wifi::sta::wpa2::Esp32s31Wpa2Message4Protection,
    _interface: open_esp_radio::wifi::softmac::interface::BoundVirtualInterface,
    _failure: ConnectedAssemblyFailure,
    _task_reservation: ConnectedTaskReservationOwner,
}

/// Non-reusable connected owner retained at the exact failed transition.
/// No variant exposes the ordinary disconnected owner required for retry.
pub enum ConnectedStationFault<'state, 'security> {
    TaskControlUnavailable {
        _resources: ConnectedStationResources<'state, 'security>,
        _error: ConnectedTaskControlError,
    },
    InvalidConnectedPolicy {
        _resources: ConnectedStationResources<'state, 'security>,
        _error: Esp32s31ConnectedStaConfigError,
    },
    DriverAssembly {
        _fault: ConnectedDriverAssemblyFault,
    },
    OrdinaryTxUnavailable {
        _runtime: ProductionStationRuntime<'state>,
        _started: ConnectedDriverStarted,
        _stack: Stack<'static>,
        _network: NetworkRunner,
        _initial_network_task: Option<NetworkStackRunner>,
        _plan: open_esp_radio_esp32s31_wifi_embassy::connected_sta_port::Esp32s31ConnectedStaPlan,
        _pairwise: open_esp_radio::esp32s31::wifi::mac::crypto::StaPairwiseCcmpSlot,
        _group: StaGroupCcmpSlot,
        _security: Esp32s31StaAttemptSecurity<'security>,
        _task_reservation: ConnectedTaskReservationOwner,
        _error: open_esp_radio::esp32s31::wifi::sta::tx_epoch::Esp32s31StaTxEpochError,
    },
    InitialStaticResourcesUnavailable {
        _runtime: ProductionStationRuntime<'state>,
        _hardware: RadioRegisters,
        _receive: ConnectedPreconnectedRx,
        _stack: Stack<'static>,
        _network: NetworkRunner,
        _initial_network_task: Option<NetworkStackRunner>,
        _plan: open_esp_radio_esp32s31_wifi_embassy::connected_sta_port::Esp32s31ConnectedStaPlan,
        _pairwise: open_esp_radio::esp32s31::wifi::mac::crypto::StaPairwiseCcmpSlot,
        _group: StaGroupCcmpSlot,
        _security: Esp32s31StaAttemptSecurity<'security>,
        _task_reservation: ConnectedTaskReservationOwner,
    },
    InterruptActivation {
        _started: ConnectedNetworkStarted<'state, 'security>,
    },
    EpochStart {
        _runtime: ProductionStationRuntime<'state>,
        _failure: ConnectedEpochStartFault,
        _stack: Stack<'static>,
        _network: NetworkRunner,
        _initial_network_task: Option<NetworkStackRunner>,
        _plan: open_esp_radio_esp32s31_wifi_embassy::connected_sta_port::Esp32s31ConnectedStaPlan,
        _pairwise: open_esp_radio::esp32s31::wifi::mac::crypto::StaPairwiseCcmpSlot,
        _group: open_esp_radio::esp32s31::wifi::mac::crypto::StaGroupCcmpSlot,
        _security: Esp32s31StaAttemptSecurity<'security>,
    },
    DriverTeardown {
        _runtime: ProductionStationRuntime<'state>,
        _network: ConnectedRunningNetwork,
        _control_resources: &'static ControlResources,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain:
            open_esp_radio_esp32s31_wifi_embassy::embassy_irq::Esp32s31MacInterruptEpochDrain,
        _error: ConnectedDriverTeardownFailure,
        _pmk: Pmk,
        _supplicant_nonce: [u8; 32],
        _message4_protection:
            open_esp_radio::esp32s31::wifi::sta::wpa2::Esp32s31Wpa2Message4Protection,
    },
    TxRestore {
        _runtime: ProductionStationRuntime<'state>,
        _network: ConnectedRunningNetwork,
        _control_resources: &'static ControlResources,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain:
            open_esp_radio_esp32s31_wifi_embassy::embassy_irq::Esp32s31MacInterruptEpochDrain,
        _hardware: ConnectedHardware,
        _stopped_rx: ConnectedStoppedRx,
        _aggregate: ConnectedAmpduStorage,
        _control_report: Esp32s31ConnectedControlShutdown,
        _keys: StaCcmpClearReport,
        _sequences: StaTxSequenceCounters,
        _error: open_esp_radio::esp32s31::wifi::sta::tx_epoch::Esp32s31StaTxEpochError,
        _returned_control: ControlTx,
        _pmk: Pmk,
        _supplicant_nonce: [u8; 32],
        _message4_protection:
            open_esp_radio::esp32s31::wifi::sta::wpa2::Esp32s31Wpa2Message4Protection,
    },
}

pub enum ConnectedStationRunExit<'state, 'security> {
    Returned(ConnectedStationReturn<'state, 'security>),
    Faulted(ConnectedStationFault<'state, 'security>),
}

/// Exact owners transferred from a successful finite station attempt.
pub type ConnectedStationResources<'state, 'security> = Esp32s31ConnectedServiceResources<
    'security,
    ProductionStationRuntime<'state>,
    ConnectedStationEpoch,
    StationNetwork,
>;

pub(super) fn initialize_ethernet_frame() -> &'static mut [u8] {
    ETHERNET_FRAME.take().as_mut_slice()
}

/// Materialize the large connected-RX arena once; every later connected
/// epoch receives and returns this exact lease through the station owner.
pub(super) fn initialize_connected_rx_protocol_runtime() -> &'static mut ConnectedRxProtocolStorage
{
    RX_PROTOCOL_RUNTIME.init_with(Esp32s31ConnectedRxProtocolStorage::new)
}

/// Bind the one-time A-MPDU descriptor arena before the radio supervisor can
/// enter an active station epoch.
pub(super) fn initialize_connected_static_resources()
-> Result<InitialConnectedStaticResources, HtAmpduTxError> {
    let aggregate = AggregateTxResources::single(
        HtAmpduTxResources::pin_static(TX_AMPDU_STORAGE.take(), TX_AMPDU_DMA_STORAGE.take())?,
        TX_AMPDU_RETENTION.init_with(RetainedAmpduDmaStorage::new),
    );
    Ok(InitialConnectedStaticResources {
        registers: REGISTER_ARENA.take(),
        aggregate,
        control: &*CONTROL_RESOURCES.take(),
    })
}

#[esp_hal::handler]
// The handler must execute while flash access may be unavailable. This is a
// declarative linker placement at the combined esp-hal/Embassy integration
// boundary; it performs no raw memory operation.
#[allow(unsafe_code)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn mac_interrupt() {
    let _ = service_mac_interrupt(&IRQ_RUNTIME);
}

#[esp_hal::handler]
#[allow(unsafe_code)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn power_interrupt() {
    let _ = service_power_interrupt(&POWER_IRQ_RUNTIME);
}

#[embassy_executor::task]
async fn network_runner_worker(starts: Receiver<'static, NoopRawMutex, NetworkStackRunner, 1>) {
    let mut runner = starts.receive().await;
    runner.run().await
}

#[embassy_executor::task]
async fn rx_protocol_worker(
    starts: Receiver<'static, CriticalSectionRawMutex, ConnectedProtocolStart, 1>,
) {
    loop {
        let ConnectedProtocolStart { protocol, endpoint } = starts.receive().await;
        protocol.run_controlled_task(endpoint, |_| {}).await;
    }
}

/// Start owner-free workers before the physical radio supervisor. Subsequent
/// epochs publish only software protocol/network owners to these workers.
pub(super) fn spawn_connected_workers(
    spawner: Spawner,
) -> Result<(ConnectedWorkerPublishers, Esp32s31StationNetworkEvents), SpawnError> {
    let network_runner_start = NETWORK_RUNNER_START.init(Channel::new());
    let network_application_start = NETWORK_APPLICATION_START.init(Channel::new());
    let protocol = rx_protocol_worker(RX_PROTOCOL_START.receiver())?;
    let network = network_runner_worker(network_runner_start.receiver())?;
    spawner.spawn(protocol);
    spawner.spawn(network);
    Ok((
        ConnectedWorkerPublishers {
            network_runner: network_runner_start.sender(),
            network_application: network_application_start.sender(),
        },
        Esp32s31StationNetworkEvents {
            receiver: network_application_start.receiver(),
        },
    ))
}

pub(super) const fn connected_config() -> Esp32s31ConnectedStaConfig {
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
pub async fn run_connected<'state, 'security>(
    station_control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    resources: ConnectedStationResources<'state, 'security>,
) -> ConnectedStationRunExit<'state, 'security> {
    let task_reservation = match RX_PROTOCOL_TASK_CONTROL.reserve() {
        Ok(reservation) => reservation,
        Err(error) => {
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::TaskControlUnavailable {
                    _resources: resources,
                    _error: error,
                },
            );
        }
    };
    let prepared = match prepare_esp32s31_connected_service::<
        TX_AMPDU_FRAME_COUNT,
        RX_REORDER_WINDOW,
        _,
        _,
        _,
    >(resources)
    {
        Ok(prepared) => prepared,
        Err(failure) => {
            let error = failure.error;
            let resources = failure.into_resources();
            task_reservation.abort_unused();
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::InvalidConnectedPolicy {
                    _resources: resources,
                    _error: error,
                },
            );
        }
    };
    let mut started = prepared.start_network(|_runtime, device, plan| {
        let station_address = plan.link().station_address;
        let stack_resources = NETWORK_STACK_RESOURCES.take();
        let mut seed = [0_u8; 8];
        seed[..6].copy_from_slice(&station_address);
        seed[6..].copy_from_slice(&0x31a5_u16.to_le_bytes());
        let (stack, stack_runner) = embassy_net::new(
            device,
            Config::dhcpv4(Default::default()),
            stack_resources,
            u64::from_le_bytes(seed),
        );
        (stack, stack_runner)
    });
    let activation = {
        let (radio, _storage, _board) = started.runtime_mut().split_mut();
        let (_phy, platform, interrupt) = radio.parts_mut();
        activate_esp32s31_connected_epoch(interrupt, platform)
    };
    if let Err(error) = activation {
        esp_println::println!(
            "open-radio: MAC interrupt activation invariant failed: {error:?}; quarantined"
        );
        task_reservation.abort_unused();
        return ConnectedStationRunExit::Faulted(ConnectedStationFault::InterruptActivation {
            _started: started,
        });
    }
    let Esp32s31ConnectedNetworkStartedParts {
        runtime,
        epoch,
        stack,
        network: network_runner,
        initial_network_task: stack_runner,
        plan,
        pairwise,
        group,
        security,
    } = started.into_parts();
    let runtime = runtime.into_parts();
    let (mut role, interrupt_epoch) = runtime.radio.into_parts();
    let (dma, tx_storage, scan_table, frame, ethernet) = runtime.storage.into_parts();
    let mut board = runtime.board;

    let (staged_sender, staged_receiver) = STAGED_RX_QUEUE.split();
    let start = match epoch {
        ConnectedStationEpoch::Initial { hardware, receive } => {
            let initial = match board.initial_connected.take() {
                Some(initial) => initial,
                None => {
                    return ConnectedStationRunExit::Faulted(
                        ConnectedStationFault::InitialStaticResourcesUnavailable {
                            _runtime: production_station_runtime(
                                role,
                                interrupt_epoch,
                                dma,
                                tx_storage,
                                scan_table,
                                frame,
                                ethernet,
                                board,
                            ),
                            _hardware: hardware,
                            _receive: receive,
                            _stack: stack,
                            _network: network_runner,
                            _initial_network_task: stack_runner,
                            _plan: plan,
                            _pairwise: pairwise,
                            _group: group,
                            _security: security,
                            _task_reservation: task_reservation,
                        },
                    );
                }
            };
            let rx = Esp32s31RxEpochResources::new(
                dma.storage(),
                &RX_STAGE_POOL,
                staged_sender,
                EmbassyEsp32s31RxReloadDelay,
            );
            start_esp32s31_initial_connected_epoch(hardware, receive, initial.with_rx(rx)).await
        }
        ConnectedStationEpoch::Reconnected(epoch) => {
            start_esp32s31_reconnected_connected_epoch(epoch).await
        }
    };
    let started = match start {
        Ok(started) => started,
        Err(failure) => {
            match &failure {
                Esp32s31ConnectedEpochStartFailure::RegisterPublication { error, .. } => {
                    esp_println::println!(
                        "open-radio: connected register publication failed: {error:?}; quarantined"
                    );
                }
                Esp32s31ConnectedEpochStartFailure::Receive { phase, error, .. } => {
                    esp_println::println!(
                        "open-radio: connected RX arm failed phase={phase:?} error={error:?}; quarantined"
                    );
                }
            }
            task_reservation.abort_unused();
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::EpochStart {
                _runtime: production_station_runtime(
                    role,
                    interrupt_epoch,
                    dma,
                    tx_storage,
                    scan_table,
                    frame,
                    ethernet,
                    board,
                ),
                _failure: failure,
                _stack: stack,
                _network: network_runner,
                _initial_network_task: stack_runner,
                _plan: plan,
                _pairwise: pairwise,
                _group: group,
                _security: security,
            });
        }
    };
    let control_tx = match tx_storage.take_control() {
        Ok(control) => control,
        Err(error) => {
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::OrdinaryTxUnavailable {
                    _runtime: production_station_runtime(
                        role,
                        interrupt_epoch,
                        dma,
                        tx_storage,
                        scan_table,
                        frame,
                        ethernet,
                        board,
                    ),
                    _started: started,
                    _stack: stack,
                    _network: network_runner,
                    _initial_network_task: stack_runner,
                    _plan: plan,
                    _pairwise: pairwise,
                    _group: group,
                    _security: security,
                    _task_reservation: task_reservation,
                    _error: error,
                },
            );
        }
    };
    let ProductionStationBoardResources {
        interface,
        rx_protocol_runtime,
        initial_connected,
        workers,
    } = board;
    let Esp32s31StaAttemptSecurity {
        pmk,
        supplicant_nonce,
        sequences,
        message4_protection,
        ..
    } = security;
    let (_phy, platform) = role.radio_mut();
    let Esp32s31ConnectedEpochStarted {
        hardware,
        rx,
        aggregate_tx: aggregate,
        control: control_resources,
    } = started;

    let network_rx = network_runner.rx_publisher();
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new(network_rx, control_publisher);
    let (reorder_sender, reorder_receiver) = RX_REORDER_COMMANDS.split();
    let tx_sequences = sequences;
    let assembled =
        match assemble_esp32s31_connected_driver(Esp32s31ConnectedDriverAssemblyResources {
            plan,
            irq: &IRQ_RUNTIME,
            network: network_runner,
            hardware,
            rx,
            protocol: Esp32s31ConnectedStaRxProtocolResources {
                frames: staged_receiver,
                irq: &IRQ_RUNTIME,
                sink: rx_sink,
                mpdu: frame,
                ethernet,
                reorder_commands: reorder_receiver,
                reorder_storage: &RX_REORDER_STORAGE,
                runtime: rx_protocol_runtime,
                reorder_scratch: None,
                pipeline_observer: None,
            },
            tx: Esp32s31ConnectedStaTxResources {
                control: control_tx,
                aggregate,
                pairwise_key: pairwise,
                sequences: tx_sequences,
                aggregate_tx_observer: None,
                network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
            },
            control: Esp32s31ConnectedStaControlResources {
                receiver: control_receiver,
                reorder_commands: reorder_sender,
            },
            map_services: core::convert::identity::<ConnectedDriverServices>
                as ConnectedServicesMapper,
        }) {
            Ok(assembled) => assembled,
            Err(failure) => {
                esp_println::println!(
                    "open-radio: connected TX handoff found a live owner; quarantined"
                );
                return ConnectedStationRunExit::Faulted(ConnectedStationFault::DriverAssembly {
                    _fault: ConnectedDriverAssemblyFault {
                        _role: role,
                        _interrupt: interrupt_epoch,
                        _dma: dma,
                        _tx_storage: tx_storage,
                        _scan_table: scan_table,
                        _stack: stack,
                        _initial_network_task: stack_runner,
                        _control_resources: control_resources,
                        _group: group,
                        _pmk: pmk,
                        _supplicant_nonce: supplicant_nonce,
                        _message4_protection: message4_protection,
                        _interface: interface,
                        _failure: failure,
                        _task_reservation: task_reservation,
                    },
                });
            }
        };
    let Esp32s31ConnectedDriverAssembly {
        runner: radio_runner,
        protocol: rx_protocol,
        report,
    } = assembled;

    let (tasks, protocol_endpoint) = task_reservation.into_endpoints();

    if let Some(stack_runner) = stack_runner {
        workers.network_runner.send(stack_runner).await;
        workers.network_application.send(stack).await;
    }
    RX_PROTOCOL_START
        .sender()
        .send(ConnectedProtocolStart {
            protocol: rx_protocol,
            endpoint: protocol_endpoint,
        })
        .await;
    esp_println::println!(
        "open-radio: connected datapath active phy={} tx={}kbps ampdu={}kbps",
        report.link.association_phy.name(),
        report.data_tx_rate.nominal_kbps(),
        report.aggregate_tx_rate.nominal_kbps(),
    );

    let mut observer = NoopEsp32s31ConnectedRunObserver;
    let stopped = match run_and_quiesce_esp32s31_connected_epoch(
        interrupt_epoch,
        platform,
        radio_runner,
        station_control,
        tasks,
        &mut observer,
        |exit, _runner| match exit {
            Esp32s31ConnectedStationExit::Disconnected => ConnectedStationOutcome::Disconnected,
            Esp32s31ConnectedStationExit::ReconnectRequested { .. } => {
                ConnectedStationOutcome::ReconnectRequested
            }
            Esp32s31ConnectedStationExit::StationStopped(command) => {
                ConnectedStationOutcome::StationStopped(command)
            }
            Esp32s31ConnectedStationExit::HardwareFailure(_) => {
                ConnectedStationOutcome::HardwareFailure
            }
        },
    )
    .await
    {
        Ok(stopped) => stopped,
        Err(mut pending) => loop {
            esp_println::println!(
                "open-radio: MAC interrupt quiescence still pending: {:?}",
                pending.error
            );
            Timer::after_millis(1).await;
            match pending.retry_quiesce(platform).await {
                Ok(stopped) => break stopped,
                Err(returned) => pending = returned,
            }
        },
    };
    let outcome = stopped.exit;
    esp_println::println!("open-radio: connected runner stopped: {outcome:?}");
    let shutdown = stopped.quiesced.tasks.shutdown();
    esp_println::println!(
        "open-radio: RX protocol stopped queued={} retained={} commands={} active={}",
        shutdown.queued_frames,
        shutdown.retained_frames,
        shutdown.reorder_commands,
        shutdown.active_reorders,
    );
    let teardown = match stopped.try_teardown(group) {
        Ok(teardown) => teardown,
        Err(failure) => {
            let message = match &failure.error {
                Esp32s31ConnectedStaTeardownFailure::Control { .. } => {
                    "connected control teardown failed"
                }
                Esp32s31ConnectedStaTeardownFailure::Rx { .. } => {
                    "connected RX DMA teardown failed"
                }
                Esp32s31ConnectedStaTeardownFailure::TxActive { .. } => {
                    "connected TX remained active"
                }
            };
            esp_println::println!("open-radio: {message}; quarantined");
            let open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31ConnectedServiceTeardownFailure {
                exit,
                interrupt,
                interrupt_drain,
                network,
                tasks,
                error,
            } = failure;
            let (frame, ethernet, rx_protocol_runtime) = tasks.into_parts();
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::DriverTeardown {
                _runtime: production_station_runtime(
                    role,
                    interrupt,
                    dma,
                    tx_storage,
                    scan_table,
                    frame,
                    ethernet,
                    ProductionStationBoardResources {
                        interface,
                        rx_protocol_runtime,
                        initial_connected,
                        workers,
                    },
                ),
                _network: RunningStationNetwork::new(stack, network),
                _control_resources: control_resources,
                _outcome: exit,
                _interrupt_drain: interrupt_drain,
                _error: error,
                _pmk: pmk,
                _supplicant_nonce: supplicant_nonce,
                _message4_protection: message4_protection,
            });
        }
    };
    let stopped_protocol = teardown.tasks;
    let (frame, ethernet, rx_protocol_runtime) = stopped_protocol.into_parts();
    let network_runner = teardown.network;
    let interrupt_epoch = teardown.interrupt;
    let interrupt_drain = teardown.interrupt_drain;
    let teardown = teardown.driver;
    let sequences = teardown.sequences;
    if let Err(failure) = tx_storage.restore_resources(teardown.tx_resources) {
        esp_println::println!("open-radio: connected TX return found a live owner; quarantined");
        let (error, returned_control) = failure;
        return ConnectedStationRunExit::Faulted(ConnectedStationFault::TxRestore {
            _runtime: production_station_runtime(
                role,
                interrupt_epoch,
                dma,
                tx_storage,
                scan_table,
                frame,
                ethernet,
                ProductionStationBoardResources {
                    interface,
                    rx_protocol_runtime,
                    initial_connected,
                    workers,
                },
            ),
            _network: RunningStationNetwork::new(stack, network_runner),
            _control_resources: control_resources,
            _outcome: outcome,
            _interrupt_drain: interrupt_drain,
            _hardware: teardown.hardware,
            _stopped_rx: teardown.stopped_rx,
            _aggregate: teardown.aggregate,
            _control_report: teardown.control,
            _keys: teardown.keys,
            _sequences: sequences,
            _error: error,
            _returned_control: returned_control,
            _pmk: pmk,
            _supplicant_nonce: supplicant_nonce,
            _message4_protection: message4_protection,
        });
    }
    let disconnected: ConnectedDisconnectedEpoch = Esp32s31DisconnectedStaEpoch::new(
        RunningStationNetwork::new(stack, network_runner),
        teardown.hardware,
        teardown.stopped_rx,
        teardown.aggregate,
        control_resources,
    );
    ConnectedStationRunExit::Returned(ConnectedStationReturn {
        disconnected,
        runtime: production_station_runtime(
            role,
            interrupt_epoch,
            dma,
            tx_storage,
            scan_table,
            frame,
            ethernet,
            crate::station::ProductionStationBoardResources {
                interface,
                rx_protocol_runtime,
                initial_connected,
                workers,
            },
        ),
        security: Esp32s31StaAttemptSecurity::new(
            pmk,
            supplicant_nonce,
            sequences,
            message4_protection,
        ),
        outcome,
    })
}

/// Allocate the one-time Embassy network owner before the station lifecycle.
pub fn initialize_station_network(station_address: [u8; 6]) -> StationNetwork {
    let network_resources = NETWORK_RESOURCES.take();
    let tx_pool = NetworkTxPool::pin_static(NETWORK_TX_POOL.take());
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
