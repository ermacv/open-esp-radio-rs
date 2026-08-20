//! Production ESP32-S31 STA/AP/monitor composition and sole radio owner.
//!
//! The target owns board allocation and application policy. Every radio
//! transition is supplied by a PAC-backed driver or reusable integration
//! owner; no HIL protocol, telemetry or benchmark configuration is linked.

use core::{
    cell::Cell,
    future::Future,
    marker::PhantomData,
    pin::Pin,
};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::esp32s31::supervisor::{
    Esp32s31RadioSupervisorTask, Esp32s31StationSupervisorEpoch, Esp32s31StationSupervisorHooks,
    Esp32s31WifiSupervisorStopped, drive_esp32s31_monitor_role, prepare_esp32s31_radio_supervisor,
    run_esp32s31_station_supervisor_epoch,
};
use open_esp_radio::esp32s31::wifi::device::tx::ControlTxConfig;
use open_esp_radio::esp32s31::wifi::device::lower_wifi_channel;
use open_esp_radio::esp32s31::wifi::sta::attempt::{
    Esp32s31StaAttemptObserver, Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStage,
    Esp32s31StaAttemptStation, Esp32s31StaIdentity,
};
use open_esp_radio::esp32s31::wifi::sta::channel::Esp32s31ScanPhy;
use open_esp_radio::esp32s31::wifi::sta::tx_epoch::Esp32s31StaTxEpoch;
#[cfg(feature = "qualification")]
use open_esp_radio::wifi::sta::station::StaBackoffReason;
use open_esp_radio::wifi::wpa2::frames::Wpa2Gtk;
use open_esp_radio::{
    AccessPointRequest, StationDiscovery, StationRequest, StationScanPolicy, StationSecurity,
    WIFI_SCAN_RESULT_CAPACITY, WifiAccessPointConfig, WifiConfig, WifiScanFailure, WifiScanReport,
    WifiScanRequest, WifiScanResult, WifiServicePlanningError, WifiServiceRequest, WifiSsid,
    WifiStartFailure, WifiStartReport, WifiStationConfig, WifiStopReport,
    WifiSupervisorConfiguration,
    embassy_supervisor::{
        EmbassyWifiRoleEpochOutcome, EmbassyWifiRoleEpochRunner, EmbassyWifiRoleFrontier,
        EmbassyWifiStartKind, EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorControlResources,
        EmbassyWifiSupervisorEndpoint, EmbassyWifiSupervisorResponse,
        finish_embassy_wifi_active_role,
    },
    esp32s31::{
        Esp32s31RadioStartConfig, Esp32s31WifiMacStartConfig, Esp32s31WifiStartConfig,
        hal::{Radio, RadioRuntimeOwner},
        phy::{NoopPhyTargetObserver, PhyTxTargetPowerProfile},
        start_esp32s31_radio,
        wifi::ap::{
            engine::Esp32s31ApEngine, mac::Esp32s31ApMac, tx::Esp32s31ApTxConfig,
        },
        wifi::device::runtime::{
            Esp32s31WifiRoleOwner, Esp32s31WifiRoleStopped, materialize_esp32s31_wifi_role,
        },
        wifi::embassy::monitor::{
            Esp32s31MonitorChannelSwitchError, Esp32s31MonitorTaskExit,
            prepare_esp32s31_monitor_task,
        },
        wifi::mac::{init::activate_promiscuous_receive, rx::RxRingError, tx::TxSlot},
        wifi::sta::control_tx::{Esp32s31ControlTx, WifiTxResources},
    },
    wifi::{
        ap::AccessPointService,
        ieee80211::{
            channel::WifiChannel,
            scan::{SCAN_RECORD_CAPACITY, ScanObservation, ScanTable},
            station::StaTxSequenceCounters,
        },
        softmac::interface::BoundVirtualInterface,
        sta::station::{
            StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition,
            StaNextCandidate, StaReconnectPolicy,
        },
        wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::embassy_irq::{
    EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime,
};
pub(super) use open_esp_radio_esp32s31_wifi_embassy::resource_profile::{
    ESP32S31_DEFAULT_RX_BUFFER_SIZE as RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT as RX_DESCRIPTOR_COUNT,
};
#[cfg(feature = "qualification")]
use open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationEngineObserver;
use open_esp_radio_esp32s31_wifi_embassy::{
    access_point::{
        Esp32s31AccessPointControl, Esp32s31AccessPointRxReorder,
        Esp32s31AccessPointStopped as EmbassyAccessPointStopped,
    },
    phy_delay::EmbassyEsp32s31PhyDelay,
    rx_frontier::{EmbassyEsp32s31RxFrontierDelay, Esp32s31RxFrontier},
    resource_profile::{
        ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_BUFFER_STORAGE_SIZE,
        ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
        ESP32S31_DEFAULT_TX_BUFFER_SIZE as TX_BUFFER_SIZE, Esp32s31DefaultWifiMemory,
    },
    rx_dma_service::Esp32s31RxDmaStorage,
    scan_port::EmbassyEsp32s31ScanTimer,
    scan_rx::{Esp32s31RunningScanRx, Esp32s31ScanFrameObserver, Esp32s31ScanRx},
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
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
        Esp32s31StationScanDecision, Esp32s31StationScanPlan, Esp32s31StationScanResources,
        Esp32s31StationServiceOwner, Esp32s31StationServicePhase, Esp32s31StationStartResources,
        Esp32s31StationStoppedPhaseResources,
        Esp32s31StationStorageResources, Esp32s31StationTask,
        complete_esp32s31_station_initial_scan, complete_esp32s31_station_running_scan,
        esp32s31_station_scan_failure_disposition,
        prepare_esp32s31_station_task, run_esp32s31_station_join, run_esp32s31_station_scan,
        try_rebind_esp32s31_station_phase, try_reclaim_esp32s31_station_runtime,
        try_restore_esp32s31_station_phase,
    },
    station_epoch::Esp32s31RunningScanEpochParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral, mac_interrupt_epoch::EspHalMacInterruptRoute,
};
use open_esp_radio_wifi_embassy::await_stack_boundary;
use static_cell::{ConstStaticCell, StaticCell};

#[cfg(feature = "qualification")]
use crate::connected::configure_mac_irq_observer;
use crate::connected::{
    ConnectedDisconnectedEpoch, ConnectedReconnectedEpoch, ConnectedRxEpochResources,
    ConnectedRxProtocolStorage,
    ConnectedStationEpoch, ConnectedStationFault, ConnectedStationOutcome,
    ConnectedStationResources, ConnectedStationRunExit, ConnectedStoppedRx, ControlResources,
    Esp32s31WifiProtocolRunner, InitialConnectedStaticResources, MacInterruptEpoch,
    ProductionAccessPointRxConsumer, ProductionAccessPointRxProducer, access_point_rx_pipeline,
    connected_config, initialize_connected_rx_protocol_runtime,
    initialize_connected_static_resources, initialize_ethernet_frame, initialize_station_network,
    mac_interrupt_epoch, publish_access_point_shared_network_rx, run_connected,
};
use crate::radio_resources::{
    NetworkRunner, RadioAmpduStorage, RadioTxBacking, RunningWifiNetwork, WifiNetworkResources,
};
use crate::monitor::{
    CaptureResources, MonitorMemory, MonitorResourcesError, ProductionMonitorBuildFailure,
    ProductionMonitorResources, ProductionMonitorTask, initialize_monitor_resources,
};
use crate::{
    Esp32s31NewError, Esp32s31Radio, Esp32s31RadioError, Esp32s31RadioInitialization, Esp32s31Wifi,
};

mod access_point;

use access_point::{
    ProductionAccessPointPreparationFault, ProductionAccessPointResources,
    ProductionAccessPointTask, ProductionAccessPointTeardownFault,
    ProductionStationRoleResources, ProductionWifiPhysicalResources,
    join_station_activation_resources, try_split_wifi_stopped_resources,
};

const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
const TX_COMPLETION_TIMEOUT_US: u64 = 250_000;

pub(super) type RxStorage =
    Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
pub(super) type ControlTx = Esp32s31ControlTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
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
// These two owners contain runtime-derived DMA and PHY values. StaticCell
// retains their final address while `init_with` below avoids a by-value
// intermediate during construction.
static TX_SLOT_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
static TX_STATE: StaticCell<TxStorage> = StaticCell::new();
// Fifteen WPA2 peer state machines exceed the permitted cooperative task
// frame. They are CPU-only state (not DMA descriptors), so a separate normal
// static keeps them out of both task stacks and the DMA-only WIFI_MEMORY arena.
static AP_PEER_STORAGE: ConstStaticCell<open_esp_radio::wifi::ap::AccessPointPeerStorage> =
    ConstStaticCell::new(open_esp_radio::wifi::ap::AccessPointPeerStorage::new());
// Per-peer retry/sequence history is another AP-epoch table. Its address must
// stay stable while RX processing awaits IRQ and network work.
static AP_RX_DISPATCHER: StaticCell<
    open_esp_radio::esp32s31::wifi::ap::rx::Esp32s31ApRxDispatcher,
> = StaticCell::new();
static AP_RX_BLOCK_ACK: StaticCell<
    open_esp_radio_esp32s31_wifi_embassy::sta_ap::Esp32s31StaApRxBlockAck,
> = StaticCell::new();
static AP_RX_REORDER: StaticCell<Esp32s31AccessPointRxReorder<'static, RX_BUFFER_SIZE>> =
    StaticCell::new();
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
    open_esp_radio::esp32s31::wifi::ap::security::Esp32s31ApPairwiseKeyStorage,
> = ConstStaticCell::new(
    open_esp_radio::esp32s31::wifi::ap::security::Esp32s31ApPairwiseKeyStorage::new(),
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
        qualification_debug!("open-radio: attempt stage={stage:?} state=start");
    }

    fn stage_completed(&mut self, stage: Esp32s31StaAttemptStage) {
        qualification_debug!("open-radio: attempt stage={stage:?} state=complete");
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
    peer: open_esp_radio::esp32s31::wifi::sta::peer::Esp32s31ConnectedStaPeer,
    pairwise: open_esp_radio::esp32s31::wifi::mac::crypto::StaPairwiseCcmpSlot,
    group: open_esp_radio::esp32s31::wifi::mac::crypto::StaGroupCcmpSlot,
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
    ConnectedStoppedRx,
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
    interrupt_route: EspHalMacInterruptRoute,
    mac_runtime: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
    power_runtime: &'static EmbassyPowerIrqRuntime<CriticalSectionRawMutex>,
}

type ProductionStationStopped<'security> =
    Esp32s31WifiRoleStopped<EspHalRadioPeripheral, ProductionWifiReusableResources<'security>>;

struct ProductionWifiFreshResources {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    rx_ring:
        Option<open_esp_radio::esp32s31::wifi::mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>>,
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
    EspHalRadioPeripheral,
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

/// Eternal owner-holding task. PAC, DMA and ISR capabilities never leave this
/// value or the futures it drives.
pub struct Esp32s31RadioRunner {
    supervisor:
        Esp32s31RadioSupervisorTask<'static, CriticalSectionRawMutex, ProductionWifiEpochRunner>,
}

/// Explicitly placed eternal runners returned by [`new`].
///
/// The hardware runner owns PAC/DMA/ISR state. The protocol runner owns only
/// the connected 802.11 processing task endpoint. Keeping them separate lets
/// an application select a core topology without changing driver behavior.
pub struct Esp32s31RadioRunners {
    pub hardware: Esp32s31RadioRunner,
    pub wifi_protocol: Esp32s31WifiProtocolRunner,
}

/// Complete initialized radio root and all eternal execution obligations.
pub struct Esp32s31RadioSystem {
    pub radio: Esp32s31Radio,
    pub runners: Esp32s31RadioRunners,
}

impl Esp32s31RadioRunner {
    pub async fn run(self) -> ! {
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
        _wifi: open_esp_radio::esp32s31::wifi::device::runtime::Esp32s31WifiStopped<
            EspHalRadioPeripheral,
        >,
        _resources: ProductionWifiStoppedResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
}

struct ProductionInitialRxFault {
    _error: RxRingError,
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupt_setup: open_esp_radio::esp32s31::hal::MacInterruptSetup,
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
    _interrupt_setup: open_esp_radio::esp32s31::hal::MacInterruptSetup,
    _storage: ProductionStationStorage,
    _board: ProductionStationBoardResources,
    _phase: ProductionStationStoppedPhase,
    _previous_security: Esp32s31StaAttemptSecurity<'static>,
    _requested_security: Esp32s31StaAttemptSecurity<'static>,
    _interrupt_route: EspHalMacInterruptRoute,
    _mac_runtime: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
    _power_runtime: &'static EmbassyPowerIrqRuntime<CriticalSectionRawMutex>,
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
    let (interrupt_route, interrupt_setup, mac_runtime, power_runtime) =
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
    if let Some(channel) = selected_channel {
        role.set_current_channel(channel);
    }
    Ok(role.into_stopped(
        registers,
        interrupt_setup,
        ProductionWifiReusableResources {
            storage,
            board,
            phase,
            security,
            interrupt_route,
            mac_runtime,
            power_runtime,
        },
    ))
}

pub(super) struct ProductionStationBoardResources {
    pub(super) interface: BoundVirtualInterface,
    pub(super) rx_protocol_runtime: &'static mut ConnectedRxProtocolStorage,
    pub(super) initial_connected: Option<InitialConnectedStaticResources>,
    #[cfg(feature = "qualification")]
    pub(super) qualification: Option<crate::Esp32s31QualificationHooks>,
}

struct ProductionStationEnginePort<O> {
    scan_only: bool,
    scan_completed: Cell<bool>,
    access_point: ProductionAccessPointResources,
    monitor: ProductionMonitorResources,
    _owner: PhantomData<fn() -> O>,
}

impl<O> ProductionStationEnginePort<O> {
    fn new(
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    ) -> Self {
        Self {
            scan_only: false,
            scan_completed: Cell::new(false),
            access_point,
            monitor,
            _owner: PhantomData,
        }
    }

    fn standalone_scan(
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
    ) -> Self {
        Self {
            scan_only: true,
            scan_completed: Cell::new(false),
            access_point,
            monitor,
            _owner: PhantomData,
        }
    }

    fn scan_completed(&self) -> bool {
        self.scan_completed.get()
    }

    fn into_parked_roles(
        self,
    ) -> (ProductionAccessPointResources, ProductionMonitorResources) {
        (self.access_point, self.monitor)
    }
}

pub(super) fn production_station_runtime<'state>(
    role: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    interrupt_epoch: MacInterruptEpoch,
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_storage: &'static mut TxStorage,
    scan_table: &'static mut ScanTable,
    frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    board: ProductionStationBoardResources,
) -> ProductionStationRuntime<'state> {
    Esp32s31StationRuntimeResources::new(
        Esp32s31StationRadioResources::new(role, interrupt_epoch),
        Esp32s31StationStorageResources::new(dma, tx_storage, scan_table, frame, ethernet),
        board,
    )
}

impl<'state, 'security> ProductionStationEnginePort<ProductionStationOwner<'state, 'security>> {
    #[inline(never)]
    async fn run_initial_scan_epoch(
        &self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRuntimeOwner,
            Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
            WifiNetworkResources,
        >,
        discovery: StationDiscovery,
    ) -> Esp32s31StationInitialScanExit<
        'security,
        ProductionStationRuntime<'state>,
        RadioRuntimeOwner,
        Esp32s31RxFrontier<
            'static,
            EmbassyEsp32s31RxFrontierDelay,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
        >,
        WifiNetworkResources,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (mut runtime, hardware, receive, network, identity, mut security) = phase.into_parts();
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, interrupt_epoch) = radio_resources.parts_mut();
        let (_, tx_storage, scan_table, frame, _) = storage_resources.parts_mut();
        let control = tx_storage
            .take_control()
            .expect("initial scan owns the ordinary TX owner");
        let interrupt_setup = interrupt_epoch
            .setup()
            .expect("initial scan requires a quiesced interrupt epoch");
        let scan_plan = Esp32s31StationScanPlan::new(discovery, None);
        let scan_request = scan_plan.request(identity.station_address);
        let scan_request = if self.scan_only {
            scan_request.without_candidate_selection()
        } else {
            scan_request
        };
        let scan = run_esp32s31_station_scan(
            Esp32s31StationScanResources {
                phy,
                platform,
                phy_observer: NoopPhyTargetObserver,
                phy_delay: EmbassyEsp32s31PhyDelay,
                hardware,
                receive,
                control,
                interrupt_setup,
                table: scan_table,
                frame,
                scan_observer: ProductionScanObserver,
                sequence: security.sequences.non_qos_mut(),
                timer: EmbassyEsp32s31ScanTimer,
            },
            scan_request,
        )
        .await;
        let decision = scan.decision;
        if self.scan_only && matches!(decision, Esp32s31StationScanDecision::NoCandidate { .. }) {
            self.scan_completed.set(true);
        }
        let open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationScanReturned {
            hardware,
            receive,
            control,
            table: _,
            frame: _,
            sequence: _,
            phy_observer: _,
            phy_delay: _,
            scan_observer: _,
            timer: _,
            telemetry: _,
            transmit: _,
        } = scan.returned;
        tx_storage
            .restore_control(control)
            .unwrap_or_else(|_| panic!("initial scan returned over a live TX owner"));
        complete_esp32s31_station_initial_scan(
            Esp32s31StationInitialScanReturned {
                runtime,
                hardware,
                receive,
                network,
                identity,
                security,
            },
            decision,
            |receive| {
                receive
                    .into_halted()
                    .map(Esp32s31RxFrontier::from_halted)
            },
            |runtime, hardware, receive, network, identity, security| {
                ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::InitialScan {
                        hardware,
                        receive,
                        network,
                        identity,
                    },
                    security.into_role(),
                )
            },
            Esp32s31StationInitialScanFailures {
                no_candidate: Esp32s31StaAttemptStage::Candidate,
                receive_handoff: Esp32s31StaAttemptStage::Candidate,
                transaction: Esp32s31StaAttemptStage::Candidate,
                invalid_plan: Esp32s31StaAttemptStage::Candidate,
            },
        )
    }

    #[inline(never)]
    async fn run_connected_epoch(
        &mut self,
        phase: Esp32s31StationConnectedPhase<
            'security,
            ProductionStationRuntime<'state>,
            ProductionConnectedPhase,
        >,
        control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> StaAttemptOutcome<
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (runtime, connected, security) = phase.into_parts();
        let ProductionConnectedPhase {
            epoch,
            network,
            station,
            peer,
            pairwise,
            group,
        } = connected;
        let interface = runtime.board().interface;
        let returned = run_connected(
            control,
            ConnectedStationResources::new(
                runtime,
                epoch,
                network,
                interface,
                connected_config(),
                peer,
                pairwise,
                group,
                security,
            ),
        )
        .await;
        let returned = match returned {
            ConnectedStationRunExit::Returned(returned) => returned,
            ConnectedStationRunExit::Faulted(fault) => {
                return StaAttemptOutcome::Faulted {
                    fault: ProductionStationFault {
                        _connected: fault,
                        _station: station,
                    },
                };
            }
        };
        let owner = ProductionStationOwner::new(
            returned.runtime,
            ProductionStationPhase::RunningScan {
                disconnected: returned.disconnected,
                station,
            },
            returned.security,
        );
        match returned.outcome {
            ConnectedStationOutcome::Disconnected(_)
            | ConnectedStationOutcome::ReconnectRequested => StaAttemptOutcome::Disconnected {
                owner,
                next_candidate: StaNextCandidate::Refresh,
            },
            ConnectedStationOutcome::StationStopped(_) => StaAttemptOutcome::Stopped { owner },
            ConnectedStationOutcome::HardwareFailure => StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    open_esp_radio::wifi::sta::station::StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    Esp32s31StaAttemptStage::ConnectedEntry,
                ),
            },
        }
    }

    #[inline(never)]
    async fn run_running_scan_epoch(
        &mut self,
        phase: Esp32s31StationRunningScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            ConnectedDisconnectedEpoch,
        >,
        discovery: StationDiscovery,
    ) -> Esp32s31StationRunningScanExit<
        'security,
        ProductionStationRuntime<'state>,
        ConnectedReconnectedEpoch,
        WifiNetworkResources,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
        let (mut runtime, disconnected, station, mut security) = phase.into_parts();
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, interrupt_epoch) = radio_resources.parts_mut();
        let (_, tx_storage, scan_table, frame, _) = storage_resources.parts_mut();
        let Esp32s31RunningScanEpochParts {
            retained,
            hardware,
            rx,
        } = disconnected.into_running_scan_parts();
        let control = tx_storage
            .take_control()
            .expect("connected teardown returned the ordinary TX owner");
        let interrupt_setup = interrupt_epoch
            .setup()
            .expect("running scan requires a quiesced interrupt epoch");
        let scan_plan = Esp32s31StationScanPlan::new(discovery, None);
        let scan_request = scan_plan.request(station.station_address);
        let scan_request = if self.scan_only {
            scan_request.without_candidate_selection()
        } else {
            scan_request
        };
        let scan = run_esp32s31_station_scan(
            Esp32s31StationScanResources {
                phy,
                platform,
                phy_observer: NoopPhyTargetObserver,
                phy_delay: EmbassyEsp32s31PhyDelay,
                hardware,
                receive: Esp32s31RunningScanRx::from_stopped(rx),
                control,
                interrupt_setup,
                table: scan_table,
                frame,
                scan_observer: ProductionScanObserver,
                sequence: security.sequences.non_qos_mut(),
                timer: EmbassyEsp32s31ScanTimer,
            },
            scan_request,
        )
        .await;
        let scan_completed = matches!(
            &scan.decision,
            Esp32s31StationScanDecision::NoCandidate { .. }
        );
        let scan_result = match scan.decision {
            Esp32s31StationScanDecision::Selected { candidate, .. } => {
                Esp32s31StationRunningScanCompletion::Selected(candidate)
            }
            Esp32s31StationScanDecision::NoCandidate { .. } => {
                Esp32s31StationRunningScanCompletion::Failed {
                    disposition: StaFailureDisposition::RefreshCandidate,
                    error: Esp32s31StaAttemptStage::Candidate,
                }
            }
            Esp32s31StationScanDecision::Stopped { .. } => {
                Esp32s31StationRunningScanCompletion::Stopped
            }
            Esp32s31StationScanDecision::Failed { error, .. } => {
                let disposition = esp32s31_station_scan_failure_disposition(&error);
                Esp32s31StationRunningScanCompletion::Failed {
                    disposition,
                    error: Esp32s31StaAttemptStage::Candidate,
                }
            }
            Esp32s31StationScanDecision::InvalidPlan { .. } => {
                Esp32s31StationRunningScanCompletion::Failed {
                    disposition: StaFailureDisposition::Terminal,
                    error: Esp32s31StaAttemptStage::Candidate,
                }
            }
        };
        if self.scan_only && scan_completed {
            self.scan_completed.set(true);
        }
        let open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationScanReturned {
            hardware,
            receive,
            control,
            table: _,
            frame: _,
            sequence: _,
            phy_observer: _,
            phy_delay: _,
            scan_observer: _,
            timer: _,
            telemetry: _,
            transmit: _,
        } = scan.returned;
        let rx = receive.into_stopped().unwrap_or_else(|rx| {
            panic!(
                "running scan did not return a halted RX owner: {:?}",
                rx.phase()
            )
        });
        tx_storage
            .restore_control(control)
            .unwrap_or_else(|_| panic!("running scan returned over a live TX owner"));
        let disconnected = retained.restore(hardware, rx);
        complete_esp32s31_station_running_scan(
            runtime,
            disconnected,
            station,
            security,
            scan_result,
            |disconnected| {
                let (network, epoch) =
                    disconnected.prepare_reconnect::<EmbassyEsp32s31RxFrontierDelay>();
                (WifiNetworkResources::Running(network), epoch)
            },
            |runtime, disconnected, station, security| {
                ProductionStationOwner::new(
                    runtime,
                    ProductionStationPhase::RunningScan {
                        disconnected,
                        station,
                    },
                    security.into_role(),
                )
            },
        )
    }
}

