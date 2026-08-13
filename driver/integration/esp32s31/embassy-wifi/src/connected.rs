//! Connected Embassy composition for the standalone station application.
//!
//! This module chooses board allocation and application network policy. The
//! reusable driver owns PAC/DMA/IRQ and 802.11 protocol transitions; no HIL
//! command, benchmark or qualification telemetry is part of this graph.

#[cfg(feature = "qualification")]
use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

#[cfg(feature = "qualification")]
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, Receiver},
};
use embassy_time::{Instant, Timer};
use open_esp_radio::esp32s31::wifi::device::register_arena::Esp32s31RadioRegistersArena;
#[cfg(feature = "qualification")]
use open_esp_radio::esp32s31::wifi::dma::descriptor::{rx_done, rx_rearm_word};
#[cfg(feature = "qualification")]
use open_esp_radio::esp32s31::wifi::mac::connected_rx::{
    ConnectedRxEvent, ConnectedRxSink as MacConnectedRxSink,
};
#[cfg(feature = "qualification")]
use open_esp_radio::esp32s31::wifi::mac::irq::{
    IrqSink, MAC_INT_COLLISION, MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
};
use open_esp_radio::esp32s31::wifi::sta::attempt::Esp32s31StaAttemptSecurity;
use open_esp_radio::esp32s31::wifi::sta::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio::esp32s31::{
    hal::RadioRegisters,
    registers::MacInterruptSetup,
    wifi::dma::tx_ampdu_storage::AmpduDmaStorage,
    wifi::mac::{
        crypto::{StaCcmpClearReport, StaGroupCcmpSlot},
        rx::RxIngressConfig,
        rx::RxRingError,
        rx_pool::RxStagePool,
        tx::{HeEdcaTxopLimit, HtGuardInterval, HtMcs, LegacyRate},
        tx_ampdu::{HtAmpduTxError, HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage},
    },
};
use open_esp_radio::wifi::{ieee80211::station::StaTxSequenceCounters, wpa2::Pmk};
use open_esp_radio_embassy_net::{
    PinnedTxFrame, PinnedTxPool, SharedPinnedRxQueue, SharedRxSplitPinnedDevice,
    SplitPinnedRadioRunner, SplitPinnedResources,
};
#[cfg(feature = "qualification")]
use open_esp_radio_esp32s31_wifi_embassy::connected_rx_protocol::ConnectedRxProtocolSink;
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
    monitor::Esp32s31MonitorInterrupts,
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
    rx_reorder::{RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandResources, RxReorderFrameStorage},
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        ConnectedControlShutdown as Esp32s31ConnectedControlShutdown, ConnectedWpa2Security,
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
use open_esp_radio_wifi_embassy::{
    await_stack_boundary,
    connected_tasks::{
        ConnectedTaskControlError, ConnectedTaskControlResources, ConnectedTaskEndpoint,
        ConnectedTaskReservation,
    },
    station_network::{RunningStationNetwork, StationNetworkResources},
};
use static_cell::ConstStaticCell;

