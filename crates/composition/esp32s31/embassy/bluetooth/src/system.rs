//! Static placement, start and stop on the shared radio, interrupt dispatch
//! and the runner.

use core::cell::Cell;

use critical_section::Mutex;
use embassy_futures::select::{Either, Either4, select, select4};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Instant;
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
use oer_bluetooth_radio::RadioActivity;
use oer_esp32s31_bluetooth::{
    ble_phy::{BlePhyRetainedOwners, BlePhyRuntime},
    controller_hal::ControllerHalInitialized,
    interrupt::{
        InterruptOwnerRestartStorage, InterruptOwnerStorage, PrimaryPublishedInterruptStep,
        SchedulerWakeCell, SchedulerWakePublication,
    },
    low_power::{ControllerLowPowerHardwareInitializationFailure, ControllerRuntimeEndpoints},
    modem_lp_timer_queue::{
        ModemLpTimerPublishedInterruptStep, ModemLpTimerWorkerWakeCell,
        ModemLpTimerWorkerWakePublication,
    },
    modem_timer::{ControllerModemTimerTask, ModemLpTimerInterruptDispatchStorage},
    phy::{ControllerPhyJoinFailure, ControllerPhyJoined},
    runtime_resources::{ControllerEventCells, ControllerRuntimeResources},
    shutdown::{
        ControllerRetired, ControllerShutDown, ControllerShutdownError, ControllerShutdownFailure,
        retire,
    },
};
use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineBindFailure, BlePhyEngineCpuOwned, BlePhyEngineStorage,
    DirectionFindingWorkspaceBindFailure, DirectionFindingWorkspaceCpuOwned,
    DirectionFindingWorkspaceStorage, DtmPool, DtmStorage, LeDeviceTable, LeDeviceTableBindError,
    LeDeviceTableStorage, LeRxChain, LeRxChainBindError, LeRxChainStorage, LegacyAdvertisingPool,
    LegacyAdvertisingStorage, LegacyConnectableAdvertisingPool,
    LegacyConnectableAdvertisingStorage, LegacyScanPool, LegacyScanStorage,
    PeripheralConnectionPool, PeripheralConnectionStorage, RxMemoryListClass,
    SchedulerAllocationConfig, SchedulerPoolBindError, SchedulerRolePoolStorage,
};
use oer_esp32s31_bluetooth_radio::{BluetoothRadio, BluetoothRadioMemory};
use oer_esp32s31_bluetooth_runtime::{
    BluetoothInstallError, BluetoothRuntime, BluetoothRuntimeFault, LiveBluetoothHardware,
    ModemTimerFault, run_modem_timer, settle_modem_timer,
};
use oer_esp32s31_coex::{CoexError, CoexStatusType};
use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothClockError, BluetoothPrimaryFaultSources, ClockedOwner, ColdOwner,
        InterruptRegistersOwner, ModemLpTimerInterruptReadyOwner, PoweredOwner,
    },
    root::BluetoothPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
};
use oer_esp32s31_phy::RomShortDelay;
use oer_esp32s31_phy::{
    ConcurrentPhyTrackingError, ConcurrentRfError, NoopPhyTargetObserver,
    bluetooth_client::BluetoothPhyClientError,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError},
    maintain_concurrent_phy,
};
use oer_esp32s31_radio_esp_hal::{
    BoundEspHalBluetoothInterruptEpoch, EspHalBluetoothInterruptDisposition,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptRoutes,
    EspHalBluetoothInterruptSource, EspHalBluetoothInterruptStorage,
    EspHalBluetoothInterruptStorageError, EspHalBluetoothModemLpTimerStorageError,
    EspHalBluetoothNrtInterruptStep, EspHalBluetoothPrimaryInterruptStep,
    EspHalBluetoothSchedulerRunInterruptError, PublishedEspHalBluetoothInterruptOwners,
};
use oer_time_embassy::EmbassyClock;

use crate::coex::BleCoexStatusChange;

/// Every Bluetooth LE schedule status bit, withdrawn when the Controller
/// stops as the status hook's task-disable index does.
const BLE_COEX_STATUS_ALL: u16 = 0xffff;
use oer_esp32s31_radio_runtime::{RadioGuard, RadioPhyError, RadioSystem};
use static_cell::{ConstStaticCell, StaticCell};

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
/// Window the joining Controller grants shared PHY tracking. No scheduler
/// work can start before the BLE PHY engine is initialized, so the bound
/// only limits how long tracking may hold the shared PHY.
const TRACKING_WINDOW_MICROS: u64 = 1_000_000;

