//! Production ownership and final IRQ composition for ESP32-S31 Bluetooth.
//!
//! This crate owns placement and one-time acquisition for the BLE PHY
//! environment, the controller-global direction-finding workspace, one DTM
//! event graph, one legacy-advertising event graph, one passive-scanner
//! receive graph, one peripheral-connection allocation and its transferable
//! non-scanning RX pool, and one response-capable legacy-advertising graph.
//! It also installs the final target-only
//! bridge from the three typed ESP-HAL routes through the complete chip ISR
//! service to durable Embassy notification. Controller-memory layout and
//! address validation remain in the chip memory crate. Command/HCI ownership
//! is composed by the outer Controller runner rather than by an IRQ callback.
//!
//! IRQ composition has an explicit reversible boundary: failed binding returns
//! the unpublished service, while successful same-core three-route disable
//! returns `BluetoothInterruptDisabled`. Callback access stays inside a critical
//! section; the service and its durable first fault remain owned through rebind.
//! Dropping a live route owner leaves its publication occupied. The unstarted
//! hardware runner can cycle its routes while retaining all HCI and timer owners.
//! `retire_modem_timer` additionally returns the drained timer's HAL owner after
//! IRQ disable; `resume` restores it before rebind. Its idle HCI retirement join
//! retains command, timer and inactive IRQ owners in `BluetoothHardwareRetired`.
//! `run_until_idle(request)` also returns the runner from live command service
//! after the request resolves, timer work drains and both HCI FIFOs are observed
//! empty. It leaves IRQ and HCI barriers to the explicit retirement methods;
//! concurrent producers can still cause their lossless rejection. Active roles
//! require their normal Host stop commands. Keep the consuming future alive
//! until it returns; dropping it does not implement ownership retirement.
//! Terminal quarantine closes both HCI directions before disabling routes, so
//! blocked transport operations observe closure. Queued packets and unfinished
//! command/timer owners remain retained; graceful ACL credit drain is separate.
//! These transitions do not stop BTBB/DMA or release the shared PHY. Exclusive
//! task HAL, PHY/DF and role-memory leases follow the actor through handoff and quarantine.
//! Configured `run_with_phy_maintenance` additionally preserves a hard PHY
//! deadline even through terminal quarantine. At expiry, typed shared-RF
//! fail-stop closes HCI and the current S31 backend resets the full SoC because
//! local active-DTM/all-RF quiescence is not proven. It never pauses DTM or
//! synthesizes Host Test End. Normal idle/ACL maintenance borrows the matching
//! platform and restores the same HCI, timer and IRQ owners.
//! Terminal maintenance errors, including a missed peripheral restoration
//! deadline, escalate immediately rather than waiting for the next PHY due
//! time. The composition retains the failed frontier and requests system reset
//! through the SoC adapter; radio drivers do not own a watchdog or reset
//! peripheral. IRQ cleanup and Host acknowledgement do not gate that request.
//! Other terminal faults retain the quarantine/hard-deadline behavior above.
//! In addition, cold start requires a stable caller-owned [`WatchdogConfig`]
//! with explicit startup, maintenance and shutdown budgets. The board owns the
//! independent SoC TIMG1 service. One lease covers each admitted physical
//! transaction through restoration; peripheral maintenance retains it until
//! guarded RUN or proven idle, not merely PHY return. Cancellation does not
//! disarm it. No default or qualified reset-to-RF-off latency is supplied.
//! Cold start arms before clock/PHY initialization; powered restart arms before
//! its physical restart and completes after timer/IRQ rebinding. Physical
//! release arms before RF close and completes only at cold ownership or a
//! retained, proven-safe rejected frontier. Manual maintenance starts from
//! synchronously retired idle timer/IRQ owners, before extracting/executing PHY;
//! automatic maintenance arms before disabling their routes. Protocol admission
//! and waiting for a future legal window do not arm a physical lease.
//! HCI retirement extracts their actual owners; rejection preserves the runnable
//! actor. Cold start also returns `platform` beside `system`: retain it until
//! `BluetoothHardwareRetired::try_retire_interrupts` removes the actual shared
//! primary/NRT register owner from inactive ISR storage. Its post-route HAL state
//! admits `try_release_controller_output` only after a hardware idle/head/fault
//! preflight. The resulting state cannot reactivate routes. Empty
//! ISR slots remain reserved for this epoch while static Controller borrows exist.
//! `BluetoothHardwareOutputReleased::join_platform` checks the same HCI epoch and extracts
//! the platform reservation. `BluetoothHardwareRetiredWithPlatform` keeps its Drop
//! suppressed through `release_physical`: last-client RF close, temperature
//! power-down, Bluetooth reset and checked clock restoration return
//! `BluetoothHardwareColdReleased`. Mismatched joins return both owners.
//! RF-close ambiguity and restart PHY execution without completed cleanup
//! request the same system reset as terminal maintenance. Pre-close rejection,
//! completed registration cleanup, already-closed hardware reunion and non-PHY
//! restart failures instead retain their non-runnable frontiers. No generic
//! error code substitutes for the retained hardware stage in this decision.
//! Initial cold start applies the same rule to registration and first tracking.
//! Manual idle maintenance distinguishes admission from execution/restoration;
//! only the latter request reset, without dropping PHY, IRQ or HCI owners first.
//! Role retirement requires all five allocation owners, including advertising
//! generations and the peripheral RX topology, before closing HCI. Extracted
//! owners remain private; stable software storage is still borrowed. A
//! cold restart uses those original leases through `BluetoothHardwareColdReleased::restart`:
//! it reinitializes the returned radio, restores the same ISR reservation and
//! issues a new HCI generation whose old Host handles remain closed.
//! `BluetoothHardwareTimerRetired::maintain_phy` instead executes due tracking
//! with the same open HCI and powered counter epoch, then restores owners and
//! routes. This manual entry requires an idle runner. Its optional calibration
//! debug policy temporarily selects the existing thermal thresholds, preserves
//! real sensor readings and restores the original policy on success. Failed or
//! cancelled physical work cannot resume. The automatic entry also admits ACL
//! windows; active DTM stays non-preemptible.
//!
//! [`maintenance_observation`] records real PHY child completions and durations,
//! physical return and the driver's guarded RUN publication. Boot aggregates
//! survive idle/ACL transitions; timestamps describe the latest transaction,
//! which has no RUN when performed in idle. A physical return does not count as
//! restored RUN. Observations grant no RF authority, and measured maxima include
//! scheduling/observer overhead without establishing worst-case time bounds.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(target_arch = "riscv32")]
mod cold_start;
#[cfg(target_arch = "riscv32")]
pub mod entropy;
#[cfg(target_arch = "riscv32")]
mod watchdog;
#[cfg(target_arch = "riscv32")]
pub use watchdog::WatchdogConfig;
#[cfg(any(test, target_arch = "riscv32"))]
mod interrupt_fault;
#[cfg(any(test, target_arch = "riscv32"))]
mod interrupt_publication;
#[cfg(target_arch = "riscv32")]
mod interrupt_runtime;
#[cfg(any(test, target_arch = "riscv32"))]
mod runner_policy;
#[cfg(target_arch = "riscv32")]
mod system;
#[cfg(target_arch = "riscv32")]
mod system_storage;
mod trouble;

