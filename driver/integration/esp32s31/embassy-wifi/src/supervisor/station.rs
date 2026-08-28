#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc connected outcomes retain concrete reusable or faulted owner frontiers"
)]

//! Connected Embassy composition for the standalone station application.
//!
//! This module chooses board allocation and application network policy. The
//! reusable driver owns PAC/DMA/IRQ and 802.11 protocol transitions; no HIL
//! command, benchmark or diagnostics telemetry is part of this graph.

#[cfg(feature = "diagnostics")]
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "mac-irq-diagnostics")]
use embassy_sync::once_lock::OnceLock;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Timer;
use open_esp_radio_embassy_net::SharedPinnedRxQueue;
use open_esp_radio_esp32s31_hal::radio_arena::Esp32s31RadioOwnerArena;
use open_esp_radio_esp32s31_hal::{MacInterruptSetup, RadioRuntimeOwner};
use open_esp_radio_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;
#[cfg(feature = "diagnostics")]
use open_esp_radio_esp32s31_wifi_dma::descriptor::{rx_done, rx_rearm_word};
#[cfg(feature = "diagnostics")]
use open_esp_radio_esp32s31_wifi_embassy::roles::station::rx_protocol::ConnectedRxProtocolSink;
use open_esp_radio_esp32s31_wifi_embassy::{
    composition::resources::{
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
    datapath::DatapathRunner,
    datapath::irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch},
    datapath::rx::dma::{Esp32s31RxEpochResources, Esp32s31StagedRxProducer, Esp32s31StoppedRx},
    datapath::rx::frontier::{
        EmbassyEsp32s31RxFrontierDelay, Esp32s31RxFrontier, Esp32s31RxFrontierError,
    },
    datapath::rx::hardware::EmbassyEsp32s31RxDmaObservationDelay,
    datapath::rx::reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandResources, RxReorderFrameStorage,
    },
    datapath::rx::staging::Esp32s31StagedRxQueue,
    roles::monitor::Esp32s31MonitorInterrupts,
    roles::station::connected::{
        ConnectedControlPublisher, ConnectedControlResources,
        ConnectedControlShutdown as Esp32s31ConnectedControlShutdown, ConnectedWpa2Security,
        EmbassyNetConnectedRxSink, Esp32s31ConnectedDriverAssemblyFailure,
        Esp32s31ConnectedDriverServices, Esp32s31ConnectedDriverTeardownFailure,
        Esp32s31ConnectedEpochResources, Esp32s31ConnectedEpochStartFailure,
        Esp32s31ConnectedEpochStarted, Esp32s31ConnectedNetworkStarted,
        Esp32s31ConnectedNetworkStartedParts, Esp32s31ConnectedRxProtocol,
        Esp32s31ConnectedRxProtocolStopped, Esp32s31ConnectedRxProtocolStorage,
        Esp32s31ConnectedServiceResources, Esp32s31ConnectedStaBlockAckPolicy,
        Esp32s31ConnectedStaCcmpReplayFailure, Esp32s31ConnectedStaCompositionFailure,
        Esp32s31ConnectedStaConfig, Esp32s31ConnectedStaConfigError,
        Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaGroupSecurity,
        Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaPort,
        Esp32s31ConnectedStaRateConfig, Esp32s31ConnectedStaRxPolicy,
        Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaRxService,
        Esp32s31ConnectedStaRxStopped, Esp32s31ConnectedStaSecurityStopReport,
        Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTxHandoffFailure,
        Esp32s31ConnectedStaTxPolicy, Esp32s31ConnectedStaTxResources,
        Esp32s31ConnectedStationExit, Esp32s31ConnectedTx, Esp32s31DisconnectedStaEpoch,
        Esp32s31InitialConnectedEpochResources, Esp32s31ReconnectedStaEpoch, Esp32s31StaTxEpochExt,
        Esp32s31StationCommand, Esp32s31StationCommandReceiver, NoopEsp32s31ConnectedRunObserver,
        activate_esp32s31_connected_epoch, prepare_esp32s31_connected_service,
        run_and_quiesce_esp32s31_connected_epoch, start_esp32s31_initial_connected_epoch,
        start_esp32s31_reconnected_connected_epoch,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_esp32s31_wifi_esp_hal::mac_interrupt_epoch::{
    EspHalMacInterruptRoute, service_mac_interrupt, service_power_interrupt,
};
#[cfg(feature = "mac-irq-diagnostics")]
use open_esp_radio_esp32s31_wifi_mac::irq::{
    IrqSink, MAC_INT_COLLISION, MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{StaGroupCcmpKeyMaterial, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
    rx::{RxIngressConfig, RxRingError},
    rx_pool::RxStagePool,
    tx::{HeEdcaTxopLimit, HtGuardInterval, HtMcs, LegacyRate},
    tx_ampdu::HtAmpduTxError,
};
#[cfg(feature = "diagnostics")]
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxEvent, ConnectedRxSink as MacConnectedRxSink,
};
use open_esp_radio_esp32s31_wifi_sta::{
    attempt::{
        Esp32s31StaAttemptSecurity, Esp32s31StaAttemptSecurityMaterial,
        Esp32s31StaInstalledSecurity,
    },
    connected_rx::{StaCcmpRxReplayResource, StaCcmpRxReplayStartFailure},
    single_mpdu_tx::ConnectedTxSecurity,
};
use open_esp_radio_ieee80211::station::StaTxSequenceCounters;
use open_esp_radio_wifi_embassy::{
    await_stack_boundary,
    station_network::RunningStationNetwork,
};
use static_cell::{ConstStaticCell, StaticCell};

#[cfg(feature = "diagnostics")]
use crate::diagnostics::{Esp32s31ConnectedRxObservation, Esp32s31ConnectedRxObserver};
use crate::radio_resources::{
    NETWORK_TX_HEADROOM, NetworkRunner, RadioAmpduStorage, RadioTxBacking, RunningWifiNetwork,
    TX_AMPDU_BUFFER_SIZE, WifiNetworkResources,
};
use crate::supervisor::{
    ControlTx, ProductionStationBoardResources, ProductionStationRuntime, RX_BUFFER_SIZE,
    RX_DESCRIPTOR_COUNT, RxStorage, StationPowerMode, TxStorage, production_station_runtime,
};
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
#[cfg(feature = "diagnostics")]
type ConnectedRxSink = ObservedConnectedRxSink<EmbassyConnectedRxSink>;
#[cfg(not(feature = "diagnostics"))]
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
type ConnectedLiveRx = Esp32s31StagedRxProducer<
    'static,
    'static,
    'static,
    EmbassyEsp32s31RxDmaObservationDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
>;
type ConnectedRxService = Esp32s31ConnectedStaRxService<ConnectedLiveRx, ConnectedRxProtocol>;
type ConnectedStoppedRxService =
    Esp32s31ConnectedStaRxStopped<ConnectedStoppedRx, ConnectedRxProtocolStoppedOwner>;
pub(super) type ProductionAccessPointRxProducer =
    open_esp_radio_esp32s31_wifi_embassy::roles::access_point::Esp32s31AccessPointRxProducer<
        'static,
        'static,
        'static,
        EmbassyEsp32s31RxDmaObservationDelay,
        CriticalSectionRawMutex,
        RX_STAGE_SLOT_COUNT,
        RX_DESCRIPTOR_COUNT,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
        RX_BUFFER_SIZE,
        { RX_BUFFER_SIZE + 4 },
    >;
pub(super) type ProductionAccessPointRxConsumer =
    open_esp_radio_esp32s31_wifi_embassy::roles::access_point::Esp32s31AccessPointRxConsumer<
        'static,
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_SLOT_COUNT,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    >;

pub(super) fn access_point_rx_pipeline(
    ring: open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>,
    storage: &'static RxStorage,
    #[cfg(feature = "diagnostics")] pipeline_observer: Option<
        &'static dyn open_esp_radio_esp32s31_wifi_embassy::diagnostics::rx_pipeline::RxPipelineObserver,
    >,
) -> (
    ProductionAccessPointRxProducer,
    ProductionAccessPointRxConsumer,
) {
    #[cfg(feature = "diagnostics")]
    if let Some(observer) = pipeline_observer {
        return open_esp_radio_esp32s31_wifi_embassy::roles::access_point::Esp32s31AccessPointRxProducer::from_halted_with_pipeline_observer(
            ring,
            storage,
            &RX_STAGE_POOL,
            &STAGED_RX_QUEUE,
            EmbassyEsp32s31RxDmaObservationDelay,
            observer,
        );
    }
    open_esp_radio_esp32s31_wifi_embassy::roles::access_point::Esp32s31AccessPointRxProducer::from_halted(
        ring,
        storage,
        &RX_STAGE_POOL,
        &STAGED_RX_QUEUE,
        EmbassyEsp32s31RxDmaObservationDelay,
    )
}

#[cfg(feature = "diagnostics")]
fn log_rx_ring_topology(label: &str, rx: &ConnectedLiveRx) {
    let topology = rx.ring().topology_snapshot();
    let reload = rx.ring().reload_repair_evidence();
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
    diagnostics_event!(
        "open-radio: connected RX reload label={} observations={} upper_only={} base_repairs={} last_next={:#010x} last_last={:?} last_head={:?}",
        label,
        reload.observations,
        reload.nonzero_word_with_zero_address,
        reload.base_repairs,
        reload.last_next_word,
        reload.last_last_low,
        reload.last_repair_head,
    );
    diagnostics_event!(
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
    diagnostics_event!(
        "open-radio: connected RX buffers label={} detached={} released={}",
        label,
        rx.storage().detached_buffer_count(),
        rx.storage().released_buffer_count(),
    );
    if !topology.valid {
        for index in 0..RX_DESCRIPTOR_COUNT {
            if let Some(descriptor) = rx.ring().descriptor_snapshot(index) {
                diagnostics_event!(
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
    EmbassyEsp32s31RxDmaObservationDelay,
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
    EmbassyEsp32s31RxDmaObservationDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
>;
type ConnectedLiveTx = Esp32s31ConnectedTx<
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    {
        open_esp_radio_esp32s31_wifi_embassy::composition::resources::ESP32S31_DEFAULT_TX_BUFFER_SIZE
    },
>;
type ConnectedDriverServices = Esp32s31ConnectedDriverServices<
    'static,
    CriticalSectionRawMutex,
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedRxProtocol,
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
    RadioTxBacking,
    open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    {
        open_esp_radio_esp32s31_wifi_embassy::composition::resources::ESP32S31_DEFAULT_TX_BUFFER_SIZE
    },
>;
type ConnectedAssemblyComposition = Esp32s31ConnectedStaCompositionFailure<
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedProtocolAssemblyResources,
    ConnectedControlAssemblyResources,
    ConnectedTxAssemblyFailure,
>;
type ConnectedAssemblyFailure =
    open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedDriverAssemblyFailure<
        NetworkRunner,
        ConnectedAssemblyComposition,
        ConnectedServicesMapper,
    >;
type ConnectedDriverStarted = Esp32s31ConnectedEpochStarted<
    ConnectedHardware,
    ConnectedLiveRx,
    RadioAmpduStorage,
    &'static ControlResources,
>;
type ReturnedConnectedTxResources = open_esp_radio_esp32s31_wifi_sta::control_tx::WifiTxResources<
    'static,
    open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    {
        open_esp_radio_esp32s31_wifi_embassy::composition::resources::ESP32S31_DEFAULT_TX_BUFFER_SIZE
    },
>;
type ConnectedDriverTeardownFailure = Esp32s31ConnectedDriverTeardownFailure<
    'static,
    CriticalSectionRawMutex,
    ConnectedHardware,
    ConnectedRxService,
    ConnectedStoppedRxService,
    ConnectedLiveTx,
    CONTROL_QUEUE_DEPTH,
    RxRingError,
>;
// A failed connected-driver teardown is a terminal quarantine frontier: the
// supervisor retains it forever and can never begin another role epoch. Keep
// that complete no-alloc owner graph at a stable address so wrapping the fault
// for the terminal actor does not copy several kilobytes through each enum
// layer on the task stack.
static CONNECTED_DRIVER_TEARDOWN_FAULT: StaticCell<ConnectedDriverTeardownFailure> =
    StaticCell::new();
pub type ConnectedReconnectedEpoch = Esp32s31ReconnectedStaEpoch<
    ConnectedHardware,
    Esp32s31RxFrontier<
        'static,
        EmbassyEsp32s31RxFrontierDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    ConnectedRxEpochResources,
    RadioAmpduStorage,
    &'static ControlResources,
>;
pub type ConnectedDisconnectedEpoch = Esp32s31DisconnectedStaEpoch<
    RunningStationNetwork<(), NetworkRunner>,
    ConnectedHardware,
    ConnectedStoppedRx,
    RadioAmpduStorage,
    &'static ControlResources,
>;
pub type MacInterruptEpoch =
    Esp32s31MacInterruptEpoch<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;

pub(super) static IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> =
    EmbassyMacIrqRuntime::new_with_rx_moderation(
        open_esp_radio_esp32s31_wifi_esp_hal::mac_interrupt_epoch::unmask_active_mac_rx_delivery_interrupts,
    );
static POWER_IRQ_RUNTIME: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
    EmbassyPowerIrqRuntime::new();
// This SRAM object stores only the 32 affine handoff records. Each admitted
// frame retains its original DMA buffer; the other half of the 64-entry ring
// remains available to the hardware walker and a negotiated BA-16 window.
#[allow(
    unsafe_code,
    reason = "the linker must retain latency-critical RX staging in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_stage")]
pub(super) static RX_STAGE_POOL: RxStagePool<RX_STAGE_SLOT_COUNT, RX_STAGE_CAPACITY> =
    RxStagePool::new();
pub(super) static STAGED_RX_QUEUE: Esp32s31StagedRxQueue<
    'static,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
> = Esp32s31StagedRxQueue::new();
pub(super) static STA_AP_STAGED_RX_QUEUE:
    open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::Esp32s31StaApStagedRxQueue<
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_SLOT_COUNT,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    > = open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::Esp32s31StaApStagedRxQueue::new();
static STATION_SHARED_NETWORK_RX_QUEUE: SharedPinnedRxQueue<
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
> = SharedPinnedRxQueue::new();
static ACCESS_POINT_SHARED_NETWORK_RX_QUEUE: SharedPinnedRxQueue<
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
> = SharedPinnedRxQueue::new();

#[inline(always)]
pub(super) fn publish_access_point_shared_network_rx(index: u8) {
    ACCESS_POINT_SHARED_NETWORK_RX_QUEUE
        .publisher()
        .publish(index);
}

#[inline(never)]
fn notify_shared_network_rx_release() {
    IRQ_RUNTIME.notify_rx_capacity();
}
pub(super) static RX_REORDER_COMMANDS: RxReorderCommandResources<CriticalSectionRawMutex> =
    RxReorderCommandResources::new();
pub(super) static RX_REORDER_STORAGE: RxReorderFrameStorage<
    RX_STAGE_CAPACITY,
    RX_REORDER_BACKING_SLOT_COUNT,
> = RxReorderFrameStorage::new();
// Reorder machines and retained RX lease tokens are long-lived protocol
// state. Keeping their arena static prevents this multi-kilobyte owner table
// from being copied through the connected async state machine's stack frame.
static RX_PROTOCOL_RUNTIME: ConstStaticCell<ConnectedRxProtocolStorage> =
    ConstStaticCell::new(Esp32s31ConnectedRxProtocolStorage::new());
static CONTROL_RESOURCES: ConstStaticCell<ControlResources> =
    ConstStaticCell::new(ControlResources::new());
pub(super) static STA_CCMP_RX_REPLAY: StaCcmpRxReplayResource = StaCcmpRxReplayResource::new();
#[cfg(feature = "mac-irq-diagnostics")]
static MAC_IRQ_OBSERVER: OnceLock<fn(Esp32s31MacIrqObservation)> = OnceLock::new();
#[cfg(feature = "diagnostics")]
static DIAGNOSTIC_LINK_BANDWIDTH_MHZ: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "diagnostics")]
static DIAGNOSTIC_DATA_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "diagnostics")]
static DIAGNOSTIC_AGGREGATE_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
static REGISTER_ARENA: ConstStaticCell<Esp32s31RadioOwnerArena> =
    ConstStaticCell::new(Esp32s31RadioOwnerArena::new());
// Ethernet scratch belongs to the driver datapath. Application socket buffers
// are deliberately not allocated by this product integration.
static ETHERNET_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
// One station protocol turn may close a reorder gap and synchronously release
// the current MPDU plus a complete negotiated BlockAck window. Each released
// MPDU can itself be an A-MSDU, and the deferred publication image adds one
// two-byte record prefix per decoded Ethernet subframe. This PSRAM-owned arena
// therefore covers the full typed release frontier rather than only one DMA
// unit; network backpressure can retain that finite batch without poisoning
// the affine paired owner graph with a synthetic `BatchCapacity` fault.
const STA_AP_STATION_RX_BATCH_CAPACITY: usize = (RX_REORDER_WINDOW + 1) * RX_STAGE_CAPACITY * 2;
static STA_AP_STATION_RX_BATCH: ConstStaticCell<[u8; STA_AP_STATION_RX_BATCH_CAPACITY]> =
    ConstStaticCell::new([0; STA_AP_STATION_RX_BATCH_CAPACITY]);

/// Hardware frontier accepted by one connected epoch.
pub type ConnectedStationEpoch = Esp32s31ConnectedEpochResources<
    RadioRuntimeOwner,
    Esp32s31RxFrontier<
        'static,
        EmbassyEsp32s31RxFrontierDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    ConnectedReconnectedEpoch,
>;
type ConnectedRxFrontier = Esp32s31RxFrontier<
    'static,
    EmbassyEsp32s31RxFrontierDelay,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
>;
type InitialConnectedResources = Esp32s31InitialConnectedEpochResources<
    'static,
    ConnectedRxEpochResources,
    RadioAmpduStorage,
    &'static ControlResources,
>;
type ConnectedEpochStartFault = Esp32s31ConnectedEpochStartFailure<
    InitialConnectedResources,
    ConnectedHardware,
    ConnectedRxFrontier,
    ConnectedRxEpochResources,
    RadioAmpduStorage,
    &'static ControlResources,
    Esp32s31RxFrontierError,
>;

/// Initial-only connected resources materialized before the supervisor can
/// activate any station IRQ or DMA epoch.
pub(crate) struct InitialConnectedStaticResources {
    registers: &'static Esp32s31RadioOwnerArena,
    aggregate: Option<RadioAmpduStorage>,
    control: &'static ControlResources,
}

/// Read-only, value-returning diagnostics view of the connected register
/// arena. It cannot mutate registers, outlive a synchronous borrow, or keep
/// the PAC owner published during role shutdown.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy)]
pub struct Esp32s31DiagnosticSnapshot {
    registers: &'static Esp32s31RadioOwnerArena,
}

#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31DiagnosticTxVector {
    pub bandwidth_mhz: u16,
    pub data_rate_kbps: u32,
    pub aggregate_rate_kbps: u32,
}