type Storage = PublishedEspHalBluetoothInterruptOwners;

/// The radio runtime of this composition.
pub type BluetoothSystemRuntime = BluetoothRuntime<
    CriticalSectionRawMutex,
    LiveBluetoothHardware<'static, Storage>,
    EmbassyClock,
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

type Radio =
    BluetoothRadio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>;
type ModemTimerTask = ControllerModemTimerTask<'static, Storage, MODEM_TIMER_CAPACITY>;

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
controller_memory!(SCANNER_POOL: SchedulerRolePoolStorage<LegacyScanStorage, SCANNERS> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_passive_scan");
controller_memory!(CONNECTION_POOL:
    SchedulerRolePoolStorage<PeripheralConnectionStorage, CONNECTIONS> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_peripheral_connection");
controller_memory!(DTM_POOL: SchedulerRolePoolStorage<DtmStorage, 1> =
    SchedulerRolePoolStorage::new(), ".dma.data.open_radio_bluetooth_dtm");
controller_memory!(DEVICE_TABLE: LeDeviceTableStorage = LeDeviceTableStorage::new(),
    ".dma.data.open_radio_bluetooth_device_table");
controller_memory!(SCANNING_CHAIN: LeRxChainStorage<SCAN_PACKETS> = LeRxChainStorage::new(),
    ".dma.data.open_radio_bluetooth_scanning_chain");
controller_memory!(NON_SCANNING_CHAIN: LeRxChainStorage<RX_PACKETS> = LeRxChainStorage::new(),
    ".dma.data.open_radio_bluetooth_non_scanning_chain");

static CELLS: ControllerEventCells = ControllerEventCells::new();
static PUBLISHED: StaticCell<Storage> = StaticCell::new();
static RUNTIME: BluetoothSystemRuntime = BluetoothRuntime::new(EmbassyClock);
static MODEM_TIMER_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static INTERRUPT_FAULT: Signal<CriticalSectionRawMutex, BluetoothInterruptFault> = Signal::new();
static DISPATCH: Mutex<Cell<Option<Dispatch>>> = Mutex::new(Cell::new(None));

/// Why the static controller memory could not be claimed.
#[derive(Debug)]
pub enum BluetoothMemoryError {
    /// The memory was claimed before in this boot.
    AlreadyClaimed,
    /// Linker placement failed the BLE PHY environment contract.
    BlePhy(BlePhyEngineBindFailure),
    /// Linker placement failed the direction-finding workspace contract.
    DirectionFinding(DirectionFindingWorkspaceBindFailure),
    /// Linker placement failed a role pool.
    Pool(SchedulerPoolBindError),
    /// Linker placement failed a receive chain.
    Chain(LeRxChainBindError),
    /// Linker placement failed the device table.
    DeviceTable(LeDeviceTableBindError),
}

/// The controller memory of the composition, reused by every epoch.
struct ControllerMemory {
    ble_phy: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceCpuOwned,
    radio: BluetoothSystemMemory,
}

/// The Bluetooth partition and the controller memory while the Controller
/// is stopped.
#[must_use = "the parked Bluetooth client retains its partition and memory"]
pub struct BluetoothParked {
    partition: BluetoothPartition,
    memory: ControllerMemory,
    /// Interrupt-owner storage published by an earlier epoch of this boot.
    published: Option<&'static Storage>,
}

impl BluetoothParked {
    /// Claim the static controller memory once per boot and park it with
    /// `partition`; keep `interrupts`, the routes of the Controller's sources
    /// in the image's interrupt table, for every epoch of the boot.
    ///
    /// # Errors
    ///
    /// The memory was claimed before, or the linker placement fails its
    /// contract.
    pub fn new(
        partition: BluetoothPartition,
        interrupts: EspHalBluetoothInterruptRoutes,
    ) -> Result<Self, BluetoothMemoryError> {
        let numbers =
            SchedulerAllocationConfig::new((LEGACY + CONNECTABLE) as u16, CONNECTIONS as u16, 0)
                .expect("the product profile numbers fit their field");
        let pool = BluetoothMemoryError::Pool;
        fn claim<T>(cell: Option<T>) -> Result<T, BluetoothMemoryError> {
            cell.ok_or(BluetoothMemoryError::AlreadyClaimed)
        }
        let ble_phy = BlePhyEngineStorage::pin_static(claim(BLE_PHY.try_take())?)
            .map_err(BluetoothMemoryError::BlePhy)?;
        let direction_finding =
            DirectionFindingWorkspaceStorage::pin_static(claim(DIRECTION_FINDING.try_take())?)
                .map_err(BluetoothMemoryError::DirectionFinding)?;
        let radio = BluetoothSystemMemory {
            legacy: LegacyAdvertisingPool::bind(
                claim(LEGACY_POOL.try_take())?,
                numbers
                    .advertising(0, LEGACY as u16)
                    .expect("within the profile"),
            )
            .map_err(pool)?,
            connectable: LegacyConnectableAdvertisingPool::bind(
                claim(CONNECTABLE_POOL.try_take())?,
                numbers
                    .advertising(LEGACY as u16, CONNECTABLE as u16)
                    .expect("within the profile"),
            )
            .map_err(pool)?,
            scanners: LegacyScanPool::bind(claim(SCANNER_POOL.try_take())?, numbers.scanning())
                .map_err(pool)?,
            connections: PeripheralConnectionPool::bind(
                claim(CONNECTION_POOL.try_take())?,
                numbers.connections(),
            )
            .map_err(pool)?,
            dtm: DtmPool::bind(claim(DTM_POOL.try_take())?, numbers.direct_test_mode())
                .map_err(pool)?,
            scanning: LeRxChain::bind(
                claim(SCANNING_CHAIN.try_take())?,
                RxMemoryListClass::Scanning,
            )
            .map_err(BluetoothMemoryError::Chain)?,
            non_scanning: LeRxChain::bind(
                claim(NON_SCANNING_CHAIN.try_take())?,
                RxMemoryListClass::NonScanning,
            )
            .map_err(BluetoothMemoryError::Chain)?,
            direction_finding: direction_finding.binding().link(),
            device_table: LeDeviceTable::bind(claim(DEVICE_TABLE.try_take())?)
                .map_err(BluetoothMemoryError::DeviceTable)?,
        };
        // The memory above is claimed once per boot, so are the routes.
        if EspHalBluetoothInterruptStorage::new()
            .install_routes(interrupts)
            .is_err()
        {
            unreachable!("the boot claimed the Controller memory, and with it the routes, once");
        }
        Ok(Self {
            partition,
            memory: ControllerMemory {
                ble_phy,
                direction_finding,
                radio,
            },
            published: None,
        })
    }

    /// Return the partition, giving up the controller memory for this boot.
    pub fn into_partition(self) -> BluetoothPartition {
        self.partition
    }
}

/// Why bring-up stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothStartError {
    /// Common radio power could not be entered.
    Power(CommonRadioPowerError),
    /// The Bluetooth clocks, resets or low-power clock could not be set up.
    Clocks(BluetoothClockError),
    /// The source-127 low-power hardware refused initialization.
    LowPower,
    /// The shared PHY domain could not be registered or woken.
    Phy(RadioPhyError),
    /// Bluetooth could not join the shared PHY domain or BTBB.
    PhyClient(BluetoothPhyClientError),
    /// Tracking due at the join was rejected before it started.
    Tracking(ConcurrentPhyError),
    /// Tracking due at the join failed after it started.
    TrackingFailed,
    /// Interrupt storage refused both owners.
    InterruptPublication(EspHalBluetoothInterruptStorageError),
    /// ESP-HAL refused the three CPU routes.
    InterruptRoutes(EspHalBluetoothInterruptRouteError),
    /// The runtime refused the radio.
    Install(BluetoothInstallError),
}

/// Owners retained after a failure that may have left hardware in an
/// unknown state. They are never returned; the chip must be reset.
#[must_use = "a fail-stop owner keeps the radio until the chip resets"]
pub struct BluetoothFailStop {
    _owner: FailStopOwner,
}

type Controller = oer_esp32s31_bluetooth::low_power::ControllerLowPowerHardwareInitialized<
    'static,
    MODEM_TIMER_CAPACITY,
>;
type TaskRuntime = oer_esp32s31_bluetooth::runtime_resources::ControllerPoweredTaskRuntime<'static>;

#[allow(
    dead_code,
    clippy::large_enum_variant,
    reason = "the retained owners are never read or moved again"
)]
enum FailStopOwner {
    Powered(PoweredOwner, ControllerMemory),
    Clocked(ClockedOwner, ControllerMemory),
    LowPower(
        ControllerLowPowerHardwareInitializationFailure<'static, MODEM_TIMER_CAPACITY>,
        ControllerMemory,
    ),
    Controller(Controller, ControllerMemory),
    PhyJoin(
        ControllerPhyJoinFailure<'static, MODEM_TIMER_CAPACITY>,
        ControllerMemory,
    ),
    Joined(
        ControllerPhyJoined<'static, MODEM_TIMER_CAPACITY>,
        ControllerMemory,
    ),
    Published {
        task: TaskRuntime,
        modem_timer: ModemTimerTask,
        retained: BlePhyRetainedOwners,
        bound: Option<BoundEspHalBluetoothInterruptEpoch<'static>>,
        memory: BluetoothSystemMemory,
    },
    Unpublished {
        endpoints: ControllerRuntimeEndpoints<'static, MODEM_TIMER_CAPACITY>,
        retained: BlePhyRetainedOwners,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
        memory: BluetoothSystemMemory,
    },
    Installed(Epoch),
    TimerRetired {
        role: Radio,
        task: TaskRuntime,
        timer: ModemLpTimerInterruptReadyOwner,
        retained: BlePhyRetainedOwners,
    },
    Uninstalled {
        role: Radio,
        task: TaskRuntime,
        modem_timer: ModemTimerTask,
        bound: Option<BoundEspHalBluetoothInterruptEpoch<'static>>,
        retained: BlePhyRetainedOwners,
    },
    Shutdown(Radio, ControllerShutdownFailure),
    Retired(Radio, ControllerRetired),
}