#[cfg(target_arch = "riscv32")]
pub use cold_start::{
    BluetoothBlePhyMemoryFailure, BluetoothClaimedMemory, BluetoothColdStartConfig,
    BluetoothColdStartError, BluetoothColdStartOutput, BluetoothDirectionFindingMemoryFailure,
    BluetoothDtmMemoryFailure, BluetoothLegacyAdvertisingMemoryFailure,
    BluetoothLegacyConnectableAdvertisingMemoryFailure, BluetoothPassiveScanMemoryFailure,
    BluetoothPeripheralConnectionMemoryFailure, BluetoothPoweredFailure,
    BluetoothRecheckStartFailure, BluetoothReservedFailure, BluetoothUnpoweredOwners,
    start_esp32s31_bluetooth,
};
#[cfg(target_arch = "riscv32")]
pub use interrupt_runtime::{
    BluetoothInterruptBindError, BluetoothInterruptBindFailure, BluetoothInterruptDisableFailure,
    BluetoothInterruptDisabled, BluetoothInterruptFault, BluetoothInterruptRuntime,
    bind_production_bluetooth_interrupt_runtime,
};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_bluetooth_embassy::time::phy::{EmbassyPhyTime, EmbassyPhyTimeError};
#[cfg(target_arch = "riscv32")]
pub use system::{
    BluetoothHardwareColdReleased, BluetoothHardwareInterruptsRetired,
    BluetoothHardwareMaintenanceError, BluetoothHardwareMaintenanceFailure,
    BluetoothHardwareOutputReleased, BluetoothHardwareRestartError,
    BluetoothHardwareRestartFailure, BluetoothHardwareRetired,
    BluetoothHardwareRetiredWithPlatform, BluetoothHardwareRouteCycleFailure,
    BluetoothHardwareRunner, BluetoothHardwareShutdownFailure, BluetoothHardwareTimerError,
    BluetoothHardwareTimerFailure, BluetoothHardwareTimerRetired, BluetoothHostAclCredits,
    BluetoothHostController, BluetoothInterruptCompositionFailure, BluetoothPlatformJoin,
    BluetoothRunners, BluetoothSystem, BluetoothSystemBuildError,
    compose_esp32s31_bluetooth_system,
};
#[cfg(target_arch = "riscv32")]
pub use system_storage::{
    BluetoothPublishedController, BluetoothSystemReady, BluetoothSystemSlot,
    BluetoothSystemStorage, BluetoothSystemStorageInUse,
};
pub use trouble::BluetoothTroubleSystem;

