#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc supervisor states retain concrete reusable and faulted hardware frontiers"
)]
#![expect(
    clippy::manual_async_fn,
    reason = "the epoch runner implementation keeps its trait's borrowed Future contract explicit"
)]
#![expect(
    clippy::result_large_err,
    reason = "supervisor failures return the exact physical and role owners instead of allocating"
)]
#![expect(
    clippy::too_many_arguments,
    reason = "runtime assembly exposes independent affine hardware and board-resource owners"
)]

//! Production ESP32-S31 STA/AP/monitor composition and sole radio owner.
//!
//! The target owns board allocation and application policy. Every radio
//! transition is supplied by a PAC-backed driver or reusable integration
//! owner; no HIL protocol, telemetry or benchmark configuration is linked.

use core::{future::Future, marker::PhantomData, pin::Pin};

use crate::composition::start::{Esp32s31RadioStartConfig, start_esp32s31_radio};
use crate::composition::supervisor::{
    Esp32s31RadioSupervisorTask, Esp32s31StationSupervisorEpoch, Esp32s31StationSupervisorHooks,
    Esp32s31WifiSupervisorStopped, drive_esp32s31_monitor_role, prepare_esp32s31_radio_supervisor,
    run_esp32s31_station_supervisor_epoch,
};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::{
    AccessPointRequest, AccessPointSecurity, StationDiscovery, StationPowerMode, StationRequest,
    StationSecurity, WIFI_SCAN_RESULT_CAPACITY, WifiAccessPointConfig, WifiConfig, WifiScanFailure,
    WifiScanReport, WifiScanRequest, WifiScanResult, WifiServicePlanningError, WifiServiceRequest,
    WifiStartFailure, WifiStartReport, WifiStationConfig, WifiStopReport,
    WifiSupervisorConfiguration,
    runtime::embassy::{
        EmbassyWifiRoleEpochOutcome, EmbassyWifiRoleEpochRunner, EmbassyWifiRoleFrontier,
        EmbassyWifiStartKind, EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorControlResources,
        EmbassyWifiSupervisorEndpoint, EmbassyWifiSupervisorResponse,
        finish_embassy_wifi_active_role,
    },
};
use open_esp_radio_esp32s31_hal::{Radio, RadioRuntimeOwner};
use open_esp_radio_esp32s31_phy::{NoopPhyTargetObserver, PhyTxTargetPowerProfile};
use open_esp_radio_esp32s31_wifi::lower_wifi_channel;
use open_esp_radio_esp32s31_wifi::runtime::{
    Esp32s31WifiRoleOwner, Esp32s31WifiStopped, materialize_esp32s31_wifi_role,
};
use open_esp_radio_esp32s31_wifi::tx::ControlTxConfig;
use open_esp_radio_esp32s31_wifi::{
    cold_start::Esp32s31WifiColdStartConfig as Esp32s31WifiStartConfig,
    mac_start::Esp32s31WifiMacStartConfig,
};
use open_esp_radio_esp32s31_wifi_ap::{
    engine::Esp32s31ApEngine, mac::Esp32s31ApMac, tx::Esp32s31ApTxConfig,
};
pub(super) use open_esp_radio_esp32s31_wifi_embassy::composition::resources::{
    ESP32S31_DEFAULT_RX_BUFFER_SIZE as RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT as RX_DESCRIPTOR_COUNT,
};
use open_esp_radio_esp32s31_wifi_embassy::roles::monitor::{
    Esp32s31MonitorChannelSwitchError, Esp32s31MonitorRadio, Esp32s31MonitorTaskExit,
    prepare_esp32s31_monitor_task,
};
#[cfg(feature = "diagnostics")]
use open_esp_radio_esp32s31_wifi_embassy::roles::station::Esp32s31StationEngineObserver;
use open_esp_radio_esp32s31_wifi_embassy::{
    composition::phy::EmbassyEsp32s31PhyDelay,
    composition::resources::{
        ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_BUFFER_STORAGE_SIZE,
        ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
        ESP32S31_DEFAULT_TX_BUFFER_SIZE as TX_BUFFER_SIZE, Esp32s31DefaultWifiMemory,
    },
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::{EmbassyEsp32s31RxFrontierDelay, Esp32s31RxFrontier},
    roles::access_point::{
        Esp32s31AccessPointControl, Esp32s31AccessPointRxReorder,
        Esp32s31AccessPointStopped as EmbassyAccessPointStopped,
    },
    roles::scan::port::EmbassyEsp32s31ScanTimer,
    roles::scan::rx::{Esp32s31RunningScanRx, Esp32s31ScanFrameObserver, Esp32s31ScanRx},
    roles::station::epoch::Esp32s31RunningScanEpochParts,
    roles::station::tx_epoch::Esp32s31StaTxEpochExt,
    roles::station::{
        ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY, ESP32S31_STATION_PROBE_RATES,
        Esp32s31RadioOwnerRepublish, Esp32s31StationCommandReceiver, Esp32s31StationConfig,
        Esp32s31StationConnectedPhase, Esp32s31StationControlResources, Esp32s31StationController,
        Esp32s31StationDmaResources, Esp32s31StationEngine, Esp32s31StationEnginePort,
        Esp32s31StationExit, Esp32s31StationInitialJoinPhase, Esp32s31StationInitialScanExit,
        Esp32s31StationInitialScanFailures, Esp32s31StationInitialScanPhase,
        Esp32s31StationInitialScanReturned, Esp32s31StationJoinExit, Esp32s31StationJoinOutcome,
        Esp32s31StationJoinResources, Esp32s31StationPrepareFailure, Esp32s31StationRadioResources,
        Esp32s31StationReconnectedPhase, Esp32s31StationRunningScanCompletion,
        Esp32s31StationRunningScanExit, Esp32s31StationRunningScanPhase,
        Esp32s31StationRuntimeReclaimFailure, Esp32s31StationRuntimeResources,
        Esp32s31StationScanDecision, Esp32s31StationScanPlan, Esp32s31StationScanRequest,
        Esp32s31StationScanResources, Esp32s31StationServiceOwner, Esp32s31StationServicePhase,
        Esp32s31StationStartResources, Esp32s31StationStoppedPhaseResources,
        Esp32s31StationStorageResources, Esp32s31StationTask,
        complete_esp32s31_station_initial_scan, complete_esp32s31_station_running_scan,
        esp32s31_station_scan_failure_disposition, prepare_esp32s31_station_task,
        run_esp32s31_station_join, run_esp32s31_station_scan, try_rebind_esp32s31_station_phase,
        try_reclaim_esp32s31_station_runtime, try_restore_esp32s31_station_phase,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_esp32s31_wifi_mac::{
    init::activate_promiscuous_receive,
    rx::{RxDmaBufferAddresses, RxRingError},
    tx::TxSlot,
};
use open_esp_radio_esp32s31_wifi_sta::attempt::{
    Esp32s31StaAttemptObserver, Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStage,
    Esp32s31StaAttemptStation, Esp32s31StaIdentity,
};
use open_esp_radio_esp32s31_wifi_sta::channel::Esp32s31ScanPhy;
use open_esp_radio_esp32s31_wifi_sta::control_tx::{Esp32s31ControlTx, WifiTxResources};
use open_esp_radio_esp32s31_wifi_sta::tx_epoch::Esp32s31StaTxEpoch;
use open_esp_radio_ieee80211::{
    channel::WifiChannel,
    scan::{SCAN_RECORD_CAPACITY, ScanObservation, ScanTable},
    station::StaTxSequenceCounters,
};
use open_esp_radio_wifi_ap::AccessPointService;
use open_esp_radio_wifi_embassy::await_stack_boundary;
use open_esp_radio_wifi_softmac::interface::BoundVirtualInterface;
#[cfg(feature = "diagnostics")]
use open_esp_radio_wifi_sta::station::StaBackoffReason;
use open_esp_radio_wifi_sta::station::{
    StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition,
    StaNextCandidate,
};
use open_esp_radio_wpa2::frames::Wpa2Gtk;
use static_cell::{ConstStaticCell, StaticCell};

#[cfg(feature = "mac-irq-diagnostics")]
use crate::interrupts::configure_mac_irq_observer;
use crate::interrupts::{MacInterruptEpoch, mac_interrupt_epoch};
use crate::monitor::{
    CaptureResources, MonitorMemory, MonitorResourcesError, ProductionMonitorBuildFailure,
    ProductionMonitorResources, ProductionMonitorTask, initialize_monitor_resources,
};
use crate::radio_resources::{
    NetworkRunner, RadioAmpduStorage, RadioNetworkTxBacking, RadioTxBacking, RunningWifiNetwork,
    WifiNetworkResources,
};
use crate::supervisor::station::{
    ConnectedDisconnectedEpoch, ConnectedParkedRx, ConnectedReconnectedEpoch,
    ConnectedRxEpochResources, ConnectedRxProtocolStorage, ConnectedStationEpoch,
    ConnectedStationFault, ConnectedStationOutcome, ConnectedStationResources,
    ConnectedStationRunExit, ControlResources, InitialConnectedStaticResources,
    ProductionAccessPointRxConsumer, ProductionAccessPointRxProducer, access_point_rx_pipeline,
    connected_config, initialize_connected_datapath_mailbox,
    initialize_connected_rx_protocol_runtime, initialize_connected_static_resources,
    initialize_ethernet_frame, initialize_sta_ap_station_rx_batch, run_connected,
};
use crate::{
    Esp32s31NewError, Esp32s31Radio, Esp32s31RadioError, Esp32s31RadioInitialization, Esp32s31Wifi,
};

mod access_point;
#[cfg(feature = "diagnostics")]
mod access_point_observation;
mod concurrent;
mod physical;
mod role_transition;
pub(crate) mod station;

use access_point::{
    ProductionAccessPointPreparationFault, ProductionAccessPointResources,
    ProductionAccessPointTask, ProductionAccessPointTeardownFault,
};
pub(crate) use physical::ProductionRxRing;
use physical::ProductionWifiPhysicalResources;
use role_transition::{
    ProductionStationRoleResources, join_station_activation_resources,
    try_split_wifi_stopped_resources,
};

const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
const TX_COMPLETION_TIMEOUT_US: u64 = 250_000;

pub(super) type RxStorage =
    Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
pub(super) type ControlTx = Esp32s31ControlTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
pub(super) type TxStorage = Esp32s31StaTxEpoch<ControlTx>;
pub(super) type ProductionStationRuntime<'state> = Esp32s31StationRuntimeResources<
    'state,
    'static,
    Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    MacInterruptEpoch,
    Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    &'static mut TxStorage,
    ProductionStationBoardResources,
    SCAN_RECORD_CAPACITY,
>;
// The complete board-independent DMA/scan/control arena is acquired as one
// owner graph. This keeps large buffers out of the async task stack and avoids
// partially taking several unrelated global cells. The aggregate currently
// contains the RX and ordinary-TX allocations addressed directly by the
// Wi-Fi DMA master, so the entire owner must remain in DMA-visible SRAM until
// that storage type is split into independently placed sub-owners.
#[allow(
    unsafe_code,
    reason = "the linker must retain the production Wi-Fi DMA owner in DMA-visible SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_station")]