impl<'state, 'security> ProductionStationEnginePort<ProductionStationOwner<'state, 'security>> {
    #[inline(never)]
    async fn run_initial_join_epoch<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialJoinPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRuntimeOwner,
            Esp32s31RxFrontier<
                'static,
                EmbassyEsp32s31RxFrontierDelay,
                RX_DESCRIPTOR_COUNT,
                RX_BUFFER_SIZE,
            >,
            WifiNetworkResources,
        >,
        context: StaAttemptContext,
    ) -> Esp32s31StationJoinExit<
        'security,
        ProductionStationRuntime<'state>,
        ProductionConnectedPhase,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    >
    where
        'security: 'a,
        'state: 'a,
    {
        qualification_event!(
            "open-radio: station lifecycle attempt generation={} attempt={}",
            context.generation,
            context.attempt
        );
        let (mut runtime, mut hardware, receive, network, station, security) = phase.into_parts();
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, _) = radio_resources.parts_mut();
        let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
        let join = run_esp32s31_station_join::<
            _,
            _,
            _,
            EmbassyEsp32s31PhyDelay,
            _,
            _,
            (),
            _,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
            RX_BUFFER_STORAGE_SIZE,
        >(Esp32s31StationJoinResources {
            hardware: &mut hardware,
            phy,
            platform,
            phy_observer: NoopPhyTargetObserver,
            receive,
            rx_storage: dma.storage(),
            transmit: tx_storage
                .control_mut()
                .expect("station attempt owns ordinary TX"),
            frame,
            station,
            security,
            attempt_observer: ProductionAttemptObserver,
        })
        .await;
        match join {
            Esp32s31StationJoinOutcome::Failed {
                returned,
                stage,
                disposition,
                error,
                progress,
                ..
            } => {
                qualification_event!(
                    "open-radio: station attempt failed stage={stage:?} \
                     disposition={disposition:?} completed={} error={error:?}",
                    progress.completed_count()
                );
                Esp32s31StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::InitialJoin {
                            hardware,
                            receive: returned.receive,
                            network,
                            station: returned.station,
                        },
                        returned.security,
                    ),
                    failure: StaAttemptFailure::new(stage.lifecycle_stage(), disposition, stage),
                })
            }
            Esp32s31StationJoinOutcome::Connected {
                returned,
                peer,
                pairwise,
                group,
                report,
                progress,
            } => {
                qualification_event!(
                    "open-radio: station joined phases={} auth={} assoc={} wpa2={} m4={}",
                    progress.completed_count(),
                    report.authentication.is_some(),
                    report.association.is_some(),
                    report.wpa2.is_some(),
                    report.message4.is_some()
                );
                Esp32s31StationJoinExit::connected_ready(
                    runtime,
                    ProductionConnectedPhase {
                        epoch: ConnectedStationEpoch::Initial {
                            hardware,
                            receive: returned.receive,
                        },
                        network,
                        station: returned.station,
                        peer,
                        pairwise,
                        group,
                    },
                    returned.security,
                )
            }
        }
    }

    #[inline(never)]
    async fn run_reconnected_epoch<'a>(
        &'a mut self,
        phase: Esp32s31StationReconnectedPhase<
            'security,
            ProductionStationRuntime<'state>,
            ConnectedReconnectedEpoch,
            WifiNetworkResources,
        >,
        context: StaAttemptContext,
    ) -> Esp32s31StationJoinExit<
        'security,
        ProductionStationRuntime<'state>,
        ProductionConnectedPhase,
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    >
    where
        'security: 'a,
        'state: 'a,
    {
        qualification_event!(
            "open-radio: station lifecycle attempt generation={} attempt={}",
            context.generation,
            context.attempt
        );
        let (mut runtime, mut reconnect, network, station, security) = phase.into_parts();
        let (hardware, receive_slot) = reconnect.hardware_and_rx_mut();
        let receive = match receive_slot.take() {
            Ok(receive) => receive,
            Err(_) => {
                return Esp32s31StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::Reconnected {
                            epoch: reconnect,
                            network,
                            station,
                        },
                        security,
                    ),
                    failure: StaAttemptFailure::new(
                        open_esp_radio::wifi::sta::station::StaLifecycleStage::Hardware,
                        open_esp_radio::wifi::sta::station::StaFailureDisposition::Terminal,
                        Esp32s31StaAttemptStage::Candidate,
                    ),
                });
            }
        };
        let (radio_resources, storage_resources, _) = runtime.split_mut();
        let (phy, platform, _) = radio_resources.parts_mut();
        let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
        let join = run_esp32s31_station_join::<
            _,
            _,
            _,
            EmbassyEsp32s31PhyDelay,
            _,
            _,
            (),
            _,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
            RX_BUFFER_STORAGE_SIZE,
        >(Esp32s31StationJoinResources {
            hardware,
            phy,
            platform,
            phy_observer: NoopPhyTargetObserver,
            receive,
            rx_storage: dma.storage(),
            transmit: tx_storage
                .control_mut()
                .expect("station attempt owns ordinary TX"),
            frame,
            station,
            security,
            attempt_observer: ProductionAttemptObserver,
        })
        .await;
        match join {
            Esp32s31StationJoinOutcome::Failed {
                returned,
                stage,
                disposition,
                error,
                progress,
                ..
            } => {
                qualification_event!(
                    "open-radio: reconnect attempt failed stage={stage:?} \
                     disposition={disposition:?} completed={} error={error:?}",
                    progress.completed_count()
                );
                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                *receive_slot = returned.receive;
                Esp32s31StationJoinExit::complete(StaAttemptOutcome::Failed {
                    owner: ProductionStationOwner::new(
                        runtime,
                        ProductionStationPhase::Reconnected {
                            epoch: reconnect,
                            network,
                            station: returned.station,
                        },
                        returned.security,
                    ),
                    failure: StaAttemptFailure::new(stage.lifecycle_stage(), disposition, stage),
                })
            }
            Esp32s31StationJoinOutcome::Connected {
                returned,
                peer,
                pairwise,
                group,
                report,
                progress,
            } => {
                qualification_event!(
                    "open-radio: station rejoined phases={} auth={} assoc={} wpa2={} m4={}",
                    progress.completed_count(),
                    report.authentication.is_some(),
                    report.association.is_some(),
                    report.wpa2.is_some(),
                    report.message4.is_some()
                );
                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                *receive_slot = returned.receive;
                Esp32s31StationJoinExit::connected_ready(
                    runtime,
                    ProductionConnectedPhase {
                        epoch: ConnectedStationEpoch::Reconnected(reconnect),
                        network,
                        station: returned.station,
                        peer,
                        pairwise,
                        group,
                    },
                    returned.security,
                )
            }
        }
    }
}