// Cold-start inputs outside the driver's facade path, so an application can
// start the Controller through this crate (or the `oer` facade) alone.
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_bluetooth_embassy::controller::DtmRecheckPeriod;
pub use oer_esp32s31_bluetooth_memory::{
    DtmSchedulerAllocationConfig, PassiveScanDefaultTxPowerDbm,
    PassiveScanSchedulerAllocationConfig, PeripheralConnectionDefaultTxPowerDbm,
};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_radio_platform_esp_hal::{EspHalBluetoothPlatform, EspHalRadioPlatform};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_soc::watchdog::{DeadlineBudget, DeadlineWatchdog};

#[cfg(not(target_arch = "riscv32"))]
use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineModelAddress, DirectionFindingWorkspaceModelAddress, DtmMemoryGraphModelAddress,
    LegacyAdvertisingMemoryGraphModelAddress, LegacyConnectableAdvertisingMemoryGraphModelAddress,
    NonScanningRxMemoryModelAddress, PassiveScanMemoryGraphModelAddress,
    PeripheralConnectionMemoryGraphModelAddress,
};

use core::sync::atomic::{AtomicBool, Ordering};

use oer_esp32s31_bluetooth::le::{
    advertising::{
        LegacyAdvertisingDefaultTxPowerDbm, LegacyAdvertisingRuntimeResources,
        LegacyConnectableAdvertisingRuntimeResources,
    },
    dtm::{DtmRuntimeConfig, DtmRuntimeResources},
    peripheral::{
        PeripheralConnectionRuntimeClaimError, PeripheralConnectionRuntimeConfig,
        PeripheralConnectionRuntimeResources,
    },
    scanning::{PassiveScanRuntimeConfig, PassiveScanRuntimeResources},
};

use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineBindFailure, BlePhyEngineCpuOwned, BlePhyEngineStorage,
    DirectionFindingWorkspaceBindFailure, DirectionFindingWorkspaceCpuOwned,
    DirectionFindingWorkspaceStorage, DtmMemoryGraphBindFailure, DtmMemoryGraphStorage,
    LegacyAdvertisingMemoryGraphBindFailure, LegacyAdvertisingMemoryGraphStorage,
    LegacyConnectableAdvertisingMemoryGraphBindFailure,
    LegacyConnectableAdvertisingMemoryGraphStorage, NonScanningRxMemoryStorage,
    PassiveScanMemoryGraphBindFailure, PassiveScanMemoryGraphStorage,
    PeripheralConnectionMemoryGraphStorage,
};

use static_cell::ConstStaticCell;