/// Application-safe subset of hardware RX counters used by diagnostics.
/// No register block or PAC capability crosses this boundary.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31DiagnosticRxStatistics {
    pub mpdu_count: u16,
    pub data_success: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
    pub brx_agc_error: u16,
    pub brx_error: u16,
    pub nrx_error: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
    pub nrx_he_format: u16,
    pub nrx_ht_sig: u16,
    pub nrx_he_unsupported: u16,
    pub nrx_he_sig_a_crc: u16,
    pub rx_hang: u8,
    pub tx_hang: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

/// Value-only hard-IRQ observation exported only by diagnostics builds.
#[cfg(feature = "mac-irq-diagnostics")]
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

#[cfg(feature = "mac-irq-diagnostics")]
pub(super) fn configure_mac_irq_observer(observer: fn(Esp32s31MacIrqObservation)) {
    MAC_IRQ_OBSERVER
        .init(observer)
        .unwrap_or_else(|_| panic!("MAC IRQ observer was configured more than once"));
}

#[cfg(feature = "mac-irq-diagnostics")]
#[inline]
fn observe_mac_irq(observation: Esp32s31MacIrqObservation) {
    if let Some(observer) = MAC_IRQ_OBSERVER.try_get() {
        observer(observation);
    }
}

#[cfg(feature = "mac-irq-diagnostics")]
struct DiagnosticMacIrqSink;

