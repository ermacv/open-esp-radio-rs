//! Static placement, cold start, interrupt dispatch and the runner.

use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, Ordering},
};

use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use oer_esp32s31_bluetooth::{
    ble_phy::ControllerBlePhyEngineInitialized,
    clock::ClockEnableFailure,
    interrupt::{
        InterruptOwnerStorage, PrimaryPublishedInterruptStep, SchedulerWakeCell,
        SchedulerWakePublication,
    },
    low_power::ControllerLowPowerHardwareInitializationFailure,
    modem_lp_timer_queue::{
        ModemLpTimerPublishedInterruptStep, ModemLpTimerWorkerWakeCell,
        ModemLpTimerWorkerWakePublication,
    },
    modem_timer::{ControllerModemTimerTask, ModemLpTimerInterruptDispatchStorage},
    phy::{
        ControllerPhyClientAcquireFailure, ControllerPhyInitializationFailure,
        ControllerPhyTrackingFailure, PhyInitializationConfig,
    },
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};
use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineBindFailure, BlePhyEngineStorage, DirectionFindingWorkspaceBindFailure,
    DirectionFindingWorkspaceStorage, DtmPool, DtmStorage, LeRxChain, LeRxChainBindError,
    LeRxChainStorage, LegacyAdvertisingPool, LegacyAdvertisingStorage,
    LegacyConnectableAdvertisingPool, LegacyConnectableAdvertisingStorage, PassiveScanPool,
    PassiveScanStorage, PeripheralConnectionPool, PeripheralConnectionStorage, RxMemoryListClass,
    SchedulerAllocationConfig, SchedulerPoolBindError, SchedulerRolePoolStorage,
};
use oer_esp32s31_bluetooth_radio::BluetoothRadioMemory;
use oer_esp32s31_bluetooth_runtime::{
    BluetoothInstallError, BluetoothRuntime, BluetoothRuntimeFault, LiveBluetoothHardware,
    ModemTimerFault, run_modem_timer,
};
use oer_esp32s31_hal::bluetooth::BluetoothPrimaryFaultSources;
use oer_esp32s31_phy::{NoopPhyTargetObserver, PhyCalibrationCache, PhyCalibrationSnapshot};
use oer_esp32s31_phy_runtime::{EmbassyPhyTime, EmbassyPhyTimeError};
use oer_esp32s31_radio_esp_hal::{
    BluetoothPlatformBusy, BoundEspHalBluetoothInterruptEpoch, EspHalBluetoothInterruptDisposition,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptSource,
    EspHalBluetoothInterruptStorage, EspHalBluetoothModemLpTimerStorageError,
    EspHalBluetoothNrtInterruptStep, EspHalBluetoothPlatform, EspHalBluetoothPrimaryInterruptStep,
    EspHalRadioPlatform, PublishedEspHalBluetoothInterruptOwners,
};
use static_cell::{ConstStaticCell, StaticCell};

type Platform = EspHalBluetoothPlatform<'static>;

/// Slots of the source-127 software timer queue.
pub const MODEM_TIMER_CAPACITY: usize = 4;
/// Scheduler items the radio role tracks: three per advertising set, three
/// for the scanner, one per connection and one for Direct Test Mode.
pub const ITEMS: usize = 16;
/// Outcomes waiting for the consumer.
pub const EVENTS: usize = 16;
const LEGACY: usize = 1;
const CONNECTABLE: usize = 1;
const SCANNERS: usize = 1;
const CONNECTIONS: usize = 1;
const SCAN_PACKETS: usize = 4;
const RX_PACKETS: usize = 4;
/// Worst-case accuracy of the board's sleep clock. 500 ppm is the largest
/// value the Link Layer defines; the qualified secure GATT peripheral used it.
const LOCAL_SLEEP_CLOCK_PPM: u16 = 500;

/// The radio runtime of this composition.
pub type BluetoothSystemRuntime = BluetoothRuntime<
    CriticalSectionRawMutex,
    LiveBluetoothHardware<'static, BoundEspHalBluetoothInterruptEpoch<'static>>,
    LEGACY,
    CONNECTABLE,
    SCANNERS,
    CONNECTIONS,
    SCAN_PACKETS,
    RX_PACKETS,
    ITEMS,
    EVENTS,