use crate::runtime::{
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
pub type Esp32s31WifiDevice = SharedRxSplitPinnedDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
>;
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
type EmbassyConnectedRxSink = EmbassyNetConnectedRxSink<
    'static,
    CriticalSectionRawMutex,
    ControlPublisher,
    NETWORK_FRAME_CAPACITY,
    NETWORK_RX_QUEUE_DEPTH,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
>;
#[cfg(feature = "qualification")]
type ConnectedRxSink = ObservedConnectedRxSink<EmbassyConnectedRxSink>;
#[cfg(not(feature = "qualification"))]
type ConnectedRxSink = EmbassyConnectedRxSink;
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
    RX_REORDER_BACKING_SLOT_COUNT,
>;
pub(super) type ConnectedRxProtocolStorage = Esp32s31ConnectedRxProtocolStorage<
    'static,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_BACKING_SLOT_COUNT,
>;
type ConnectedRxProtocolStoppedOwner = Esp32s31ConnectedRxProtocolStopped<
    'static,
    'static,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_BACKING_SLOT_COUNT,
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

#[cfg(feature = "qualification")]
fn log_rx_ring_topology(label: &str, rx: &ConnectedLiveRx) {
    let topology = rx.ring().topology_snapshot();
    let mut armed_mask = 0_u64;
    let mut completed_mask = 0_u64;
    let mut invalid_mask = 0_u64;
    for index in 0..RX_DESCRIPTOR_COUNT {
        let Some(descriptor) = rx.ring().descriptor_snapshot(index) else {
            invalid_mask |= 1_u64 << index;
            continue;
        };
        if rx_done(descriptor.word0) {
            completed_mask |= 1_u64 << index;
        } else if rx_rearm_word(descriptor.word0) == Some(descriptor.word0) {
            armed_mask |= 1_u64 << index;
        } else {
            invalid_mask |= 1_u64 << index;
        }
    }
    qualification_event!(
        "open-radio: connected RX topology label={} valid={} base={:#010x} start={} head={:#010x} head_next={:#010x} tail={} tail_address={:#010x} visited={} terminals={} armed_mask={:#018x} completed_mask={:#018x} invalid_mask={:#018x}",
        label,
        topology.valid,
        topology.descriptor_base,
        topology.start_index,
        topology.head_address,
        topology.head_next_address,
        topology.tail_index,
        topology.tail_address,
        topology.visited_descriptors,
        topology.terminal_descriptors,
        armed_mask,
        completed_mask,
        invalid_mask,
    );
    if !topology.valid {
        for index in 0..RX_DESCRIPTOR_COUNT {
            if let Some(descriptor) = rx.ring().descriptor_snapshot(index) {
                qualification_event!(
                    "open-radio: connected RX descriptor label={} index={} address={:#010x} word0={:#010x} buffer={:#010x} next={:#010x}",
                    label,
                    descriptor.index,
                    descriptor.address,
                    descriptor.word0,
                    descriptor.buffer_address,
                    descriptor.next_address,
                );
            }
        }
    }
}

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
    RX_REORDER_BACKING_SLOT_COUNT,
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
    RunningStationNetwork<(), NetworkRunner>,
    ConnectedHardware,
    ConnectedStoppedRx,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
pub(super) type ConnectedRunningNetwork = RunningStationNetwork<(), NetworkRunner>;
pub type MacInterruptEpoch =
    Esp32s31MacInterruptEpoch<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;

static IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> = EmbassyMacIrqRuntime::new();
static POWER_IRQ_RUNTIME: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
    EmbassyPowerIrqRuntime::new();
// Every admitted frame is copied here before its DMA descriptors are returned.
// Keeping this bounded hot working set in internal SRAM avoids PSRAM cache
// misses extending the hardware BUFFER_FULL interval under one 32-frame burst.
#[allow(
    unsafe_code,
    reason = "the linker must retain latency-critical RX staging in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_stage")]
static RX_STAGE_POOL: RxStagePool<RX_STAGE_SLOT_COUNT, RX_STAGE_CAPACITY> = RxStagePool::new();
static STAGED_RX_QUEUE: Esp32s31StagedRxQueue<
    'static,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
> = Esp32s31StagedRxQueue::new();
static SHARED_NETWORK_RX_QUEUE: SharedPinnedRxQueue<CriticalSectionRawMutex, RX_STAGE_SLOT_COUNT> =
    SharedPinnedRxQueue::new();

#[inline(never)]
fn notify_shared_network_rx_release() {
    IRQ_RUNTIME.notify_rx_capacity();
}
static RX_REORDER_COMMANDS: RxReorderCommandResources<CriticalSectionRawMutex> =
    RxReorderCommandResources::new();
static RX_REORDER_STORAGE: RxReorderFrameStorage<RX_STAGE_CAPACITY, RX_REORDER_BACKING_SLOT_COUNT> =
    RxReorderFrameStorage::new();
// Reorder machines and retained RX lease tokens are long-lived protocol
// state. Keeping their arena static prevents this multi-kilobyte owner table
// from being copied through the connected async state machine's stack frame.
static RX_PROTOCOL_RUNTIME: ConstStaticCell<ConnectedRxProtocolStorage> =
    ConstStaticCell::new(Esp32s31ConnectedRxProtocolStorage::new());
static CONTROL_RESOURCES: ConstStaticCell<ControlResources> =
    ConstStaticCell::new(ControlResources::new());
// Network RX slots, pinned TX slots and Embassy socket state are the largest
// standalone owners. Const static initialization guarantees they are never
// returned by value through an async task stack during startup.
static NETWORK_RESOURCES: ConstStaticCell<NetworkResources> =
    ConstStaticCell::new(NetworkResources::new());
// These pinned slots become the external backing addresses published through
// ordinary and aggregate Wi-Fi TX descriptors. They must remain in the
// ESP32-S31 Wi-Fi DMA aperture; CPU-only queue state stays in NETWORK_RESOURCES.
#[allow(
    unsafe_code,
    reason = "the linker must retain production network TX backing in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_network_tx")]