fn fail_stop(error: BluetoothStartError, owner: FailStopOwner) -> BluetoothStartFailure {
    BluetoothStartFailure {
        error,
        owner: Err(BluetoothFailStop { _owner: owner }),
    }
}

fn stop_fail(error: BluetoothStopError, owner: FailStopOwner) -> BluetoothStopFailure {
    BluetoothStopFailure {
        error,
        owner: BluetoothFailStop { _owner: owner },
    }
}

/// Failed bring-up.
#[must_use = "a failed start still owns the partition or keeps it fail-stop"]
pub struct BluetoothStartFailure {
    /// The first error.
    pub error: BluetoothStartError,
    /// The parked client when every earlier step was rolled back, or the
    /// fail-stop owner otherwise.
    pub owner: Result<BluetoothParked, BluetoothFailStop>,
}

/// Why teardown stopped.
#[derive(Debug)]
pub enum BluetoothStopError {
    /// The runtime could not stop the scheduler.
    Runtime(BluetoothRuntimeFault<EspHalBluetoothSchedulerRunInterruptError>),
    /// The source-127 task could not return its owner to storage.
    ModemTimer(
        ModemTimerFault<
            EspHalBluetoothModemLpTimerStorageError,
            EspHalBluetoothModemLpTimerStorageError,
        >,
    ),
    /// The CPU routes must be removed on their binding core.
    Routes(EspHalBluetoothInterruptRouteError),
    /// Stable storage did not return the source-127 timer owner.
    TimerStorage,
    /// Stable storage did not return the controller interrupt owner.
    InterruptStorage,
    /// The Controller shutdown failed.
    Shutdown(ControllerShutdownError),
    /// Coexistence could not be disabled for Bluetooth.
    Coex(CoexError),
    /// Shared RF could not be closed after the last PHY client left.
    PhyClose(ConcurrentRfError),
    /// The Bluetooth clocks could not be released.
    Clocks(BluetoothClockError),
    /// Common radio power could not be left.
    Power(CommonRadioPowerError),
}

