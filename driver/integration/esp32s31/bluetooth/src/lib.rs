//! Production ownership of ESP32-S31 Bluetooth controller storage.
//!
//! This crate owns placement and one-time acquisition. Controller-memory
//! layout and address validation remain in the chip memory crate; a future
//! async runner will aggregate the returned CPU owner with HAL and IRQ owners.

#![no_std]
#![deny(unsafe_code)]

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphBindFailure, BluetoothDtmMemoryGraphCpuOwned,
    BluetoothDtmMemoryGraphStorage,
};
use static_cell::ConstStaticCell;

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
    /// The returned graph remains CPU-owned and unreachable by hardware.
    #[cfg(target_arch = "riscv32")]
    pub fn claim(
        &'static self,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, Esp32s31BluetoothDtmMemoryClaimError> {
        let storage = self.begin_claim()?;
        BluetoothDtmMemoryGraphStorage::pin_static(storage)
            .map_err(Esp32s31BluetoothDtmMemoryClaimError::Placement)
    }

    /// Claim this arena with a deterministic native model address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_model(
        &'static self,
        base: BluetoothDtmMemoryGraphModelAddress,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, Esp32s31BluetoothDtmMemoryClaimError> {
        let storage = self.begin_claim()?;
        BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base)
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

/// Claim the sole production DTM controller-memory graph.
///
/// The section name is consumed by the board linker, which must place all
/// `.dma.bss.*` inputs in available internal SRAM. Runtime address validation
/// remains mandatory and fails closed if that contract drifts.
#[cfg(target_arch = "riscv32")]
pub fn claim_production_dtm_memory()
-> Result<BluetoothDtmMemoryGraphCpuOwned, Esp32s31BluetoothDtmMemoryClaimError> {
    PRODUCTION_DTM_MEMORY.claim()
}

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the production linker must retain controller storage in internal SRAM"
)]
#[unsafe(link_section = ".dma.bss.open_radio_bluetooth_dtm")]
static PRODUCTION_DTM_MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothDtmMemoryGraphBindError,
        BluetoothDtmMemoryGraphModelAddress,
    };

    use super::{Esp32s31BluetoothDtmMemory, Esp32s31BluetoothDtmMemoryClaimError};

    #[test]
    fn model_arena_is_claimed_once_as_one_bound_graph() {
        static MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

        let base =
            BluetoothDtmMemoryGraphModelAddress::new(0x2f00_1000).expect("model base is encodable");
        let owner = MEMORY
            .claim_model(base)
            .expect("fresh model arena binds once");
        assert_eq!(owner.binding().range(), (0x2f00_1000, 0x2f00_13a8));
        assert!(matches!(
            MEMORY.claim_model(base),
            Err(Esp32s31BluetoothDtmMemoryClaimError::InUse)
        ));
    }

    #[test]
    fn placement_failure_is_sticky_and_retains_the_allocation() {
        static MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

        let crossing = BluetoothDtmMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8 + 4,
        )
        .expect("crossing model base is still encodable");
        let failure = match MEMORY.claim_model(crossing) {
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
            MEMORY.claim_model(valid),
            Err(Esp32s31BluetoothDtmMemoryClaimError::InUse)
        ));
    }
}
