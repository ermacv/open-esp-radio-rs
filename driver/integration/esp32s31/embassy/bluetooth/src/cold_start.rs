//! Complete production cold start for one ESP32-S31 Bluetooth Controller.
//!
//! This is the application-facing composition boundary. It validates every
//! value-only input before reserving permanent placement, then claims all
//! static SRAM before the first Controller write. The remaining steps are the
//! chip typestate chain; no register capability or partially initialized
//! success state escapes this module.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio_bluetooth_hci::{
    BootstrapConfigError, LeControllerBootstrapConfig, LeControllerHciResources,
    LeControllerHciResourcesError,
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothBasebandInitializationReport, BluetoothBlePhyInitializationReport,
    BluetoothClockEnableFailure, BluetoothControllerHciBindFailure,
    BluetoothControllerInterruptOwnerPublicationFailure, BluetoothControllerInterruptOwnersReady,
    BluetoothControllerLowPowerHardwareInitializationFailure,
    BluetoothControllerPhyClientAcquireFailure, BluetoothControllerPhyInitializationFailure,
    BluetoothControllerPhyTrackingFailure, BluetoothControllerRuntimeResources,
    BluetoothDtmRuntimeConfig, BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    BluetoothLegacyAdvertisingRuntimeResources,
    BluetoothLegacyConnectableAdvertisingRuntimeResources, BluetoothPassiveScanRuntimeConfig,
    BluetoothPassiveScanRuntimeResources, BluetoothPeripheralConnectionRuntimeConfig,
    BluetoothPeripheralConnectionRuntimeResources, BluetoothPhyInitializationConfig,
    BluetoothPhyInitializationReport, BluetoothRadioHardware, BluetoothStopped,
};
use open_esp_radio_esp32s31_bluetooth_embassy::{
    EmbassyBluetoothDtmAbsoluteRecheck, EmbassyBluetoothDtmRecheckPeriod,
    EmbassyBluetoothDtmRecheckStartError,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothBlePhyEngineCpuOwned, BluetoothDirectionFindingWorkspaceCpuOwned,
};
use open_esp_radio_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationCache, PhyCalibrationSnapshot,
};
use open_esp_radio_esp32s31_radio_platform_esp_hal::{
    BluetoothPlatformBusy, EspHalBluetoothInterruptStorage, EspHalBluetoothPlatform,
    EspHalRadioPlatform,
};

use crate::{
    EmbassyEsp32s31PhyTime, EmbassyEsp32s31PhyTimeError, Esp32s31BluetoothBlePhyMemoryClaimError,
    Esp32s31BluetoothDirectionFindingMemoryClaimError, Esp32s31BluetoothDtmMemoryClaimError,
    Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
    Esp32s31BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    Esp32s31BluetoothPassiveScanMemoryClaimError,
    Esp32s31BluetoothPeripheralConnectionMemoryClaimError, Esp32s31BluetoothSystem,
    Esp32s31BluetoothSystemBuildError, Esp32s31BluetoothSystemSlot, Esp32s31BluetoothSystemStorage,
    Esp32s31BluetoothSystemStorageInUse, claim_production_ble_phy_memory,
    claim_production_direction_finding_memory, claim_production_dtm_runtime,
    claim_production_legacy_advertising_runtime,
    claim_production_legacy_connectable_advertising_runtime, claim_production_passive_scan_runtime,
    claim_production_peripheral_connection_runtime,
};

type Platform = EspHalBluetoothPlatform<'static>;

type HciResources<const H2C: usize, const C2H: usize, const PC: usize> =
    LeControllerHciResources<CriticalSectionRawMutex, H2C, C2H, PC>;

type Slot<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize> =
    Esp32s31BluetoothSystemSlot<Platform, MT, SC, H2C, C2H, PC>;

type HciBindFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> = BluetoothControllerHciBindFailure<
    Platform,
    CriticalSectionRawMutex,
    open_esp_radio_esp32s31_radio_platform_esp_hal::PublishedEspHalBluetoothInterruptOwners,
    MT,
    SC,
    H2C,
    C2H,
    PC,
>;

type LowPowerInitializationFailure<const MT: usize, const SC: usize> =
    BluetoothControllerLowPowerHardwareInitializationFailure<Platform, MT, SC>;

type PhyInitializationFailure<const MT: usize, const SC: usize> =
    BluetoothControllerPhyInitializationFailure<Platform, MT, SC>;

type PhyClientAcquireFailure<const MT: usize, const SC: usize> =
    BluetoothControllerPhyClientAcquireFailure<Platform, MT, SC>;