/// Failed teardown. The Controller cannot run again in this boot.
#[must_use = "a failed stop keeps the radio fail-stop"]
pub struct BluetoothStopFailure {
    /// The first error.
    pub error: BluetoothStopError,
    /// The fail-stop owner.
    pub owner: BluetoothFailStop,
}

/// First fatal interrupt-service result of the live epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInterruptFault {
    /// Source 124 found the shared owner missing from storage.
    PrimaryUnavailable,
    /// Source 124 classified a Controller fault from these sources.
    Primary(BluetoothPrimaryFaultSources),
    /// Source 124's scheduler BUSY sample never settled within the
    /// diagnostic attempt budget.
    PrimaryDiagnosticUnsettled,
    /// Source 127 could not service its owner.
    ModemLpTimer(EspHalBluetoothModemLpTimerStorageError),
    /// Source 133 found the shared owner missing from storage.
    NrtUnavailable,
}

/// Why the runner stopped.
#[derive(Debug)]
pub enum BluetoothRunnerFault {
    /// The radio runtime stopped on a hardware fault.
    Radio(BluetoothRuntimeFault<EspHalBluetoothSchedulerRunInterruptError>),
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

/// The owners of one running epoch outside the runtime.
struct Epoch {
    modem_timer: ModemTimerTask,
    bound: BoundEspHalBluetoothInterruptEpoch<'static>,
    retained: BlePhyRetainedOwners,
    published: &'static Storage,
}

/// The running Bluetooth client.
///
/// The radio role and the task-side Controller live in the runtime; this
/// value keeps the PHY membership, the BLE PHY graph, the source-127 task and
/// the bound CPU routes until [`Self::stop`].
#[must_use = "the running Bluetooth client must be stopped"]
// CAPABILITY: bluetooth-powered-shutdown, bluetooth-always-awake-controller, bluetooth-internal-wi-fi-bluetooth-coexistence, bluetooth-wi-fi-bluetooth-ieee-802-15-4-radio-coordination, coex-protocol-integration-and-lifetime-live-bt-ble-requests
pub struct BluetoothSystem {
    epoch: Epoch,
}

impl BluetoothSystem {
    /// The radio runtime: admits requests and yields outcomes.
    pub fn runtime(&self) -> &'static BluetoothSystemRuntime {
        &RUNTIME
    }