>;

/// The radio memory of this composition.
pub type BluetoothSystemMemory =
    BluetoothRadioMemory<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS>;

type Engine = ControllerBlePhyEngineInitialized<Platform, MODEM_TIMER_CAPACITY>;
type ModemTimerTask = ControllerModemTimerTask<
    'static,
    BoundEspHalBluetoothInterruptEpoch<'static>,
    MODEM_TIMER_CAPACITY,
>;

// Controller memory. The board linker places `.dma.data.*` in internal SRAM,
// initialized from the image; every bind still validates its extent.
macro_rules! controller_memory {
    ($name:ident: $ty:ty = $init:expr, $section:literal) => {
        #[allow(
            unsafe_code,
            reason = "the production linker must retain controller storage in internal SRAM"
        )]
        #[unsafe(link_section = $section)]
        static $name: ConstStaticCell<$ty> = ConstStaticCell::new($init);
    };
}

controller_memory!(BLE_PHY: BlePhyEngineStorage = BlePhyEngineStorage::new(),
    ".dma.data.open_radio_bluetooth_ble_phy");
controller_memory!(DIRECTION_FINDING: DirectionFindingWorkspaceStorage =
    DirectionFindingWorkspaceStorage::new(), ".dma.data.open_radio_bluetooth_direction_finding");