#[cfg(not(feature = "qualification"))]
type ProductionStationRunner<'state, 'security> = Esp32s31StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
>;

#[cfg(feature = "qualification")]
type ProductionStationRunner<'state, 'security> = Esp32s31StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
    ProductionStationObserver,
>;

#[cfg(feature = "qualification")]
#[derive(Clone, Copy)]
struct ProductionStationObserver {
    lifecycle: Option<fn(crate::Esp32s31StationLifecycleObservation)>,
}

#[cfg(feature = "qualification")]
impl<'state, 'security>
    Esp32s31StationEngineObserver<
        'security,
        CriticalSectionRawMutex,
        ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
    > for ProductionStationObserver
{
    fn backoff_started(&mut self, _delay_millis: u32, reason: StaBackoffReason) {
        let StaBackoffReason::AttemptFailed { stage, attempt } = reason else {
            return;
        };
        if let Some(lifecycle) = self.lifecycle {
            lifecycle(crate::Esp32s31StationLifecycleObservation::AttemptFailed { attempt, stage });
        }
    }
}

impl<'state, 'security> Esp32s31StationEnginePort<'security, CriticalSectionRawMutex>
    for ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>
{
    type Runtime = ProductionStationRuntime<'state>;
    type InitialHardware = RadioRuntimeOwner;
    type InitialScanRx =
        Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
    type RxFrontier = Esp32s31RxFrontier<
        'static,
        EmbassyEsp32s31RxFrontierDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >;
    type Network = WifiNetworkResources;
    type Disconnected = ConnectedDisconnectedEpoch;
    type Reconnected = ConnectedReconnectedEpoch;
    type Connected = ProductionConnectedPhase;
    type Error = Esp32s31StaAttemptStage;
    type Fault = ProductionStationFault<'state, 'security>;

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_initial_scan_epoch(phase, discovery)
    }

    fn run_initial_join<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_initial_join_epoch(phase, context)
    }

    fn run_running_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_running_scan_epoch(phase, discovery)
    }

    fn run_reconnected<'a>(
        &'a mut self,
        phase: Esp32s31StationReconnectedPhase<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
        >,
        context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = Esp32s31StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_reconnected_epoch(phase, context)
    }

    fn run_connected<'a>(
        &'a mut self,
        phase: Esp32s31StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        _context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StaAttemptOutcome<
            ProductionStationOwner<'state, 'security>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        self.run_connected_epoch(phase, control)
    }

    fn candidate_refresh_contract_error(&mut self) -> Self::Error {
        Esp32s31StaAttemptStage::Candidate
    }
}