static WIFI_MEMORY: Esp32s31DefaultWifiMemory<CriticalSectionRawMutex> =
    Esp32s31DefaultWifiMemory::new();
// This immutable table binds every descriptor index to the final DMA buffer
// address. RX service validates it on every ownership transfer, so it is
// safety-critical hot state rather than bulk DMA storage. Keeping the 256-byte
// table in a separately named critical section prevents unrelated owner-graph
// layout changes from silently changing the authority used to rearm a buffer.
#[allow(
    unsafe_code,
    reason = "the linker must retain the RX DMA address binding in internal SRAM"
)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_addresses")]
static RX_BUFFER_ADDRESSES: ConstStaticCell<RxDmaBufferAddresses<RX_DESCRIPTOR_COUNT>> =
    ConstStaticCell::new([0; RX_DESCRIPTOR_COUNT]);
// These two owners contain runtime-derived DMA and PHY values. StaticCell
// retains their final address while `init_with` below avoids a by-value
// intermediate during construction.
static TX_SLOT_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
static TX_STATE: StaticCell<TxStorage> = StaticCell::new();
// Fifteen WPA2 peer state machines exceed the permitted cooperative task
// frame. They are CPU-only state (not DMA descriptors), so a separate normal
// static keeps them out of both task stacks and the DMA-only WIFI_MEMORY arena.
static AP_PEER_STORAGE: ConstStaticCell<open_esp_radio_wifi_ap::AccessPointPeerStorage> =
    ConstStaticCell::new(open_esp_radio_wifi_ap::AccessPointPeerStorage::new());