controller_memory!(LEGACY_POOL: SchedulerRolePoolStorage<LegacyAdvertisingStorage, LEGACY> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_legacy_advertising");
controller_memory!(CONNECTABLE_POOL:
    SchedulerRolePoolStorage<LegacyConnectableAdvertisingStorage, CONNECTABLE> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_connectable_advertising");
controller_memory!(SCANNER_POOL: SchedulerRolePoolStorage<PassiveScanStorage, SCANNERS> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_passive_scan");
controller_memory!(CONNECTION_POOL:
    SchedulerRolePoolStorage<PeripheralConnectionStorage, CONNECTIONS> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_peripheral_connection");
controller_memory!(DTM_POOL: SchedulerRolePoolStorage<DtmStorage, 1> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_dtm");
controller_memory!(SCANNING_CHAIN: LeRxChainStorage<SCAN_PACKETS> = LeRxChainStorage::new(),
    ".dma.data.open_radio_bluetooth_scanning_chain");
controller_memory!(NON_SCANNING_CHAIN: LeRxChainStorage<RX_PACKETS> = LeRxChainStorage::new(),
    ".dma.data.open_radio_bluetooth_non_scanning_chain");

static STARTED: AtomicBool = AtomicBool::new(false);
static ENGINE: StaticCell<Engine> = StaticCell::new();
static PUBLISHED: StaticCell<PublishedEspHalBluetoothInterruptOwners> = StaticCell::new();
static BOUND: StaticCell<BoundEspHalBluetoothInterruptEpoch<'static>> = StaticCell::new();
static RUNTIME: BluetoothSystemRuntime = BluetoothRuntime::new();
static MODEM_TIMER_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static INTERRUPT_FAULT: Signal<CriticalSectionRawMutex, BluetoothInterruptFault> = Signal::new();
static DISPATCH: Mutex<Cell<Option<Dispatch>>> = Mutex::new(Cell::new(None));

/// Why cold start stopped. Failures from `Clock` on retain their powered
/// owners, which keep the hardware in its fail-stop state.
#[allow(
    clippy::large_enum_variant,
    reason = "the no-alloc error retains exact affine hardware owners"
)]
pub enum BluetoothStartError {
    /// A cold start already ran in this boot.
    AlreadyStarted(BluetoothRadioHardware),
    /// The Embassy timebase cannot drive PHY timing.
    Timebase(EmbassyPhyTimeError, BluetoothRadioHardware),
    /// The retained calibration snapshot has another schema.
    CalibrationSnapshot(BluetoothRadioHardware),
    /// Another radio holds the platform.
    PlatformBusy(BluetoothPlatformBusy, BluetoothRadioHardware),
    /// Linker placement failed the BLE PHY environment contract.
    BlePhyMemory(BlePhyEngineBindFailure),
    /// Linker placement failed the direction-finding workspace contract.
    DirectionFindingMemory(DirectionFindingWorkspaceBindFailure),
    /// Linker placement failed a role pool.
    PoolMemory(SchedulerPoolBindError),
    /// Linker placement failed a receive chain.
    ChainMemory(LeRxChainBindError),
    /// Clock and reset setup failed and rolled back.
    Clock(ClockEnableFailure<Platform>),
    /// The source-127 low-power hardware refused initialization.
    LowPower(ControllerLowPowerHardwareInitializationFailure<Platform, MODEM_TIMER_CAPACITY>),
    /// Common-PHY registration failed.
    PhyInitialization(ControllerPhyInitializationFailure<Platform, MODEM_TIMER_CAPACITY>),
    /// The registered PHY refused the Bluetooth client.
    PhyClientAcquire(ControllerPhyClientAcquireFailure<Platform, MODEM_TIMER_CAPACITY>),
    /// Initial PHY tracking failed.
    PhyTracking(ControllerPhyTrackingFailure<Platform, MODEM_TIMER_CAPACITY>),
    /// Interrupt storage refused both owners.
    InterruptPublication,
    /// ESP-HAL refused the three CPU routes.
    InterruptRoutes(EspHalBluetoothInterruptRouteError),
    /// The runtime refused the radio.
    Install(BluetoothInstallError),
}

impl core::fmt::Debug for BluetoothStartError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match self {
            Self::AlreadyStarted(_) => "AlreadyStarted",
            Self::Timebase(..) => "Timebase",
            Self::CalibrationSnapshot(_) => "CalibrationSnapshot",
            Self::PlatformBusy(..) => "PlatformBusy",
            Self::BlePhyMemory(_) => "BlePhyMemory",
            Self::DirectionFindingMemory(_) => "DirectionFindingMemory",
            Self::PoolMemory(_) => "PoolMemory",
            Self::ChainMemory(_) => "ChainMemory",
            Self::Clock(_) => "Clock",
            Self::LowPower(_) => "LowPower",
            Self::PhyInitialization(_) => "PhyInitialization",
            Self::PhyClientAcquire(_) => "PhyClientAcquire",
            Self::PhyTracking(_) => "PhyTracking",
            Self::InterruptPublication => "InterruptPublication",
            Self::InterruptRoutes(_) => "InterruptRoutes",
            Self::Install(_) => "Install",
        };
        formatter.write_str(name)
    }
}

/// First fatal interrupt-service result of the live epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInterruptFault {
    /// Source 124 found the shared owner missing from storage.
    PrimaryUnavailable,
    /// Source 124 classified a Controller fault from these sources.
    Primary(BluetoothPrimaryFaultSources),
    /// Source 127 could not service its owner.
    ModemLpTimer(EspHalBluetoothModemLpTimerStorageError),
    /// Source 133 found the shared owner missing from storage.
    NrtUnavailable,
}

/// Why the runner stopped.
#[derive(Debug)]
pub enum BluetoothRunnerFault {
    /// The radio runtime stopped on a hardware fault.
    Radio(
        BluetoothRuntimeFault<
            oer_esp32s31_radio_esp_hal::EspHalBluetoothSchedulerRunInterruptError,
        >,
    ),
    /// The source-127 task could not exchange its owner with storage.
    ModemTimer(
        ModemTimerFault<
            EspHalBluetoothModemLpTimerStorageError,
            EspHalBluetoothModemLpTimerStorageError,
        >,
    ),
    /// An interrupt service failed.
    Interrupt(BluetoothInterruptFault),
}

/// The installed radio and its runner.
pub struct BluetoothSystem {
    /// Admits requests and yields outcomes.
    pub runtime: &'static BluetoothSystemRuntime,
    /// Spawn [`BluetoothRunner::run`] on one task for the whole epoch.
    pub runner: BluetoothRunner,
    /// How the common PHY was obtained.
    pub phy: oer_esp32s31_bluetooth::common_phy_state::ControllerPhyEntry,
    /// Calibration result of this epoch, for the next cold start.
    pub calibration: Option<PhyCalibrationSnapshot>,
}

