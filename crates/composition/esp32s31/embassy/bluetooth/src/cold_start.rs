//! Complete production cold start for one ESP32-S31 Bluetooth Controller.
//!
//! This is the application-facing composition boundary. It validates every
//! value-only input before reserving permanent placement, then claims all
//! static SRAM before the first Controller write. The remaining steps are the
//! chip typestate chain; no register capability or partially initialized
//! success state escapes this module.

use crate::{
    BluetoothBlePhyMemoryClaimError, BluetoothDirectionFindingMemoryClaimError,
    BluetoothDtmMemoryClaimError, BluetoothLegacyAdvertisingMemoryClaimError,
    BluetoothLegacyConnectableAdvertisingMemoryClaimError, BluetoothPassiveScanMemoryClaimError,
    BluetoothPeripheralConnectionMemoryClaimError, BluetoothSystem, BluetoothSystemBuildError,
    BluetoothSystemSlot, BluetoothSystemStorage, BluetoothSystemStorageInUse, EmbassyPhyTime,
    EmbassyPhyTimeError, claim_production_ble_phy_memory,
    claim_production_direction_finding_memory, claim_production_dtm_runtime,
    claim_production_legacy_advertising_runtime,
    claim_production_legacy_connectable_advertising_runtime, claim_production_passive_scan_runtime,
    claim_production_peripheral_connection_runtime,
};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_bluetooth_hci::{
    BootstrapConfigError, LeControllerBootstrapConfig, LeControllerHciResources,
    LeControllerHciResourcesError,
};

use oer_esp32s31_bluetooth::{
    baseband::BasebandInitializationReport,
    ble_phy::BlePhyInitializationReport,
    clock::ClockEnableFailure,
    common_phy_state::PhyInitializationReport,
    controller::{
        ControllerInterruptOwnerPublicationFailure, ControllerInterruptOwnersReady,
        hci::ControllerHciBindFailure,
    },
    le::{
        advertising::{
            LegacyAdvertisingDefaultTxPowerDbm, LegacyAdvertisingRuntimeResources,
            LegacyConnectableAdvertisingRuntimeResources,
        },
        dtm::DtmRuntimeConfig,
        peripheral::{PeripheralConnectionRuntimeConfig, PeripheralConnectionRuntimeResources},
        scanning::{PassiveScanRuntimeConfig, PassiveScanRuntimeResources},
    },
    low_power::ControllerLowPowerHardwareInitializationFailure,
    phy::{
        ControllerPhyClientAcquireFailure, ControllerPhyInitializationFailure,
        ControllerPhyTrackingFailure, PhyInitializationConfig,
    },
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};

use oer_esp32s31_bluetooth_embassy::controller::{
    DtmAbsoluteRecheck, DtmRecheckPeriod, DtmRecheckStartError,
};

use oer_esp32s31_bluetooth_memory::{BlePhyEngineCpuOwned, DirectionFindingWorkspaceCpuOwned};

use oer_esp32s31_phy::{NoopPhyTargetObserver, PhyCalibrationCache, PhyCalibrationSnapshot};

use oer_esp32s31_radio_platform_esp_hal::{
    BluetoothPlatformBusy, EspHalBluetoothInterruptStorage, EspHalBluetoothPlatform,
    EspHalRadioPlatform,
};

type Platform = EspHalBluetoothPlatform<'static>;

type HciResources<const H2C: usize, const C2H: usize, const PC: usize> =
    LeControllerHciResources<CriticalSectionRawMutex, H2C, C2H, PC>;

type Slot<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize> =
    BluetoothSystemSlot<Platform, MT, SC, H2C, C2H, PC>;

type HciBindFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> = ControllerHciBindFailure<
    Platform,
    CriticalSectionRawMutex,
    oer_esp32s31_radio_platform_esp_hal::PublishedEspHalBluetoothInterruptOwners,
    MT,
    SC,
    H2C,
    C2H,
    PC,