type ProductionStationTask = Esp32s31StationTask<
    'static,
    CriticalSectionRawMutex,
    ProductionStationRunner<'static, 'static>,
>;
type ProductionStationControl = Esp32s31StationController<'static, CriticalSectionRawMutex>;
type ProductionStationExit = Esp32s31StationExit<
    ProductionStationOwner<'static, 'static>,
    ProductionStationRunner<'static, 'static>,
    Esp32s31StaAttemptStage,
    ProductionStationFault<'static, 'static>,
>;

impl ProductionWifiEpochRunner {
    fn initialize_tx_epoch(
        &self,
        tx: ProductionOrdinaryTxResources,
        power: PhyTxTargetPowerProfile,
    ) -> &'static mut TxStorage {
        match tx {
            ProductionOrdinaryTxResources::Uninitialized(tx_slot) => TX_STATE.init_with(|| {
                TxStorage::from_slot(
                    tx_slot,
                    power,
                    tx_entropy as fn() -> u32,
                    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
                    ControlTxConfig {
                        unicast_attempt_limit: 4,
                        completion_timeout_us: TX_COMPLETION_TIMEOUT_US,
                        poll_interval_us: 1,
                    },
                )
            }),
            ProductionOrdinaryTxResources::Epoch(tx) => tx,
        }
    }

    fn fresh_security(&self, security: StationSecurity) -> Esp32s31StaAttemptSecurity<'static> {
        let mut supplicant_nonce = [0; 32];
        for word in supplicant_nonce.chunks_exact_mut(4) {
            word.copy_from_slice(&self.trng.random().to_le_bytes());
        }
        Esp32s31StaAttemptSecurity::new(
            security.into_pmk(),
            supplicant_nonce,
            StaTxSequenceCounters::new((self.trng.random() & 0x0fff) as u16),
            open_esp_radio::esp32s31::wifi::sta::wpa2::Esp32s31Wpa2Message4Protection::Unprotected,
        )
    }

    fn prepare_station_task(
        &self,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
        scan_only: bool,
    ) -> Result<(ProductionStationControl, ProductionStationTask), ProductionWifiFault> {
        let (discovery, security, reconnect) = request.into_parts();
        let (wifi, physical_resources, station_role, access_point_resources, monitor_resources) =
            stopped.into_parts();
        let station_resources =
            join_station_activation_resources(physical_resources, station_role);
        let security = self.fresh_security(security);
        let owner = match station_resources {
            ProductionWifiStoppedResources::Fresh(fresh) => {
                let mut materialized = materialize_esp32s31_wifi_role(wifi, fresh);
                let ProductionWifiFreshResources {
                    dma,
                    rx_ring,
                    tx,
                    scan_table,
                    scan_frame,
                    ethernet,
                    network,
                    board,
                    station_address,
                } = materialized.resources;
                let mut registers = materialized.registers;
                let (phy, _) = materialized.owner.radio_mut();
                let tx_storage = self.initialize_tx_epoch(tx, phy.tx_target_power_profile());
                activate_promiscuous_receive(&mut registers);
                let scan_rx = match rx_ring {
                    Some(ring) => Esp32s31ScanRx::from_halted(ring, dma.storage()),
                    None => match Esp32s31ScanRx::prepare_initial(
                        &mut registers,
                        dma.storage(),
                        dma.descriptor_base(),
                        dma.buffer_addresses(),
                    ) {
                        Ok(scan_rx) => scan_rx,
                        Err(error) => {
                            return Err(ProductionWifiFault::InitialRx {
                                _fault: ProductionInitialRxFault {
                                    _error: error,
                                    _owner: materialized.owner,
                                    _registers: registers,
                                    _interrupt_setup: materialized.interrupt_setup,
                                    _dma: dma,
                                    _tx_storage: tx_storage,
                                    _scan_table: scan_table,
                                    _scan_frame: scan_frame,
                                    _ethernet: ethernet,
                                    _network: network,
                                    _board: board,
                                    _station_address: station_address,
                                    _security: security,
                                },
                            });
                        }
                    },
                };
                ProductionStationOwner::new(
                    production_station_runtime(
                        materialized.owner,
                        mac_interrupt_epoch(materialized.interrupt_setup),
                        dma,
                        tx_storage,
                        scan_table,
                        scan_frame,
                        ethernet,
                        board,
                    ),
                    ProductionStationPhase::InitialScan {
                        hardware: registers,
                        receive: scan_rx,
                        network,
                        identity: Esp32s31StaIdentity {
                            station_address,
                            association_preference: discovery.scan().association_preference(),
                        },
                    },
                    security,
                )
            }
            ProductionWifiStoppedResources::Returned(returned) => {
                let materialized = materialize_esp32s31_wifi_role(wifi, returned);
                let ProductionWifiReusableResources {
                    storage,
                    board,
                    phase,
                    security: previous_security,
                    interrupt_route,
                    mac_runtime,
                    power_runtime,
                } = materialized.resources;
                let rx_storage = storage.parts().0.storage();
                let identity = Esp32s31StaIdentity {
                    station_address: board.interface.interface.address,
                    association_preference: discovery.scan().association_preference(),
                };
                let phase = match try_rebind_esp32s31_station_phase(phase, rx_storage, identity) {
                    Ok(phase) => phase,
                    Err(failure) => {
                        return Err(ProductionWifiFault::Resume {
                            _fault: ProductionStationResumeFault {
                                _owner: materialized.owner,
                                _registers: materialized.registers,
                                _interrupt_setup: materialized.interrupt_setup,
                                _storage: storage,
                                _board: board,
                                _phase: failure.resources,
                                _previous_security: previous_security,
                                _requested_security: security,
                                _interrupt_route: interrupt_route,
                                _mac_runtime: mac_runtime,
                                _power_runtime: power_runtime,
                            },
                        });
                    }
                };
                let phase = match try_restore_esp32s31_station_phase(materialized.registers, phase)
                {
                    Ok(phase) => phase,
                    Err(failure) => {
                        return Err(ProductionWifiFault::Resume {
                            _fault: ProductionStationResumeFault {
                                _owner: materialized.owner,
                                _registers: failure.registers,
                                _interrupt_setup: materialized.interrupt_setup,
                                _storage: storage,
                                _board: board,
                                _phase: failure.resources,
                                _previous_security: previous_security,
                                _requested_security: security,
                                _interrupt_route: interrupt_route,
                                _mac_runtime: mac_runtime,
                                _power_runtime: power_runtime,
                            },
                        });
                    }
                };
                let interrupt = MacInterruptEpoch::new(
                    interrupt_route,
                    materialized.interrupt_setup,
                    mac_runtime,
                    power_runtime,
                );
                // Dropping the previous security value here zeroizes its PMK
                // only after the old finite station task returned completely.
                drop(previous_security);
                ProductionStationOwner::new(
                    Esp32s31StationRuntimeResources::new(
                        Esp32s31StationRadioResources::new(materialized.owner, interrupt),
                        storage,
                        board,
                    ),
                    phase,
                    security,
                )
            }
        };
        let port = if scan_only {
            ProductionStationEnginePort::standalone_scan(
                access_point_resources,
                monitor_resources,
            )
        } else {
            ProductionStationEnginePort::new(access_point_resources, monitor_resources)
        };
        #[cfg(feature = "qualification")]
        let runner = ProductionStationRunner::with_observer(
            port,
            discovery,
            ProductionStationObserver {
                lifecycle: owner
                    .runtime
                    .board()
                    .qualification
                    .map(|hooks| hooks.station_lifecycle),
            },
        );
        #[cfg(not(feature = "qualification"))]
        let runner = ProductionStationRunner::new(port, discovery);
        prepare_esp32s31_station_task(
            Esp32s31StationConfig::new(reconnect),
            Esp32s31StationStartResources::new(owner),
            self.station_control,
            runner,
        )
        .map_err(|failure| ProductionWifiFault::TaskPreparation { _failure: failure })
    }
}