#[cfg(feature = "mac-irq-diagnostics")]
impl IrqSink for DiagnosticMacIrqSink {
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

    #[inline]
    fn moderate_rx_success(&self) -> bool {
        IrqSink::moderate_rx_success(&IRQ_RUNTIME)
    }
}

#[cfg(feature = "diagnostics")]
pub(crate) struct ObservedConnectedRxSink<S> {
    inner: S,
    observer: &'static dyn Esp32s31ConnectedRxObserver,
}

#[cfg(feature = "diagnostics")]
impl<S: MacConnectedRxSink> MacConnectedRxSink for ObservedConnectedRxSink<S> {
    fn wants_power_save_delivery(&self) -> bool {
        self.inner.wants_power_save_delivery()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let include_phy = match event {
            ConnectedRxEvent::Ethernet { frame, .. } => self.observer.requests_phy(frame.into()),
            _ => false,
        };
        self.observer
            .observe(Esp32s31ConnectedRxObservation::decode(event, include_phy));
        self.inner.publish(event);
    }
}

#[cfg(feature = "diagnostics")]
impl<S, const CAPACITY: usize, const SLOTS: usize> ConnectedRxProtocolSink<CAPACITY, SLOTS>
    for ObservedConnectedRxSink<S>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    fn staged_rx_admission(
        &self,
    ) -> open_esp_radio_esp32s31_wifi_embassy::roles::station::rx_protocol::StagedRxAdmission {
        self.inner.staged_rx_admission()
    }

    fn wait_ready(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.inner.wait_ready()
    }

    fn wait_staged_ready(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.inner.wait_staged_ready()
    }

    fn publish_staged(
        &mut self,
        frame: open_esp_radio_esp32s31_wifi_embassy::datapath::rx::staging::Esp32s31StagedRxFrame<
            '_,
            CAPACITY,
            SLOTS,
        >,
        ethernet: open_esp_radio_esp32s31_wifi_embassy::datapath::rx::staging::StagedEthernetPublication,
    ) -> open_esp_radio_esp32s31_wifi_embassy::datapath::rx::staging::StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let payload =
                &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
            let event = ConnectedRxEvent::Ethernet {
                frame: open_esp_radio_ieee80211::data::EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            };
            let include_phy = self.observer.requests_phy(event_frame(event).into());
            self.observer
                .observe(Esp32s31ConnectedRxObservation::decode(event, include_phy));
        }
        self.inner.publish_staged(frame, ethernet)
    }
}