>;

type LowPowerInitializationFailure<const MT: usize, const SC: usize> =
    ControllerLowPowerHardwareInitializationFailure<Platform, MT, SC>;

type PhyInitializationFailure<const MT: usize, const SC: usize> =
    ControllerPhyInitializationFailure<Platform, MT, SC>;

type PhyClientAcquireFailure<const MT: usize, const SC: usize> =
    ControllerPhyClientAcquireFailure<Platform, MT, SC>;

type PhyTrackingFailure<const MT: usize, const SC: usize> =
    ControllerPhyTrackingFailure<Platform, MT, SC>;

type InterruptPublicationFailure<const MT: usize, const SC: usize> =
    ControllerInterruptOwnerPublicationFailure<Platform, EspHalBluetoothInterruptStorage, MT, SC>;

type InterruptOwnersReady<const MT: usize, const SC: usize> =
    ControllerInterruptOwnersReady<Platform, MT, SC>;

/// Every product input for one production Controller epoch.
///
/// Recovered target facts, including the normal BLE-PHY policy, remain owned
/// by the chip driver and cannot be overridden by an application.
pub struct BluetoothColdStartConfig {
    le_acl_data_packet_length: u16,
    total_num_le_acl_data_packets: u8,
    retained_calibration: Option<PhyCalibrationSnapshot>,
    dtm: DtmRuntimeConfig,
    passive_scan: PassiveScanRuntimeConfig,
    recheck_period: DtmRecheckPeriod,
}

impl BluetoothColdStartConfig {
    /// Bind all source-reviewed inputs for one non-cancellable cold start.
    pub const fn new(
        le_acl_data_packet_length: u16,
        total_num_le_acl_data_packets: u8,
        retained_calibration: Option<PhyCalibrationSnapshot>,
        dtm: DtmRuntimeConfig,
        passive_scan: PassiveScanRuntimeConfig,
        recheck_period: DtmRecheckPeriod,
    ) -> Self {
        Self {
            le_acl_data_packet_length,
            total_num_le_acl_data_packets,
            retained_calibration,
            dtm,
            passive_scan,
            recheck_period,
        }
    }
}

/// Unpowered owners retained across a failure before the first Controller MMIO.
#[must_use = "the radio root and platform reservation can still be recovered"]
pub struct BluetoothUnpoweredOwners {
    platform: Platform,
    hardware: BluetoothRadioHardware,
}

impl BluetoothUnpoweredOwners {
    /// Recover the unchanged platform reservation and radio root.
    pub fn into_parts(self) -> (Platform, BluetoothRadioHardware) {
        (self.platform, self.hardware)
    }
}

/// All seven permanent SRAM owners before they enter Controller state.
#[must_use = "claimed static memory belongs to the retained cold-start epoch"]
pub struct BluetoothClaimedMemory {
    ble_phy: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceCpuOwned,
    dtm: oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
    legacy_advertising: LegacyAdvertisingRuntimeResources,
    passive_scan: PassiveScanRuntimeResources,
    peripheral_connection: PeripheralConnectionRuntimeResources,
    legacy_connectable_advertising: LegacyConnectableAdvertisingRuntimeResources,
}

impl BluetoothClaimedMemory {
    fn into_parts(
        self,
    ) -> (
        BlePhyEngineCpuOwned,
        DirectionFindingWorkspaceCpuOwned,
        oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
        LegacyAdvertisingRuntimeResources,
        PassiveScanRuntimeResources,
        PeripheralConnectionRuntimeResources,
        LegacyConnectableAdvertisingRuntimeResources,
    ) {
        (
            self.ble_phy,
            self.direction_finding,
            self.dtm,
            self.legacy_advertising,
            self.passive_scan,
            self.peripheral_connection,
            self.legacy_connectable_advertising,
        )
    }
}