// Per-peer retry/sequence history is another AP-epoch table. Its address must
// stay stable while RX processing awaits IRQ and network work.
static AP_RX_DISPATCHER: StaticCell<open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxDispatcher> =
    StaticCell::new();
pub(super) static PRODUCTION_RX_BLOCK_ACK:
    open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::Esp32s31StaApRxBlockAck =
    match open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::Esp32s31StaApRxBlockAck::with_maximum_window(
        open_esp_radio_esp32s31_wifi_embassy::composition::resources::ESP32S31_DEFAULT_RX_REORDER_WINDOW
            as u16,
    ) {
        Ok(sessions) => sessions,
        Err(_) => panic!("the production RX BlockAck window is statically validated"),
    };
static AP_RX_REORDER: StaticCell<Esp32s31AccessPointRxReorder<'static, RX_BUFFER_SIZE>> =
    StaticCell::new();
#[cfg(feature = "diagnostics")]
static AP_OBSERVATION_STORAGE: StaticCell<
    open_esp_radio_esp32s31_wifi_embassy::diagnostics::access_point::AccessPointObservationStorage,
> = StaticCell::new();
// Simultaneous STA+AP cannot borrow the station scan and Ethernet scratch.
// These are role-local CPU buffers; DMA never addresses them directly.
static AP_RX_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
static AP_TX_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
// Hardware pairwise-key capabilities are stable epoch state. Keeping their
// bounded table static avoids duplicating all 15 tokens in async rollback
// variants while `stop` still clears every hardware slot before returning it.
static AP_PAIRWISE_KEYS: ConstStaticCell<
    open_esp_radio_esp32s31_wifi_ap::security::Esp32s31ApPairwiseKeyStorage,