/// One statically placed BLE PHY environment and resolving-list arena.
///
/// Claiming is permanent because the initialized BLE PHY engine retains both
/// published addresses until a future verified controller teardown.
pub struct BluetoothBlePhyMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<BlePhyEngineStorage>,
}

impl BluetoothBlePhyMemory {
    /// Reserve one fresh arena without touching controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(BlePhyEngineStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<&'static mut BlePhyEngineStorage, BluetoothBlePhyMemoryClaimError> {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothBlePhyMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    ///
    /// The returned owner is still CPU-owned. The Controller lifecycle must
    /// consume and retain it before publishing either contained address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(&'static self) -> Result<BlePhyEngineCpuOwned, BluetoothBlePhyMemoryClaimError> {
        let storage = self.begin_claim()?;
        BlePhyEngineStorage::pin_static(storage).map_err(BluetoothBlePhyMemoryClaimError::Placement)
    }

    /// Claim this arena with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: BlePhyEngineModelAddress,
    ) -> Result<BlePhyEngineCpuOwned, BluetoothBlePhyMemoryClaimError> {
        let storage = self.begin_claim()?;
        BlePhyEngineStorage::pin_static_model(storage, base)
            .map_err(BluetoothBlePhyMemoryClaimError::Placement)
    }
}

impl Default for BluetoothBlePhyMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production BLE PHY arena could not become a CPU-owned graph.
#[derive(Debug)]
pub enum BluetoothBlePhyMemoryClaimError {
    /// The unique static allocation was already claimed.
    InUse,
    /// Linker placement failed the complete BLE PHY storage contract.
    ///
    /// This retains the exact allocation and does not reopen the one-shot
    /// production claim after a placement failure.
    Placement(BlePhyEngineBindFailure),
}

/// Claim the sole production BLE PHY environment and resolving-list graph.
///
/// The board linker places `.dma.bss.*` inputs in internal SRAM. Runtime
/// validation still checks the complete extent before any reviewed pointer is
/// installed in the storage graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_ble_phy_memory()
-> Result<BlePhyEngineCpuOwned, BluetoothBlePhyMemoryClaimError> {
    PRODUCTION_BLE_PHY_MEMORY.claim()
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_ble_phy")]
static PRODUCTION_BLE_PHY_MEMORY: BluetoothBlePhyMemory = BluetoothBlePhyMemory::new();

/// One statically placed controller-global direction-finding workspace.
///
/// Ordinary BLE roles retain the disabled-CTE baseline even when IQ sampling
/// is not enabled. The claim is therefore part of Controller cold start rather
/// than a per-connection or optional direction-finding allocation.
pub struct BluetoothDirectionFindingMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<DirectionFindingWorkspaceStorage>,
}

impl BluetoothDirectionFindingMemory {
    /// Reserve one fresh workspace without touching controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(DirectionFindingWorkspaceStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<
        &'static mut DirectionFindingWorkspaceStorage,
        BluetoothDirectionFindingMemoryClaimError,
    > {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothDirectionFindingMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this workspace using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
    ) -> Result<DirectionFindingWorkspaceCpuOwned, BluetoothDirectionFindingMemoryClaimError> {
        let storage = self.begin_claim()?;
        DirectionFindingWorkspaceStorage::pin_static(storage)
            .map_err(BluetoothDirectionFindingMemoryClaimError::Placement)
    }

    /// Claim this workspace with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: DirectionFindingWorkspaceModelAddress,
    ) -> Result<DirectionFindingWorkspaceCpuOwned, BluetoothDirectionFindingMemoryClaimError> {
        let storage = self.begin_claim()?;
        DirectionFindingWorkspaceStorage::pin_static_model(storage, base)
            .map_err(BluetoothDirectionFindingMemoryClaimError::Placement)
    }
}

impl Default for BluetoothDirectionFindingMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production direction-finding workspace could not be claimed.
#[derive(Debug)]
pub enum BluetoothDirectionFindingMemoryClaimError {
    /// The unique static allocation was already claimed.
    InUse,
    /// Linker placement failed the complete workspace contract.
    Placement(DirectionFindingWorkspaceBindFailure),
}