static NETWORK_TX_POOL: ConstStaticCell<NetworkTxPool> = ConstStaticCell::new(NetworkTxPool::new());
static TX_AMPDU_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
// BUFFER_SIZE is zero: only the hardware-walked descriptor array in this
// allocation needs the internal DMA aperture. Frame bytes remain owned by the
// separately pinned NETWORK_TX_POOL leases.
#[allow(
    unsafe_code,
    reason = "the linker must retain production A-MPDU descriptors in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_tx_ampdu_descriptors")]
static TX_AMPDU_DMA_STORAGE: ConstStaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    ConstStaticCell::new(AmpduDmaStorage::new());
// Network lease tokens and their descriptor identities live for an entire
// connected epoch. Keep this multi-slot arena static so it is borrowed by the
// movable TX handle instead of being copied through nested async stack frames.
static TX_AMPDU_RETENTION: ConstStaticCell<ConnectedAmpduRetention> =
    ConstStaticCell::new(RetainedAmpduDmaStorage::new());
// The standby arena remains software-owned while the primary descriptor chain
// is in flight. Both arenas reference the same pinned network pool.
static TX_AMPDU_STANDBY_STORAGE: ConstStaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = ConstStaticCell::new(HtAmpduTxStorage::new());
#[allow(
    unsafe_code,
    reason = "the linker must retain standby A-MPDU descriptors in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_tx_ampdu_standby_descriptors")]
static TX_AMPDU_STANDBY_DMA_STORAGE: ConstStaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    ConstStaticCell::new(AmpduDmaStorage::new());
static TX_AMPDU_STANDBY_RETENTION: ConstStaticCell<ConnectedAmpduRetention> =
    ConstStaticCell::new(RetainedAmpduDmaStorage::new());
#[cfg(feature = "qualification")]
static MAC_IRQ_OBSERVER: Mutex<
    CriticalSectionRawMutex,
    RefCell<Option<fn(Esp32s31MacIrqObservation)>>,
> = Mutex::new(RefCell::new(None));
#[cfg(feature = "qualification")]
static QUALIFICATION_LINK_BANDWIDTH_MHZ: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "qualification")]
static QUALIFICATION_DATA_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "qualification")]
static QUALIFICATION_AGGREGATE_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
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

struct ConnectedProtocolStart {
    protocol: ConnectedRxProtocol,
    endpoint:
        ConnectedTaskEndpoint<'static, CriticalSectionRawMutex, ConnectedRxProtocolStoppedOwner>,
}

/// Persistent network device ownership is split before the radio runner
/// starts. The application owns `Esp32s31WifiDevice`; this frontier retains
/// only the radio-side queue endpoint across association epochs.
pub type StationNetwork = StationNetworkResources<(), NetworkRunner, ()>;

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

/// Read-only, value-returning qualification view of the connected register
/// arena. It cannot mutate registers, outlive a synchronous borrow, or keep
/// the PAC owner published during role shutdown.
#[cfg(feature = "qualification")]
#[derive(Clone, Copy)]
pub struct Esp32s31QualificationSnapshot {
    registers: &'static Esp32s31RadioRegistersArena,
}

#[cfg(feature = "qualification")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31QualificationTxVector {
    pub bandwidth_mhz: u16,
    pub data_rate_kbps: u32,
    pub aggregate_rate_kbps: u32,
}

/// Observation-only connected RX hook available only in qualification
/// firmware. The event is borrowed and is always forwarded unchanged to the
/// production network sink after observation.
#[cfg(feature = "qualification")]
pub trait Esp32s31ConnectedRxObserver: Sync {
    fn observe(&self, event: &ConnectedRxEvent<'_>);
}

/// Value-only hard-IRQ observation exported only by qualification builds.
#[cfg(feature = "qualification")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacIrqObservation {
    RxEpoch,
    TxEpoch,
    Entry {
        first_status: u32,
        observed_status: u32,
        nonzero_snapshots: u8,
    },
}

#[cfg(feature = "qualification")]
pub(super) fn configure_mac_irq_observer(observer: fn(Esp32s31MacIrqObservation)) {
    MAC_IRQ_OBSERVER.lock(|slot| {
        *slot.borrow_mut() = Some(observer);
    });
}

#[cfg(feature = "qualification")]
#[inline]
fn observe_mac_irq(observation: Esp32s31MacIrqObservation) {
    MAC_IRQ_OBSERVER.lock(|slot| {
        if let Some(observer) = *slot.borrow() {
            observer(observation);
        }
    });
}