    /// Drive the radio runtime and the source-127 timer task until a fault
    /// stops either or an interrupt service fails, and publish the active
    /// roles to the coexistence schedule of `radio`. While another radio
    /// shares the antenna, the events carry the vendor's coexistence
    /// priorities.
    ///
    /// Cancel it before [`Self::stop`]; the stop settles whatever work the
    /// cancelled future left and withdraws every Bluetooth LE status bit.
    pub async fn run<P, C: PlatformClockProvider>(
        &mut self,
        radio: &RadioSystem<P, C, EmbassyClock>,
    ) -> BluetoothRunnerFault {
        let workers = select(
            RUNTIME.run(),
            run_modem_timer(
                &mut self.epoch.modem_timer,
                &MODEM_TIMER_WAKE,
                &EmbassyClock,
            ),
        );
        match select4(
            workers,
            INTERRUPT_FAULT.wait(),
            publish_coex_status(radio),
            follow_coex_sharing(radio),
        )
        .await
        {
            Either4::First(Either::First(fault)) => BluetoothRunnerFault::Radio(fault),
            Either4::First(Either::Second(fault)) => BluetoothRunnerFault::ModemTimer(fault),
            Either4::Second(fault) => BluetoothRunnerFault::Interrupt(fault),
            Either4::Third(never) | Either4::Fourth(never) => match never {},
        }
    }