/// Claim the sole production controller-global direction-finding workspace.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_direction_finding_memory()
-> Result<DirectionFindingWorkspaceCpuOwned, BluetoothDirectionFindingMemoryClaimError> {
    PRODUCTION_DIRECTION_FINDING_MEMORY.claim()
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_direction_finding")]
static PRODUCTION_DIRECTION_FINDING_MEMORY: BluetoothDirectionFindingMemory =
    BluetoothDirectionFindingMemory::new();

/// One statically placed DTM allocation arena.
///
/// Claiming is permanent: there is no proven controller quiescence transition
/// that could safely make this allocation globally available again.
pub struct BluetoothDtmMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<DtmMemoryGraphStorage>,
}

impl BluetoothDtmMemory {
    /// Reserve one fresh arena without touching controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(DtmMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<&'static mut DtmMemoryGraphStorage, BluetoothDtmMemoryClaimError> {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothDtmMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    ///
    /// The caller must supply the controller limits and reviewed private fact
    /// from the source configuration associated with this firmware build. The
    /// returned runtime retains an idle CPU-owned graph unreachable by hardware.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        config: DtmRuntimeConfig,
    ) -> Result<DtmRuntimeResources, BluetoothDtmMemoryClaimError> {
        let storage = self.begin_claim()?;
        DtmRuntimeResources::claim_static(storage, config)
            .map_err(BluetoothDtmMemoryClaimError::Placement)
    }

    /// Claim this arena with a deterministic native model address and explicit
    /// source-owned scheduler allocation configuration.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: DtmMemoryGraphModelAddress,
        config: DtmRuntimeConfig,
    ) -> Result<DtmRuntimeResources, BluetoothDtmMemoryClaimError> {
        let storage = self.begin_claim()?;
        DtmRuntimeResources::claim_static_model(storage, base, config)
            .map_err(BluetoothDtmMemoryClaimError::Placement)
    }
}

impl Default for BluetoothDtmMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production DTM arena could not become a CPU-owned graph.
#[derive(Debug)]
pub enum BluetoothDtmMemoryClaimError {
    /// The unique static allocation was already claimed.
    InUse,
    /// Linker placement failed the controller-memory binding contract.
    ///
    /// This variant retains the exact allocation; it is deliberately not
    /// discarded or made available for an unsafe retry at another address.
    Placement(DtmMemoryGraphBindFailure),
}

/// Claim the sole production DTM runtime and controller-memory graph.
///
/// The section name is consumed by the board linker, which must place all
/// `.dma.bss.*` inputs in available internal SRAM. Runtime address validation
/// remains mandatory and fails closed if that contract drifts.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_dtm_runtime(
    config: DtmRuntimeConfig,
) -> Result<DtmRuntimeResources, BluetoothDtmMemoryClaimError> {
    PRODUCTION_DTM_MEMORY.claim(config)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_dtm")]
static PRODUCTION_DTM_MEMORY: BluetoothDtmMemory = BluetoothDtmMemory::new();

/// One statically placed legacy advertising graph.
///
/// The arena is independent from DTM because their affine lifecycles may not
/// exchange descriptors or synthesize ownership from an idle slot.
pub struct BluetoothLegacyAdvertisingMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<LegacyAdvertisingMemoryGraphStorage>,
}

impl BluetoothLegacyAdvertisingMemory {
    /// Reserve one fresh arena without touching Controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(LegacyAdvertisingMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<
        &'static mut LegacyAdvertisingMemoryGraphStorage,
        BluetoothLegacyAdvertisingMemoryClaimError,
    > {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothLegacyAdvertisingMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<LegacyAdvertisingRuntimeResources, BluetoothLegacyAdvertisingMemoryClaimError> {
        let storage = self.begin_claim()?;
        LegacyAdvertisingRuntimeResources::claim_static(storage, default_tx_power_dbm)
            .map_err(BluetoothLegacyAdvertisingMemoryClaimError::Placement)
    }

    /// Claim this arena with a deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: LegacyAdvertisingMemoryGraphModelAddress,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<LegacyAdvertisingRuntimeResources, BluetoothLegacyAdvertisingMemoryClaimError> {
        let storage = self.begin_claim()?;
        LegacyAdvertisingRuntimeResources::claim_static_model(storage, base, default_tx_power_dbm)
            .map_err(BluetoothLegacyAdvertisingMemoryClaimError::Placement)
    }
}

impl Default for BluetoothLegacyAdvertisingMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production legacy advertising graph could not be claimed.
#[derive(Debug)]
pub enum BluetoothLegacyAdvertisingMemoryClaimError {
    InUse,
    Placement(LegacyAdvertisingMemoryGraphBindFailure),
}

/// Claim the sole production legacy advertising graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_legacy_advertising_runtime(
    default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
) -> Result<LegacyAdvertisingRuntimeResources, BluetoothLegacyAdvertisingMemoryClaimError> {
    PRODUCTION_LEGACY_ADVERTISING_MEMORY.claim(default_tx_power_dbm)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_legacy_advertising")]
static PRODUCTION_LEGACY_ADVERTISING_MEMORY: BluetoothLegacyAdvertisingMemory =
    BluetoothLegacyAdvertisingMemory::new();

/// One statically placed passive-scanner graph.
pub struct BluetoothPassiveScanMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<PassiveScanMemoryGraphStorage>,
}

impl BluetoothPassiveScanMemory {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(PassiveScanMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<&'static mut PassiveScanMemoryGraphStorage, BluetoothPassiveScanMemoryClaimError>
    {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothPassiveScanMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        config: PassiveScanRuntimeConfig,
    ) -> Result<PassiveScanRuntimeResources, BluetoothPassiveScanMemoryClaimError> {
        let storage = self.begin_claim()?;
        PassiveScanRuntimeResources::claim_static(storage, config)
            .map_err(BluetoothPassiveScanMemoryClaimError::Placement)
    }

    /// Claim this arena with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: PassiveScanMemoryGraphModelAddress,
        config: PassiveScanRuntimeConfig,
    ) -> Result<PassiveScanRuntimeResources, BluetoothPassiveScanMemoryClaimError> {
        let storage = self.begin_claim()?;
        PassiveScanRuntimeResources::claim_static_model(storage, base, config)
            .map_err(BluetoothPassiveScanMemoryClaimError::Placement)
    }
}

impl Default for BluetoothPassiveScanMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production passive-scanner graph could not be claimed.
#[derive(Debug)]
pub enum BluetoothPassiveScanMemoryClaimError {
    InUse,
    Placement(PassiveScanMemoryGraphBindFailure),
}

/// Claim the sole production passive-scanner graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_passive_scan_runtime(
    config: PassiveScanRuntimeConfig,
) -> Result<PassiveScanRuntimeResources, BluetoothPassiveScanMemoryClaimError> {
    PRODUCTION_PASSIVE_SCAN_MEMORY.claim(config)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_passive_scan")]
static PRODUCTION_PASSIVE_SCAN_MEMORY: BluetoothPassiveScanMemory =
    BluetoothPassiveScanMemory::new();

/// One statically placed peripheral-connection allocation graph.
///
/// The graph is claimed during cold start but remains CPU-owned and cannot be
/// published until the connection event-image boundary is recovered.
pub struct BluetoothPeripheralConnectionMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<PeripheralConnectionMemoryGraphStorage>,
    receive_storage: ConstStaticCell<NonScanningRxMemoryStorage>,
}

impl BluetoothPeripheralConnectionMemory {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(PeripheralConnectionMemoryGraphStorage::new()),
            receive_storage: ConstStaticCell::new(NonScanningRxMemoryStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<
        (
            &'static mut PeripheralConnectionMemoryGraphStorage,
            &'static mut NonScanningRxMemoryStorage,
        ),
        BluetoothPeripheralConnectionMemoryClaimError,
    > {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothPeripheralConnectionMemoryClaimError::InUse);
        }
        Ok((self.storage.take(), self.receive_storage.take()))
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        config: PeripheralConnectionRuntimeConfig,
    ) -> Result<PeripheralConnectionRuntimeResources, BluetoothPeripheralConnectionMemoryClaimError>
    {
        let (storage, receive_storage) = self.begin_claim()?;
        PeripheralConnectionRuntimeResources::claim_static(storage, receive_storage, config)
            .map_err(BluetoothPeripheralConnectionMemoryClaimError::Placement)
    }

    /// Claim this arena with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: PeripheralConnectionMemoryGraphModelAddress,
        receive_base: NonScanningRxMemoryModelAddress,
        config: PeripheralConnectionRuntimeConfig,
    ) -> Result<PeripheralConnectionRuntimeResources, BluetoothPeripheralConnectionMemoryClaimError>
    {
        let (storage, receive_storage) = self.begin_claim()?;
        PeripheralConnectionRuntimeResources::claim_static_model(
            storage,
            base,
            receive_storage,
            receive_base,
            config,
        )
        .map_err(BluetoothPeripheralConnectionMemoryClaimError::Placement)
    }
}

impl Default for BluetoothPeripheralConnectionMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production peripheral-connection allocation could not be claimed.
#[derive(Debug)]
pub enum BluetoothPeripheralConnectionMemoryClaimError {
    InUse,
    Placement(PeripheralConnectionRuntimeClaimError),
}

/// Claim the sole production peripheral-connection allocation graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_peripheral_connection_runtime(
    config: PeripheralConnectionRuntimeConfig,
) -> Result<PeripheralConnectionRuntimeResources, BluetoothPeripheralConnectionMemoryClaimError> {
    PRODUCTION_PERIPHERAL_CONNECTION_MEMORY.claim(config)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_peripheral_connection")]
static PRODUCTION_PERIPHERAL_CONNECTION_MEMORY: BluetoothPeripheralConnectionMemory =
    BluetoothPeripheralConnectionMemory::new();

/// One statically placed response-capable legacy-advertising graph.
///
/// This allocation is disjoint from the nonconnectable advertiser and the
/// peripheral-connection graph. Cold start claims it last and the published
/// task service retains it until connectable scheduling is implemented.
pub struct BluetoothLegacyConnectableAdvertisingMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<LegacyConnectableAdvertisingMemoryGraphStorage>,
}

impl BluetoothLegacyConnectableAdvertisingMemory {
    /// Reserve one fresh arena without touching Controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(LegacyConnectableAdvertisingMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<
        &'static mut LegacyConnectableAdvertisingMemoryGraphStorage,
        BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    > {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BluetoothLegacyConnectableAdvertisingMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<
        LegacyConnectableAdvertisingRuntimeResources,
        BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    > {
        let storage = self.begin_claim()?;
        LegacyConnectableAdvertisingRuntimeResources::claim_static(storage, default_tx_power_dbm)
            .map_err(BluetoothLegacyConnectableAdvertisingMemoryClaimError::Placement)
    }

    /// Claim this arena with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: LegacyConnectableAdvertisingMemoryGraphModelAddress,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<
        LegacyConnectableAdvertisingRuntimeResources,
        BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    > {
        let storage = self.begin_claim()?;
        LegacyConnectableAdvertisingRuntimeResources::claim_static_model(
            storage,
            base,
            default_tx_power_dbm,
        )
        .map_err(BluetoothLegacyConnectableAdvertisingMemoryClaimError::Placement)
    }
}

impl Default for BluetoothLegacyConnectableAdvertisingMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the response-capable legacy-advertising graph could not be claimed.
#[derive(Debug)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryClaimError {
    InUse,
    Placement(LegacyConnectableAdvertisingMemoryGraphBindFailure),
}

/// Claim the sole production response-capable legacy-advertising graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_legacy_connectable_advertising_runtime(
    default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
) -> Result<
    LegacyConnectableAdvertisingRuntimeResources,
    BluetoothLegacyConnectableAdvertisingMemoryClaimError,
> {
    PRODUCTION_LEGACY_CONNECTABLE_ADVERTISING_MEMORY.claim(default_tx_power_dbm)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_legacy_connectable_advertising")]
static PRODUCTION_LEGACY_CONNECTABLE_ADVERTISING_MEMORY:
    BluetoothLegacyConnectableAdvertisingMemory =
    BluetoothLegacyConnectableAdvertisingMemory::new();

#[cfg(test)]
mod tests;

#[cfg(any(test, target_arch = "riscv32"))]
pub mod diagnostics;

/// Measured production PHY handoff and guarded successor publication.
pub mod maintenance_observation;