/// Drives the radio runtime and the source-127 timer task.
pub struct BluetoothRunner {
    modem_timer: ModemTimerTask,
    _platform: oer_esp32s31_bluetooth::resources::platform_retirement::ControllerPlatformLease<
        'static,
        Platform,
    >,
    _physical: oer_esp32s31_bluetooth::resources::runtime_owner::RuntimeOwnerLease<
        'static,
        oer_esp32s31_bluetooth::ble_phy::BlePhyRetainedOwners,
    >,
}

impl BluetoothRunner {
    /// Run until the radio, the timer task or an interrupt service faults.
    pub async fn run(mut self) -> BluetoothRunnerFault {
        let workers = select(
            RUNTIME.run(),
            run_modem_timer(&mut self.modem_timer, &MODEM_TIMER_WAKE),
        );
        match select(workers, INTERRUPT_FAULT.wait()).await {
            Either::First(Either::First(fault)) => BluetoothRunnerFault::Radio(fault),
            Either::First(Either::Second(fault)) => BluetoothRunnerFault::ModemTimer(fault),
            Either::Second(fault) => BluetoothRunnerFault::Interrupt(fault),
        }
    }
}

#[derive(Clone, Copy)]
struct Dispatch {
    published: &'static PublishedEspHalBluetoothInterruptOwners,
    scheduler_wake: &'static SchedulerWakeCell,
    modem_timer_wake: &'static ModemLpTimerWorkerWakeCell,
}

fn fault(fault: BluetoothInterruptFault) -> EspHalBluetoothInterruptDisposition {
    INTERRUPT_FAULT.signal(fault);
    EspHalBluetoothInterruptDisposition::Quarantine
}

fn dispatch(source: EspHalBluetoothInterruptSource) -> EspHalBluetoothInterruptDisposition {
    let Some(dispatch) = critical_section::with(|cs| DISPATCH.borrow(cs).get()) else {
        return EspHalBluetoothInterruptDisposition::Quarantine;
    };
    match source {
        EspHalBluetoothInterruptSource::Primary => {
            match dispatch.published.service_primary_interrupt() {
                EspHalBluetoothPrimaryInterruptStep::Unavailable => {
                    fault(BluetoothInterruptFault::PrimaryUnavailable)
                }
                EspHalBluetoothPrimaryInterruptStep::Serviced(step) => {
                    match step.publish(dispatch.scheduler_wake) {
                        PrimaryPublishedInterruptStep::Fault(controller) => {
                            fault(BluetoothInterruptFault::Primary(controller.sources()))
                        }
                        PrimaryPublishedInterruptStep::Scheduler {
                            scheduler: SchedulerWakePublication::WakeWorker,
                            ..
                        } => {
                            RUNTIME.on_scheduler_wake();
                            EspHalBluetoothInterruptDisposition::Serviced
                        }
                        PrimaryPublishedInterruptStep::Scheduler { .. }
                        | PrimaryPublishedInterruptStep::NoSchedulerWork(_) => {
                            EspHalBluetoothInterruptDisposition::Serviced
                        }
                    }
                }
            }
        }
        EspHalBluetoothInterruptSource::ModemLpTimer => {
            match ModemLpTimerInterruptDispatchStorage::service_modem_lp_timer_interrupt(
                dispatch.published,
            ) {
                Ok(step) => {
                    if let ModemLpTimerPublishedInterruptStep::AwaitingSoftware(
                        ModemLpTimerWorkerWakePublication::WakeWorker,
                    )
                    | ModemLpTimerPublishedInterruptStep::SoftwarePending(
                        ModemLpTimerWorkerWakePublication::WakeWorker,
                    ) = step.publish(dispatch.modem_timer_wake)
                    {
                        MODEM_TIMER_WAKE.signal(());
                    }
                    EspHalBluetoothInterruptDisposition::Serviced
                }
                Err(error) => fault(BluetoothInterruptFault::ModemLpTimer(error)),
            }
        }
        EspHalBluetoothInterruptSource::NrtDefault => {
            match dispatch.published.service_nrt_default_interrupt() {
                EspHalBluetoothNrtInterruptStep::Serviced(_) => {
                    EspHalBluetoothInterruptDisposition::Serviced
                }
                EspHalBluetoothNrtInterruptStep::Unavailable => {
                    fault(BluetoothInterruptFault::NrtUnavailable)
                }
            }
        }
    }
}

