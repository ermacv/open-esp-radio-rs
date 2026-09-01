//! Production ownership and final IRQ composition for ESP32-S31 Bluetooth.
//!
//! This crate owns placement and one-time acquisition for the BLE PHY
//! environment, one DTM event graph, one legacy-advertising event graph and
//! one passive-scanner receive graph.
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
#[cfg(any(test, target_arch = "riscv32"))]
mod phy_time;
#[cfg(any(test, target_arch = "riscv32"))]
mod runner_policy;
#[cfg(target_arch = "riscv32")]
mod system;
#[cfg(target_arch = "riscv32")]
mod system_storage;

#[cfg(target_arch = "riscv32")]
pub use cold_start::{
    Esp32s31BluetoothBlePhyMemoryFailure, Esp32s31BluetoothClaimedMemory,
    Esp32s31BluetoothColdStartConfig, Esp32s31BluetoothColdStartError,
    Esp32s31BluetoothColdStartOutput, Esp32s31BluetoothDtmMemoryFailure,
    Esp32s31BluetoothLegacyAdvertisingMemoryFailure, Esp32s31BluetoothPassiveScanMemoryFailure,
    Esp32s31BluetoothPoweredFailure, Esp32s31BluetoothRecheckStartFailure,
    Esp32s31BluetoothReservedFailure, Esp32s31BluetoothUnpoweredOwners, start_esp32s31_bluetooth,
};
#[cfg(target_arch = "riscv32")]
pub use interrupt_runtime::{
    Esp32s31BluetoothInterruptBindError, Esp32s31BluetoothInterruptDisableFailure,
    Esp32s31BluetoothInterruptFault, Esp32s31BluetoothInterruptRuntime,
    bind_production_bluetooth_interrupt_runtime,
};
#[cfg(target_arch = "riscv32")]
pub use phy_time::{EmbassyEsp32s31PhyTime, EmbassyEsp32s31PhyTimeError};
#[cfg(target_arch = "riscv32")]
pub use system::{
    Esp32s31BluetoothHardwareRunner, Esp32s31BluetoothHostController,
    Esp32s31BluetoothInterruptCompositionFailure, Esp32s31BluetoothRunners,
    Esp32s31BluetoothSystem, Esp32s31BluetoothSystemBuildError, compose_esp32s31_bluetooth_system,
};
#[cfg(target_arch = "riscv32")]
pub use system_storage::{
    Esp32s31BluetoothPublishedController, Esp32s31BluetoothSystemSlot,
    Esp32s31BluetoothSystemStorage, Esp32s31BluetoothSystemStorageInUse,
};

use core::sync::atomic::{AtomicBool, Ordering};

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmRuntimeConfig, BluetoothDtmRuntimeResources,
    BluetoothLegacyAdvertisingDefaultTxPowerDbm, BluetoothLegacyAdvertisingRuntimeResources,
    BluetoothPassiveScanRuntimeConfig, BluetoothPassiveScanRuntimeResources,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothBlePhyEngineBindFailure, BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineStorage,
    BluetoothDtmMemoryGraphBindFailure, BluetoothDtmMemoryGraphStorage,
    BluetoothLegacyAdvertisingMemoryGraphBindFailure, BluetoothLegacyAdvertisingMemoryGraphStorage,
    BluetoothPassiveScanMemoryGraphBindFailure, BluetoothPassiveScanMemoryGraphStorage,
};
#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothBlePhyEngineModelAddress, BluetoothDtmMemoryGraphModelAddress,
    BluetoothLegacyAdvertisingMemoryGraphModelAddress, BluetoothPassiveScanMemoryGraphModelAddress,
};
use static_cell::ConstStaticCell;

/// One statically placed BLE PHY environment and resolving-list arena.
///
/// Claiming is permanent because the initialized BLE PHY engine retains both
/// published addresses until a future verified controller teardown.
pub struct Esp32s31BluetoothBlePhyMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<BluetoothBlePhyEngineStorage>,
}