type PhyTrackingFailure<const MT: usize, const SC: usize> =
    BluetoothControllerPhyTrackingFailure<Platform, MT, SC>;

type InterruptPublicationFailure<const MT: usize, const SC: usize> =
    BluetoothControllerInterruptOwnerPublicationFailure<
        Platform,
        EspHalBluetoothInterruptStorage,
        MT,
        SC,
    >;

type InterruptOwnersReady<const MT: usize, const SC: usize> =
    BluetoothControllerInterruptOwnersReady<Platform, MT, SC>;

/// Every product input for one production Controller epoch.
///
/// Recovered target facts, including the normal BLE-PHY policy, remain owned
/// by the chip driver and cannot be overridden by an application.
pub struct Esp32s31BluetoothColdStartConfig {
    le_acl_data_packet_length: u16,
    total_num_le_acl_data_packets: u8,
    retained_calibration: Option<PhyCalibrationSnapshot>,
    dtm: BluetoothDtmRuntimeConfig,
    passive_scan: BluetoothPassiveScanRuntimeConfig,
    recheck_period: EmbassyBluetoothDtmRecheckPeriod,
}

impl Esp32s31BluetoothColdStartConfig {
    /// Bind all source-reviewed inputs for one non-cancellable cold start.
    pub const fn new(
        le_acl_data_packet_length: u16,
        total_num_le_acl_data_packets: u8,
        retained_calibration: Option<PhyCalibrationSnapshot>,
        dtm: BluetoothDtmRuntimeConfig,
        passive_scan: BluetoothPassiveScanRuntimeConfig,
        recheck_period: EmbassyBluetoothDtmRecheckPeriod,
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
pub struct Esp32s31BluetoothUnpoweredOwners {
    platform: Platform,
    hardware: BluetoothRadioHardware,
}

impl Esp32s31BluetoothUnpoweredOwners {
    /// Recover the unchanged platform reservation and radio root.
    pub fn into_parts(self) -> (Platform, BluetoothRadioHardware) {
        (self.platform, self.hardware)
    }
}

/// All seven permanent SRAM owners before they enter Controller state.
#[must_use = "claimed static memory belongs to the retained cold-start epoch"]
pub struct Esp32s31BluetoothClaimedMemory {
    ble_phy: BluetoothBlePhyEngineCpuOwned,
    direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
    dtm: open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
    legacy_advertising: BluetoothLegacyAdvertisingRuntimeResources,
    passive_scan: BluetoothPassiveScanRuntimeResources,
    peripheral_connection: BluetoothPeripheralConnectionRuntimeResources,
    legacy_connectable_advertising: BluetoothLegacyConnectableAdvertisingRuntimeResources,
}

impl Esp32s31BluetoothClaimedMemory {
    fn into_parts(
        self,
    ) -> (
        BluetoothBlePhyEngineCpuOwned,
        BluetoothDirectionFindingWorkspaceCpuOwned,
        open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
        BluetoothLegacyAdvertisingRuntimeResources,
        BluetoothPassiveScanRuntimeResources,
        BluetoothPeripheralConnectionRuntimeResources,
        BluetoothLegacyConnectableAdvertisingRuntimeResources,
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
pub struct Esp32s31BluetoothPoweredFailure<F> {
    /// Exact chip-typestate failure.
    pub failure: F,
    /// All claimed graphs not yet consumed by a lower hardware owner.
    pub memory: Esp32s31BluetoothClaimedMemory,
}

/// Failure after final-placement reservation and before final composition.
#[must_use = "retain both the lower failure and its unique final slot"]
pub struct Esp32s31BluetoothReservedFailure<
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
    Esp32s31BluetoothReservedFailure<F, MT, SC, H2C, C2H, PC>
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
pub struct Esp32s31BluetoothBlePhyMemoryFailure {
    /// Exact graph claim failure.
    pub error: Esp32s31BluetoothBlePhyMemoryClaimError,
    /// Owners untouched by Controller MMIO.
    pub owners: Esp32s31BluetoothUnpoweredOwners,
}

/// Rejected direction-finding claim retaining the preceding BLE-PHY claim.
pub struct Esp32s31BluetoothDirectionFindingMemoryFailure {
    /// Exact workspace claim failure.
    pub error: Esp32s31BluetoothDirectionFindingMemoryClaimError,
    /// Owners untouched by Controller MMIO.
    pub owners: Esp32s31BluetoothUnpoweredOwners,
    /// Already claimed BLE-PHY graph.
    pub ble_phy: BluetoothBlePhyEngineCpuOwned,
}

/// Rejected DTM graph claim retaining both preceding global-memory claims.
pub struct Esp32s31BluetoothDtmMemoryFailure {
    /// Exact graph claim failure.
    pub error: Esp32s31BluetoothDtmMemoryClaimError,
    /// Owners untouched by Controller MMIO.
    pub owners: Esp32s31BluetoothUnpoweredOwners,
    /// Already claimed BLE-PHY graph.
    pub ble_phy: BluetoothBlePhyEngineCpuOwned,
    /// Already claimed controller-global direction-finding workspace.
    pub direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
}

/// Rejected advertising graph claim retaining both preceding graph claims.
pub struct Esp32s31BluetoothLegacyAdvertisingMemoryFailure {
    pub error: Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
    pub owners: Esp32s31BluetoothUnpoweredOwners,
    pub ble_phy: BluetoothBlePhyEngineCpuOwned,
    pub direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
    pub dtm: open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
}

/// Rejected passive-scanner graph claim retaining all preceding graph claims.
pub struct Esp32s31BluetoothPassiveScanMemoryFailure {
    pub error: Esp32s31BluetoothPassiveScanMemoryClaimError,
    pub owners: Esp32s31BluetoothUnpoweredOwners,
    pub ble_phy: BluetoothBlePhyEngineCpuOwned,
    pub direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
    pub dtm: open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
    pub legacy_advertising: BluetoothLegacyAdvertisingRuntimeResources,
}

/// Rejected peripheral-connection graph claim retaining all earlier claims.
pub struct Esp32s31BluetoothPeripheralConnectionMemoryFailure {
    pub error: Esp32s31BluetoothPeripheralConnectionMemoryClaimError,
    pub owners: Esp32s31BluetoothUnpoweredOwners,
    pub ble_phy: BluetoothBlePhyEngineCpuOwned,
    pub direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
    pub dtm: open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
    pub legacy_advertising: BluetoothLegacyAdvertisingRuntimeResources,
    pub passive_scan: BluetoothPassiveScanRuntimeResources,
}

/// Rejected response-capable advertising graph retaining all earlier claims.
pub struct Esp32s31BluetoothLegacyConnectableAdvertisingMemoryFailure {
    pub error: Esp32s31BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    pub owners: Esp32s31BluetoothUnpoweredOwners,
    pub ble_phy: BluetoothBlePhyEngineCpuOwned,
    pub direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
    pub dtm: open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
    pub legacy_advertising: BluetoothLegacyAdvertisingRuntimeResources,
    pub passive_scan: BluetoothPassiveScanRuntimeResources,
    pub peripheral_connection: BluetoothPeripheralConnectionRuntimeResources,
}

/// Absolute-recheck anchoring failed after hardware initialization.
#[must_use = "the pre-route Controller and DTM graph remain one fail-stop epoch"]
pub struct Esp32s31BluetoothRecheckStartFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    error: EmbassyBluetoothDtmRecheckStartError,
    _controller: InterruptOwnersReady<MT, SC>,
    _dtm: open_esp_radio_esp32s31_bluetooth::BluetoothDtmRuntimeResources,
    _legacy_advertising: BluetoothLegacyAdvertisingRuntimeResources,
    _passive_scan: BluetoothPassiveScanRuntimeResources,
    _peripheral_connection: BluetoothPeripheralConnectionRuntimeResources,
    _legacy_connectable_advertising: BluetoothLegacyConnectableAdvertisingRuntimeResources,
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    Esp32s31BluetoothRecheckStartFailure<MT, SC, H2C, C2H, PC>
{
    /// Inspect the exact monotonic-timeline failure.
    pub const fn error(&self) -> EmbassyBluetoothDtmRecheckStartError {
        self.error
    }
}

/// Successful cold-start output and value-only evidence from the exact epoch.
#[must_use = "spawn the hardware runner and retain its HCI facade"]
pub struct Esp32s31BluetoothColdStartOutput<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    /// Final `bt-hci` facade and sole hardware runner.
    pub system: Esp32s31BluetoothSystem<MT, SC, H2C, C2H, PC>,
    /// Complete common-PHY target execution report.
    pub phy: BluetoothPhyInitializationReport,
    /// Finite BTBB initialization input projected from the PHY owner.
    pub baseband: BluetoothBasebandInitializationReport,
    /// Exact BLE-PHY source configuration consumed by this epoch.
    pub ble_phy: BluetoothBlePhyInitializationReport,
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
pub enum Esp32s31BluetoothColdStartError<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    /// The board timebase cannot represent the PHY microsecond contract.
    Timebase {
        /// Exact timebase error.
        error: EmbassyEsp32s31PhyTimeError,
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
        error: Esp32s31BluetoothSystemStorageInUse,
        /// Unpowered owners acquired during successful preflight.
        owners: Esp32s31BluetoothUnpoweredOwners,
    },
    /// The permanent BLE-PHY SRAM arena could not be claimed.
    BlePhyMemory(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothBlePhyMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The permanent controller-global direction-finding workspace was rejected.
    DirectionFindingMemory(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothDirectionFindingMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The permanent DTM arena was rejected after both global-memory claims.
    DtmMemory(
        Esp32s31BluetoothReservedFailure<Esp32s31BluetoothDtmMemoryFailure, MT, SC, H2C, C2H, PC>,
    ),
    /// The permanent advertising arena was rejected after global and DTM placement.
    LegacyAdvertisingMemory(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothLegacyAdvertisingMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The permanent passive-scanner arena was rejected after the preceding claims.
    PassiveScanMemory(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPassiveScanMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The permanent peripheral-connection arena was rejected after the preceding claims.
    PeripheralConnectionMemory(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPeripheralConnectionMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The response-capable advertising arena was rejected after all preceding claims.
    LegacyConnectableAdvertisingMemory(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothLegacyConnectableAdvertisingMemoryFailure,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Clock/reset setup failed and completed its verified rollback.
    Clock(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPoweredFailure<BluetoothClockEnableFailure<Platform>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// A supposedly pristine HCI epoch was rejected after interrupt publication.
    HciBind(
        Esp32s31BluetoothReservedFailure<
            HciBindFailure<MT, SC, H2C, C2H, PC>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The disjoint source-127 hardware owner rejected initialization.
    LowPower(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPoweredFailure<LowPowerInitializationFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Common-PHY registration failed after entering the powered epoch.
    PhyInitialization(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPoweredFailure<PhyInitializationFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The registered PHY owner rejected Bluetooth-client acquisition.
    PhyClientAcquire(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPoweredFailure<PhyClientAcquireFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Due initial parameter tracking failed and poisoned the powered epoch.
    PhyTracking(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothPoweredFailure<PhyTrackingFailure<MT, SC>>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// The first runtime-relative recheck cannot fit the monotonic timeline.
    RecheckStart(
        Esp32s31BluetoothReservedFailure<
            Esp32s31BluetoothRecheckStartFailure<MT, SC, H2C, C2H, PC>,
            MT,
            SC,
            H2C,
            C2H,
            PC,
        >,
    ),
    /// Stable publication of the two interrupt owners was rejected.
    InterruptPublication(
        Esp32s31BluetoothReservedFailure<InterruptPublicationFailure<MT, SC>, MT, SC, H2C, C2H, PC>,
    ),
    /// The published owner could not complete its one-time runtime split/bind.
    SystemBuild(Esp32s31BluetoothSystemBuildError<MT, SC, H2C, C2H, PC>),
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
    memory: Esp32s31BluetoothClaimedMemory,
    slot: Slot<MT, SC, H2C, C2H, PC>,
) -> Esp32s31BluetoothReservedFailure<Esp32s31BluetoothPoweredFailure<F>, MT, SC, H2C, C2H, PC> {
    Esp32s31BluetoothReservedFailure::new(Esp32s31BluetoothPoweredFailure { failure, memory }, slot)
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
    storage: &'static Esp32s31BluetoothSystemStorage<Platform, MT, SC, H2C, C2H, PC>,
    config: Esp32s31BluetoothColdStartConfig,
) -> Result<
    Esp32s31BluetoothColdStartOutput<MT, SC, H2C, C2H, PC>,
    Esp32s31BluetoothColdStartError<MT, SC, H2C, C2H, PC>,
> {
    let Esp32s31BluetoothColdStartConfig {
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
                return Err(Esp32s31BluetoothColdStartError::CalibrationSnapshotSchema {
                    observed: snapshot.schema,
                    hardware,
                });
            }
        },
        None => None,
    };
    if let Err(error) = EmbassyEsp32s31PhyTime::validate_timebase() {
        return Err(Esp32s31BluetoothColdStartError::Timebase { error, hardware });
    }
    let platform = match platform_root.try_bluetooth() {
        Ok(platform) => platform,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::PlatformBusy { error, hardware });
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
            return Err(Esp32s31BluetoothColdStartError::HciConfig { error, hardware });
        }
    };
    let hci = match HciResources::<H2C, C2H, PC>::new(hci_config) {
        Ok(hci) => hci,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::HciResources { error, hardware });
        }
    };
    let owners = Esp32s31BluetoothUnpoweredOwners { platform, hardware };
    let slot = match storage.reserve() {
        Ok(slot) => slot,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::StorageInUse { error, owners });
        }
    };
    let ble_phy_memory = match claim_production_ble_phy_memory() {
        Ok(memory) => memory,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::BlePhyMemory(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothBlePhyMemoryFailure { error, owners },
                    slot,
                ),
            ));
        }
    };
    let direction_finding_memory = match claim_production_direction_finding_memory() {
        Ok(memory) => memory,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::DirectionFindingMemory(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothDirectionFindingMemoryFailure {
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
            return Err(Esp32s31BluetoothColdStartError::DtmMemory(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothDtmMemoryFailure {
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
        BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(dtm.default_tx_power_dbm().dbm()),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::LegacyAdvertisingMemory(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothLegacyAdvertisingMemoryFailure {
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
            return Err(Esp32s31BluetoothColdStartError::PassiveScanMemory(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothPassiveScanMemoryFailure {
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
        BluetoothPeripheralConnectionRuntimeConfig::new(
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionDefaultTxPowerDbm::new(
                dtm.default_tx_power_dbm().dbm(),
            ),
        ),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::PeripheralConnectionMemory(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothPeripheralConnectionMemoryFailure {
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
            BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(dtm.default_tx_power_dbm().dbm()),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                return Err(
                    Esp32s31BluetoothColdStartError::LegacyConnectableAdvertisingMemory(
                        Esp32s31BluetoothReservedFailure::new(
                            Esp32s31BluetoothLegacyConnectableAdvertisingMemoryFailure {
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
                    ),
                );
            }
        };
    let memory = Esp32s31BluetoothClaimedMemory {
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
            return Err(Esp32s31BluetoothColdStartError::Clock(reserved_powered(
                failure, memory, slot,
            )));
        }
    };
    let scheduler = clocked
        .initialize_controller_hal()
        .initialize_scheduler(BluetoothControllerRuntimeResources::<MT, SC>::new());
    let low_power = match scheduler.initialize_modem_lp_timer_hardware() {
        Ok(low_power) => low_power,
        Err(failure) => {
            return Err(Esp32s31BluetoothColdStartError::LowPower(reserved_powered(
                failure, memory, slot,
            )));
        }
    };

    let mut phy_config = BluetoothPhyInitializationConfig::new(calibration_identity);
    if let Some(cache) = retained_calibration {
        phy_config = phy_config.with_calibration_cache(cache);
    }
    let registered = match low_power
        .initialize_common_phy::<EmbassyEsp32s31PhyTime, NoopPhyTargetObserver>(
            phy_config,
            NoopPhyTargetObserver,
        )
        .await
    {
        Ok(registered) => registered,
        Err(failure) => {
            return Err(Esp32s31BluetoothColdStartError::PhyInitialization(
                reserved_powered(failure, memory, slot),
            ));
        }
    };
    let mut clock = EmbassyEsp32s31PhyTime;
    let acquisition = match registered.acquire_phy_client(&mut clock) {
        Ok(acquisition) => acquisition,
        Err(failure) => {
            return Err(Esp32s31BluetoothColdStartError::PhyClientAcquire(
                reserved_powered(failure, memory, slot),
            ));
        }
    };
    let initialized = match acquisition.into_owner() {
        Ok(initialized) => initialized,
        Err(pending) => match pending
            .begin_tracking()
            .complete_tracking::<EmbassyEsp32s31PhyTime, NoopPhyTargetObserver>(
                NoopPhyTargetObserver,
            )
            .await
        {
            Ok(initialized) => initialized,
            Err(failure) => {
                return Err(Esp32s31BluetoothColdStartError::PhyTracking(
                    reserved_powered(failure, memory, slot),
                ));
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
    let recheck = match EmbassyBluetoothDtmAbsoluteRecheck::after_period(recheck_period) {
        Ok(recheck) => recheck,
        Err(error) => {
            return Err(Esp32s31BluetoothColdStartError::RecheckStart(
                Esp32s31BluetoothReservedFailure::new(
                    Esp32s31BluetoothRecheckStartFailure {
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
            return Err(Esp32s31BluetoothColdStartError::InterruptPublication(
                Esp32s31BluetoothReservedFailure::new(failure, slot),
            ));
        }
    };
    let bound = match published.bind_hci(hci) {
        Ok(bound) => bound,
        Err(failure) => {
            return Err(Esp32s31BluetoothColdStartError::HciBind(
                Esp32s31BluetoothReservedFailure::new(failure, slot),
            ));
        }
    };
    let system = slot
        .compose(bound, recheck)
        .map_err(Esp32s31BluetoothColdStartError::SystemBuild)?;

    Ok(Esp32s31BluetoothColdStartOutput {
        system,
        phy,
        baseband,
        ble_phy,
        calibration,
    })
}