/// One exact lower failure plus memory not yet consumed by that lower owner.
#[must_use = "the failure and permanent SRAM owners belong to one epoch"]
pub struct BluetoothPoweredFailure<F> {
    /// Exact chip-typestate failure.
    pub failure: F,
    /// All claimed graphs not yet consumed by a lower hardware owner.
    pub memory: BluetoothClaimedMemory,
}

/// Failure after final-placement reservation and before final composition.
#[must_use = "retain both the lower failure and its unique final slot"]
pub struct BluetoothReservedFailure<
    F,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    failure: F,
    slot: Slot<MT, SC, H2C, C2H, PC>,
}

impl<F, const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothReservedFailure<F, MT, SC, H2C, C2H, PC>
{
    fn new(failure: F, slot: Slot<MT, SC, H2C, C2H, PC>) -> Self {
        Self { failure, slot }
    }

    /// Borrow the exact lower failure without losing final placement.
    pub const fn failure(&self) -> &F {
        &self.failure
    }

    /// Recover both affine values for an explicitly authorized lower recovery.
    pub fn into_parts(self) -> (F, Slot<MT, SC, H2C, C2H, PC>) {
        (self.failure, self.slot)
    }
}

/// Rejected BLE-PHY graph claim retaining all unchanged cold owners.
pub struct BluetoothBlePhyMemoryFailure {
    /// Exact graph claim failure.
    pub error: BluetoothBlePhyMemoryClaimError,
    /// Owners untouched by Controller MMIO.
    pub owners: BluetoothUnpoweredOwners,
}

/// Rejected direction-finding claim retaining the preceding BLE-PHY claim.
pub struct BluetoothDirectionFindingMemoryFailure {
    /// Exact workspace claim failure.
    pub error: BluetoothDirectionFindingMemoryClaimError,
    /// Owners untouched by Controller MMIO.
    pub owners: BluetoothUnpoweredOwners,
    /// Already claimed BLE-PHY graph.
    pub ble_phy: BlePhyEngineCpuOwned,
}

/// Rejected DTM graph claim retaining both preceding global-memory claims.
pub struct BluetoothDtmMemoryFailure {
    /// Exact graph claim failure.
    pub error: BluetoothDtmMemoryClaimError,
    /// Owners untouched by Controller MMIO.
    pub owners: BluetoothUnpoweredOwners,
    /// Already claimed BLE-PHY graph.
    pub ble_phy: BlePhyEngineCpuOwned,
    /// Already claimed controller-global direction-finding workspace.
    pub direction_finding: DirectionFindingWorkspaceCpuOwned,
}

/// Rejected advertising graph claim retaining both preceding graph claims.
pub struct BluetoothLegacyAdvertisingMemoryFailure {
    pub error: BluetoothLegacyAdvertisingMemoryClaimError,
    pub owners: BluetoothUnpoweredOwners,
    pub ble_phy: BlePhyEngineCpuOwned,
    pub direction_finding: DirectionFindingWorkspaceCpuOwned,
    pub dtm: oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
}

/// Rejected passive-scanner graph claim retaining all preceding graph claims.
pub struct BluetoothPassiveScanMemoryFailure {
    pub error: BluetoothPassiveScanMemoryClaimError,
    pub owners: BluetoothUnpoweredOwners,
    pub ble_phy: BlePhyEngineCpuOwned,
    pub direction_finding: DirectionFindingWorkspaceCpuOwned,
    pub dtm: oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
    pub legacy_advertising: LegacyAdvertisingRuntimeResources,
}

/// Rejected peripheral-connection graph claim retaining all earlier claims.
pub struct BluetoothPeripheralConnectionMemoryFailure {
    pub error: BluetoothPeripheralConnectionMemoryClaimError,
    pub owners: BluetoothUnpoweredOwners,
    pub ble_phy: BlePhyEngineCpuOwned,
    pub direction_finding: DirectionFindingWorkspaceCpuOwned,
    pub dtm: oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
    pub legacy_advertising: LegacyAdvertisingRuntimeResources,
    pub passive_scan: PassiveScanRuntimeResources,
}