impl Esp32s31BluetoothBlePhyMemory {
    /// Reserve one fresh arena without touching controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(BluetoothBlePhyEngineStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<&'static mut BluetoothBlePhyEngineStorage, Esp32s31BluetoothBlePhyMemoryClaimError>
    {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    ///
    /// The returned owner is still CPU-owned. The Controller lifecycle must
    /// consume and retain it before publishing either contained address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
    ) -> Result<BluetoothBlePhyEngineCpuOwned, Esp32s31BluetoothBlePhyMemoryClaimError> {
        let storage = self.begin_claim()?;
        BluetoothBlePhyEngineStorage::pin_static(storage)
            .map_err(Esp32s31BluetoothBlePhyMemoryClaimError::Placement)
    }

    /// Claim this arena with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: BluetoothBlePhyEngineModelAddress,
    ) -> Result<BluetoothBlePhyEngineCpuOwned, Esp32s31BluetoothBlePhyMemoryClaimError> {
        let storage = self.begin_claim()?;
        BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .map_err(Esp32s31BluetoothBlePhyMemoryClaimError::Placement)
    }
}

impl Default for Esp32s31BluetoothBlePhyMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production BLE PHY arena could not become a CPU-owned graph.
#[derive(Debug)]
pub enum Esp32s31BluetoothBlePhyMemoryClaimError {
    /// The unique static allocation was already claimed.
    InUse,
    /// Linker placement failed the complete BLE PHY storage contract.
    ///
    /// This retains the exact allocation and does not reopen the one-shot
    /// production claim after a placement failure.
    Placement(BluetoothBlePhyEngineBindFailure),
}

/// Claim the sole production BLE PHY environment and resolving-list graph.
///
/// The board linker places `.dma.bss.*` inputs in internal SRAM. Runtime
/// validation still checks the complete extent before any reviewed pointer is
/// installed in the storage graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_ble_phy_memory()
-> Result<BluetoothBlePhyEngineCpuOwned, Esp32s31BluetoothBlePhyMemoryClaimError> {
    PRODUCTION_BLE_PHY_MEMORY.claim()
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_ble_phy")]
static PRODUCTION_BLE_PHY_MEMORY: Esp32s31BluetoothBlePhyMemory =
    Esp32s31BluetoothBlePhyMemory::new();

/// One statically placed DTM allocation arena.
///
/// Claiming is permanent: there is no proven controller quiescence transition
/// that could safely make this allocation globally available again.
pub struct Esp32s31BluetoothDtmMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<BluetoothDtmMemoryGraphStorage>,
}

impl Esp32s31BluetoothDtmMemory {
    /// Reserve one fresh arena without touching controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(BluetoothDtmMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<&'static mut BluetoothDtmMemoryGraphStorage, Esp32s31BluetoothDtmMemoryClaimError>
    {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31BluetoothDtmMemoryClaimError::InUse);
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
        config: BluetoothDtmRuntimeConfig,
    ) -> Result<BluetoothDtmRuntimeResources, Esp32s31BluetoothDtmMemoryClaimError> {
        let storage = self.begin_claim()?;
        BluetoothDtmRuntimeResources::claim_static(storage, config)
            .map_err(Esp32s31BluetoothDtmMemoryClaimError::Placement)
    }

    /// Claim this arena with a deterministic native model address and explicit
    /// source-owned scheduler allocation configuration.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: BluetoothDtmMemoryGraphModelAddress,
        config: BluetoothDtmRuntimeConfig,
    ) -> Result<BluetoothDtmRuntimeResources, Esp32s31BluetoothDtmMemoryClaimError> {
        let storage = self.begin_claim()?;
        BluetoothDtmRuntimeResources::claim_static_model(storage, base, config)
            .map_err(Esp32s31BluetoothDtmMemoryClaimError::Placement)
    }
}

impl Default for Esp32s31BluetoothDtmMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production DTM arena could not become a CPU-owned graph.
#[derive(Debug)]
pub enum Esp32s31BluetoothDtmMemoryClaimError {
    /// The unique static allocation was already claimed.
    InUse,
    /// Linker placement failed the controller-memory binding contract.
    ///
    /// This variant retains the exact allocation; it is deliberately not
    /// discarded or made available for an unsafe retry at another address.
    Placement(BluetoothDtmMemoryGraphBindFailure),
}