> = ConstStaticCell::new(
    open_esp_radio_esp32s31_wifi_ap::security::Esp32s31ApPairwiseKeyStorage::new(),
);

#[derive(Clone, Copy, Debug, Default)]
struct ProductionScanObserver;

impl Esp32s31ScanFrameObserver for ProductionScanObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

#[derive(Clone, Copy, Debug, Default)]
struct ProductionAttemptObserver;

impl Esp32s31StaAttemptObserver for ProductionAttemptObserver {
    fn stage_started(&mut self, stage: Esp32s31StaAttemptStage) {
        diagnostics_debug!("open-radio: attempt stage={stage:?} state=start");
    }

    fn stage_completed(&mut self, stage: Esp32s31StaAttemptStage) {
        diagnostics_debug!("open-radio: attempt stage={stage:?} state=complete");
    }
}

fn tx_entropy() -> u32 {
    Rng::new().random()
}

type ProductionStationPhase = Esp32s31StationServicePhase<
    RadioRuntimeOwner,
    Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
    Esp32s31RxFrontier<
        'static,
        EmbassyEsp32s31RxFrontierDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    WifiNetworkResources,
    ConnectedDisconnectedEpoch,
    ConnectedReconnectedEpoch,
    ProductionConnectedPhase,
>;

struct ProductionConnectedPhase {
    epoch: ConnectedStationEpoch,
    network: WifiNetworkResources,
    station: Esp32s31StaAttemptStation,
    peer: open_esp_radio_esp32s31_wifi_sta::peer::Esp32s31ConnectedStaPeer,
    installed_security: open_esp_radio_esp32s31_wifi_sta::attempt::Esp32s31StaInstalledSecurity,
}

type ProductionStationOwner<'state, 'security> = Esp32s31StationServiceOwner<
    'security,
    ProductionStationRuntime<'state>,
    ProductionStationPhase,
>;

struct ProductionStationFault<'state, 'security> {
    _connected: ConnectedStationFault<'state, 'security>,
    _station: Esp32s31StaAttemptStation,
}

type ProductionStationStorage = Esp32s31StationStorageResources<
    'static,
    Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    &'static mut TxStorage,
    SCAN_RECORD_CAPACITY,
>;
type ProductionStationStoppedPhase = Esp32s31StationStoppedPhaseResources<
    'static,
    Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
    Esp32s31RxFrontier<
        'static,
        EmbassyEsp32s31RxFrontierDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    WifiNetworkResources,
    RunningWifiNetwork,
    ConnectedParkedRx,
    RadioAmpduStorage,
    &'static ControlResources,
    ConnectedRxEpochResources,
>;

/// Exact role-local graph parked while role-neutral Wi-Fi is stopped.
///
/// The register route and Embassy wake domains remain here rather than being
/// reacquired from board singletons. `phase` and `security` describe an exact
/// resume of the stopped service; converting them into a fresh station request
/// is a separate, still-explicit normalization transaction.
struct ProductionWifiReusableResources<'security> {
    storage: ProductionStationStorage,
    board: ProductionStationBoardResources,
    phase: ProductionStationStoppedPhase,
    security: Esp32s31StaAttemptSecurity<'security>,
}

/// Physical radio frontier between logical Wi-Fi roles.
///
/// `Cold` exists only until the first IRQ-driven role activates the MAC
/// route. `Live` then carries that exact route, register owner and runtime
/// context across STA/AP/monitor/scan cutovers. A logical role stop therefore
/// cannot accidentally mask a still-running MAC or remap its interrupt.
enum ProductionWifiOwner {
    Cold(Esp32s31WifiStopped<EspHalRadioPeripheral>),
    Live {
        owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        registers: RadioRuntimeOwner,
        interrupts: MacInterruptEpoch,
    },
}

impl ProductionWifiOwner {
    fn current_channel(&self) -> WifiChannel {
        match self {
            Self::Cold(wifi) => wifi.current_channel(),
            Self::Live { owner, .. } => owner.current_channel(),
        }
    }
}

struct ProductionWifiMaterialized<L> {
    owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    registers: RadioRuntimeOwner,
    interrupts: MacInterruptEpoch,
    resources: L,
}

fn materialize_production_wifi<L>(
    wifi: ProductionWifiOwner,
    resources: L,
) -> ProductionWifiMaterialized<L> {
    match wifi {
        ProductionWifiOwner::Cold(wifi) => {
            let materialized = materialize_esp32s31_wifi_role(wifi, resources);
            ProductionWifiMaterialized {
                owner: materialized.owner,
                registers: materialized.registers,
                interrupts: mac_interrupt_epoch(materialized.interrupt_setup),
                resources: materialized.resources,
            }
        }
        ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        } => ProductionWifiMaterialized {
            owner,
            registers,
            interrupts,
            resources,
        },
    }
}