#[cfg(feature = "qualification")]
struct QualificationMacIrqSink;

#[cfg(feature = "qualification")]
impl IrqSink for QualificationMacIrqSink {
    #[inline]
    fn post(&self, pending: u32) {
        if pending & MAC_INT_RX_SUCCESS != 0 && !IRQ_RUNTIME.rx_signaled() {
            observe_mac_irq(Esp32s31MacIrqObservation::RxEpoch);
        }
        const TX_EVENTS: u32 = MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION;
        if pending & TX_EVENTS != 0 && !IRQ_RUNTIME.tx_signaled() {
            observe_mac_irq(Esp32s31MacIrqObservation::TxEpoch);
        }
        IRQ_RUNTIME.publish(pending);
    }

    #[inline]
    fn record_unhandled(&self, bits: u32) {
        IRQ_RUNTIME.record_unhandled(bits);
    }
}

#[cfg(feature = "qualification")]
struct ObservedConnectedRxSink<S> {
    inner: S,
    observer: &'static dyn Esp32s31ConnectedRxObserver,
}

#[cfg(feature = "qualification")]
impl<S: MacConnectedRxSink> MacConnectedRxSink for ObservedConnectedRxSink<S> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        self.observer.observe(&event);
        self.inner.publish(event);
    }
}

#[cfg(feature = "qualification")]
impl<S, const CAPACITY: usize, const SLOTS: usize> ConnectedRxProtocolSink<CAPACITY, SLOTS>
    for ObservedConnectedRxSink<S>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    fn wait_ready(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.inner.wait_ready()
    }

    fn wait_staged_ready(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.inner.wait_staged_ready()
    }

    fn publish_staged(
        &mut self,
        frame: open_esp_radio_esp32s31_wifi_embassy::connected_rx_protocol::Esp32s31StagedRxFrame<
            '_,
            CAPACITY,
            SLOTS,
        >,
        ethernet: open_esp_radio_esp32s31_wifi_embassy::connected_rx_protocol::StagedEthernetPublication,
    ) -> open_esp_radio_esp32s31_wifi_embassy::connected_rx_protocol::StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let payload =
                &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
            self.observer.observe(&ConnectedRxEvent::Ethernet {
                frame: open_esp_radio::wifi::ieee80211::data::EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            });
        }
        self.inner.publish_staged(frame, ethernet)
    }
}

#[cfg(feature = "qualification")]
impl Esp32s31QualificationSnapshot {
    /// Snapshot the associated-STA receive filters and BSSID identity while
    /// the connected epoch owns the register arena.
    pub fn sta_receive_policy(
        self,
    ) -> Option<open_esp_radio::esp32s31::registers::MacStaReceivePolicySnapshot> {
        self.registers
            .try_with_ref(|registers| registers.sta_receive_policy_snapshot())
            .ok()
    }

    /// Snapshot hardware RX counters only while a connected epoch owns the
    /// register arena. `None` means the role is stopped/transitioning or a
    /// driver transaction currently has the bounded borrow.
    pub fn rx_statistics(
        self,
    ) -> Option<open_esp_radio::esp32s31::registers::MacRxStatisticsSnapshot> {
        self.registers
            .try_with_ref(|registers| registers.rx_statistics_snapshot())
            .ok()
    }

    /// Wrapping count of real `RX_SUCCESS` publications from the shared hard
    /// interrupt path. Handoff/capacity wakes are deliberately excluded.
    pub fn rx_interrupt_posts(self) -> u32 {
        IRQ_RUNTIME.rx_post_count()
    }

    /// OR-image of MAC interrupt bits not owned by the qualified dispatcher.
    pub fn unhandled_interrupt_bits(self) -> u32 {
        IRQ_RUNTIME.observed_unhandled()
    }

    /// Current associated-link TX vector. `None` means no connected epoch
    /// owns the datapath.
    pub fn tx_vector(self) -> Option<Esp32s31QualificationTxVector> {
        let bandwidth_mhz = QUALIFICATION_LINK_BANDWIDTH_MHZ.load(Ordering::Acquire);
        (bandwidth_mhz != 0).then(|| Esp32s31QualificationTxVector {
            bandwidth_mhz: bandwidth_mhz as u16,
            data_rate_kbps: QUALIFICATION_DATA_RATE_KBPS.load(Ordering::Relaxed),
            aggregate_rate_kbps: QUALIFICATION_AGGREGATE_RATE_KBPS.load(Ordering::Relaxed),
        })
    }
}