    /// Stop the Controller and leave the shared radio.
    ///
    /// The steps reverse [`start`]: stop the scheduler and take the radio
    /// out of the runtime, settle the source-127 task, remove the CPU routes
    /// and recover both interrupt owners, release the Controller output and
    /// leave the shared PHY domain and BTBB (the radio system closes RF after
    /// the last PHY client), reset the Controller, release the clocks and
    /// leave common power. The controller memory returns to its
    /// allocation-time image for the next [`start`].
    ///
    /// # Errors
    ///
    /// A step failed; the Controller cannot run again in this boot.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains every owner of the epoch"
    )]
    // CAPABILITY: bluetooth-idle-powered-release
    pub async fn stop<P, C: PlatformClockProvider>(
        self,
        radio: &RadioSystem<P, C, EmbassyClock>,
    ) -> Result<BluetoothParked, BluetoothStopFailure> {
        let epoch = self.epoch;
        let (role, hardware) = match RUNTIME.uninstall().await {
            Ok(parts) => parts,
            Err(error) => {
                return Err(stop_fail(
                    BluetoothStopError::Runtime(error),
                    FailStopOwner::Installed(epoch),
                ));
            }
        };
        let Epoch {
            mut modem_timer,
            bound,
            retained,
            published,
        } = epoch;
        let task = hardware.into_task();
        let uninstalled = |role, task, modem_timer, bound, retained| FailStopOwner::Uninstalled {
            role,
            task,
            modem_timer,
            bound,
            retained,
        };
        if let Err(error) = settle_modem_timer(&mut modem_timer, &EmbassyClock).await {
            return Err(stop_fail(
                BluetoothStopError::ModemTimer(error),
                uninstalled(role, task, modem_timer, Some(bound), retained),
            ));
        }
        if let Err(failure) = bound.disable() {
            let (error, bound) = failure.into_parts();
            return Err(stop_fail(
                BluetoothStopError::Routes(error),
                uninstalled(role, task, modem_timer, Some(bound), retained),
            ));
        }
        critical_section::with(|cs| DISPATCH.borrow(cs).set(None));
        let timer = match modem_timer.try_retire() {
            Ok(retired) => retired.into_shutdown_parts().0,
            Err((_, modem_timer)) => {
                return Err(stop_fail(
                    BluetoothStopError::TimerStorage,
                    uninstalled(role, task, modem_timer, None, retained),
                ));
            }
        };
        let output = match published.retire_interrupt_registers_after_routes_disabled() {
            Ok(output) => output.into_output_owner(),
            Err(_) => {
                // The timer owner is out of storage; nothing restarts it.
                return Err(stop_fail(
                    BluetoothStopError::InterruptStorage,
                    FailStopOwner::TimerRetired {
                        role,
                        task,
                        timer,
                        retained,
                    },
                ));
            }
        };

        let mut guard = radio.lock().await;
        // The scheduler is stopped: no role is active any more.
        guard.clear_coex_status_bits(CoexStatusType::Ble, BLE_COEX_STATUS_ALL);
        RUNTIME.clear_activity();
        RUNTIME.set_coexistence(false).await;
        let (retired, last) = match retire(guard.lease(), task, retained, output, timer) {
            Ok(retired) => retired,
            Err(failure) => {
                return Err(stop_fail(
                    BluetoothStopError::Shutdown(failure.error()),
                    FailStopOwner::Shutdown(role, failure),
                ));
            }
        };
        // As `coex_disable` after the stack is disabled.
        if let Err(error) = guard.disable_coex() {
            return Err(stop_fail(
                BluetoothStopError::Coex(error),
                FailStopOwner::Retired(role, retired),
            ));
        }
        if last && let Err(error) = guard.close_phy_if_idle().await {
            return Err(stop_fail(
                BluetoothStopError::PhyClose(error),
                FailStopOwner::Retired(role, retired),
            ));
        }
        let (lease, _, _) = guard.parts();
        let shut_down = match retired.shut_down(lease, &CELLS) {
            Ok(shut_down) => shut_down,
            Err(failure) => {
                return Err(stop_fail(
                    BluetoothStopError::Shutdown(failure.error()),
                    FailStopOwner::Shutdown(role, failure),
                ));
            }
        };
        let ControllerShutDown {
            clocked,
            ble_phy,
            direction_finding,
            reset,
        } = shut_down;
        let radio_memory = role.into_memory(&reset);
        let powered = match clocked.disable_clocks(lease) {
            Ok(powered) => powered,
            Err(failure) => {
                let error = failure.error();
                return Err(stop_fail(
                    BluetoothStopError::Clocks(error),
                    FailStopOwner::Clocked(
                        failure.into_owner(),
                        ControllerMemory {
                            ble_phy,
                            direction_finding,
                            radio: radio_memory,
                        },
                    ),
                ));
            }
        };
        let memory = ControllerMemory {
            ble_phy,
            direction_finding,
            radio: radio_memory,
        };
        match powered.power_down(lease) {
            Ok(cold) => Ok(BluetoothParked {
                partition: cold.into_partition(),
                memory,
                published: Some(published),
            }),
            Err(failure) => Err(stop_fail(
                BluetoothStopError::Power(failure.error()),
                FailStopOwner::Powered(failure.into_owner(), memory),
            )),
        }
    }
}