fn park_production_wifi(
    owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    registers: RadioRuntimeOwner,
    interrupts: MacInterruptEpoch,
) -> ProductionWifiOwner {
    if interrupts.is_active() {
        return ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        };
    }
    match interrupts.try_into_inactive_parts() {
        Ok((_route, setup, _mac_runtime, _power_runtime)) => {
            ProductionWifiOwner::Cold(owner.into_stopped(registers, setup, ()).wifi)
        }
        Err(interrupts) => ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        },
    }
}

struct ProductionStationStopped<'security> {
    wifi: ProductionWifiOwner,
    resources: ProductionWifiReusableResources<'security>,
}

struct ProductionWifiFreshResources {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    rx_ring: Option<ProductionRxRing>,
    tx: ProductionOrdinaryTxResources,
    scan_table: &'static mut ScanTable,
    scan_frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    network: WifiNetworkResources,
    board: ProductionStationBoardResources,
    station_address: [u8; 6],
}

/// The one ordinary descriptor before and after its first role epoch.
///
/// The slot is initialized exactly once. Later STA or AP epochs receive the
/// same reusable TX state; neither role allocates a shadow descriptor.
enum ProductionOrdinaryTxResources {
    Uninitialized(Pin<&'static mut TxSlot<TX_BUFFER_SIZE>>),
    Epoch(&'static mut TxStorage),
}

enum ProductionWifiStoppedResources {
    Fresh(ProductionWifiFreshResources),
    Returned(ProductionWifiReusableResources<'static>),
}

/// Quiescent production frontier with physical hardware and each role's
/// storage represented by independent owners.
#[allow(clippy::result_large_err)]
type ProductionSupervisorStopped = Esp32s31WifiSupervisorStopped<
    ProductionWifiOwner,
    ProductionWifiPhysicalResources,
    ProductionStationRoleResources,
    ProductionAccessPointResources,
    ProductionMonitorResources,
>;

static RADIO_SUPERVISOR_CONTROL: EmbassyWifiSupervisorControlResources<
    CriticalSectionRawMutex,
    Esp32s31RadioError,
> = EmbassyWifiSupervisorControlResources::new();

struct ProductionWifiEpochRunner {
    trng: Trng,
    station_control: &'static Esp32s31StationControlResources<CriticalSectionRawMutex>,
    monitor_capture: &'static CaptureResources,
}

/// Eternal supervisor for the Core0 radio ownership domain. Controlled child
/// execution returns the exact owner through the local datapath rendezvous
/// before quiescence; PAC, DMA and ISR capabilities never become independent
/// owners in another executor or a shared command channel.
pub struct Esp32s31RadioRunner {
    supervisor:
        Esp32s31RadioSupervisorTask<'static, CriticalSectionRawMutex, ProductionWifiEpochRunner>,
    connected_datapath: &'static station::ConnectedDatapathMailbox,
}

/// Eternal hardware/radio runner returned by [`new`].
pub struct Esp32s31RadioRunners {
    pub hardware: Esp32s31RadioRunner,
}

/// Complete initialized radio root and all eternal execution obligations.
pub struct Esp32s31RadioSystem {
    pub radio: Esp32s31Radio,
    pub runners: Esp32s31RadioRunners,
}

impl Esp32s31RadioRunner {
    pub async fn run(self, spawner: Spawner) -> ! {
        self.connected_datapath.bind(spawner);
        await_stack_boundary!(self.supervisor.run())
    }
}

enum ProductionStationReclaimFault<'security> {
    Runtime {
        _failure: Esp32s31StationRuntimeReclaimFailure<ProductionStationOwner<'static, 'security>>,
    },
    InterruptInvariant {
        _registers: RadioRuntimeOwner,
        _role: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _interrupt: MacInterruptEpoch,
        _storage: ProductionStationStorage,
        _board: ProductionStationBoardResources,
        _phase: ProductionStationStoppedPhase,
        _security: Esp32s31StaAttemptSecurity<'security>,
        _selected_channel: Option<WifiChannel>,
    },
}

enum ProductionWifiFault {
    PairedStationPhase {
        _owner: ProductionStationOwner<'static, 'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
    PairedConnected {
        _fault: ConnectedStationFault<'static, 'static>,
        _station: Esp32s31StaAttemptStation,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    PairedChannelMismatch {
        _started: crate::supervisor::station::ConnectedNetworkStarted<'static, 'static>,
        _station: Esp32s31StaAttemptStation,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    PairedSecurityMismatch {
        _started: crate::supervisor::station::ConnectedNetworkStarted<'static, 'static>,
        _station: Esp32s31StaAttemptStation,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
        _access_point_request: AccessPointRequest,
    },
    PairedReclaim {
        _station: ProductionStationReclaimFault<'static>,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    PairedStopped {
        _stopped: ProductionSupervisorStopped,
    },
    StandaloneScanInitialRx {
        _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
        _error: RxRingError,
    },
    StandaloneScanReturn {
        _fault: ProductionStandaloneScanReturnFault,
    },
    AccessPointPreparation {
        _fault: ProductionAccessPointPreparationFault,
    },
    AccessPointRuntime {
        _task: ProductionAccessPointTask,
    },
    AccessPointTeardown {
        _fault: ProductionAccessPointTeardownFault,
    },
    Station {
        _fault: ProductionStationFault<'static, 'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
    Reclaim {
        _station: ProductionStationReclaimFault<'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
    TaskPreparation {
        _failure: Esp32s31StationPrepareFailure<
            ProductionStationOwner<'static, 'static>,
            ProductionStationRunner<'static, 'static>,
        >,
    },
    Resume {
        _fault: ProductionStationResumeFault,
    },
    InitialRx {
        _fault: ProductionInitialRxFault,
    },
    MonitorBuild {
        _failure: ProductionMonitorBuildFailure,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
    },
    MonitorChannel {
        _error: Esp32s31MonitorChannelSwitchError,
        _task: ProductionMonitorTask,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
    },
    MonitorRuntime {
        _task: ProductionMonitorTask,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
    },
    StoppedOwner {
        _wifi: ProductionWifiOwner,
        _resources: ProductionWifiStoppedResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
}

enum ProductionStandaloneScanReturnFault {
    TxRestore {
        _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        _receive:
            Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
        _tx_epoch: &'static mut TxStorage,
        _aggregate_tx: RadioAmpduStorage,
        _station: ProductionStationRoleResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
        _error: open_esp_radio_esp32s31_wifi_sta::tx_epoch::Esp32s31StaTxEpochError,
        _returned_control: ControlTx,
    },
    ReceiveNotLive {
        _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        _receive:
            Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
        _tx_epoch: &'static mut TxStorage,
        _aggregate_tx: RadioAmpduStorage,
        _station: ProductionStationRoleResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
}

struct ProductionInitialRxFault {
    _error: RxRingError,
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupts: MacInterruptEpoch,
    _dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    _tx_storage: &'static mut TxStorage,
    _scan_table: &'static mut ScanTable,
    _scan_frame: &'static mut [u8],
    _ethernet: &'static mut [u8],
    _network: WifiNetworkResources,
    _board: ProductionStationBoardResources,
    _station_address: [u8; 6],
    _security: Esp32s31StaAttemptSecurity<'static>,
}

struct ProductionStationResumeFault {
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupts: MacInterruptEpoch,
    _storage: ProductionStationStorage,
    _board: ProductionStationBoardResources,
    _phase: ProductionStationStoppedPhase,
    _previous_security: Esp32s31StaAttemptSecurity<'static>,
    _requested_security: Esp32s31StaAttemptSecurity<'static>,
}

/// Recover role-neutral Wi-Fi only from a clean finite station exit.
///
/// Any contradiction retains the exact owner set in a fault frontier. It
/// never acknowledges stop by manufacturing PAC, DMA or interrupt capabilities.
fn try_reclaim_production_station<'security>(
    owner: ProductionStationOwner<'static, 'security>,
) -> Result<ProductionStationStopped<'security>, ProductionStationReclaimFault<'security>> {
    let reclaimed = match try_reclaim_esp32s31_station_runtime(owner) {
        Ok(reclaimed) => reclaimed,
        Err(failure) => {
            return Err(ProductionStationReclaimFault::Runtime { _failure: failure });
        }
    };
    let (registers, mut role, interrupt, storage, board, phase, security, primary_channel) =
        reclaimed.into_parts();
    let selected_channel = primary_channel;
    if let Some(channel) = selected_channel {
        role.set_current_channel(channel);
    }
    let wifi = if interrupt.is_active() {
        ProductionWifiOwner::Live {
            owner: role,
            registers,
            interrupts: interrupt,
        }
    } else {
        let (_route, interrupt_setup, _mac_runtime, _power_runtime) =
            match interrupt.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(interrupt) => {
                    return Err(ProductionStationReclaimFault::InterruptInvariant {
                        _registers: registers,
                        _role: role,
                        _interrupt: interrupt,
                        _storage: storage,
                        _board: board,
                        _phase: phase,
                        _security: security,
                        _selected_channel: selected_channel,
                    });
                }
            };
        ProductionWifiOwner::Cold(role.into_stopped(registers, interrupt_setup, ()).wifi)
    };
    Ok(ProductionStationStopped {
        wifi,
        resources: ProductionWifiReusableResources {
            storage,
            board,
            phase,
            security,
        },
    })
}

pub(super) struct ProductionStationBoardResources {
    pub(super) interface: BoundVirtualInterface,
    pub(super) connected_datapath: &'static station::ConnectedDatapathMailbox,
    pub(super) rx_protocol_runtime: &'static mut ConnectedRxProtocolStorage,
    pub(super) sta_ap_rx_batch: &'static mut [u8],
    pub(super) initial_connected: Option<InitialConnectedStaticResources>,
    #[cfg(feature = "diagnostics")]
    pub(super) diagnostics: Option<crate::Esp32s31DiagnosticObservers>,
}

mod station_epoch;
use station_epoch::{
    ProductionStationExit, ProductionStationMode, ProductionStationRunner,
    production_station_runtime, restore_production_station_frontier,
};

mod role_dispatch;

/// Materialize the public controller, persistent network device and sole
/// owner-holding runner. This function does not start a Wi-Fi role and does
/// not construct an IP stack.
#[allow(
    large_assignments,
    reason = "radio start returns one unique typed owner graph; the post-LTO stack-frame audit rejects any actual oversized live frame"
)]
pub async fn new(
    platform: EspHalRadioPeripheral,
    trng: Trng,
    config: crate::Esp32s31RadioConfig,
) -> Result<Esp32s31RadioSystem, Esp32s31NewError> {
    diagnostics_event!("open-radio: cold PHY start");

    let crate::Esp32s31RadioConfig {
        station_mac,
        access_point_mac,
        calibration,
        initial_channel,
        calibration_cache,
        maximum_tx_power_quarter_dbm,
        #[cfg(feature = "connected-datapath-cycle-telemetry")]
        connected_datapath_poll_observer,
        #[cfg(feature = "diagnostics")]
        diagnostics,
    } = config;
    #[cfg(feature = "mac-irq-diagnostics")]
    if let Some(hooks) = diagnostics {
        configure_mac_irq_observer(hooks.mac_irq);
    }
    #[cfg(feature = "diagnostics")]
    open_esp_radio_esp32s31_wifi_embassy::roles::station::rx_protocol::configure_deferred_shared_rx_admission_for_diagnostics(
        diagnostics.is_some_and(|hooks| {
            hooks.rx_admission == crate::Esp32s31DiagnosticRxAdmission::DeferredReady
        }),
    );
    let owned = Radio::claim(platform).map_err(|_| Esp32s31NewError::RadioAlreadyClaimed)?;
    let mut wifi_start = Esp32s31WifiStartConfig::new(calibration, initial_channel);
    if let Some(maximum) = maximum_tx_power_quarter_dbm {
        wifi_start = wifi_start.with_maximum_tx_power_quarter_dbm(maximum);
    }
    let ready = await_stack_boundary!(start_esp32s31_radio::<_, EmbassyEsp32s31PhyDelay, _>(
        owned,
        Esp32s31RadioStartConfig::new(
            wifi_start,
            Esp32s31WifiMacStartConfig::new(
                MAC_HANDSHAKE_SAMPLE_LIMIT,
                station_mac,
                access_point_mac,
            ),
        ),
        calibration_cache,
        NoopPhyTargetObserver,
    ))
    .map_err(|_| Esp32s31NewError::RadioStart)?;
    let station_interface = WifiConfig::station(WifiStationConfig::new(station_mac))
        .validate(open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
        .map_err(|_| Esp32s31NewError::StationRole)?
        .station()
        .ok_or(Esp32s31NewError::StationRole)?;
    let station_address = station_interface.interface.address;
    let (wifi, calibration_cache) = ready.into_parts();
    let initialization = Esp32s31RadioInitialization {
        start: wifi.start_report(),
        transition: wifi.transition_report(),
        calibration_cache,
    };
    diagnostics_event!(
        "open-radio: cold PHY ready, full_calibration={}",
        initialization
            .start
            .wifi
            .registration
            .full_calibration_performed
    );

    let memory = match WIFI_MEMORY.claim() {
        Ok(memory) => memory,
        Err(_error) => return Err(Esp32s31NewError::StationMemoryInUse),
    };
    let rx_storage: &'static RxStorage = memory.rx_dma;
    let buffer_addresses = RX_BUFFER_ADDRESSES.take();
    let monitor_memory = MonitorMemory::new(rx_storage, buffer_addresses)
        .map_err(|_| Esp32s31NewError::RxDmaLayout)?;
    let descriptor_base = monitor_memory.descriptor_base();
    let tx_dma =
        open_esp_radio_esp32s31_wifi_dma::tx_storage::TxDmaStorage::pin_static(memory.tx_dma)
            .map_err(|_| Esp32s31NewError::TxDmaLayout)?;
    let tx_slot = Pin::static_mut(TX_SLOT_STORAGE.init_with(|| TxSlot::from_dma(tx_dma)));
    diagnostics_event!(
        "open-radio: MAC ready handshake_samples={} interrupt_mask={:?}",
        initialization.start.mac.handshake_samples,
        initialization.transition.cold_interrupt_mask
    );

    let scan_table = memory.scan_table;
    scan_table.clear();
    let scan_frame = memory.scan_frame;

    let initial_connected = match initialize_connected_static_resources() {
        Ok(resources) => resources,
        Err(_error) => return Err(Esp32s31NewError::ConnectedResources),
    };
    #[cfg(feature = "diagnostics")]
    let diagnostics_snapshot = initial_connected.diagnostics;
    let initial_connected = initial_connected.resources;
    let (network_devices, station_network) =
        crate::radio_resources::initialize_network(station_address, access_point_mac.bytes());
    let monitor = initialize_monitor_resources(monitor_memory)
        .map_err(|MonitorResourcesError::InUse| Esp32s31NewError::MonitorResources)?;
    let connected_datapath = initialize_connected_datapath_mailbox(
        #[cfg(feature = "connected-datapath-cycle-telemetry")]
        connected_datapath_poll_observer,
    );
    let initial_resources = ProductionWifiStoppedResources::Fresh(ProductionWifiFreshResources {
        dma: Esp32s31StationDmaResources::new(
            monitor_memory.storage(),
            descriptor_base,
            monitor_memory.buffer_addresses(),
        ),
        rx_ring: None,
        tx: ProductionOrdinaryTxResources::Uninitialized(tx_slot),
        scan_table,
        scan_frame,
        ethernet: initialize_ethernet_frame(),
        network: station_network,
        board: ProductionStationBoardResources {
            interface: station_interface,
            connected_datapath,
            rx_protocol_runtime: initialize_connected_rx_protocol_runtime(),
            sta_ap_rx_batch: initialize_sta_ap_station_rx_batch(),
            initial_connected: Some(initial_connected),
            #[cfg(feature = "diagnostics")]
            diagnostics,
        },
        station_address,
    });
    let (physical, station) = match try_split_wifi_stopped_resources(initial_resources) {
        Ok(resources) => resources,
        Err(_) => unreachable!("fresh Wi-Fi resources are always splittable"),
    };
    let stopped = Esp32s31WifiSupervisorStopped::new(
        ProductionWifiOwner::Cold(wifi),
        physical,
        station,
        ProductionAccessPointResources {
            address: access_point_mac.bytes(),
            beacon: memory.ap_beacon,
            rx_frame: AP_RX_FRAME.take().as_mut_slice(),
            tx_frame: AP_TX_FRAME.take().as_mut_slice(),
            peer_storage: AP_PEER_STORAGE.take(),
            pairwise_storage: AP_PAIRWISE_KEYS.take(),
            rx_dispatcher: AP_RX_DISPATCHER.init_with(|| {
                open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxDispatcher::new(
                    open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxConfig {
                        access_point: access_point_mac.bytes(),
                        ingress: open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig {
                            ring_entry_limit: 1,
                            csi_config: 0,
                            flags: 0,
                        },
                        security:
                            open_esp_radio_ieee80211::security::WifiSecurityMode::Wpa2Personal,
                    },
                )
            }),
            rx_block_ack: &PRODUCTION_RX_BLOCK_ACK,
            rx_reorder: AP_RX_REORDER.init_with(Esp32s31AccessPointRxReorder::new),
            rx_reorder_storage: &crate::supervisor::station::RX_REORDER_STORAGE,
            #[cfg(feature = "diagnostics")]
            observation_storage: AP_OBSERVATION_STORAGE.init_with(Default::default),
        },
        monitor.role,
    );
    let configuration = WifiSupervisorConfiguration::new(
        open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    )
    .with_station(WifiStationConfig::new(station_mac))
    .with_access_point(WifiAccessPointConfig::new(access_point_mac))
    .with_standalone_scan()
    .with_standalone_monitor();
    let (controller, supervisor) = match prepare_esp32s31_radio_supervisor(
        &RADIO_SUPERVISOR_CONTROL,
        configuration,
        ProductionWifiEpochRunner {
            trng,
            // The control storage itself is reusable after a clean station
            // epoch. Only its static reference is acquired once; each epoch
            // leases fresh controller/task endpoints through `split()`.
            station_control: memory.station_control,
            monitor_capture: monitor.capture,
        },
        stopped,
    ) {
        Ok(prepared) => prepared,
        Err(_failure) => return Err(Esp32s31NewError::SupervisorInUse),
    };
    Ok(Esp32s31RadioSystem {
        radio: Esp32s31Radio::new(
            Esp32s31Wifi::new(
                controller.into_wifi(),
                network_devices,
                monitor.frames,
                #[cfg(feature = "diagnostics")]
                diagnostics_snapshot,
            ),
            initialization,
        ),
        runners: Esp32s31RadioRunners {
            hardware: Esp32s31RadioRunner {
                supervisor,
                connected_datapath,
            },
        },
    })
}