/// Rejected response-capable advertising graph retaining all earlier claims.
pub struct BluetoothLegacyConnectableAdvertisingMemoryFailure {
    pub error: BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    pub owners: BluetoothUnpoweredOwners,
    pub ble_phy: BlePhyEngineCpuOwned,
    pub direction_finding: DirectionFindingWorkspaceCpuOwned,
    pub dtm: oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
    pub legacy_advertising: LegacyAdvertisingRuntimeResources,
    pub passive_scan: PassiveScanRuntimeResources,
    pub peripheral_connection: PeripheralConnectionRuntimeResources,
}

/// Absolute-recheck anchoring failed after hardware initialization.
#[must_use = "the pre-route Controller and DTM graph remain one fail-stop epoch"]
pub struct BluetoothRecheckStartFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    error: DtmRecheckStartError,
    _controller: InterruptOwnersReady<MT, SC>,
    _dtm: oer_esp32s31_bluetooth::le::dtm::DtmRuntimeResources,
    _legacy_advertising: LegacyAdvertisingRuntimeResources,
    _passive_scan: PassiveScanRuntimeResources,
    _peripheral_connection: PeripheralConnectionRuntimeResources,
    _legacy_connectable_advertising: LegacyConnectableAdvertisingRuntimeResources,
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothRecheckStartFailure<MT, SC, H2C, C2H, PC>
{
    /// Inspect the exact monotonic-timeline failure.
    pub const fn error(&self) -> DtmRecheckStartError {
        self.error
    }
}

/// Successful cold-start output and value-only evidence from the exact epoch.
#[must_use = "spawn the hardware runner and retain its HCI facade"]
pub struct BluetoothColdStartOutput<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    /// Final `bt-hci` facade and sole hardware runner.
    pub system: BluetoothSystem<MT, SC, H2C, C2H, PC>,
    /// Complete common-PHY target execution report.
    pub phy: PhyInitializationReport,
    /// Finite BTBB initialization input projected from the PHY owner.
    pub baseband: BasebandInitializationReport,
    /// Exact BLE-PHY source configuration consumed by this epoch.
    pub ble_phy: BlePhyInitializationReport,
    /// Stable calibration result copied before the final owner hides it.
    pub calibration: Option<PhyCalibrationSnapshot>,
}