#[derive(Clone, Copy)]
struct Dispatch {
    published: &'static Storage,
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
                        PrimaryPublishedInterruptStep::DiagnosticUnsettled(_) => {
                            fault(BluetoothInterruptFault::PrimaryDiagnosticUnsettled)
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

fn unwind_powered(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    powered: PoweredOwner,
    memory: ControllerMemory,
    published: Option<&'static Storage>,
    error: BluetoothStartError,
) -> BluetoothStartFailure {
    match powered.power_down(lease) {
        Ok(cold) => BluetoothStartFailure {
            error,
            owner: Ok(BluetoothParked {
                partition: cold.into_partition(),
                memory,
                published,
            }),
        },
        Err(failure) => fail_stop(error, FailStopOwner::Powered(failure.into_owner(), memory)),
    }
}

/// Bring the Bluetooth Controller up as a client of the shared radio.
///
/// The steps follow the reviewed ESP32-S31 Controller init and enable:
///
/// 1. enter common radio power; enable the Bluetooth module clocks, reset the
///    Controller domains and select the low-power timer clock;
/// 2. run the Controller HAL, scheduler and modem low-power initialization;
/// 3. prepare the shared PHY through the radio system (register the domain
///    when no client did, or wake its closed RF), join it and the shared
///    BTBB baseband, and run PHY tracking inside the Controller's quiescence
///    window when the join makes it due;
/// 4. enable the BLE base stack and publish the public address;
/// 5. prepare the Controller output, start the runtime timer, publish both
///    interrupt owners and bind the three CPU routes to one dispatcher;
/// 6. install the radio role in the runtime.
///
/// # Errors
///
/// A step failed. Power and clock failures roll back and return the parked
/// client; every failure after the first Controller write is fail-stop.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure returns the parked partition and memory"
)]
#[allow(
    large_assignments,
    reason = "the powered owner graph crosses the PHY poll boundaries once; the linked-image stack-frame audit independently bounds this future"
)]
// CAPABILITY: controller-initialization, bluetooth-same-storage-powered-restart
pub async fn start<P, C: PlatformClockProvider>(
    radio: &RadioSystem<P, C, EmbassyClock>,
    parked: BluetoothParked,
    public_address: BluetoothPublicDeviceAddress,
) -> Result<BluetoothSystem, BluetoothStartFailure> {
    let BluetoothParked {
        partition,
        memory,
        published,
    } = parked;
    let mut guard = radio.lock().await;
    let (lease, _, clocks) = guard.parts();

    let powered = match ColdOwner::from_partition(partition).power_up(lease, clocks) {
        Ok(powered) => powered,
        Err(failure) => {
            return Err(BluetoothStartFailure {
                error: BluetoothStartError::Power(failure.error()),
                owner: Ok(BluetoothParked {
                    partition: failure.into_owner().into_partition(),
                    memory,
                    published,
                }),
            });
        }
    };
    let clocked = match powered.enable_clocks(lease, clocks) {
        Ok(clocked) => clocked,
        Err(failure) => {
            let error = BluetoothStartError::Clocks(failure.error());
            if failure.error() == BluetoothClockError::ModemClocks(ModemClockError::Poisoned) {
                return Err(fail_stop(
                    error,
                    FailStopOwner::Powered(failure.into_owner(), memory),
                ));
            }
            return Err(unwind_powered(
                lease,
                failure.into_owner(),
                memory,
                published,
                error,
            ));
        }
    };

    let controller = match ControllerHalInitialized::initialize(clocked)
        .initialize_scheduler(ControllerRuntimeResources::<MODEM_TIMER_CAPACITY>::new(
            &CELLS,
        ))
        .initialize_modem_lp_timer_hardware()
    {
        Ok(controller) => controller,
        Err(failure) => {
            return Err(fail_stop(
                BluetoothStartError::LowPower,
                FailStopOwner::LowPower(failure, memory),
            ));
        }
    };

    if let Err(error) = guard.prepare_phy().await {
        return Err(fail_stop(
            BluetoothStartError::Phy(error),
            FailStopOwner::Controller(controller, memory),
        ));
    }
    let (lease, platform, _) = guard.parts();
    let (mut joined, acquired) = match controller.join_phy(lease, &EmbassyClock) {
        Ok(joined) => joined,
        Err(failure) => {
            let error = BluetoothStartError::PhyClient(failure.error());
            return Err(fail_stop(error, FailStopOwner::PhyJoin(failure, memory)));
        }
    };
    if acquired == ConcurrentAcquire::TrackingDue {
        let issued_at = Instant::now().as_micros();
        let tracked = match joined.quiescence(issued_at, issued_at + TRACKING_WINDOW_MICROS) {
            Ok(proof) => {
                maintain_concurrent_phy::<P, RomShortDelay, _>(
                    &EmbassyClock,
                    lease,
                    platform,
                    &[proof],
                    NoopPhyTargetObserver,
                )
                .await
            }
            Err(_) => Err(ConcurrentPhyTrackingError::Rejected(
                ConcurrentPhyError::WindowClosed,
            )),
        };
        match tracked {
            Ok(_outcome) => {}
            Err(ConcurrentPhyTrackingError::Rejected(error)) => {
                return Err(fail_stop(
                    BluetoothStartError::Tracking(error),
                    FailStopOwner::Joined(joined, memory),
                ));
            }
            Err(ConcurrentPhyTrackingError::Failed(_)) => {
                return Err(fail_stop(
                    BluetoothStartError::TrackingFailed,
                    FailStopOwner::Joined(joined, memory),
                ));
            }
        }
    }
    let (lease, _, _) = guard.parts();
    let ControllerMemory {
        ble_phy,
        direction_finding,
        radio: radio_memory,
    } = memory;
    let runtime = joined
        .initialize_ble_phy_engine(lease, ble_phy, direction_finding, public_address)
        .activate();
    // `esp_bt_controller_enable` enables coexistence for Bluetooth; a later
    // start failure is fail-stop and keeps it enabled.
    guard.enable_coex();
    drop(guard);

    publish_and_install(runtime, radio_memory, published).await
}

