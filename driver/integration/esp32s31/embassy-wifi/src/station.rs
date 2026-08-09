//! Production ESP32-S31 station composition.
//!
//! The target owns board allocation and application policy. Every radio
//! transition is supplied by a PAC-backed driver or reusable integration
//! owner; no HIL protocol, telemetry or benchmark configuration is linked.

use core::{
    cell::{Cell, RefCell},
    future::Future,
    marker::PhantomData,
    pin::Pin,
};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::esp32s31::supervisor::{
    Esp32s31RadioSupervisorTask, Esp32s31StationSupervisorEpoch, Esp32s31StationSupervisorHooks,
    Esp32s31WifiSupervisorStopped, drive_esp32s31_monitor_role, prepare_esp32s31_radio_supervisor,
    run_esp32s31_station_supervisor_epoch,
};
use open_esp_radio::esp32s31::wifi::sta::attempt::{
    Esp32s31StaAttemptObserver, Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStage,
    Esp32s31StaAttemptStation, Esp32s31StaIdentity,
};
use open_esp_radio::esp32s31::wifi::sta::tx::ControlTxConfig;
use open_esp_radio::esp32s31::wifi::sta::tx_epoch::Esp32s31StaTxEpoch;
use open_esp_radio::{
    StationDiscovery, StationRequest, StationScanPolicy, StationSecurity,
    WIFI_SCAN_RESULT_CAPACITY, WifiConfig, WifiScanFailure, WifiScanReport, WifiScanRequest,
    WifiScanResult, WifiServicePlanningError, WifiServiceRequest, WifiSsid, WifiStartFailure,
    WifiStartReport, WifiStationConfig, WifiSupervisorConfiguration,
    embassy_supervisor::{
        EmbassyWifiRoleEpochOutcome, EmbassyWifiRoleEpochRunner, EmbassyWifiRoleFrontier,
        EmbassyWifiSupervisorControlResources, EmbassyWifiSupervisorEndpoint,
        EmbassyWifiSupervisorResponse, finish_embassy_wifi_active_role,
    },
    esp32s31::{
        Esp32s31RadioStartConfig, Esp32s31WifiStartConfig,
        hal::{Radio, RadioRegisters},
        phy::{NoopPhyTargetObserver, PhyTxTargetPowerProfile},
        start_esp32s31_radio,
        wifi::embassy::monitor::{
            Esp32s31MonitorChannelSwitchError, Esp32s31MonitorTaskExit,
            prepare_esp32s31_monitor_task,
        },
        wifi::mac::{init::activate_promiscuous_receive, tx::TxSlot},
        wifi::sta::control_tx::Esp32s31ControlTx,
    },
    wifi::{
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
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay,
    preconnected_rx::{EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRx},
    resource_profile::{
        ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_BUFFER_STORAGE_SIZE,
        ESP32S31_DEFAULT_TX_BUFFER_SIZE as TX_BUFFER_SIZE, Esp32s31DefaultStationMemory,
    },
    rx_dma_service::Esp32s31RxDmaStorage,
    scan_port::EmbassyEsp32s31ScanTimer,
    scan_rx::{Esp32s31RunningScanRx, Esp32s31ScanFrameObserver, Esp32s31ScanRx},
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        Esp32s31StationCommandReceiver, Esp32s31StationConfig, Esp32s31StationControlResources,
        Esp32s31StationController, Esp32s31StationDmaResources, Esp32s31StationEngine,
        Esp32s31StationEnginePort, Esp32s31StationExit, Esp32s31StationInitialJoinPhase,
        Esp32s31StationInitialScanExit, Esp32s31StationInitialScanFailures,
        Esp32s31StationInitialScanPhase, Esp32s31StationInitialScanReturned,
        Esp32s31StationJoinOutcome, Esp32s31StationJoinResources, Esp32s31StationPrepareFailure,
        Esp32s31StationRadioResources, Esp32s31StationReconnectedPhase,
        Esp32s31StationRunningScanCompletion, Esp32s31StationRunningScanExit,
        Esp32s31StationRunningScanPhase, Esp32s31StationRuntimeReclaimFailure,
        Esp32s31StationRuntimeResources, Esp32s31StationScanDecision, Esp32s31StationScanPlan,
        Esp32s31StationScanResources, Esp32s31StationServiceOwner, Esp32s31StationServicePhase,
        Esp32s31StationStartResources, Esp32s31StationStopped,
        Esp32s31StationStoppedPhaseResources, Esp32s31StationStorageResources, Esp32s31StationTask,
        complete_esp32s31_station_initial_scan, complete_esp32s31_station_running_scan,
        esp32s31_station_scan_failure_disposition, materialize_esp32s31_station,
        prepare_esp32s31_station_task, run_esp32s31_station_join, run_esp32s31_station_scan,
        try_rebind_esp32s31_station_phase, try_reclaim_esp32s31_station_runtime,
        try_restore_esp32s31_station_phase,
    },
    station_epoch::Esp32s31RunningScanEpochParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral, mac_interrupt_epoch::EspHalMacInterruptRoute,
};
use static_cell::StaticCell;