#[cfg(feature = "qualification")]
fn log_sta_receive_policy(
    edge: &str,
    policy: open_esp_radio::esp32s31::registers::MacStaReceivePolicySnapshot,
) {
    let bssid = policy.bssid;
    qualification_event!(
        "open-radio: sta-rx-policy edge={} q0={:08x} q3={:08x} bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} aid={} spacing={} check={} ap={} rx={} beacon={:02x}",
        edge,
        policy.queue_zero_policy,
        policy.queue_three_policy,
        bssid[0],
        bssid[1],
        bssid[2],
        bssid[3],
        bssid[4],
        bssid[5],
        policy.association_id,
        policy.minimum_mpdu_start_spacing,
        policy.bssid_address_check_enabled,
        policy.interface_is_soft_ap,
        policy.interface_rx_policy_enabled,
        policy.beacon_filter_control,
    );
}

pub(super) struct InitialConnectedInitialization {
    pub(super) resources: InitialConnectedStaticResources,
    #[cfg(feature = "qualification")]
    pub(super) qualification: Esp32s31QualificationSnapshot,
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
    (),
    NetworkRunner,
    (),
>;

/// Normal outcome returned after all connected owners are quiescent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStationOutcome {
    Disconnected(open_esp_radio_esp32s31_wifi_embassy::connected_runner::ConnectedDisconnectReason),
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
    _stack: (),
    _initial_network_task: Option<()>,
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
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
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
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
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
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
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
    RX_PROTOCOL_RUNTIME.take()
}

/// Bind the one-time A-MPDU descriptor arena before the radio supervisor can
/// enter an active station epoch.
pub(super) fn initialize_connected_static_resources()
-> Result<InitialConnectedInitialization, HtAmpduTxError> {
    let aggregate = AggregateTxResources::pipelined(
        HtAmpduTxResources::pin_static(TX_AMPDU_STORAGE.take(), TX_AMPDU_DMA_STORAGE.take())?,
        TX_AMPDU_RETENTION.take(),
        HtAmpduTxResources::pin_static(
            TX_AMPDU_STANDBY_STORAGE.take(),
            TX_AMPDU_STANDBY_DMA_STORAGE.take(),
        )?,
        TX_AMPDU_STANDBY_RETENTION.take(),
    );
    let registers: &'static Esp32s31RadioRegistersArena = REGISTER_ARENA.take();
    Ok(InitialConnectedInitialization {
        resources: InitialConnectedStaticResources {
            registers,
            aggregate,
            control: &*CONTROL_RESOURCES.take(),
        },
        #[cfg(feature = "qualification")]
        qualification: Esp32s31QualificationSnapshot { registers },
    })
}

#[esp_hal::handler]
// The handler must execute while flash access may be unavailable. This is a
// declarative linker placement at the combined esp-hal/Embassy integration
// boundary; it performs no raw memory operation.
#[allow(
    unsafe_code,
    reason = "esp-hal requires an unsafe link_section attribute for an IRAM ISR declaration"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn mac_interrupt() {
    #[cfg(feature = "qualification")]
    let report = service_mac_interrupt(&QualificationMacIrqSink);
    #[cfg(not(feature = "qualification"))]
    let _report = service_mac_interrupt(&IRQ_RUNTIME);
    #[cfg(feature = "qualification")]
    observe_mac_irq(Esp32s31MacIrqObservation::Entry {
        first_status: report.first_status,
        observed_status: report.observed_status,
        nonzero_snapshots: report.nonzero_snapshots,
    });
}

#[esp_hal::handler]
#[allow(
    unsafe_code,
    reason = "esp-hal requires an unsafe link_section attribute for an IRAM ISR declaration"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn power_interrupt() {
    let _ = service_power_interrupt(&POWER_IRQ_RUNTIME);
}

/// Owner-free protocol runner returned by radio initialization.
///
/// The application chooses the executor and core explicitly. The runner owns
/// no PAC, DMA descriptor, or interrupt capability; active epochs are lent to
/// it through the private start channel and returned through the typed task
/// endpoint during teardown.
pub struct Esp32s31WifiProtocolRunner {
    starts: Receiver<'static, CriticalSectionRawMutex, ConnectedProtocolStart, 1>,
    poll_observer: Option<fn(u64)>,
}

impl Esp32s31WifiProtocolRunner {
    pub(super) fn new(poll_observer: Option<fn(u64)>) -> Self {
        Self {
            starts: RX_PROTOCOL_START.receiver(),
            poll_observer,
        }
    }