/// Move the published status from `published` to `activity`.
fn apply_coex_status<P, C: PlatformClockProvider>(
    guard: &mut RadioGuard<'_, P, C, EmbassyClock>,
    published: RadioActivity,
    activity: RadioActivity,
) {
    let change = BleCoexStatusChange::between(published, activity);
    if change.clear != 0 {
        guard.clear_coex_status_bits(CoexStatusType::Ble, change.clear);
    }
    if change.set != 0 {
        guard.set_coex_status_bits(CoexStatusType::Ble, change.set);
    }
}

/// Publish every change of the Controller's active roles as Bluetooth LE
/// schedule status. [`BluetoothSystem::stop`] withdraws what is left.
async fn publish_coex_status<P, C: PlatformClockProvider>(
    radio: &RadioSystem<P, C, EmbassyClock>,
) -> core::convert::Infallible {
    let mut published = RadioActivity::IDLE;
    loop {
        let activity = RUNTIME.next_activity().await;
        if activity == published {
            continue;
        }
        apply_coex_status(&mut radio.lock().await, published, activity);
        published = activity;
    }
}

/// Follow whether another radio shares the antenna, as the vendor BLE
/// Controller's `coex_register_ble_cb` start and stop callbacks do.
async fn follow_coex_sharing<P, C: PlatformClockProvider>(
    radio: &RadioSystem<P, C, EmbassyClock>,
) -> core::convert::Infallible {
    loop {
        RUNTIME
            .set_coexistence(radio.bluetooth_coex_started().await)
            .await;
    }
}

// Keep interrupt publication, route binding and installation out of the PHY
// poll frame of `start`.
#[inline(never)]
async fn publish_and_install(
    runtime: BlePhyRuntime<'static, MODEM_TIMER_CAPACITY>,
    radio_memory: BluetoothSystemMemory,
    published: Option<&'static Storage>,
) -> Result<BluetoothSystem, BluetoothStartFailure> {
    let BlePhyRuntime {
        endpoints,
        output,
        timer,
        timing: _,
        retained,
        direction_finding: _,
    } = runtime;
    let interrupts = output.stage_for_cpu_routes();
    let timer = timer.start_runtime_timer().stage_for_interrupt();
    let published = match published {
        Some(published) => {
            match published.restore_initialized_interrupt_owners(interrupts, timer) {
                Ok(()) => published,
                Err((error, interrupts, timer)) => {
                    return Err(fail_stop(
                        BluetoothStartError::InterruptPublication(error),
                        FailStopOwner::Unpublished {
                            endpoints,
                            retained,
                            interrupts,
                            timer,
                            memory: radio_memory,
                        },
                    ));
                }
            }
        }
        None => match EspHalBluetoothInterruptStorage::new().publish(interrupts, timer) {
            Ok(published) => PUBLISHED.init(published),
            Err((error, _, interrupts, timer)) => {
                return Err(fail_stop(
                    BluetoothStartError::InterruptPublication(error),
                    FailStopOwner::Unpublished {
                        endpoints,
                        retained,
                        interrupts,
                        timer,
                        memory: radio_memory,
                    },
                ));
            }
        },
    };
    let ControllerRuntimeEndpoints {
        interrupt,
        task,
        modem_timer,
    } = endpoints;
    let modem_timer = ControllerModemTimerTask::new(published, modem_timer);
    critical_section::with(|cs| {
        DISPATCH.borrow(cs).set(Some(Dispatch {
            published,
            scheduler_wake: interrupt.scheduler_wake(),
            modem_timer_wake: interrupt.modem_lp_timer_worker_wake(),
        }));
    });
    let bound = match published.bind_routes(dispatch) {
        Ok(bound) => bound,
        Err(error) => {
            return Err(fail_stop(
                BluetoothStartError::InterruptRoutes(error),
                FailStopOwner::Published {
                    task,
                    modem_timer,
                    retained,
                    bound: None,
                    memory: radio_memory,
                },
            ));
        }
    };
    if let Err((error, radio_memory, hardware)) = RUNTIME
        .install(
            radio_memory,
            LiveBluetoothHardware::new(task, published, LOCAL_SLEEP_CLOCK_PPM),
        )
        .await
    {
        return Err(fail_stop(
            BluetoothStartError::Install(error),
            FailStopOwner::Published {
                task: hardware.into_task(),
                modem_timer,
                retained,
                bound: Some(bound),
                memory: radio_memory,
            },
        ));
    }
    Ok(BluetoothSystem {
        epoch: Epoch {
            modem_timer,
            bound,
            retained,
            published,
        },
    })
}