use crate::connected::{
    ConnectedAmpduStorage, ConnectedDisconnectedEpoch, ConnectedReconnectedEpoch,
    ConnectedRunningNetwork, ConnectedRxEpochResources, ConnectedRxProtocolStorage,
    ConnectedStationEpoch, ConnectedStationFault, ConnectedStationOutcome,
    ConnectedStationResources, ConnectedStationRunExit, ConnectedStoppedRx,
    ConnectedWorkerPublishers, ControlResources, InitialConnectedStaticResources,
    MacInterruptEpoch, StationNetwork, connected_config, initialize_connected_rx_protocol_runtime,
    initialize_connected_static_resources, initialize_ethernet_frame, initialize_station_network,
    mac_interrupt_epoch, run_connected, spawn_connected_workers,
};
use crate::monitor::{
    CaptureResources, MonitorMemory, MonitorResourcesError, ProductionMonitorBuildFailure,
    ProductionMonitorResources, ProductionMonitorTask, initialize_monitor_resources,
};
use crate::{
    Esp32s31NewError, Esp32s31Radio, Esp32s31RadioError, Esp32s31RadioInitialization, Esp32s31Wifi,
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
    open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRoleOwner<EspHalRadioPeripheral>,
    MacInterruptEpoch,
    Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    &'static mut TxStorage,
    ProductionStationBoardResources,
    SCAN_RECORD_CAPACITY,
>;
// The complete board-independent DMA/scan/control arena is acquired as one
// owner graph. This keeps large buffers out of the async task stack and avoids
// partially taking several unrelated global cells.
static STATION_MEMORY: Esp32s31DefaultStationMemory<CriticalSectionRawMutex> =
    Esp32s31DefaultStationMemory::new();
// These two owners contain runtime-derived DMA and PHY values. StaticCell
// retains their final address while `init_with` below avoids a by-value
// intermediate during construction.
static TX_SLOT_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
static TX_STATE: StaticCell<TxStorage> = StaticCell::new();

#[derive(Clone, Copy, Debug, Default)]
struct ProductionScanObserver;

impl Esp32s31ScanFrameObserver for ProductionScanObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

#[derive(Clone, Copy, Debug, Default)]
struct ProductionAttemptObserver;

impl Esp32s31StaAttemptObserver for ProductionAttemptObserver {
    fn stage_started(&mut self, stage: Esp32s31StaAttemptStage) {
        qualification_event!("open-radio: attempt stage={stage:?} state=start");
    }

    fn stage_completed(&mut self, stage: Esp32s31StaAttemptStage) {
        qualification_event!("open-radio: attempt stage={stage:?} state=complete");
    }
}

fn tx_entropy() -> u32 {
    Rng::new().random()
}

type ProductionStationPhase = Esp32s31StationServicePhase<
    RadioRegisters,
    Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
    Esp32s31PreconnectedRx<
        'static,
        EmbassyEsp32s31PreconnectedRxDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    StationNetwork,
    ConnectedDisconnectedEpoch,
    ConnectedReconnectedEpoch,
>;

enum ProductionStationJoinPhase {
    InitialJoin {
        hardware: RadioRegisters,
        receive: Esp32s31PreconnectedRx<
            'static,
            EmbassyEsp32s31PreconnectedRxDelay,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
        >,
        network: StationNetwork,
    },
    Reconnected {
        epoch: ConnectedReconnectedEpoch,
        network: StationNetwork,
    },
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
    Esp32s31PreconnectedRx<
        'static,
        EmbassyEsp32s31PreconnectedRxDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >,
    StationNetwork,
    ConnectedRunningNetwork,
    ConnectedStoppedRx,
    ConnectedAmpduStorage,
    &'static ControlResources,
    ConnectedRxEpochResources,
>;

/// Exact role-local graph parked while role-neutral Wi-Fi is stopped.
///
/// The register route and Embassy wake domains remain here rather than being
/// reacquired from board singletons. `phase` and `security` describe an exact
/// resume of the stopped service; converting them into a fresh station request
/// is a separate, still-explicit normalization transaction.
struct ProductionStationReusableResources<'security> {
    storage: ProductionStationStorage,
    board: ProductionStationBoardResources,
    phase: ProductionStationStoppedPhase,
    security: Esp32s31StaAttemptSecurity<'security>,
    interrupt_route: EspHalMacInterruptRoute,
    mac_runtime: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
    power_runtime: &'static EmbassyPowerIrqRuntime<CriticalSectionRawMutex>,
}

type ProductionStationStopped<'security> =
    Esp32s31StationStopped<EspHalRadioPeripheral, ProductionStationReusableResources<'security>>;

struct ProductionStationFreshResources {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_slot: Pin<&'static mut TxSlot<TX_BUFFER_SIZE>>,
    scan_table: &'static mut ScanTable,
    scan_frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    network: StationNetwork,
    board: ProductionStationBoardResources,
    station_address: [u8; 6],
}

enum ProductionStationResources {
    Fresh(ProductionStationFreshResources),
    Returned(ProductionStationReusableResources<'static>),
}

type ProductionSupervisorStopped = Esp32s31WifiSupervisorStopped<
    EspHalRadioPeripheral,
    ProductionStationResources,
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
    parked_monitor: RefCell<Option<ProductionMonitorResources>>,
}

/// Eternal owner-holding task. PAC, DMA and ISR capabilities never leave this
/// value or the futures it drives.
pub struct Esp32s31RadioRunner {
    supervisor:
        Esp32s31RadioSupervisorTask<'static, CriticalSectionRawMutex, ProductionWifiEpochRunner>,
}

impl Esp32s31RadioRunner {
    pub async fn run(self) -> ! {
        self.supervisor.run().await
    }
}

enum ProductionStationReclaimFault<'security> {
    Runtime {
        _failure: Esp32s31StationRuntimeReclaimFailure<ProductionStationOwner<'static, 'security>>,
    },
    InterruptInvariant {
        _registers: RadioRegisters,
        _role: open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRoleOwner<
            EspHalRadioPeripheral,
        >,
        _interrupt: MacInterruptEpoch,
        _storage: ProductionStationStorage,
        _board: ProductionStationBoardResources,
        _phase: ProductionStationStoppedPhase,
        _security: Esp32s31StaAttemptSecurity<'security>,
        _selected_channel: Option<WifiChannel>,
    },
}