fn standalone_scan_station_request(request: WifiScanRequest) -> StationRequest {
    let ssid = WifiSsid::new(b"open-radio-standalone-scan")
        .expect("the private scan-only SSID is bounded");
    StationRequest::new(
        ssid,
        StationSecurity::wpa2_personal(Pmk::from_bytes([0; 32])),
        StaReconnectPolicy::new(1, 1, 1, 1).expect("one finite scan attempt is valid"),
        StationScanPolicy::new(
            request.channels(),
            core::num::NonZeroU16::new(request.dwell_millis())
                .expect("WifiScanRequest stores a nonzero dwell"),
            open_esp_radio::wifi::ieee80211::station::StaAssociationPreference::PreferHe20,
        ),
    )
}

fn standalone_scan_report(
    owner: &mut ProductionStationOwner<'static, 'static>,
    generation: open_esp_radio::RadioSubsystemGeneration,
) -> WifiScanReport {
    let (_, storage, _) = owner.runtime.split_mut();
    let table = storage.parts().2;
    let summary = table.summary();
    let mut results = [WifiScanResult::EMPTY; WIFI_SCAN_RESULT_CAPACITY];
    for (destination, source) in results.iter_mut().zip(table.records()) {
        *destination = WifiScanResult::new(
            source.ssid,
            source.ssid_len,
            source.bssid,
            source.channel,
            source.rssi,
            source.privacy,
            source.rsn,
            source.legacy_wpa,
            source.ht_capability_ie_present,
            source.he_capability_ie_len != 0,
        );
    }
    WifiScanReport::new(
        generation,
        results,
        summary.records as u8,
        summary.observed_frames,
        summary.dropped_unique_bss,
    )
}