/// Why complete production cold start did not produce a runnable system.
///
/// Every pre-reservation failure returns the caller's Bluetooth radio root.
/// Every post-reservation failure retains the slot, and every post-claim
/// failure retains static memory until it is nested in the corresponding lower
/// owner.
#[must_use = "a failed cold start retains affine resources"]
#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc error retains exact affine hardware owners"
)]
pub enum BluetoothColdStartError<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    /// The board timebase cannot represent the PHY microsecond contract.
    Timebase {
        /// Exact timebase error.
        error: EmbassyPhyTimeError,
        /// Unchanged caller-owned radio root.
        hardware: BluetoothRadioHardware,
    },
    /// Another protocol owns the ESP-HAL coordinator.
    PlatformBusy {
        /// Exact coordinator rejection.
        error: BluetoothPlatformBusy,
        /// Unchanged caller-owned radio root.
        hardware: BluetoothRadioHardware,
    },
    /// The HCI report contains a zero ACL length or credit count.
    HciConfig {
        /// Exact HCI profile rejection.
        error: BootstrapConfigError,
        /// Unchanged caller-owned radio root.
        hardware: BluetoothRadioHardware,
    },
    /// The bounded HCI transport cannot retain its advertised packets.
    HciResources {
        /// Exact bounded-resource rejection.
        error: LeControllerHciResourcesError,
        /// Unchanged caller-owned radio root.
        hardware: BluetoothRadioHardware,
    },
    /// A retained calibration snapshot used another schema.
    CalibrationSnapshotSchema {
        /// Schema carried by the rejected snapshot.
        observed: u16,
        /// Unchanged caller-owned radio root.
        hardware: BluetoothRadioHardware,
    },
    /// Final process-lifetime placement was already reserved.
    StorageInUse {
        /// Exact placement rejection.
        error: BluetoothSystemStorageInUse,
        /// Unpowered owners acquired during successful preflight.
        owners: BluetoothUnpoweredOwners,
    },
    /// The permanent BLE-PHY SRAM arena could not be claimed.
    BlePhyMemory(BluetoothReservedFailure<BluetoothBlePhyMemoryFailure, MT, SC, H2C, C2H, PC>),
    /// The permanent controller-global direction-finding workspace was rejected.
    DirectionFindingMemory(
        BluetoothReservedFailure<BluetoothDirectionFindingMemoryFailure, MT, SC, H2C, C2H, PC>,
    ),
    /// The permanent DTM arena was rejected after both global-memory claims.
    DtmMemory(BluetoothReservedFailure<BluetoothDtmMemoryFailure, MT, SC, H2C, C2H, PC>),
    /// The permanent advertising arena was rejected after global and DTM placement.
    LegacyAdvertisingMemory(
        BluetoothReservedFailure<BluetoothLegacyAdvertisingMemoryFailure, MT, SC, H2C, C2H, PC>,
    ),
    /// The permanent passive-scanner arena was rejected after the preceding claims.
    PassiveScanMemory(
        BluetoothReservedFailure<BluetoothPassiveScanMemoryFailure, MT, SC, H2C, C2H, PC>,
    ),
    /// The permanent peripheral-connection arena was rejected after the preceding claims.
    PeripheralConnectionMemory(
        BluetoothReservedFailure<BluetoothPeripheralConnectionMemoryFailure, MT, SC, H2C, C2H, PC>,
    ),
    /// The response-capable advertising arena was rejected after all preceding claims.
    LegacyConnectableAdvertisingMemory(
        BluetoothReservedFailure<
            BluetoothLegacyConnectableAdvertisingMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Clock/reset setup failed and completed its verified rollback.
    Clock(
        BluetoothReservedFailure<
            BluetoothPoweredFailure<ClockEnableFailure<Platform>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// A supposedly pristine HCI epoch was rejected after interrupt publication.
    HciBind(BluetoothReservedFailure<HciBindFailure<MT, SC, H2C, C2H, PC>, MT, SC, H2C, C2H, PC>),
    /// The disjoint source-127 hardware owner rejected initialization.
    LowPower(
        BluetoothReservedFailure<
            BluetoothPoweredFailure<LowPowerInitializationFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Common-PHY registration failed after entering the powered epoch.
    PhyInitialization(
        BluetoothReservedFailure<
            BluetoothPoweredFailure<PhyInitializationFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The registered PHY owner rejected Bluetooth-client acquisition.
    PhyClientAcquire(
        BluetoothReservedFailure<
            BluetoothPoweredFailure<PhyClientAcquireFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Due initial parameter tracking failed and poisoned the powered epoch.
    PhyTracking(
        BluetoothReservedFailure<
            BluetoothPoweredFailure<PhyTrackingFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The first runtime-relative recheck cannot fit the monotonic timeline.
    RecheckStart(
        BluetoothReservedFailure<
            BluetoothRecheckStartFailure<MT, SC, H2C, C2H, PC>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Stable publication of the two interrupt owners was rejected.
    InterruptPublication(
        BluetoothReservedFailure<InterruptPublicationFailure<MT, SC>, MT, SC, H2C, C2H, PC>,
    ),
    /// The published owner could not complete its one-time runtime split/bind.
    SystemBuild(BluetoothSystemBuildError<MT, SC, H2C, C2H, PC>),
}

fn reserved_powered<
    F,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    failure: F,
    memory: BluetoothClaimedMemory,
    slot: Slot<MT, SC, H2C, C2H, PC>,
) -> BluetoothReservedFailure<BluetoothPoweredFailure<F>, MT, SC, H2C, C2H, PC> {
    BluetoothReservedFailure::new(BluetoothPoweredFailure { failure, memory }, slot)
}

/// Cold-start one complete production Controller and publish its task runners.
///
/// Preflight returns the supplied radio root unchanged. After final-slot
/// reservation, all seven static memory graphs are claimed before `enable_clocks`
/// begins MMIO. Once the first MMIO future is polled this operation is
/// deliberately non-cancellable: the hardware typestate has no implicit or
/// legacy teardown path.
pub async fn start_esp32s31_bluetooth<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    platform_root: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
    storage: &'static BluetoothSystemStorage<Platform, MT, SC, H2C, C2H, PC>,
    config: BluetoothColdStartConfig,
) -> Result<
    BluetoothColdStartOutput<MT, SC, H2C, C2H, PC>,
    BluetoothColdStartError<MT, SC, H2C, C2H, PC>,
> {
    let BluetoothColdStartConfig {
        le_acl_data_packet_length,
        total_num_le_acl_data_packets,
        retained_calibration,
        dtm,
        passive_scan,
        recheck_period,
    } = config;

    let retained_calibration = match retained_calibration {
        Some(snapshot) => match PhyCalibrationCache::from_snapshot(snapshot) {
            Some(cache) => Some(cache),
            None => {
                return Err(BluetoothColdStartError::CalibrationSnapshotSchema {
                    observed: snapshot.schema,
                    hardware,
                });
            }
        },
        None => None,
    };
    if let Err(error) = EmbassyPhyTime::validate_timebase() {
        return Err(BluetoothColdStartError::Timebase { error, hardware });
    }
    let platform = match platform_root.try_bluetooth() {
        Ok(platform) => platform,
        Err(error) => {
            return Err(BluetoothColdStartError::PlatformBusy { error, hardware });
        }
    };
    let calibration_identity = platform.phy_calibration_identity();
    let public_address = platform.bluetooth_public_address();
    let hci_config = match LeControllerBootstrapConfig::new(
        public_address,
        le_acl_data_packet_length,
        total_num_le_acl_data_packets,
    ) {
        Ok(config) => config,
        Err(error) => {
            return Err(BluetoothColdStartError::HciConfig { error, hardware });
        }
    };
    let hci = match HciResources::<H2C, C2H, PC>::new(hci_config) {
        Ok(hci) => hci,
        Err(error) => {
            return Err(BluetoothColdStartError::HciResources { error, hardware });
        }
    };
    let owners = BluetoothUnpoweredOwners { platform, hardware };
    let slot = match storage.reserve() {
        Ok(slot) => slot,
        Err(error) => {
            return Err(BluetoothColdStartError::StorageInUse { error, owners });
        }
    };
    let ble_phy_memory = match claim_production_ble_phy_memory() {
        Ok(memory) => memory,
        Err(error) => {
            return Err(BluetoothColdStartError::BlePhyMemory(
                BluetoothReservedFailure::new(BluetoothBlePhyMemoryFailure { error, owners }, slot),
            ));
        }
    };
    let direction_finding_memory = match claim_production_direction_finding_memory() {
        Ok(memory) => memory,
        Err(error) => {
            return Err(BluetoothColdStartError::DirectionFindingMemory(
                BluetoothReservedFailure::new(
                    BluetoothDirectionFindingMemoryFailure {
                        error,
                        owners,
                        ble_phy: ble_phy_memory,
                    },
                    slot,
                ),
            ));
        }
    };
    let dtm_runtime = match claim_production_dtm_runtime(dtm) {
        Ok(runtime) => runtime,
        Err(error) => {
            return Err(BluetoothColdStartError::DtmMemory(
                BluetoothReservedFailure::new(
                    BluetoothDtmMemoryFailure {
                        error,
                        owners,
                        ble_phy: ble_phy_memory,
                        direction_finding: direction_finding_memory,
                    },
                    slot,
                ),
            ));
        }
    };
    let legacy_advertising_runtime = match claim_production_legacy_advertising_runtime(
        LegacyAdvertisingDefaultTxPowerDbm::new(dtm.default_tx_power_dbm().dbm()),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            return Err(BluetoothColdStartError::LegacyAdvertisingMemory(
                BluetoothReservedFailure::new(
                    BluetoothLegacyAdvertisingMemoryFailure {
                        error,
                        owners,
                        ble_phy: ble_phy_memory,
                        direction_finding: direction_finding_memory,
                        dtm: dtm_runtime,
                    },
                    slot,
                ),
            ));
        }
    };
    let passive_scan_runtime = match claim_production_passive_scan_runtime(passive_scan) {
        Ok(runtime) => runtime,
        Err(error) => {
            return Err(BluetoothColdStartError::PassiveScanMemory(
                BluetoothReservedFailure::new(
                    BluetoothPassiveScanMemoryFailure {
                        error,
                        owners,
                        ble_phy: ble_phy_memory,
                        direction_finding: direction_finding_memory,
                        dtm: dtm_runtime,
                        legacy_advertising: legacy_advertising_runtime,
                    },
                    slot,
                ),
            ));
        }
    };
    let peripheral_connection_runtime = match claim_production_peripheral_connection_runtime(
        PeripheralConnectionRuntimeConfig::new(
            oer_esp32s31_bluetooth_memory::PeripheralConnectionDefaultTxPowerDbm::new(
                dtm.default_tx_power_dbm().dbm(),
            ),
        ),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            return Err(BluetoothColdStartError::PeripheralConnectionMemory(
                BluetoothReservedFailure::new(
                    BluetoothPeripheralConnectionMemoryFailure {
                        error,
                        owners,
                        ble_phy: ble_phy_memory,
                        direction_finding: direction_finding_memory,
                        dtm: dtm_runtime,
                        legacy_advertising: legacy_advertising_runtime,
                        passive_scan: passive_scan_runtime,
                    },
                    slot,
                ),
            ));
        }
    };
    let legacy_connectable_advertising_runtime =
        match claim_production_legacy_connectable_advertising_runtime(
            LegacyAdvertisingDefaultTxPowerDbm::new(dtm.default_tx_power_dbm().dbm()),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                return Err(BluetoothColdStartError::LegacyConnectableAdvertisingMemory(
                    BluetoothReservedFailure::new(
                        BluetoothLegacyConnectableAdvertisingMemoryFailure {
                            error,
                            owners,
                            ble_phy: ble_phy_memory,
                            direction_finding: direction_finding_memory,
                            dtm: dtm_runtime,
                            legacy_advertising: legacy_advertising_runtime,
                            passive_scan: passive_scan_runtime,
                            peripheral_connection: peripheral_connection_runtime,
                        },
                        slot,
                    ),
                ));
            }
        };
    let memory = BluetoothClaimedMemory {
        ble_phy: ble_phy_memory,
        direction_finding: direction_finding_memory,
        dtm: dtm_runtime,
        legacy_advertising: legacy_advertising_runtime,
        passive_scan: passive_scan_runtime,
        peripheral_connection: peripheral_connection_runtime,
        legacy_connectable_advertising: legacy_connectable_advertising_runtime,
    };
    let (platform, hardware) = owners.into_parts();

    let stopped = BluetoothStopped::from_hardware(platform, hardware);
    let clocked = match stopped.enable_clocks() {
        Ok(clocked) => clocked,
        Err(failure) => {
            return Err(BluetoothColdStartError::Clock(reserved_powered(
                failure, memory, slot,
            )));
        }
    };
    let scheduler = clocked
        .initialize_controller_hal()
        .initialize_scheduler(ControllerRuntimeResources::<MT, SC>::new());
    let low_power = match scheduler.initialize_modem_lp_timer_hardware() {
        Ok(low_power) => low_power,
        Err(failure) => {
            return Err(BluetoothColdStartError::LowPower(reserved_powered(
                failure, memory, slot,
            )));
        }
    };

    let mut phy_config = PhyInitializationConfig::new(calibration_identity);
    if let Some(cache) = retained_calibration {
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
        Err(failure) => {
            return Err(BluetoothColdStartError::PhyInitialization(
                reserved_powered(failure, memory, slot),
            ));
        }
    };
    let mut clock = EmbassyPhyTime;
    let acquisition = match registered.acquire_phy_client(&mut clock) {
        Ok(acquisition) => acquisition,
        Err(failure) => {
            return Err(BluetoothColdStartError::PhyClientAcquire(reserved_powered(
                failure, memory, slot,
            )));
        }
    };
    let initialized = match acquisition.into_owner() {
        Ok(initialized) => initialized,
        Err(pending) => match pending
            .begin_tracking()
            .complete_tracking::<EmbassyPhyTime, NoopPhyTargetObserver>(NoopPhyTargetObserver)
            .await
        {
            Ok(initialized) => initialized,
            Err(failure) => {
                return Err(BluetoothColdStartError::PhyTracking(reserved_powered(
                    failure, memory, slot,
                )));
            }
        },
    };

    let phy = initialized.report();
    let calibration = initialized
        .calibration_cache()
        .map(|cache| *cache.snapshot());
    let baseband_initialized = initialized.initialize_baseband();
    let baseband = baseband_initialized.baseband_report();
    let (
        ble_phy_memory,
        direction_finding_memory,
        dtm_runtime,
        legacy_advertising_runtime,
        passive_scan_runtime,
        peripheral_connection_runtime,
        legacy_connectable_advertising_runtime,
    ) = memory.into_parts();
    let ble_phy_initialized = baseband_initialized.initialize_ble_phy_engine(
        ble_phy_memory,
        direction_finding_memory,
        public_address,
    );
    let ble_phy = ble_phy_initialized.report();
    let ready = ble_phy_initialized
        .prepare_controller_output_and_start_runtime_timer()
        .stage_interrupt_owners();
    let recheck = match DtmAbsoluteRecheck::after_period(recheck_period) {
        Ok(recheck) => recheck,
        Err(error) => {
            return Err(BluetoothColdStartError::RecheckStart(
                BluetoothReservedFailure::new(
                    BluetoothRecheckStartFailure {
                        error,
                        _controller: ready,
                        _dtm: dtm_runtime,
                        _legacy_advertising: legacy_advertising_runtime,
                        _passive_scan: passive_scan_runtime,
                        _peripheral_connection: peripheral_connection_runtime,
                        _legacy_connectable_advertising: legacy_connectable_advertising_runtime,
                    },
                    slot,
                ),
            ));
        }
    };
    let published = match ready.publish_interrupt_owners(
        EspHalBluetoothInterruptStorage::new(),
        dtm_runtime,
        legacy_advertising_runtime,
        passive_scan_runtime,
        peripheral_connection_runtime,
        legacy_connectable_advertising_runtime,
    ) {
        Ok(published) => published,
        Err(failure) => {
            return Err(BluetoothColdStartError::InterruptPublication(
                BluetoothReservedFailure::new(failure, slot),
            ));
        }
    };
    let bound = match published.bind_hci(hci) {
        Ok(bound) => bound,
        Err(failure) => {
            return Err(BluetoothColdStartError::HciBind(
                BluetoothReservedFailure::new(failure, slot),
            ));
        }
    };
    let system = slot
        .compose(bound, recheck)
        .map_err(BluetoothColdStartError::SystemBuild)?;

    Ok(BluetoothColdStartOutput {
        system,
        phy,
        baseband,
        ble_phy,
        calibration,
    })
}