#[cfg(feature = "diagnostics")]
fn event_frame(
    event: ConnectedRxEvent<'_>,
) -> open_esp_radio_ieee80211::data::EthernetFrameParts<'_> {
    match event {
        ConnectedRxEvent::Ethernet { frame, .. } => frame,
        _ => unreachable!("the staged observation is always an Ethernet event"),
    }
}

#[cfg(feature = "diagnostics")]
impl Esp32s31DiagnosticSnapshot {
    /// Snapshot the associated-STA receive filters and BSSID identity while
    /// the connected epoch owns the register arena.
    pub fn sta_receive_policy(
        self,
    ) -> Option<open_esp_radio_esp32s31_hal::wifi_mac::MacStaReceivePolicySnapshot> {
        self.registers.try_station_receive_policy_snapshot().ok()
    }

    /// Snapshot hardware RX counters only while a connected epoch owns the
    /// register arena. `None` means the role is stopped/transitioning or a
    /// driver transaction currently has the bounded borrow.
    pub fn rx_statistics(self) -> Option<Esp32s31DiagnosticRxStatistics> {
        let statistics = self.registers.try_receive_statistics_snapshot().ok()?;
        let primary = statistics.primary;
        let decode = statistics.decode_errors;
        let hang = statistics.hang;
        Some(Esp32s31DiagnosticRxStatistics {
            mpdu_count: primary.mpdu_count,
            data_success: primary.data_success,
            fcs_error: primary.fcs_error,
            abort: primary.abort,
            abort_fcs_pass: primary.abort_fcs_pass,
            power_drop_error: primary.power_drop_error,
            he_sig_b_error: primary.he_sig_b_error,
            same_bm_error: primary.same_bm_error,
            signal_field: primary.signal_field,
            end: primary.end,
            other_unicast: primary.other_unicast,
            buffer_full: primary.buffer_full,
            fifo_overflow: primary.fifo_overflow,
            tkip_error: primary.tkip_error,
            bt_block_error: primary.bt_block_error,
            frequency_hop_error: primary.frequency_hop_error,
            last_unmatched_error: primary.last_unmatched_error,
            ack_interrupt: primary.ack_interrupt,
            rts_interrupt: primary.rts_interrupt,
            brx_agc_error: decode.brx_agc,
            brx_error: decode.brx,
            nrx_error: decode.nrx,
            nrx_abort: decode.nrx_abort,
            nrx_agc_exit: decode.nrx_agc_exit,
            nrx_baseband_off: decode.nrx_baseband_off,
            nrx_fdm_watchdog: decode.nrx_fdm_watchdog,
            nrx_restart: decode.nrx_restart,
            nrx_service: decode.nrx_service,
            nrx_tx_over: decode.nrx_tx_over,
            nrx_unsupported: decode.nrx_unsupported,
            nrx_he_format: decode.nrx_he_format,
            nrx_ht_sig: decode.nrx_ht_sig,
            nrx_he_unsupported: decode.nrx_he_unsupported,
            nrx_he_sig_a_crc: decode.nrx_he_sig_a_crc,
            rx_hang: hang.rx,
            tx_hang: hang.tx,
            rx_tx_hang: hang.rx_tx_hang,
            rx_tx_panic: hang.rx_tx_panic,
        })
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
    pub fn tx_vector(self) -> Option<Esp32s31DiagnosticTxVector> {
        let bandwidth_mhz = DIAGNOSTIC_LINK_BANDWIDTH_MHZ.load(Ordering::Acquire);
        (bandwidth_mhz != 0).then(|| Esp32s31DiagnosticTxVector {
            bandwidth_mhz: bandwidth_mhz as u16,
            data_rate_kbps: DIAGNOSTIC_DATA_RATE_KBPS.load(Ordering::Relaxed),
            aggregate_rate_kbps: DIAGNOSTIC_AGGREGATE_RATE_KBPS.load(Ordering::Relaxed),
        })
    }
}

#[cfg(feature = "diagnostics")]
fn log_sta_receive_policy(
    edge: &str,
    policy: open_esp_radio_esp32s31_hal::wifi_mac::MacStaReceivePolicySnapshot,
) {
    let bssid = policy.bssid;
    diagnostics_event!(
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
    #[cfg(feature = "diagnostics")]
    pub(super) diagnostics: Esp32s31DiagnosticSnapshot,
}

impl InitialConnectedStaticResources {
    pub(super) fn take_aggregate(&mut self) -> RadioAmpduStorage {
        self.aggregate
            .take()
            .expect("one STA or AP role exclusively owns the aggregate arena")
    }

    pub(super) fn restore_aggregate(&mut self, aggregate: RadioAmpduStorage) {
        assert!(
            self.aggregate.replace(aggregate).is_none(),
            "aggregate arena cannot be restored over a live role owner"
        );
    }

    pub(super) fn with_rx(
        mut self,
        rx: ConnectedRxEpochResources,
    ) -> Esp32s31InitialConnectedEpochResources<
        'static,
        ConnectedRxEpochResources,
        RadioAmpduStorage,
        &'static ControlResources,
    > {
        Esp32s31InitialConnectedEpochResources::new(
            self.registers,
            rx,
            self.take_aggregate(),
            self.control,
        )
    }
}

pub(super) type ConnectedNetworkStarted<'state, 'security> = Esp32s31ConnectedNetworkStarted<
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
    Disconnected(open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason),
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

pub(crate) struct ConnectedDriverAssemblyFault {
    _role: open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _interrupt: MacInterruptEpoch,
    _dma: open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationDmaResources<
        'static,
        RxStorage,
        RX_DESCRIPTOR_COUNT,
    >,
    _tx_storage: &'static mut TxStorage,
    _scan_table: &'static mut open_esp_radio_ieee80211::scan::ScanTable,
    _stack: (),
    _initial_network_task: Option<()>,
    _control_resources: &'static ControlResources,
    _group_security: Esp32s31ConnectedStaGroupSecurity,
    _material: Esp32s31StaAttemptSecurityMaterial,
    _interface: open_esp_radio_wifi_softmac::interface::BoundVirtualInterface,
    _failure: ConnectedAssemblyFailure,
}

/// Exact WPA2 owners retained when the shared replay arena or connected plan
/// rejects publication before driver composition.
pub(crate) enum ConnectedStationReplaySetupFailure {
    Start {
        _failure: StaCcmpRxReplayStartFailure,
        _pairwise: StaPairwiseCcmpSlot,
        _group: StaGroupCcmpSlot,
        _group_material: StaGroupCcmpKeyMaterial,
    },
    Plan {
        _failure: Esp32s31ConnectedStaCcmpReplayFailure,
        _tx_security: ConnectedTxSecurity,
        _group_security: Esp32s31ConnectedStaGroupSecurity,
    },
}

/// Non-reusable connected owner retained at the exact failed transition.
/// No variant exposes the ordinary disconnected owner required for retry.
pub enum ConnectedStationFault<'state, 'security> {
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
        _plan: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedStaPlan,
        _installed_security: Esp32s31StaInstalledSecurity,
        _security: Esp32s31StaAttemptSecurity<'security>,
        _error: open_esp_radio_esp32s31_wifi_sta::tx_epoch::Esp32s31StaTxEpochError,
    },
    SecurityOwnershipMismatch {
        _runtime: ProductionStationRuntime<'state>,
        _started: ConnectedDriverStarted,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedStaPlan,
        _installed_security: Esp32s31StaInstalledSecurity,
        _sequences: StaTxSequenceCounters,
        _material: Esp32s31StaAttemptSecurityMaterial,
        _control_tx: ControlTx,
    },
    ReplaySetup {
        _runtime: ProductionStationRuntime<'state>,
        _started: ConnectedDriverStarted,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedStaPlan,
        _failure: ConnectedStationReplaySetupFailure,
        _sequences: StaTxSequenceCounters,
        _material: Esp32s31StaAttemptSecurityMaterial,
        _control_tx: ControlTx,
    },
    InitialStaticResourcesUnavailable {
        _runtime: ProductionStationRuntime<'state>,
        _hardware: RadioRuntimeOwner,
        _receive: ConnectedRxFrontier,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedStaPlan,
        _installed_security: Esp32s31StaInstalledSecurity,
        _security: Esp32s31StaAttemptSecurity<'security>,
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
        _plan: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedStaPlan,
        _installed_security: Esp32s31StaInstalledSecurity,
        _security: Esp32s31StaAttemptSecurity<'security>,
    },
    DriverTeardown {
        _role:
            open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _interrupt: MacInterruptEpoch,
        _dma: open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationDmaResources<
            'static,
            RxStorage,
            RX_DESCRIPTOR_COUNT,
        >,
        _tx_storage: &'state mut TxStorage,
        _scan_table: &'state mut open_esp_radio_ieee80211::scan::ScanTable,
        _interface: open_esp_radio_wifi_softmac::interface::BoundVirtualInterface,
        _sta_ap_rx_batch: &'static mut [u8],
        _initial_connected: Option<InitialConnectedStaticResources>,
        #[cfg(feature = "diagnostics")]
        _diagnostics: Option<crate::Esp32s31DiagnosticObservers>,
        _network: RunningWifiNetwork,
        _control_resources: &'static ControlResources,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain:
            open_esp_radio_esp32s31_wifi_embassy::datapath::irq::Esp32s31MacInterruptEpochDrain,
        _error: &'static mut ConnectedDriverTeardownFailure,
        _material: Esp32s31StaAttemptSecurityMaterial,
    },
    SecurityTeardownMismatch {
        _runtime: ProductionStationRuntime<'state>,
        _network: RunningWifiNetwork,
        _control_resources: &'static ControlResources,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain:
            open_esp_radio_esp32s31_wifi_embassy::datapath::irq::Esp32s31MacInterruptEpochDrain,
        _hardware: ConnectedHardware,
        _stopped_rx: ConnectedStoppedRx,
        _tx_resources: ReturnedConnectedTxResources,
        _aggregate: RadioAmpduStorage,
        _control_observation: Esp32s31ConnectedControlShutdown,
        _security_stop: Esp32s31ConnectedStaSecurityStopReport,
        _sequences: StaTxSequenceCounters,
        _material: Esp32s31StaAttemptSecurityMaterial,
    },
    TxRestore {
        _runtime: ProductionStationRuntime<'state>,
        _network: RunningWifiNetwork,
        _control_resources: &'static ControlResources,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain:
            open_esp_radio_esp32s31_wifi_embassy::datapath::irq::Esp32s31MacInterruptEpochDrain,
        _hardware: ConnectedHardware,
        _stopped_rx: ConnectedStoppedRx,
        _aggregate: RadioAmpduStorage,
        _control_observation: Esp32s31ConnectedControlShutdown,
        _security_stop: Esp32s31ConnectedStaSecurityStopReport,
        _sequences: StaTxSequenceCounters,
        _error: open_esp_radio_esp32s31_wifi_sta::tx_epoch::Esp32s31StaTxEpochError,
        _returned_control: ControlTx,
        _material: Esp32s31StaAttemptSecurityMaterial,
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
    WifiNetworkResources,
>;

pub(super) fn initialize_ethernet_frame() -> &'static mut [u8] {
    ETHERNET_FRAME.take().as_mut_slice()
}

pub(super) fn initialize_sta_ap_station_rx_batch() -> &'static mut [u8] {
    STA_AP_STATION_RX_BATCH.take().as_mut_slice()
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
    let aggregate = crate::radio_resources::initialize_ampdu()?;
    let registers: &'static Esp32s31RadioOwnerArena = REGISTER_ARENA.take();
    Ok(InitialConnectedInitialization {
        resources: InitialConnectedStaticResources {
            registers,
            aggregate: Some(aggregate),
            control: &*CONTROL_RESOURCES.take(),
        },
        #[cfg(feature = "diagnostics")]
        diagnostics: Esp32s31DiagnosticSnapshot { registers },
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
    #[cfg(feature = "task-poll-telemetry")]
    let core0_cycle_started =
        open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_cycles::cycle_count();
    #[cfg(feature = "mac-irq-diagnostics")]
    let report = service_mac_interrupt(&DiagnosticMacIrqSink);
    #[cfg(not(feature = "mac-irq-diagnostics"))]
    let _report = service_mac_interrupt(&IRQ_RUNTIME);
    #[cfg(feature = "mac-irq-diagnostics")]
    observe_mac_irq(Esp32s31MacIrqObservation::Entry {
        first_status: report.first_status,
        observed_status: report.observed_status,
        nonzero_snapshots: report.nonzero_snapshots,
    });
    #[cfg(feature = "task-poll-telemetry")]
    {
        use open_esp_radio_esp32s31_wifi_embassy::diagnostics::core0_rx_cycles::{
            CORE0_RX_CYCLES, cycle_count,
        };

        CORE0_RX_CYCLES.record_mac_irq(cycle_count().wrapping_sub(core0_cycle_started));
    }
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

pub(super) const fn connected_config(power: StationPowerMode) -> Esp32s31ConnectedStaConfig {
    Esp32s31ConnectedStaConfig {
        power,
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
            he_trigger_based: None,
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
            // Keep one negotiated receive window in the live DMA ring and
            // one complete window as service-latency headroom. The hardware
            // agreement geometry remains the vendor-qualified 64 entries;
            // this value is the protocol/reorder limit returned to the peer.
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
pub(crate) async fn run_connected<'state, 'security>(
    station_control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    resources: ConnectedStationResources<'state, 'security>,
) -> ConnectedStationRunExit<'state, 'security> {
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
            open_esp_radio_esp32s31_wifi_embassy::datapath::irq::Esp32s31MacInterruptEpochActivateError::AlreadyActive
        });
        match setup {
            Ok(setup) => {
                let prepared = match epoch {
                    Esp32s31ConnectedEpochResources::Initial { hardware, .. } => {
                        Ok(setup.prepare_connected_sta_without_power_save(hardware))
                    }
                    Esp32s31ConnectedEpochResources::Reconnected(reconnected) => {
                        let access = reconnected.hardware_mut().register_access();
                        access.try_prepare_connected_sta_without_power_save(setup).map_err(|_| {
                            open_esp_radio_esp32s31_wifi_embassy::datapath::irq::Esp32s31MacInterruptEpochActivateError::AlreadyActive
                        })
                    }
                };
                prepared.and_then(|prepared| {
                    activate_esp32s31_connected_epoch(interrupt, platform, prepared)
                })
            }
            Err(error) => Err(error),
        }
    };
    if let Err(error) = activation {
        diagnostics_event!(
            "open-radio: MAC interrupt activation invariant failed: {error:?}; quarantined"
        );
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
        mut plan,
        installed_security,
        security,
    } = started.into_parts();
    let runtime = runtime.into_parts();
    let (mut role, interrupt_epoch) = runtime.radio.into_parts();
    let (dma, tx_storage, scan_table, frame, ethernet) = runtime.storage.into_parts();
    let mut board = runtime.board;

    let (start, staged_receiver) = match epoch {
        ConnectedStationEpoch::Initial { hardware, receive } => {
            let (staged_sender, staged_receiver) = STAGED_RX_QUEUE.split();
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
                            _installed_security: installed_security,
                            _security: security,
                        },
                    );
                }
            };
            let rx = Esp32s31RxEpochResources::new(
                dma.storage(),
                &RX_STAGE_POOL,
                staged_sender,
                EmbassyEsp32s31RxDmaObservationDelay,
            );
            (
                start_esp32s31_initial_connected_epoch(hardware, receive, initial.with_rx(rx)).await,
                Some(staged_receiver),
            )
        }
        ConnectedStationEpoch::Reconnected(epoch) => (
            start_esp32s31_reconnected_connected_epoch(epoch).await,
            None,
        ),
    };
    let started = match start {
        Ok(started) => started,
        Err(failure) => {
            match &failure {
                Esp32s31ConnectedEpochStartFailure::RegisterPublication { error, .. } => {
                    diagnostics_event!(
                        "open-radio: connected register publication failed: {error:?}; quarantined"
                    );
                }
                Esp32s31ConnectedEpochStartFailure::Receive { phase, error, .. } => {
                    diagnostics_event!(
                        "open-radio: connected RX arm failed phase={phase:?} error={error:?}; quarantined"
                    );
                }
            }
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
                _installed_security: installed_security,
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
                    _installed_security: installed_security,
                    _security: security,
                    _error: error,
                },
            );
        }
    };
    let ProductionStationBoardResources {
        interface,
        rx_protocol_runtime,
        sta_ap_rx_batch,
        initial_connected,
        #[cfg(feature = "diagnostics")]
        diagnostics,
    } = board;
    let (sequences, mut material) = security.into_parts();
    let (_phy, platform) = role.radio_mut();
    let Esp32s31ConnectedEpochStarted {
        hardware,
        rx,
        aggregate_tx: aggregate,
        control: control_resources,
    } = started;
    let staged_receiver = staged_receiver.unwrap_or_else(|| {
        rx.try_resume_standalone_receiver()
            .expect("reconnected station RX retains its standalone producer")
    });
    let material_is_open = matches!(&material, Esp32s31StaAttemptSecurityMaterial::Open);
    let material_has_connected_wpa2 = matches!(
        &material,
        Esp32s31StaAttemptSecurityMaterial::Wpa2Personal {
            connected: Some(_),
            ..
        }
    );
    let (tx_security, group_security) = match installed_security {
        Esp32s31StaInstalledSecurity::Open if material_is_open => (
            ConnectedTxSecurity::Open,
            Esp32s31ConnectedStaGroupSecurity::Open,
        ),
        Esp32s31StaInstalledSecurity::Wpa2Personal {
            pairwise,
            group,
            group_material,
            replay,
        } if material_has_connected_wpa2 => {
            let (replay_rx, replay_control) = match STA_CCMP_RX_REPLAY.start(replay) {
                Ok(endpoints) => endpoints,
                Err(failure) => {
                    return ConnectedStationRunExit::Faulted(ConnectedStationFault::ReplaySetup {
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
                                sta_ap_rx_batch,
                                initial_connected,
                                #[cfg(feature = "diagnostics")]
                                diagnostics,
                            },
                        ),
                        _started: Esp32s31ConnectedEpochStarted {
                            hardware,
                            rx,
                            aggregate_tx: aggregate,
                            control: control_resources,
                        },
                        _stack: stack,
                        _network: network_runner,
                        _initial_network_task: stack_runner,
                        _plan: plan,
                        _failure: ConnectedStationReplaySetupFailure::Start {
                            _failure: failure,
                            _pairwise: pairwise,
                            _group: group,
                            _group_material: group_material,
                        },
                        _sequences: sequences,
                        _material: material,
                        _control_tx: control_tx,
                    });
                }
            };
            let tx_security = ConnectedTxSecurity::Wpa2Personal(pairwise);
            let group_security = Esp32s31ConnectedStaGroupSecurity::Wpa2PersonalRekey {
                group,
                material: group_material,
                replay: replay_control,
            };
            if let Err(failure) = plan.enable_ccmp_rx_replay(replay_rx) {
                return ConnectedStationRunExit::Faulted(ConnectedStationFault::ReplaySetup {
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
                            sta_ap_rx_batch,
                            initial_connected,
                            #[cfg(feature = "diagnostics")]
                            diagnostics,
                        },
                    ),
                    _started: Esp32s31ConnectedEpochStarted {
                        hardware,
                        rx,
                        aggregate_tx: aggregate,
                        control: control_resources,
                    },
                    _stack: stack,
                    _network: network_runner,
                    _initial_network_task: stack_runner,
                    _plan: plan,
                    _failure: ConnectedStationReplaySetupFailure::Plan {
                        _failure: failure,
                        _tx_security: tx_security,
                        _group_security: group_security,
                    },
                    _sequences: sequences,
                    _material: material,
                    _control_tx: control_tx,
                });
            }
            (tx_security, group_security)
        }
        installed_security => {
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::SecurityOwnershipMismatch {
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
                            sta_ap_rx_batch,
                            initial_connected,
                            #[cfg(feature = "diagnostics")]
                            diagnostics,
                        },
                    ),
                    _started: Esp32s31ConnectedEpochStarted {
                        hardware,
                        rx,
                        aggregate_tx: aggregate,
                        control: control_resources,
                    },
                    _stack: stack,
                    _network: network_runner,
                    _initial_network_task: stack_runner,
                    _plan: plan,
                    _installed_security: installed_security,
                    _sequences: sequences,
                    _material: material,
                    _control_tx: control_tx,
                },
            );
        }
    };
    let mut group_security = Some(group_security);
    #[cfg(feature = "diagnostics")]
    log_rx_ring_topology("started", &rx);
    #[cfg(feature = "diagnostics")]
    let rx = if let Some(observer) = diagnostics.and_then(|hooks| hooks.rx_pipeline) {
        rx.with_pipeline_observer(observer)
    } else {
        rx
    };

    let network_rx = network_runner.rx_publisher(
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::STA_NETWORK_INTERFACE_ID,
    );
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new_with_shared_rx(
        network_rx,
        STATION_SHARED_NETWORK_RX_QUEUE.publisher(),
        control_publisher,
    );
    #[cfg(feature = "diagnostics")]
    let rx_sink = {
        let hooks =
            diagnostics.expect("diagnostics build must retain its configured pipeline observer");
        let rx_sink = rx_sink.with_delivery_observer(hooks.rx_delivery);
        if let Some(observer) = hooks.rx_pipeline {
            rx_sink.with_pipeline_observer(observer)
        } else {
            rx_sink
        }
    };
    #[cfg(feature = "diagnostics")]
    let rx_sink = ObservedConnectedRxSink {
        inner: rx_sink,
        observer: diagnostics
            .expect("diagnostics build must retain its configured connected RX observer")
            .connected_rx,
    };
    let (reorder_sender, reorder_receiver) = RX_REORDER_COMMANDS.split();
    let tx_sequences = sequences;
    let drivers = match Esp32s31ConnectedStaPort::compose(
        plan,
        hardware,
        rx,
        Esp32s31ConnectedStaRxProtocolResources {
            frames: staged_receiver,
            irq: &IRQ_RUNTIME,
            sink: rx_sink,
            mpdu: frame,
            ethernet,
            reorder_commands: reorder_receiver,
            reorder_storage: &RX_REORDER_STORAGE,
            runtime: rx_protocol_runtime,
            reorder_scratch: None,
            #[cfg(feature = "diagnostics")]
            pipeline_observer: diagnostics.and_then(|hooks| hooks.rx_pipeline),
            #[cfg(feature = "diagnostics")]
            reorder_observer: diagnostics.and_then(|hooks| hooks.rx_reorder),
        },
        Esp32s31ConnectedStaTxResources {
            control: control_tx,
            aggregate,
            security: tx_security,
            sequences: tx_sequences,
            #[cfg(feature = "diagnostics")]
            aggregate_tx_observer: diagnostics.map(|hooks| hooks.aggregate_tx),
            tx_block_ack_status_sink: Some(crate::status::publish_station_tx_block_ack),
            network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
        },
        Esp32s31ConnectedStaControlResources {
            receiver: control_receiver,
            reorder_commands: reorder_sender,
            rx_block_ack: &crate::supervisor::PRODUCTION_RX_BLOCK_ACK,
        },
    ) {
        Ok(drivers) => drivers,
        Err(composition) => {
            diagnostics_event!("open-radio: connected TX handoff found a live owner; quarantined");
            let failure = Esp32s31ConnectedDriverAssemblyFailure {
                network: network_runner,
                composition,
                map_services: core::convert::identity::<ConnectedDriverServices>
                    as ConnectedServicesMapper,
            };
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
                    _group_security: group_security
                        .take()
                        .expect("pre-control composition retains group security"),
                    _material: material,
                    _interface: interface,
                    _failure: failure,
                },
            });
        }
    };
    let report = drivers.report;
    let mut radio_runner = DatapathRunner::new(
        &IRQ_RUNTIME,
        network_runner,
        open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::STA_NETWORK_INTERFACE_ID,
        drivers.services,
    );
    if let Esp32s31StaAttemptSecurityMaterial::Wpa2Personal { connected, .. } = &mut material {
        let Esp32s31ConnectedStaGroupSecurity::Wpa2PersonalRekey {
            group,
            material: group_material,
            replay,
        } = group_security
            .take()
            .expect("WPA2 composition retains group security")
        else {
            unreachable!("validated WPA2 composition retains group rekey owners");
        };
        if radio_runner
            .services_mut()
            .control_mut()
            .install_wpa2_security(ConnectedWpa2Security::new(
                connected
                    .take()
                    .expect("installed WPA2 keys retain connected supplicant state"),
                group,
                group_material,
                replay,
            ))
            .is_err()
        {
            unreachable!("a fresh connected control owner has no WPA2 session");
        }
    }

    let _initial_network_marker = stack_runner;
    diagnostics_event!(
        "open-radio: connected datapath active phy={} tx={}kbps ampdu={}kbps mcs32={:?}",
        report.link.association_phy.name(),
        report.data_tx_rate.nominal_kbps(),
        report.aggregate_tx_rate.nominal_kbps(),
        report.ht_duplicate_tx_selection,
    );
    #[cfg(feature = "diagnostics")]
    // This finite register snapshot is emitted at Info in diagnostics
    // images because rare cold-start beacon loss occurs before a traffic
    // session can request the ordinary diagnostics snapshot. Production
    // images compile the event out entirely.
    log_sta_receive_policy(
        "start",
        radio_runner
            .services()
            .hardware()
            .sta_receive_policy_snapshot(),
    );
    #[cfg(feature = "diagnostics")]
    diagnostics_debug!(
        "open-radio: connected RX statistics: {:?}",
        radio_runner
            .services()
            .hardware()
            .register_access()
            .try_receive_statistics_snapshot()
            .expect("diagnostics snapshot must not overlap another MMIO transaction"),
    );
    #[cfg(feature = "diagnostics")]
    diagnostics_debug!(
        "open-radio: connected RX DMA: {:?}",
        radio_runner.services().hardware().mac_rx_dma_snapshot(),
    );
    crate::status::publish_station_connected();
    #[cfg(feature = "diagnostics")]
    {
        DIAGNOSTIC_DATA_RATE_KBPS.store(report.data_tx_rate.nominal_kbps(), Ordering::Relaxed);
        DIAGNOSTIC_AGGREGATE_RATE_KBPS
            .store(report.aggregate_tx_rate.nominal_kbps(), Ordering::Relaxed);
        DIAGNOSTIC_LINK_BANDWIDTH_MHZ.store(
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
        &mut observer,
        |exit, _runner| {
            #[cfg(feature = "diagnostics")]
            if let Esp32s31ConnectedStationExit::HardwareFailure(error) = &exit {
                // Preserve the fail-closed reason ahead of the larger exit
                // snapshots. A saturated diagnostic writer must not erase
                // the one fact needed to distinguish TX, RX and control
                // ownership failures during HIL triage.
                diagnostics_event!("open-radio: connected hardware failure: {error:?}");
                diagnostics_event!(
                    "open-radio: connected RX address mismatch: {:?}",
                    _runner
                        .services()
                        .rx()
                        .dma()
                        .first_buffer_address_mismatch()
                );
            }
            #[cfg(feature = "diagnostics")]
            {
                let control = _runner.services().control();
                let beacon = control.beacon_monitor();
                log_sta_receive_policy(
                    "exit",
                    _runner.services().hardware().sta_receive_policy_snapshot(),
                );
                diagnostics_debug!(
                    "open-radio: connected exit RX statistics: {:?}",
                    _runner
                        .services()
                        .hardware()
                        .register_access()
                        .try_receive_statistics_snapshot()
                        .expect("diagnostics snapshot must not overlap another MMIO transaction"),
                );
                diagnostics_event!(
                    "open-radio: connected exit RX DMA: {:?}",
                    _runner.services().hardware().mac_rx_dma_snapshot(),
                );
                log_rx_ring_topology("exit", _runner.services().rx().dma());
                diagnostics_debug!(
                    "open-radio: connected exit evidence beacon_lost={} beacons={} deadline={:?} hardware_beacon_frontier={:?} last_event={:?} stale_addba_responses={} last_stale_addba_token={:?} security={:?}",
                    control.beacon_lost(),
                    beacon.map_or(0, |monitor| monitor.observed()),
                    beacon.and_then(|monitor| monitor.deadline_micros()),
                    control.hardware_beacon_monitor_frontier(),
                    control.last_event(),
                    control.stale_tx_block_ack_responses(),
                    control.last_stale_tx_block_ack_token(),
                    control.wpa2_security().map(|security| security.evidence()),
                );
            }
            match exit {
                Esp32s31ConnectedStationExit::Disconnected(reason) => {
                    crate::status::publish_station_disconnected(reason);
                    ConnectedStationOutcome::Disconnected(reason)
                }
                Esp32s31ConnectedStationExit::ReconnectRequested { .. } => {
                    ConnectedStationOutcome::ReconnectRequested
                }
                Esp32s31ConnectedStationExit::StationStopped(command) => {
                    ConnectedStationOutcome::StationStopped(command)
                }
                Esp32s31ConnectedStationExit::HardwareFailure(error) => {
                    let _ = error;
                    ConnectedStationOutcome::HardwareFailure
                }
            }
        },
    )) {
        Ok(stopped) => stopped,
        Err(mut pending) => loop {
            diagnostics_event!(
                "open-radio: MAC interrupt quiescence still pending: {:?}",
                pending.error
            );
            Timer::after_millis(1).await;
            match pending.retry_quiesce(platform) {
                Ok(stopped) => break stopped,
                Err(returned) => pending = returned,
            }
        },
    };
    #[cfg(feature = "diagnostics")]
    DIAGNOSTIC_LINK_BANDWIDTH_MHZ.store(0, Ordering::Release);
    let outcome = stopped.exit;
    diagnostics_event!("open-radio: DATAPATH runner stopped: {outcome:?}");
    let group_security = match &mut material {
        Esp32s31StaAttemptSecurityMaterial::Open => group_security
            .take()
            .expect("Open connected epoch retains its no-key group marker"),
        Esp32s31StaAttemptSecurityMaterial::Wpa2Personal { connected, .. } => {
            let security = stopped
                .quiesced
                .services
                .control_mut()
                .take_wpa2_security()
                .expect("connected WPA2 control returns its association security owner");
            let (returned_connected, group) = security.into_parts();
            *connected = Some(returned_connected);
            Esp32s31ConnectedStaGroupSecurity::Wpa2Personal(group)
        }
    };
    let teardown = match stopped.try_teardown(group_security) {
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
            diagnostics_event!("open-radio: {message}; quarantined");
            let open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::Esp32s31ConnectedServiceTeardownFailure {
                exit,
                interrupt,
                interrupt_drain,
                network,
                error,
            } = failure;
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::DriverTeardown {
                _role: role,
                _interrupt: interrupt,
                _dma: dma,
                _tx_storage: tx_storage,
                _scan_table: scan_table,
                _interface: interface,
                _sta_ap_rx_batch: sta_ap_rx_batch,
                _initial_connected: initial_connected,
                #[cfg(feature = "diagnostics")]
                _diagnostics: diagnostics,
                _network: RunningStationNetwork::new(stack, network),
                _control_resources: control_resources,
                _outcome: exit,
                _interrupt_drain: interrupt_drain,
                _error: CONNECTED_DRIVER_TEARDOWN_FAULT.init(error),
                _material: material,
            });
        }
    };
    let network_runner = teardown.network;
    let interrupt_epoch = teardown.interrupt;
    let interrupt_drain = teardown.interrupt_drain;
    let teardown = teardown.driver;
    let (stopped_rx, stopped_protocol) = teardown.stopped_rx.into_parts();
    let shutdown = stopped_protocol.shutdown();
    diagnostics_event!(
        "open-radio: RX protocol stopped queued={} retained={} commands={} active={}",
        shutdown.queued_frames,
        shutdown.retained_frames,
        shutdown.reorder_commands,
        shutdown.active_reorders,
    );
    let (frame, ethernet, rx_protocol_runtime) = stopped_protocol.into_parts();
    let sequences = teardown.sequences;
    if matches!(
        teardown.security,
        Esp32s31ConnectedStaSecurityStopReport::ModeMismatchCleared { .. }
    ) {
        diagnostics_event!(
            "open-radio: connected security teardown observed unlike modes; quarantined"
        );
        return ConnectedStationRunExit::Faulted(ConnectedStationFault::SecurityTeardownMismatch {
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
                    sta_ap_rx_batch,
                    initial_connected,
                    #[cfg(feature = "diagnostics")]
                    diagnostics,
                },
            ),
            _network: RunningStationNetwork::new(stack, network_runner),
            _control_resources: control_resources,
            _outcome: outcome,
            _interrupt_drain: interrupt_drain,
            _hardware: teardown.hardware,
            _stopped_rx: stopped_rx,
            _tx_resources: teardown.tx_resources,
            _aggregate: teardown.aggregate,
            _control_observation: teardown.control,
            _security_stop: teardown.security,
            _sequences: sequences,
            _material: material,
        });
    }
    if let Err(failure) = tx_storage.restore_resources(teardown.tx_resources) {
        diagnostics_event!("open-radio: connected TX return found a live owner; quarantined");
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
                    sta_ap_rx_batch,
                    initial_connected,
                    #[cfg(feature = "diagnostics")]
                    diagnostics,
                },
            ),
            _network: RunningStationNetwork::new(stack, network_runner),
            _control_resources: control_resources,
            _outcome: outcome,
            _interrupt_drain: interrupt_drain,
            _hardware: teardown.hardware,
            _stopped_rx: stopped_rx,
            _aggregate: teardown.aggregate,
            _control_observation: teardown.control,
            _security_stop: teardown.security,
            _sequences: sequences,
            _error: error,
            _returned_control: returned_control,
            _material: material,
        });
    }
    let disconnected: ConnectedDisconnectedEpoch = Esp32s31DisconnectedStaEpoch::new(
        RunningStationNetwork::new(stack, network_runner),
        teardown.hardware,
        stopped_rx,
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
            crate::supervisor::ProductionStationBoardResources {
                interface,
                rx_protocol_runtime,
                sta_ap_rx_batch,
                initial_connected,
                #[cfg(feature = "diagnostics")]
                diagnostics,
            },
        ),
        security: match material {
            Esp32s31StaAttemptSecurityMaterial::Open => Esp32s31StaAttemptSecurity::open(sequences),
            Esp32s31StaAttemptSecurityMaterial::Wpa2Personal {
                pmk,
                supplicant_nonce,
                message4_protection,
                ..
            } => Esp32s31StaAttemptSecurity::new(
                pmk,
                supplicant_nonce,
                sequences,
                message4_protection,
            ),
        },
        outcome,
    })
}

