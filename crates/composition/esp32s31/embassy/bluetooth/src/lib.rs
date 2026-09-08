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

#![no_std]
#![deny(unsafe_code)]

#[cfg(target_arch = "riscv32")]
mod cold_start;
#[cfg(any(test, target_arch = "riscv32"))]
mod interrupt_fault;
#[cfg(target_arch = "riscv32")]
mod interrupt_runtime;
#[cfg(target_arch = "riscv32")]
mod phy_time;
#[cfg(any(test, target_arch = "riscv32"))]
mod runner_policy;
#[cfg(target_arch = "riscv32")]
mod system;
#[cfg(target_arch = "riscv32")]
mod system_storage;

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
    BluetoothInterruptBindError, BluetoothInterruptDisableFailure, BluetoothInterruptFault,
    BluetoothInterruptRuntime, bind_production_bluetooth_interrupt_runtime,
};
#[cfg(target_arch = "riscv32")]
pub use phy_time::{EmbassyPhyTime, EmbassyPhyTimeError};
#[cfg(target_arch = "riscv32")]
pub use system::{
    BluetoothHardwareRunner, BluetoothHostController, BluetoothInterruptCompositionFailure,
    BluetoothRunners, BluetoothSystem, BluetoothSystemBuildError,
    compose_esp32s31_bluetooth_system,
};
#[cfg(target_arch = "riscv32")]
pub use system_storage::{
    BluetoothPublishedController, BluetoothSystemSlot, BluetoothSystemStorage,
    BluetoothSystemStorageInUse,
};

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