/// Claimed controller memory, bound at its linked addresses.
struct ClaimedMemory {
    ble_phy: oer_esp32s31_bluetooth_memory::BlePhyEngineCpuOwned,
    direction_finding: oer_esp32s31_bluetooth_memory::DirectionFindingWorkspaceCpuOwned,
    legacy: LegacyAdvertisingPool<LEGACY>,
    connectable: LegacyConnectableAdvertisingPool<CONNECTABLE>,
    scanners: PassiveScanPool<SCANNERS>,
    connections: PeripheralConnectionPool<CONNECTIONS>,
    dtm: DtmPool<1>,
    scanning: LeRxChain<SCAN_PACKETS>,
    non_scanning: LeRxChain<RX_PACKETS>,
}

#[allow(
    clippy::result_large_err,
    reason = "the no-alloc error retains exact affine hardware owners"
)]
fn claim_memory() -> Result<ClaimedMemory, BluetoothStartError> {
    let numbers =
        SchedulerAllocationConfig::new((LEGACY + CONNECTABLE) as u16, CONNECTIONS as u16, 0)
            .expect("the product profile numbers fit their field");
    let pool = BluetoothStartError::PoolMemory;
    Ok(ClaimedMemory {
        ble_phy: BlePhyEngineStorage::pin_static(BLE_PHY.take())
            .map_err(BluetoothStartError::BlePhyMemory)?,
        direction_finding: DirectionFindingWorkspaceStorage::pin_static(DIRECTION_FINDING.take())
            .map_err(BluetoothStartError::DirectionFindingMemory)?,
        legacy: LegacyAdvertisingPool::bind(
            LEGACY_POOL.take(),
            numbers
                .advertising(0, LEGACY as u16)
                .expect("within the profile"),
        )
        .map_err(pool)?,
        connectable: LegacyConnectableAdvertisingPool::bind(
            CONNECTABLE_POOL.take(),
            numbers
                .advertising(LEGACY as u16, CONNECTABLE as u16)
                .expect("within the profile"),
        )
        .map_err(pool)?,
        scanners: PassiveScanPool::bind(SCANNER_POOL.take(), numbers.scanning()).map_err(pool)?,
        connections: PeripheralConnectionPool::bind(CONNECTION_POOL.take(), numbers.connections())
            .map_err(pool)?,
        dtm: DtmPool::bind(DTM_POOL.take(), numbers.direct_test_mode()).map_err(pool)?,
        scanning: LeRxChain::bind(SCANNING_CHAIN.take(), RxMemoryListClass::Scanning)
            .map_err(BluetoothStartError::ChainMemory)?,
        non_scanning: LeRxChain::bind(NON_SCANNING_CHAIN.take(), RxMemoryListClass::NonScanning)
            .map_err(BluetoothStartError::ChainMemory)?,
    })
}