impl ProductionWifiEpochRunner {
    async fn run_station_service(
        &self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        stopped: ProductionSupervisorStopped,
        request: StationRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        await_stack_boundary!(run_esp32s31_station_supervisor_epoch(
            endpoint,
            Esp32s31StationSupervisorEpoch::new(stopped, request, generation),
            |stopped, request| self.prepare_station_task(stopped, request, false),
            Esp32s31StationSupervisorHooks::new(
                |output: ProductionStationExit| {
                    let resources = match output {
                        Esp32s31StationExit::Stopped {
                            resources,
                            progress,
                            reason,
                        } => {
                            qualification_event!(
                                "open-radio: station epoch stopped attempts={} connected_epochs={} reason={reason:?}",
                                progress.attempts_started,
                                progress.connected_epochs,
                            );
                            resources
                        }
                        Esp32s31StationExit::RetryExhausted {
                            resources,
                            progress,
                            failure,
                        } => {
                            qualification_event!(
                                "open-radio: station epoch exhausted attempts={} stage={:?}",
                                progress.attempts_started,
                                failure.stage,
                            );
                            #[cfg(feature = "qualification")]
                            if let Some(hooks) = resources.owner().runtime.board().qualification {
                                (hooks.station_lifecycle)(
                                    crate::Esp32s31StationLifecycleObservation::RetryExhausted {
                                        attempts: progress.final_generation_attempt,
                                        stage: failure.stage,
                                    },
                                );
                            }
                            resources
                        }
                        Esp32s31StationExit::Terminal {
                            resources,
                            progress,
                            failure,
                        } => {
                            qualification_event!(
                                "open-radio: station epoch ended attempts={} stage={:?}",
                                progress.attempts_started,
                                failure.stage,
                            );
                            resources
                        }
                        Esp32s31StationExit::Faulted { fault, runner, .. } => {
                            return EmbassyWifiRoleFrontier::Faulted(
                                ProductionWifiFault::Station {
                                    _fault: fault,
                                    _runner: runner,
                                },
                            );
                        }
                    };
                    let (owner, runner) = resources.into_parts();
                    match try_reclaim_production_station(owner) {
                        Ok(stopped) => {
                            let (access_point, monitor) =
                                runner.into_port().into_parked_roles();
                            let resources =
                                ProductionWifiStoppedResources::Returned(stopped.resources);
                            match try_split_wifi_stopped_resources(resources) {
                                Ok((physical, station)) => EmbassyWifiRoleFrontier::Stopped(
                                    Esp32s31WifiSupervisorStopped::new(
                                        stopped.wifi,
                                        physical,
                                        station,
                                        access_point,
                                        monitor,
                                    ),
                                ),
                                Err(resources) => EmbassyWifiRoleFrontier::Faulted(
                                    ProductionWifiFault::StoppedOwner {
                                        _wifi: stopped.wifi,
                                        _resources: resources,
                                        _access_point: access_point,
                                        _monitor: monitor,
                                    },
                                ),
                            }
                        }
                        Err(failure) => {
                            EmbassyWifiRoleFrontier::Faulted(ProductionWifiFault::Reclaim {
                                _station: failure,
                                _runner: runner,
                            })
                        }
                    }
                },
                Esp32s31RadioError::RoleActive,
                |_faulted: &ProductionWifiFault| Esp32s31RadioError::HardwareFault,
            ),
        ))
    }
}