/// Split the persistent Embassy network device from the radio-side queue
/// endpoint before the station lifecycle starts.
pub(crate) fn initialize_station_network(
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> (
    crate::radio_resources::Esp32s31WifiDevices,
    WifiNetworkResources,
) {
    let (_station_publisher, station_consumer) = STATION_SHARED_NETWORK_RX_QUEUE.split_external(
        RX_STAGE_POOL.external_handoff_pool(),
        notify_shared_network_rx_release,
    );
    let (_access_point_publisher, access_point_consumer) = ACCESS_POINT_SHARED_NETWORK_RX_QUEUE
        .split_external(
            RX_STAGE_POOL.external_handoff_pool(),
            notify_shared_network_rx_release,
        );
    crate::radio_resources::initialize_network(
        station_address,
        access_point_address,
        station_consumer,
        access_point_consumer,
    )
}

/// Construct the reusable interrupt epoch retained by the station backend.
pub(crate) fn mac_interrupt_epoch(setup: MacInterruptSetup) -> MacInterruptEpoch {
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
pub(crate) fn monitor_interrupts()
-> Esp32s31MonitorInterrupts<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex> {
    Esp32s31MonitorInterrupts::new(
        EspHalMacInterruptRoute::new(mac_interrupt, power_interrupt),
        &IRQ_RUNTIME,
        &POWER_IRQ_RUNTIME,
    )
}