    #[allow(
        large_assignments,
        reason = "the owner-rich protocol future is constructed in its final executor task; the post-LTO stack-frame audit rejects an oversized realized frame"
    )]
    pub async fn run(self) -> ! {
        loop {
            let ConnectedProtocolStart { protocol, endpoint } = self.starts.receive().await;
            observe_protocol_task_polls(
                protocol.run_controlled_task(endpoint, |_| {}),
                self.poll_observer,
            )
            .await;
        }
    }
}

async fn observe_protocol_task_polls<F: core::future::Future>(
    future: F,
    observer: Option<fn(u64)>,
) -> F::Output {
    let Some(observer) = observer else {
        return future.await;
    };
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|context| {
        let started = Instant::now();
        let result = future.as_mut().poll(context);
        observer(started.elapsed().as_micros());
        result
    })
    .await
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
                // Prefer the qualified 150-Mbit/s HT40 vector when the AP's
                // retained HT Capability IE advertises short GI. The common
                // rate policy downgrades this request to long GI for peers
                // that do not advertise the selected-width capability.
                ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
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
            // Request exactly the number of MPDUs the retained TX arena can
            // publish. The peer may negotiate this downward; the aggregate
            // owner then enforces that returned window for every publication.
            tx_block_ack_window: TX_AMPDU_FRAME_COUNT as u16,
            tx_block_ack_negotiation_timeout_us: 500_000,
            tx_block_ack_negotiation_attempt_limit: 3,
            // Each pinned network lease is one MPDU. A-MSDU requires a jumbo
            // backing allocation and is not advertised by this 1,648-byte
            // DMA-slot profile.
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
#[allow(
    large_assignments,
    reason = "the connected teardown result is a unique owner graph returned in place; the post-LTO stack-frame audit remains authoritative for live stack use"
)]
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
    let mut started = prepared.start_network(|_runtime, (), _plan| ((), ()));
    let activation = {
        let (runtime, epoch) = started.runtime_and_epoch_mut();
        let (radio, _storage, _board) = runtime.split_mut();
        let (_phy, platform, interrupt) = radio.parts_mut();
        let setup = interrupt.setup_mut().map_err(|_| {
            open_esp_radio_esp32s31_wifi_embassy::embassy_irq::Esp32s31MacInterruptEpochActivateError::AlreadyActive
        });
        match setup {
            Ok(setup) => {
                let prepared = match epoch {
                    Esp32s31ConnectedEpochResources::Initial { hardware, .. } => {
                        setup.prepare_connected_sta_without_power_save(hardware)
                    }
                    Esp32s31ConnectedEpochResources::Reconnected(reconnected) => {
                        let access = reconnected.hardware_mut().register_access();
                        let mut registers = access.borrow_mut();
                        setup.prepare_connected_sta_without_power_save(&mut registers)
                    }
                };
                activate_esp32s31_connected_epoch(interrupt, platform, prepared)
            }
            Err(error) => Err(error),
        }
    };
    if let Err(error) = activation {
        qualification_event!(
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
                    qualification_event!(
                        "open-radio: connected register publication failed: {error:?}; quarantined"
                    );
                }
                Esp32s31ConnectedEpochStartFailure::Receive { phase, error, .. } => {
                    qualification_event!(
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
        #[cfg(feature = "qualification")]
        qualification,
    } = board;
    let Esp32s31StaAttemptSecurity {
        pmk,
        supplicant_nonce,
        sequences,
        message4_protection,
        connected,
        ..
    } = security;
    let (_phy, platform) = role.radio_mut();
    let Esp32s31ConnectedEpochStarted {
        hardware,
        rx,
        aggregate_tx: aggregate,
        control: control_resources,
    } = started;
    #[cfg(feature = "qualification")]
    log_rx_ring_topology("started", &rx);
    #[cfg(feature = "qualification")]
    let rx = rx.with_pipeline_observer(
        qualification
            .expect("qualification build must retain its configured pipeline observer")
            .rx_pipeline,
    );

    let network_rx = network_runner.rx_publisher();
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new_with_shared_rx(
        network_rx,
        SHARED_NETWORK_RX_QUEUE.publisher(),
        control_publisher,
    );
    #[cfg(feature = "qualification")]
    let rx_sink = {
        let hooks = qualification
            .expect("qualification build must retain its configured pipeline observer");
        rx_sink
            .with_delivery_observer(hooks.rx_delivery)
            .with_pipeline_observer(hooks.rx_pipeline)
    };
    #[cfg(feature = "qualification")]
    let rx_sink = ObservedConnectedRxSink {
        inner: rx_sink,
        observer: qualification
            .expect("qualification build must retain its configured connected RX observer")
            .connected_rx,
    };
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
                pipeline_observer: {
                    #[cfg(feature = "qualification")]
                    {
                        qualification.map(|hooks| hooks.rx_pipeline)
                    }
                    #[cfg(not(feature = "qualification"))]
                    {
                        None
                    }
                },
            },
            tx: Esp32s31ConnectedStaTxResources {
                control: control_tx,
                aggregate,
                pairwise_key: pairwise,
                sequences: tx_sequences,
                aggregate_tx_observer: {
                    #[cfg(feature = "qualification")]
                    {
                        qualification.map(|hooks| hooks.aggregate_tx)
                    }
                    #[cfg(not(feature = "qualification"))]
                    {
                        None
                    }
                },
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
                qualification_event!(
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
        runner: mut radio_runner,
        protocol: rx_protocol,
        report,
    } = assembled;
    if radio_runner
        .services_mut()
        .control_mut()
        .install_wpa2_security(ConnectedWpa2Security::new(
            connected.expect("installed WPA2 keys retain connected supplicant state"),
            group,
        ))
        .is_err()
    {
        unreachable!("a fresh connected control owner has no WPA2 session");
    }

    let (tasks, protocol_endpoint) = task_reservation.into_endpoints();

    let _initial_network_marker = stack_runner;
    RX_PROTOCOL_START
        .sender()
        .send(ConnectedProtocolStart {
            protocol: rx_protocol,
            endpoint: protocol_endpoint,
        })
        .await;
    qualification_event!(
        "open-radio: connected datapath active phy={} tx={}kbps ampdu={}kbps",
        report.link.association_phy.name(),
        report.data_tx_rate.nominal_kbps(),
        report.aggregate_tx_rate.nominal_kbps(),
    );
    #[cfg(feature = "qualification")]
    // This finite register snapshot is emitted at Info in qualification
    // images because rare cold-start beacon loss occurs before a traffic
    // session can request the ordinary qualification snapshot. Production
    // images compile the event out entirely.
    log_sta_receive_policy(
        "start",
        radio_runner
            .services()
            .hardware()
            .sta_receive_policy_snapshot(),
    );
    #[cfg(feature = "qualification")]
    qualification_debug!(
        "open-radio: connected RX statistics: {:?}",
        radio_runner
            .services()
            .hardware()
            .register_access()
            .borrow()
            .rx_statistics_snapshot(),
    );
    #[cfg(feature = "qualification")]
    qualification_debug!(
        "open-radio: connected RX DMA: {:?}",
        radio_runner.services().hardware().mac_rx_dma_snapshot(),
    );
    #[cfg(feature = "qualification")]
    (qualification
        .expect("qualification build must retain its configured lifecycle observer")
        .station_lifecycle)(crate::Esp32s31StationLifecycleObservation::Connected);
    #[cfg(feature = "qualification")]
    {
        QUALIFICATION_DATA_RATE_KBPS.store(report.data_tx_rate.nominal_kbps(), Ordering::Relaxed);
        QUALIFICATION_AGGREGATE_RATE_KBPS
            .store(report.aggregate_tx_rate.nominal_kbps(), Ordering::Relaxed);
        QUALIFICATION_LINK_BANDWIDTH_MHZ.store(
            u32::from(report.link.association_phy.bandwidth_mhz()),
            Ordering::Release,
        );
    }

    let mut observer = NoopEsp32s31ConnectedRunObserver;
    let mut stopped = match await_stack_boundary!(run_and_quiesce_esp32s31_connected_epoch(
        interrupt_epoch,
        platform,
        radio_runner,
        station_control,
        tasks,
        &mut observer,
        |exit, runner| {
            #[cfg(feature = "qualification")]
            {
                let control = runner.services().control();
                let beacon = control.beacon_monitor();
                log_sta_receive_policy(
                    "exit",
                    runner.services().hardware().sta_receive_policy_snapshot(),
                );
                qualification_debug!(
                    "open-radio: connected exit RX statistics: {:?}",
                    runner
                        .services()
                        .hardware()
                        .register_access()
                        .borrow()
                        .rx_statistics_snapshot(),
                );
                qualification_debug!(
                    "open-radio: connected exit RX DMA: {:?}",
                    runner.services().hardware().mac_rx_dma_snapshot(),
                );
                log_rx_ring_topology("exit", runner.services().rx());
                qualification_debug!(
                    "open-radio: connected exit evidence beacon_lost={} beacons={} deadline={:?} last_event={:?} stale_addba_responses={} last_stale_addba_token={:?} dropped_events={} security={:?}",
                    control.beacon_lost(),
                    beacon.map_or(0, |monitor| monitor.observed()),
                    beacon.and_then(|monitor| monitor.deadline_micros()),
                    control.last_event(),
                    control.stale_tx_block_ack_responses(),
                    control.last_stale_tx_block_ack_token(),
                    control.dropped_events(),
                    control.wpa2_security().map(|security| security.evidence()),
                );
            }
            match exit {
                Esp32s31ConnectedStationExit::Disconnected(reason) => {
                    #[cfg(feature = "qualification")]
                    (qualification
                        .expect("qualification build must retain its configured lifecycle observer")
                        .station_lifecycle)(
                        crate::Esp32s31StationLifecycleObservation::Disconnected(reason),
                    );
                    ConnectedStationOutcome::Disconnected(reason)
                }
                Esp32s31ConnectedStationExit::ReconnectRequested { .. } => {
                    ConnectedStationOutcome::ReconnectRequested
                }
                Esp32s31ConnectedStationExit::StationStopped(command) => {
                    ConnectedStationOutcome::StationStopped(command)
                }
                Esp32s31ConnectedStationExit::HardwareFailure(error) => {
                    #[cfg(feature = "qualification")]
                    qualification_event!("open-radio: connected hardware failure: {error:?}");
                    ConnectedStationOutcome::HardwareFailure
                }
            }
        },
    )) {
        Ok(stopped) => stopped,
        Err(mut pending) => loop {
            qualification_event!(
                "open-radio: MAC interrupt quiescence still pending: {:?}",
                pending.error
            );
            Timer::after_millis(1).await;
            match await_stack_boundary!(pending.retry_quiesce(platform)) {
                Ok(stopped) => break stopped,
                Err(returned) => pending = returned,
            }
        },
    };
    #[cfg(feature = "qualification")]
    QUALIFICATION_LINK_BANDWIDTH_MHZ.store(0, Ordering::Release);
    let outcome = stopped.exit;
    qualification_event!("open-radio: connected runner stopped: {outcome:?}");
    let shutdown = stopped.quiesced.tasks.shutdown();
    qualification_event!(
        "open-radio: RX protocol stopped queued={} retained={} commands={} active={}",
        shutdown.queued_frames,
        shutdown.retained_frames,
        shutdown.reorder_commands,
        shutdown.active_reorders,
    );
    let security = stopped
        .quiesced
        .services
        .control_mut()
        .take_wpa2_security()
        .expect("connected WPA2 control returns its association security owner");
    let (_connected, group) = security.into_parts();
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
            qualification_event!("open-radio: {message}; quarantined");
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
                        #[cfg(feature = "qualification")]
                        qualification,
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
        qualification_event!("open-radio: connected TX return found a live owner; quarantined");
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
                    #[cfg(feature = "qualification")]
                    qualification,
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
            crate::runtime::ProductionStationBoardResources {
                interface,
                rx_protocol_runtime,
                initial_connected,
                #[cfg(feature = "qualification")]
                qualification,
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

/// Split the persistent Embassy network device from the radio-side queue
/// endpoint before the station lifecycle starts.
pub fn initialize_station_network(
    station_address: [u8; 6],
) -> (Esp32s31WifiDevice, StationNetwork) {
    let network_resources = NETWORK_RESOURCES.take();
    let tx_pool = NetworkTxPool::pin_static(NETWORK_TX_POOL.take());
    let (device, runner) = network_resources.split(tx_pool, station_address);
    let (_shared_publisher, shared_consumer) = SHARED_NETWORK_RX_QUEUE.split(
        RX_STAGE_POOL.handoff_pool(),
        notify_shared_network_rx_release,
    );
    (
        device
            .with_ingress_tx_reserve()
            .with_shared_rx(shared_consumer),
        StationNetwork::Unstarted { device: (), runner },
    )
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

/// Bind a monitor epoch to the same physical IRQ route and wake domains used
/// by station epochs. The sole radio supervisor guarantees that the two roles
/// cannot own this route concurrently.
pub(super) fn monitor_interrupts()
-> Esp32s31MonitorInterrupts<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex> {
    Esp32s31MonitorInterrupts::new(
        EspHalMacInterruptRoute::new(mac_interrupt, power_interrupt),
        &IRQ_RUNTIME,
        &POWER_IRQ_RUNTIME,
    )
}