enum ProductionWifiFault {
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
    MonitorBuild {
        _failure: ProductionMonitorBuildFailure,
        _station: ProductionStationResources,
    },
    MonitorChannel {
        _error: Esp32s31MonitorChannelSwitchError,
        _task: ProductionMonitorTask,
        _station: ProductionStationResources,
    },
    MonitorRuntime {
        _task: ProductionMonitorTask,
        _station: ProductionStationResources,
    },
    StationResourceInvariant {
        _stopped: ProductionStationStopped<'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
}

struct ProductionStationResumeFault {
    _owner: open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRoleOwner<
        EspHalRadioPeripheral,
    >,
    _registers: RadioRegisters,
    _interrupt_setup: open_esp_radio::esp32s31::registers::MacInterruptSetup,
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
        ProductionStationReusableResources {
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

struct ProductionStationJoinOwner<'state, 'security> {
    phase: ProductionStationJoinPhase,
    runtime: ProductionStationRuntime<'state>,
    station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'state, 'security> ProductionStationJoinOwner<'state, 'security> {
    fn new(
        runtime: ProductionStationRuntime<'state>,
        phase: ProductionStationJoinPhase,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            phase,
            runtime,
            station,
            security,
        }
    }
}

pub(super) struct ProductionStationBoardResources {
    pub(super) interface: BoundVirtualInterface,
    pub(super) rx_protocol_runtime: &'static mut ConnectedRxProtocolStorage,
    pub(super) initial_connected: Option<InitialConnectedStaticResources>,
    pub(super) workers: ConnectedWorkerPublishers,
    #[cfg(feature = "qualification")]
    pub(super) qualification: Option<crate::Esp32s31QualificationHooks>,
}

struct ProductionStationEnginePort<O> {
    scan_only: bool,
    scan_completed: Cell<bool>,
    _owner: PhantomData<fn() -> O>,
}

impl<O> ProductionStationEnginePort<O> {
    fn new() -> Self {
        Self {
            scan_only: false,
            scan_completed: Cell::new(false),
            _owner: PhantomData,
        }
    }

    fn standalone_scan() -> Self {
        Self {
            scan_only: true,
            scan_completed: Cell::new(false),
            _owner: PhantomData,
        }
    }

    fn scan_completed(&self) -> bool {
        self.scan_completed.get()
    }
}

pub(super) fn production_station_runtime<'state>(
    role: open_esp_radio_esp32s31_wifi_embassy::station::Esp32s31StationRoleOwner<
        EspHalRadioPeripheral,
    >,
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
    async fn run_initial_scan_epoch(
        &self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            ProductionStationRuntime<'state>,
            RadioRegisters,
            Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
            StationNetwork,
        >,
        discovery: StationDiscovery,
    ) -> Esp32s31StationInitialScanExit<
        'security,
        ProductionStationRuntime<'state>,
        RadioRegisters,
        Esp32s31PreconnectedRx<
            'static,
            EmbassyEsp32s31PreconnectedRxDelay,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
        >,
        StationNetwork,
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
                    .map(Esp32s31PreconnectedRx::from_halted)
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

    async fn run_connected_epoch(
        &mut self,
        runtime: ProductionStationRuntime<'state>,
        epoch: ConnectedStationEpoch,
        network: StationNetwork,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
        peer: open_esp_radio::esp32s31::wifi::sta::peer::Esp32s31ConnectedStaPeer,
        pairwise: open_esp_radio::esp32s31::wifi::mac::crypto::StaPairwiseCcmpSlot,
        group: open_esp_radio::esp32s31::wifi::mac::crypto::StaGroupCcmpSlot,
        control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> StaAttemptOutcome<
        ProductionStationOwner<'state, 'security>,
        Esp32s31StaAttemptStage,
        ProductionStationFault<'state, 'security>,
    > {
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
            ConnectedStationOutcome::Disconnected | ConnectedStationOutcome::ReconnectRequested => {
                StaAttemptOutcome::Disconnected {
                    owner,
                    next_candidate: StaNextCandidate::Refresh,
                }
            }
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
        StationNetwork,
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
                    disconnected.prepare_reconnect::<EmbassyEsp32s31PreconnectedRxDelay>();
                (StationNetwork::Running(network), epoch)
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
    fn run_phase<'a>(
        &'a mut self,
        owner: ProductionStationJoinOwner<'state, 'security>,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<
        Output = StaAttemptOutcome<
            ProductionStationOwner<'state, 'security>,
            Esp32s31StaAttemptStage,
            ProductionStationFault<'state, 'security>,
        >,
    > + 'a
    where
        'security: 'a,
        'state: 'a,
    {
        async move {
            qualification_event!(
                "open-radio: station lifecycle attempt generation={} attempt={}",
                context.generation,
                context.attempt
            );
            let ProductionStationJoinOwner {
                phase,
                mut runtime,
                station,
                security,
            } = owner;
            let outcome = match phase {
                ProductionStationJoinPhase::InitialJoin {
                    mut hardware,
                    receive,
                    network,
                } => {
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
                            StaAttemptOutcome::Failed {
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
                                failure: StaAttemptFailure::new(
                                    stage.lifecycle_stage(),
                                    disposition,
                                    stage,
                                ),
                            }
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
                            self.run_connected_epoch(
                                runtime,
                                ConnectedStationEpoch::Initial {
                                    hardware,
                                    receive: returned.receive,
                                },
                                network,
                                returned.station,
                                returned.security,
                                peer,
                                pairwise,
                                group,
                                control,
                            )
                            .await
                        }
                    }
                }
                ProductionStationJoinPhase::Reconnected {
                    epoch: mut reconnect,
                    network,
                } => {
                    let (hardware, receive_slot) = reconnect.hardware_and_rx_mut();
                    let receive = match receive_slot.take() {
                        Ok(receive) => receive,
                        Err(_) => {
                            return StaAttemptOutcome::Failed {
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
                            };
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
                            StaAttemptOutcome::Failed {
                                owner: ProductionStationOwner::new(
                                    runtime,
                                    ProductionStationPhase::Reconnected {
                                        epoch: reconnect,
                                        network,
                                        station: returned.station,
                                    },
                                    returned.security,
                                ),
                                failure: StaAttemptFailure::new(
                                    stage.lifecycle_stage(),
                                    disposition,
                                    stage,
                                ),
                            }
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
                            self.run_connected_epoch(
                                runtime,
                                ConnectedStationEpoch::Reconnected(reconnect),
                                network,
                                returned.station,
                                returned.security,
                                peer,
                                pairwise,
                                group,
                                control,
                            )
                            .await
                        }
                    }
                }
            };
            outcome
        }
    }
}

type ProductionStationRunner<'state, 'security> = Esp32s31StationEngine<
    'security,
    ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>,
>;

impl<'state, 'security> Esp32s31StationEnginePort<'security, CriticalSectionRawMutex>
    for ProductionStationEnginePort<ProductionStationOwner<'state, 'security>>
{
    type Runtime = ProductionStationRuntime<'state>;
    type InitialHardware = RadioRegisters;
    type InitialScanRx =
        Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
    type PreconnectedRx = Esp32s31PreconnectedRx<
        'static,
        EmbassyEsp32s31PreconnectedRxDelay,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
    >;
    type Network = StationNetwork;
    type Disconnected = ConnectedDisconnectedEpoch;
    type Reconnected = ConnectedReconnectedEpoch;
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
            Self::PreconnectedRx,
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
            Self::PreconnectedRx,
            Self::Network,
        >,
        context: StaAttemptContext,
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
        let (runtime, hardware, receive, network, station, security) = phase.into_parts();
        self.run_phase(
            ProductionStationJoinOwner::new(
                runtime,
                ProductionStationJoinPhase::InitialJoin {
                    hardware,
                    receive,
                    network,
                },
                station,
                security,
            ),
            context,
            control,
        )
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
        let (runtime, epoch, network, station, security) = phase.into_parts();
        self.run_phase(
            ProductionStationJoinOwner::new(
                runtime,
                ProductionStationJoinPhase::Reconnected { epoch, network },
                station,
                security,
            ),
            context,
            control,
        )
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

impl ProductionWifiEpochRunner {
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
        let (discovery, security, reconnect, _validated_power) = request.into_parts();
        let (wifi, station_resources, monitor_resources) = stopped.into_parts();
        let previous = self.parked_monitor.replace(Some(monitor_resources));
        debug_assert!(previous.is_none(), "only one Wi-Fi role may be active");
        let security = self.fresh_security(security);
        let owner = match station_resources {
            ProductionStationResources::Fresh(fresh) => {
                let mut materialized = materialize_esp32s31_station(wifi, fresh);
                let fresh = materialized.resources;
                let mut registers = materialized.registers;
                let (phy, _) = materialized.owner.radio_mut();
                let tx_storage = TX_STATE.init_with(|| {
                    TxStorage::from_slot(
                        fresh.tx_slot,
                        phy.tx_target_power_profile(),
                        tx_entropy as fn() -> u32,
                        open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
                        ControlTxConfig {
                            unicast_attempt_limit: 4,
                            completion_timeout_us: TX_COMPLETION_TIMEOUT_US,
                            poll_interval_us: 1,
                        },
                    )
                });
                activate_promiscuous_receive(&mut registers);
                let scan_rx = Esp32s31ScanRx::prepare_initial(
                    &mut registers,
                    fresh.dma.storage(),
                    fresh.dma.descriptor_base(),
                    fresh.dma.buffer_addresses(),
                )
                .unwrap_or_else(|error| panic!("initial RX DMA ring failed: {error:?}"));
                ProductionStationOwner::new(
                    production_station_runtime(
                        materialized.owner,
                        mac_interrupt_epoch(materialized.interrupt_setup),
                        fresh.dma,
                        tx_storage,
                        fresh.scan_table,
                        fresh.scan_frame,
                        fresh.ethernet,
                        fresh.board,
                    ),
                    ProductionStationPhase::InitialScan {
                        hardware: registers,
                        receive: scan_rx,
                        network: fresh.network,
                        identity: Esp32s31StaIdentity {
                            station_address: fresh.station_address,
                            association_preference: discovery.scan().association_preference(),
                        },
                    },
                    security,
                )
            }
            ProductionStationResources::Returned(returned) => {
                let materialized = materialize_esp32s31_station(wifi, returned);
                let ProductionStationReusableResources {
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
        prepare_esp32s31_station_task(
            Esp32s31StationConfig::new(reconnect),
            Esp32s31StationStartResources::new(owner),
            self.station_control,
            ProductionStationRunner::new(
                if scan_only {
                    ProductionStationEnginePort::standalone_scan()
                } else {
                    ProductionStationEnginePort::new()
                },
                discovery,
            ),
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
                    let (controller, task) = match self.prepare_station_task(
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
                    let output = task.run().await;
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
                    let Some(monitor) = self.parked_monitor.borrow_mut().take() else {
                        let faulted = ProductionWifiFault::StationResourceInvariant {
                            _stopped: stopped,
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
                    };
                    let stopped = Esp32s31WifiSupervisorStopped::new(
                        stopped.wifi,
                        ProductionStationResources::Returned(stopped.resources),
                        monitor,
                    );
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
                    let (wifi, station_resources, monitor_resources) = stopped.into_parts();
                    self.monitor_capture.discard_queued();
                    let (mut controller, mut task) = match prepare_esp32s31_monitor_task(
                        monitor_plan,
                        wifi,
                        monitor_resources.bind(generation, snapshot_length),
                    ) {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            let faulted = ProductionWifiFault::MonitorBuild {
                                _failure: failure,
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
                    if let Err(error) = task
                        .switch_channel::<EmbassyEsp32s31PhyDelay, _>(channel, &mut observer)
                        .await
                    {
                        let faulted = ProductionWifiFault::MonitorChannel {
                            _error: error,
                            _task: task,
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
                    let exit = drive_esp32s31_monitor_role(
                        endpoint,
                        &mut controller,
                        task,
                        Esp32s31RadioError::RoleActive,
                    )
                    .await;
                    let frontier = finish_embassy_wifi_active_role(
                        endpoint,
                        generation,
                        exit,
                        |output| match output {
                            Esp32s31MonitorTaskExit::Stopped { stopped, .. } => {
                                self.monitor_capture.discard_queued();
                                EmbassyWifiRoleFrontier::Stopped(
                                    Esp32s31WifiSupervisorStopped::new(
                                        stopped.wifi,
                                        station_resources,
                                        ProductionMonitorResources::from_stopped(stopped.resources),
                                    ),
                                )
                            }
                            Esp32s31MonitorTaskExit::Faulted { task, .. } => {
                                EmbassyWifiRoleFrontier::Faulted(
                                    ProductionWifiFault::MonitorRuntime {
                                        _task: task,
                                        _station: station_resources,
                                    },
                                )
                            }
                        },
                        |_faulted| Esp32s31RadioError::HardwareFault,
                    )
                    .await;
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
                    let station_runner: &ProductionWifiEpochRunner = self;
                    let parked_monitor = &self.parked_monitor;
                    run_esp32s31_station_supervisor_epoch(
                        endpoint,
                        Esp32s31StationSupervisorEpoch::new(stopped, request, generation),
                        |stopped, request| {
                            station_runner.prepare_station_task(stopped, request, false)
                        },
                        Esp32s31StationSupervisorHooks::new(
                            |output| {
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
                                    Ok(stopped) => match parked_monitor.borrow_mut().take() {
                                        Some(monitor) => EmbassyWifiRoleFrontier::Stopped(
                                            Esp32s31WifiSupervisorStopped::new(
                                                stopped.wifi,
                                                ProductionStationResources::Returned(
                                                    stopped.resources,
                                                ),
                                                monitor,
                                            ),
                                        ),
                                        None => EmbassyWifiRoleFrontier::Faulted(
                                            ProductionWifiFault::StationResourceInvariant {
                                                _stopped: stopped,
                                                _runner: runner,
                                            },
                                        ),
                                    },
                                    Err(failure) => EmbassyWifiRoleFrontier::Faulted(
                                        ProductionWifiFault::Reclaim {
                                            _station: failure,
                                            _runner: runner,
                                        },
                                    ),
                                }
                            },
                            || Esp32s31RadioError::UnsupportedPowerPolicy,
                            Esp32s31RadioError::RoleActive,
                            |_faulted: &ProductionWifiFault| Esp32s31RadioError::HardwareFault,
                        ),
                    )
                    .await
                }
            }
        }
    }
}

/// Materialize the public controller, persistent network device and sole
/// owner-holding runner. This function does not start a Wi-Fi role and does
/// not construct an IP stack.
pub async fn new(
    spawner: Spawner,
    platform: EspHalRadioPeripheral,
    trng: Trng,
    config: crate::Esp32s31RadioConfig,
) -> Result<(Esp32s31Radio, Esp32s31RadioRunner), Esp32s31NewError> {
    qualification_event!("open-radio: cold PHY start");

    let workers = match spawn_connected_workers(spawner) {
        Ok(workers) => workers,
        Err(_error) => return Err(Esp32s31NewError::WorkerUnavailable),
    };

    let crate::Esp32s31RadioConfig {
        station_mac,
        access_point_mac,
        calibration,
        initial_channel,
        calibration_record,
        maximum_tx_power_quarter_dbm,
        #[cfg(feature = "qualification")]
        qualification,
    } = config;
    let topology = WifiConfig::station(WifiStationConfig::new(station_mac));

    let owned = Radio::claim(platform).map_err(|_| Esp32s31NewError::RadioAlreadyClaimed)?;
    let mut wifi_start = Esp32s31WifiStartConfig::new(calibration, initial_channel);
    if let Some(maximum) = maximum_tx_power_quarter_dbm {
        wifi_start = wifi_start.with_maximum_tx_power_quarter_dbm(maximum);
    }
    let started = start_esp32s31_radio::<_, EmbassyEsp32s31PhyDelay, _>(
        owned,
        Esp32s31RadioStartConfig::new(topology, wifi_start),
        calibration_record,
        NoopPhyTargetObserver,
    )
    .await
    .map_err(|_| Esp32s31NewError::RadioStart)?;
    let station = started
        .try_into_station()
        .map_err(|_| Esp32s31NewError::StationRole)?;
    let station = station
        .start_mac(MAC_HANDSHAKE_SAMPLE_LIMIT, access_point_mac)
        .map_err(|_| Esp32s31NewError::MacStart)?;
    let station_interface = station.interface();
    let station_address = station_interface.interface.address;
    let (_wifi_plan, wifi) = station.into_parts();
    let initialization = Esp32s31RadioInitialization {
        start: wifi.start_report(),
        transition: wifi.transition_report(),
        calibration_record: wifi.calibration_record().map(|record| *record.bytes()),
    };
    qualification_event!(
        "open-radio: cold PHY ready, full_calibration={}",
        initialization
            .start
            .wifi
            .registration
            .full_calibration_performed
    );

    let memory = match STATION_MEMORY.claim() {
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
    let (network_device, station_network) = initialize_station_network(station_address);
    let monitor = initialize_monitor_resources(monitor_memory)
        .map_err(|MonitorResourcesError::InUse| Esp32s31NewError::MonitorResources)?;
    let stopped = Esp32s31WifiSupervisorStopped::new(
        wifi,
        ProductionStationResources::Fresh(ProductionStationFreshResources {
            dma: Esp32s31StationDmaResources::new(
                monitor_memory.storage(),
                descriptor_base,
                monitor_memory.buffer_addresses(),
            ),
            tx_slot,
            scan_table,
            scan_frame,
            ethernet: initialize_ethernet_frame(),
            network: station_network,
            board: ProductionStationBoardResources {
                interface: station_interface,
                rx_protocol_runtime: initialize_connected_rx_protocol_runtime(),
                initial_connected: Some(initial_connected),
                workers,
                #[cfg(feature = "qualification")]
                qualification,
            },
            station_address,
        }),
        monitor.role,
    );
    let configuration = WifiSupervisorConfiguration::new(
        open_esp_radio::esp32s31::wifi::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    )
    .with_station(WifiStationConfig::new(station_mac))
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
            parked_monitor: RefCell::new(None),
        },
        stopped,
    ) {
        Ok(prepared) => prepared,
        Err(_failure) => return Err(Esp32s31NewError::SupervisorInUse),
    };
    Ok((
        Esp32s31Radio::new(
            Esp32s31Wifi::new(
                controller.into_wifi(),
                network_device,
                monitor.frames,
                #[cfg(feature = "qualification")]
                qualification_snapshot,
            ),
            initialization,
        ),
        Esp32s31RadioRunner { supervisor },
    ))
}