/// Claim the sole production DTM runtime and controller-memory graph.
///
/// The section name is consumed by the board linker, which must place all
/// `.dma.bss.*` inputs in available internal SRAM. Runtime address validation
/// remains mandatory and fails closed if that contract drifts.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_dtm_runtime(
    config: BluetoothDtmRuntimeConfig,
) -> Result<BluetoothDtmRuntimeResources, Esp32s31BluetoothDtmMemoryClaimError> {
    PRODUCTION_DTM_MEMORY.claim(config)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_dtm")]
static PRODUCTION_DTM_MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

/// One statically placed legacy advertising graph.
///
/// The arena is independent from DTM because their affine lifecycles may not
/// exchange descriptors or synthesize ownership from an idle slot.
pub struct Esp32s31BluetoothLegacyAdvertisingMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<BluetoothLegacyAdvertisingMemoryGraphStorage>,
}

impl Esp32s31BluetoothLegacyAdvertisingMemory {
    /// Reserve one fresh arena without touching Controller memory or MMIO.
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(BluetoothLegacyAdvertisingMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<
        &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
    > {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31BluetoothLegacyAdvertisingMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<
        BluetoothLegacyAdvertisingRuntimeResources,
        Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
    > {
        let storage = self.begin_claim()?;
        BluetoothLegacyAdvertisingRuntimeResources::claim_static(storage, default_tx_power_dbm)
            .map_err(Esp32s31BluetoothLegacyAdvertisingMemoryClaimError::Placement)
    }

    /// Claim this arena with a deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<
        BluetoothLegacyAdvertisingRuntimeResources,
        Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
    > {
        let storage = self.begin_claim()?;
        BluetoothLegacyAdvertisingRuntimeResources::claim_static_model(
            storage,
            base,
            default_tx_power_dbm,
        )
        .map_err(Esp32s31BluetoothLegacyAdvertisingMemoryClaimError::Placement)
    }
}

impl Default for Esp32s31BluetoothLegacyAdvertisingMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production legacy advertising graph could not be claimed.
#[derive(Debug)]
pub enum Esp32s31BluetoothLegacyAdvertisingMemoryClaimError {
    InUse,
    Placement(BluetoothLegacyAdvertisingMemoryGraphBindFailure),
}

/// Claim the sole production legacy advertising graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_legacy_advertising_runtime(
    default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
) -> Result<
    BluetoothLegacyAdvertisingRuntimeResources,
    Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
> {
    PRODUCTION_LEGACY_ADVERTISING_MEMORY.claim(default_tx_power_dbm)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_legacy_advertising")]
static PRODUCTION_LEGACY_ADVERTISING_MEMORY: Esp32s31BluetoothLegacyAdvertisingMemory =
    Esp32s31BluetoothLegacyAdvertisingMemory::new();

/// One statically placed passive-scanner graph.
pub struct Esp32s31BluetoothPassiveScanMemory {
    claimed: AtomicBool,
    storage: ConstStaticCell<BluetoothPassiveScanMemoryGraphStorage>,
}

impl Esp32s31BluetoothPassiveScanMemory {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            storage: ConstStaticCell::new(BluetoothPassiveScanMemoryGraphStorage::new()),
        }
    }