impl EmbassyWifiRoleEpochRunner<CriticalSectionRawMutex> for ProductionWifiEpochRunner {
    type Stopped = ProductionSupervisorStopped;
    type Faulted = ProductionWifiFault;
    type Error = Esp32s31RadioError;

    fn planning_error(&mut self, error: WifiServicePlanningError) -> Self::Error {
        Esp32s31RadioError::Planning(error)
    }

    fn fault_error(&mut self, faulted: &Self::Faulted) -> Self::Error {
        let _ = faulted;
        Esp32s31RadioError::HardwareFault
    }

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut EmbassyWifiSupervisorEndpoint<'_, CriticalSectionRawMutex, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> impl Future<Output = EmbassyWifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a {
        async move {
            match service {
                WifiServiceRequest::StandaloneScan { request, .. } => {
                    let (controller, mut task) = match self.prepare_station_task(
                        stopped,
                        standalone_scan_station_request(request),
                        true,
                    ) {
                        Ok(prepared) => prepared,
                        Err(faulted) => {
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                                    WifiScanFailure::Faulted {
                                        error: self.fault_error(&faulted),
                                    },
                                )))
                                .await;
                            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let output = await_stack_boundary!(task.run());
                    drop(controller);
                    let resources = match output {
                        Esp32s31StationExit::Stopped { resources, .. }
                        | Esp32s31StationExit::RetryExhausted { resources, .. }
                        | Esp32s31StationExit::Terminal { resources, .. } => resources,
                        Esp32s31StationExit::Faulted { fault, runner, .. } => {
                            let faulted = ProductionWifiFault::Station {
                                _fault: fault,
                                _runner: runner,
                            };
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                                    WifiScanFailure::Faulted {
                                        error: self.fault_error(&faulted),
                                    },
                                )))
                                .await;
                            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let (mut owner, runner) = resources.into_parts();
                    let completed = runner.port().scan_completed();
                    let report = standalone_scan_report(&mut owner, generation);
                    let stopped = match try_reclaim_production_station(owner) {
                        Ok(stopped) => stopped,
                        Err(failure) => {
                            let faulted = ProductionWifiFault::Reclaim {
                                _station: failure,
                                _runner: runner,
                            };
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                                    WifiScanFailure::Faulted {
                                        error: self.fault_error(&faulted),
                                    },
                                )))
                                .await;
                            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let (access_point, monitor) = runner.into_port().into_parked_roles();
                    let resources = ProductionWifiStoppedResources::Returned(stopped.resources);
                    let stopped = match try_split_wifi_stopped_resources(resources) {
                        Ok((physical, station)) => Esp32s31WifiSupervisorStopped::new(
                            stopped.wifi,
                            physical,
                            station,
                            access_point,
                            monitor,
                        ),
                        Err(resources) => {
                            let faulted = ProductionWifiFault::StoppedOwner {
                                _wifi: stopped.wifi,
                                _resources: resources,
                                _access_point: access_point,
                                _monitor: monitor,
                            };
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                                    WifiScanFailure::Faulted {
                                        error: self.fault_error(&faulted),
                                    },
                                )))
                                .await;
                            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let response = if completed {
                        Ok(report)
                    } else {
                        Err(WifiScanFailure::Returned {
                            request,
                            error: Esp32s31RadioError::HardwareFault,
                        })
                    };
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Scan(response))
                        .await;
                    EmbassyWifiRoleEpochOutcome::Stopped(stopped)
                }
                WifiServiceRequest::StandaloneMonitor { plan, request } => {
                    let Some(monitor_plan) = plan.standalone_monitor() else {
                        endpoint
                            .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                                WifiStartFailure::rejected(
                                    request,
                                    Esp32s31RadioError::Planning(
                                        WifiServicePlanningError::Request(
                                            open_esp_radio::WifiServiceRequestError::NotStandaloneMonitorTopology,
                                        ),
                                    ),
                                ),
                            )))
                            .await;
                        return EmbassyWifiRoleEpochOutcome::NotStarted(stopped);
                    };
                    let channel = request.channel();
                    let snapshot_length = request.capture_policy().snapshot_length();
                    let (
                        wifi,
                        physical_resources,
                        station_resources,
                        access_point_resources,
                        monitor_resources,
                    ) = stopped.into_parts();
                    let discarded = self.monitor_capture.discard_queued();
                    crate::monitor::record_discarded_monitor_frames(discarded);
                    let (mut controller, mut task) = match prepare_esp32s31_monitor_task(
                        monitor_plan,
                        wifi,
                        monitor_resources.bind(generation, snapshot_length),
                    ) {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            let faulted = ProductionWifiFault::MonitorBuild {
                                _failure: failure,
                                _physical: physical_resources,
                                _station: station_resources,
                            };
                            endpoint
                                .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                                    WifiStartFailure::faulted(self.fault_error(&faulted)),
                                )))
                                .await;
                            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                        }
                    };
                    let mut observer = NoopPhyTargetObserver;
                    if let Err(error) = await_stack_boundary!(
                        task.switch_channel::<EmbassyEsp32s31PhyDelay, _>(channel, &mut observer),
                    ) {
                        let faulted = ProductionWifiFault::MonitorChannel {
                            _error: error,
                            _task: task,
                            _physical: physical_resources,
                            _station: station_resources,
                        };
                        endpoint
                            .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                                WifiStartFailure::faulted(self.fault_error(&faulted)),
                            )))
                            .await;
                        return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
                    }
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Monitor(Ok(
                            WifiStartReport::new(generation),
                        )))
                        .await;
                    let exit = await_stack_boundary!(drive_esp32s31_monitor_role(
                        endpoint,
                        &mut controller,
                        task,
                        Esp32s31RadioError::RoleActive,
                    ));
                    let frontier = await_stack_boundary!(finish_embassy_wifi_active_role(
                        endpoint,
                        generation,
                        exit,
                        |output| match output {
                            Esp32s31MonitorTaskExit::Stopped { stopped, .. } => {
                                let discarded = self.monitor_capture.discard_queued();
                                crate::monitor::record_discarded_monitor_frames(discarded);
                                EmbassyWifiRoleFrontier::Stopped(
                                    Esp32s31WifiSupervisorStopped::new(
                                        stopped.wifi,
                                        physical_resources,
                                        station_resources,
                                        access_point_resources,
                                        ProductionMonitorResources::from_stopped(stopped.resources),
                                    ),
                                )
                            }
                            Esp32s31MonitorTaskExit::Faulted { task, .. } => {
                                EmbassyWifiRoleFrontier::Faulted(
                                    ProductionWifiFault::MonitorRuntime {
                                        _task: task,
                                        _physical: physical_resources,
                                        _station: station_resources,
                                    },
                                )
                            }
                        },
                        |_faulted| Esp32s31RadioError::HardwareFault,
                    ));
                    return match frontier {
                        EmbassyWifiRoleFrontier::Stopped(stopped) => {
                            EmbassyWifiRoleEpochOutcome::Stopped(stopped)
                        }
                        EmbassyWifiRoleFrontier::Faulted(faulted) => {
                            EmbassyWifiRoleEpochOutcome::Faulted(faulted)
                        }
                    };
                }
                WifiServiceRequest::Station { request, .. } => {
                    await_stack_boundary!(
                        self.run_station_service(endpoint, stopped, request, generation)
                    )
                }
                WifiServiceRequest::AccessPoint { request, .. } => {
                    await_stack_boundary!(
                        self.run_access_point_service(endpoint, stopped, request, generation)
                    )
                }
            }
        }
    }
}

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
    qualification_event!("open-radio: cold PHY start");

    let crate::Esp32s31RadioConfig {
        station_mac,
        access_point_mac,
        calibration,
        initial_channel,
        calibration_cache,
        maximum_tx_power_quarter_dbm,
        #[cfg(feature = "qualification")]
        qualification,
    } = config;
    let wifi_protocol = Esp32s31WifiProtocolRunner::new({
        #[cfg(feature = "qualification")]
        {
            qualification.map(|hooks| hooks.protocol_task_poll)
        }
        #[cfg(not(feature = "qualification"))]
        {
            None
        }
    });
    #[cfg(feature = "qualification")]
    if let Some(hooks) = qualification {
        configure_mac_irq_observer(hooks.mac_irq);
    }
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
        .validate(
            open_esp_radio::esp32s31::wifi::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
        )
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
    qualification_event!(
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
    let rx_storage = memory.rx_dma;
    let buffer_addresses = memory.rx_buffer_addresses;
    let monitor_memory = MonitorMemory::new(rx_storage, buffer_addresses)
        .map_err(|_| Esp32s31NewError::RxDmaLayout)?;
    let descriptor_base = monitor_memory.descriptor_base();
    let tx_dma =
        open_esp_radio::esp32s31::wifi::dma::tx_storage::TxDmaStorage::pin_static(memory.tx_dma)
            .map_err(|_| Esp32s31NewError::TxDmaLayout)?;
    let tx_slot = Pin::static_mut(TX_SLOT_STORAGE.init_with(|| TxSlot::from_dma(tx_dma)));
    qualification_event!(
        "open-radio: MAC ready handshake_samples={} interrupt_mask={:#010x}",
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
    #[cfg(feature = "qualification")]
    let qualification_snapshot = initial_connected.qualification;
    let initial_connected = initial_connected.resources;
    let (network_devices, station_network) =
        initialize_station_network(station_address, access_point_mac.bytes());
    let monitor = initialize_monitor_resources(monitor_memory)
        .map_err(|MonitorResourcesError::InUse| Esp32s31NewError::MonitorResources)?;
    let initial_resources =
        ProductionWifiStoppedResources::Fresh(ProductionWifiFreshResources {
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
                rx_protocol_runtime: initialize_connected_rx_protocol_runtime(),
                initial_connected: Some(initial_connected),
                #[cfg(feature = "qualification")]
                qualification,
            },
            station_address,
        });
    let (physical, station) = match try_split_wifi_stopped_resources(initial_resources) {
        Ok(resources) => resources,
        Err(_) => unreachable!("fresh Wi-Fi resources are always splittable"),
    };
    let stopped = Esp32s31WifiSupervisorStopped::new(
        wifi,
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
                open_esp_radio::esp32s31::wifi::ap::rx::Esp32s31ApRxDispatcher::new(
                    open_esp_radio::esp32s31::wifi::ap::rx::Esp32s31ApRxConfig {
                        access_point: access_point_mac.bytes(),
                        ingress: open_esp_radio::esp32s31::wifi::mac::rx::RxIngressConfig {
                            ring_entry_limit: 1,
                            csi_config: 0,
                            flags: 0,
                        },
                    },
                )
            }),
            rx_block_ack: AP_RX_BLOCK_ACK.init_with(|| {
                open_esp_radio::esp32s31::wifi::mac::rx_ampdu::RxBlockAckSessions::with_maximum_window(
                    open_esp_radio_esp32s31_wifi_embassy::resource_profile::ESP32S31_DEFAULT_RX_REORDER_WINDOW
                        as u16,
                )
                .expect("the production RX BlockAck window is statically validated")
            }),
            rx_reorder: AP_RX_REORDER.init_with(Esp32s31AccessPointRxReorder::new),
            rx_reorder_storage: &crate::connected::RX_REORDER_STORAGE,
        },
        monitor.role,
    );
    let configuration = WifiSupervisorConfiguration::new(
        open_esp_radio::esp32s31::wifi::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
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
                #[cfg(feature = "qualification")]
                qualification_snapshot,
            ),
            initialization,
        ),
        runners: Esp32s31RadioRunners {
            hardware: Esp32s31RadioRunner { supervisor },
            wifi_protocol,
        },
    })
}