/// Cold-start the Bluetooth radio once per boot and install its runtime.
///
/// `retained_calibration` is the snapshot of an earlier epoch, if any. Once
/// the first Controller write happens the operation is not cancellable.
#[allow(
    clippy::result_large_err,
    reason = "the no-alloc error retains exact affine hardware owners"
)]
#[allow(
    large_assignments,
    reason = "the powered owner graph crosses the PHY poll boundaries once; the linked-image stack-frame audit independently bounds this future"
)]
pub async fn start_esp32s31_bluetooth(
    platform_root: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
    retained_calibration: Option<PhyCalibrationSnapshot>,
) -> Result<BluetoothSystem, BluetoothStartError> {
    if STARTED.swap(true, Ordering::AcqRel) {
        return Err(BluetoothStartError::AlreadyStarted(hardware));
    }
    if let Err(error) = EmbassyPhyTime::validate_timebase() {
        return Err(BluetoothStartError::Timebase(error, hardware));
    }
    let calibration_cache = match retained_calibration {
        Some(snapshot) => match PhyCalibrationCache::from_snapshot(snapshot) {
            Some(cache) => Some(cache),
            None => return Err(BluetoothStartError::CalibrationSnapshot(hardware)),
        },
        None => None,
    };
    let platform = match platform_root.try_bluetooth() {
        Ok(platform) => platform,
        Err(error) => return Err(BluetoothStartError::PlatformBusy(error, hardware)),
    };
    let calibration_identity = platform.phy_calibration_identity();
    let public_address = platform.bluetooth_public_address();
    let memory = claim_memory()?;

    let clocked = BluetoothStopped::from_hardware(platform, hardware)
        .enable_clocks()
        .map_err(BluetoothStartError::Clock)?;
    let low_power = clocked
        .initialize_controller_hal()
        .initialize_scheduler(ControllerRuntimeResources::<MODEM_TIMER_CAPACITY>::new())
        .initialize_modem_lp_timer_hardware()
        .map_err(BluetoothStartError::LowPower)?;
    let mut phy_config = PhyInitializationConfig::new(calibration_identity);
    if let Some(cache) = calibration_cache {
        phy_config = phy_config.with_calibration_cache(cache);
    }
    let registered = match low_power
        .initialize_common_phy::<EmbassyPhyTime, NoopPhyTargetObserver>(
            phy_config,
            NoopPhyTargetObserver,
        )
        .await
    {
        Ok(registered) => registered,
        Err(failure) => return Err(BluetoothStartError::PhyInitialization(failure)),
    };
    let acquisition = match registered.acquire_phy_client(&mut EmbassyPhyTime) {
        Ok(acquisition) => acquisition,
        Err(failure) => return Err(BluetoothStartError::PhyClientAcquire(failure)),
    };
    let initialized = match acquisition.into_owner() {
        Ok(initialized) => initialized,
        Err(pending) => match pending
            .begin_tracking()
            .complete_tracking::<EmbassyPhyTime, NoopPhyTargetObserver>(NoopPhyTargetObserver)
            .await
        {
            Ok(initialized) => initialized,
            Err(failure) => return Err(BluetoothStartError::PhyTracking(failure)),
        },
    };
    let phy = initialized.phy_entry();
    let calibration = initialized
        .calibration_cache()
        .map(|cache| *cache.snapshot());
    let engine = ENGINE.init(initialized.initialize_baseband().initialize_ble_phy_engine(
        memory.ble_phy,
        memory.direction_finding,
        public_address,
    ));

    let (output, timer) = engine.take_activation_owners_with_output_prepared();
    let published = match EspHalBluetoothInterruptStorage::new().publish(
        output.stage_for_cpu_routes(),
        timer.start_runtime_timer().stage_for_interrupt(),
    ) {
        Ok(published) => PUBLISHED.init(published),
        Err(_) => return Err(BluetoothStartError::InterruptPublication),
    };
    let split = engine
        .split_runtime()
        .expect("the one-shot epoch splits once");
    let endpoints = split.endpoints;
    critical_section::with(|cs| {
        DISPATCH.borrow(cs).set(Some(Dispatch {
            published,
            scheduler_wake: endpoints.interrupt.scheduler_wake(),
            modem_timer_wake: endpoints.interrupt.modem_lp_timer_worker_wake(),
        }));
    });
    let bound = BOUND.init(
        published
            .bind_routes(dispatch)
            .map_err(BluetoothStartError::InterruptRoutes)?,
    );

    let radio_memory = BluetoothSystemMemory {
        legacy: memory.legacy,
        connectable: memory.connectable,
        scanners: memory.scanners,
        connections: memory.connections,
        dtm: memory.dtm,
        scanning: memory.scanning,
        non_scanning: memory.non_scanning,
        direction_finding: split.direction_finding,
    };
    if let Err((error, _, _)) = RUNTIME
        .install(
            radio_memory,
            LiveBluetoothHardware::new(endpoints.task, bound, LOCAL_SLEEP_CLOCK_PPM),
        )
        .await
    {
        return Err(BluetoothStartError::Install(error));
    }
    Ok(BluetoothSystem {
        runtime: &RUNTIME,
        runner: BluetoothRunner {
            modem_timer: ControllerModemTimerTask::new(bound, endpoints.modem_timer),
            _platform: endpoints.platform,
            _physical: split.physical,
        },
        phy,
        calibration,
    })
}