    fn begin_claim(
        &'static self,
    ) -> Result<
        &'static mut BluetoothPassiveScanMemoryGraphStorage,
        Esp32s31BluetoothPassiveScanMemoryClaimError,
    > {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31BluetoothPassiveScanMemoryClaimError::InUse);
        }
        Ok(self.storage.take())
    }

    /// Claim and bind this arena using its real ESP32-S31 address.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
        config: BluetoothPassiveScanRuntimeConfig,
    ) -> Result<BluetoothPassiveScanRuntimeResources, Esp32s31BluetoothPassiveScanMemoryClaimError>
    {
        let storage = self.begin_claim()?;
        BluetoothPassiveScanRuntimeResources::claim_static(storage, config)
            .map_err(Esp32s31BluetoothPassiveScanMemoryClaimError::Placement)
    }

    /// Claim this arena with one deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: BluetoothPassiveScanMemoryGraphModelAddress,
        config: BluetoothPassiveScanRuntimeConfig,
    ) -> Result<BluetoothPassiveScanRuntimeResources, Esp32s31BluetoothPassiveScanMemoryClaimError>
    {
        let storage = self.begin_claim()?;
        BluetoothPassiveScanRuntimeResources::claim_static_model(storage, base, config)
            .map_err(Esp32s31BluetoothPassiveScanMemoryClaimError::Placement)
    }
}

impl Default for Esp32s31BluetoothPassiveScanMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the production passive-scanner graph could not be claimed.
#[derive(Debug)]
pub enum Esp32s31BluetoothPassiveScanMemoryClaimError {
    InUse,
    Placement(BluetoothPassiveScanMemoryGraphBindFailure),
}

/// Claim the sole production passive-scanner graph.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_passive_scan_runtime(
    config: BluetoothPassiveScanRuntimeConfig,
) -> Result<BluetoothPassiveScanRuntimeResources, Esp32s31BluetoothPassiveScanMemoryClaimError> {
    PRODUCTION_PASSIVE_SCAN_MEMORY.claim(config)
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_passive_scan")]
static PRODUCTION_PASSIVE_SCAN_MEMORY: Esp32s31BluetoothPassiveScanMemory =
    Esp32s31BluetoothPassiveScanMemory::new();

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth::{
        BluetoothDtmDefaultTxPowerDbm, BluetoothDtmRuntimeConfig,
        BluetoothLegacyAdvertisingDefaultTxPowerDbm, BluetoothPassiveScanRuntimeConfig,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothBlePhyEngineBindError,
        BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage,
        BluetoothDtmMemoryGraphBindError, BluetoothDtmMemoryGraphModelAddress,
        BluetoothDtmMemoryGraphStorage, BluetoothDtmSchedulerAllocationConfig,
        BluetoothLegacyAdvertisingMemoryGraphModelAddress, BluetoothPassiveScanDefaultTxPowerDbm,
        BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanSchedulerAllocationConfig,
    };

    use super::{
        Esp32s31BluetoothBlePhyMemory, Esp32s31BluetoothBlePhyMemoryClaimError,
        Esp32s31BluetoothDtmMemory, Esp32s31BluetoothDtmMemoryClaimError,
        Esp32s31BluetoothLegacyAdvertisingMemory,
        Esp32s31BluetoothLegacyAdvertisingMemoryClaimError, Esp32s31BluetoothPassiveScanMemory,
        Esp32s31BluetoothPassiveScanMemoryClaimError,
    };

    const fn runtime_config() -> BluetoothDtmRuntimeConfig {
        BluetoothDtmRuntimeConfig::new(
            BluetoothDtmSchedulerAllocationConfig::new(2, 3, 4),
            BluetoothDtmDefaultTxPowerDbm::new(6),
        )
    }

    #[test]
    fn model_ble_phy_arena_is_claimed_once_as_one_bound_graph() {
        static MEMORY: Esp32s31BluetoothBlePhyMemory = Esp32s31BluetoothBlePhyMemory::new();

        let base =
            BluetoothBlePhyEngineModelAddress::new(0x2f00_2000).expect("model base is encodable");
        let owner = MEMORY
            .claim_model(base)
            .expect("fresh model arena binds once");
        let (start, end) = owner.binding().range();
        assert_eq!(start, 0x2f00_2000);
        assert_eq!(
            end - start,
            size_of::<BluetoothBlePhyEngineStorage>() as u32
        );
        assert!(matches!(
            MEMORY.claim_model(base),
            Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse)
        ));
    }

    #[test]
    fn model_legacy_advertising_arena_is_claimed_once() {
        static MEMORY: Esp32s31BluetoothLegacyAdvertisingMemory =
            Esp32s31BluetoothLegacyAdvertisingMemory::new();
        let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_6000)
            .expect("model base is encodable");
        let runtime = MEMORY
            .claim_model(base, BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6))
            .expect("fresh advertising arena binds once");
        assert!(runtime.event_is_idle());
        assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
        assert!(matches!(
            MEMORY.claim_model(base, BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6)),
            Err(Esp32s31BluetoothLegacyAdvertisingMemoryClaimError::InUse)
        ));
    }

    #[test]
    fn model_passive_scanner_arena_is_claimed_once() {
        static MEMORY: Esp32s31BluetoothPassiveScanMemory =
            Esp32s31BluetoothPassiveScanMemory::new();
        let base = BluetoothPassiveScanMemoryGraphModelAddress::new(0x2f00_8000)
            .expect("model base is encodable");
        let config = BluetoothPassiveScanRuntimeConfig::new(
            BluetoothPassiveScanSchedulerAllocationConfig::new(2, 3)
                .expect("the product limits fit the scanner graph"),
            BluetoothPassiveScanDefaultTxPowerDbm::new(6),
        );
        let runtime = MEMORY
            .claim_model(base, config)
            .expect("fresh scanner arena binds once");
        assert!(runtime.event_is_idle());
        assert_eq!(runtime.config(), config);
        assert!(matches!(
            MEMORY.claim_model(base, config),
            Err(Esp32s31BluetoothPassiveScanMemoryClaimError::InUse)
        ));
    }

    #[test]
    fn ble_phy_placement_failure_is_sticky_and_retains_the_allocation() {
        static MEMORY: Esp32s31BluetoothBlePhyMemory = Esp32s31BluetoothBlePhyMemory::new();

        let crossing = BluetoothBlePhyEngineModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
                - size_of::<BluetoothBlePhyEngineStorage>() as u32
                + 4,
        )
        .expect("crossing model base is still encodable");
        let failure = match MEMORY.claim_model(crossing) {
            Err(Esp32s31BluetoothBlePhyMemoryClaimError::Placement(failure)) => failure,
            Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse) => {
                panic!("fresh arena cannot already be in use")
            }
            Ok(_) => panic!("crossing placement must fail closed"),
        };
        assert_eq!(
            failure.error(),
            BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram
        );
        let (_storage, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram
        );

        let valid = BluetoothBlePhyEngineModelAddress::new(0x2f00_2000)
            .expect("valid retry address is encodable");
        assert!(matches!(
            MEMORY.claim_model(valid),
            Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse)
        ));
    }

    #[test]
    fn model_arena_is_claimed_once_as_one_bound_graph() {
        static MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

        let base =
            BluetoothDtmMemoryGraphModelAddress::new(0x2f00_1000).expect("model base is encodable");
        let runtime = MEMORY
            .claim_model(base, runtime_config())
            .expect("fresh model arena binds once");
        assert_eq!(runtime.config(), runtime_config());
        assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
        assert!(runtime.session_is_idle());
        assert!(matches!(
            MEMORY.claim_model(base, runtime_config()),
            Err(Esp32s31BluetoothDtmMemoryClaimError::InUse)
        ));
    }

    #[test]
    fn placement_failure_is_sticky_and_retains_the_allocation() {
        static MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

        let crossing = BluetoothDtmMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
                - size_of::<BluetoothDtmMemoryGraphStorage>() as u32
                + 4,
        )
        .expect("crossing model base is still encodable");
        let failure = match MEMORY.claim_model(crossing, runtime_config()) {
            Err(Esp32s31BluetoothDtmMemoryClaimError::Placement(failure)) => failure,
            Err(Esp32s31BluetoothDtmMemoryClaimError::InUse) => {
                panic!("fresh arena cannot already be in use")
            }
            Ok(_) => panic!("crossing placement must fail closed"),
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        let (_storage, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
        );

        let valid = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_1000)
            .expect("valid retry address is encodable");
        assert!(matches!(
            MEMORY.claim_model(valid, runtime_config()),
            Err(Esp32s31BluetoothDtmMemoryClaimError::InUse)
        ));
    }
}
