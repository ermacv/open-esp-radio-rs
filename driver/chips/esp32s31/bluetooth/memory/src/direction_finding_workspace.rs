//! Controller-global SRAM workspace retained by ordinary BLE link states.
//!
//! The recovered ESP32-S31 controller allocates one `0x20`-byte direction-
//! finding environment even when no IQ-sampling procedure is enabled. The
//! open driver owns that allocation directly: it initializes only the two
//! reviewed baseline members, validates the complete physical extent and
//! exposes opaque typed links rather than a vendor allocator or raw words.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use vcell::VolatileCell;

use crate::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
    BluetoothControllerSramLinkAddress,
};

/// Bytes retained by the recovered controller-global direction-finding environment.
pub const BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES: usize = 0x20;

const WORKSPACE_WORDS: usize = BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES / size_of::<u32>();
const LINK_STATE_CONFIGURATION_WORD: usize = 2;
const DISABLED_CTE_DESCRIPTOR_WORD: usize = 3;
const LINK_STATE_CONFIGURATION_OFFSET: u32 = 0x08;
const DISABLED_CTE_DESCRIPTOR_OFFSET: u32 = 0x0c;

/// Static storage for the controller-global disabled-CTE baseline.
///
/// Volatile cells reflect that the Controller may observe and later mutate the
/// published descriptor. No public API grants CPU access after publication.
#[repr(C, align(4))]
pub struct BluetoothDirectionFindingWorkspaceStorage {
    words: [VolatileCell<u32>; WORKSPACE_WORDS],
    _pin: PhantomPinned,
}

const _: () = {
    assert!(size_of::<BluetoothDirectionFindingWorkspaceStorage>() == 0x20);
    assert!(align_of::<BluetoothDirectionFindingWorkspaceStorage>() == 4);
};

/// Why the direction-finding workspace cannot become a bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDirectionFindingWorkspaceBindError {
    /// A target pointer cannot be represented by the 32-bit address space.
    AddressWidth,
    /// The base is outside the Controller pointer domain.
    InvalidBase(BluetoothControllerSramAddressError),
    /// A reviewed member cannot be represented as a non-null SRAM link.
    InvalidMember,
    /// Some byte of the retained allocation lies outside physical SRAM.
    ExtentOutsidePhysicalSram,
}

/// Failed binding that returns the exact unchanged static allocation.
#[must_use = "failed direction-finding storage remains available for inspection"]
pub struct BluetoothDirectionFindingWorkspaceBindFailure {
    storage: &'static mut BluetoothDirectionFindingWorkspaceStorage,
    error: BluetoothDirectionFindingWorkspaceBindError,
}

impl BluetoothDirectionFindingWorkspaceBindFailure {
    fn new(
        storage: &'static mut BluetoothDirectionFindingWorkspaceStorage,
        error: BluetoothDirectionFindingWorkspaceBindError,
    ) -> Self {
        Self { storage, error }
    }

    /// Inspect the finite binding failure.
    pub const fn error(&self) -> BluetoothDirectionFindingWorkspaceBindError {
        self.error
    }

    /// Recover the unchanged allocation and failure reason.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothDirectionFindingWorkspaceStorage,
        BluetoothDirectionFindingWorkspaceBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothDirectionFindingWorkspaceBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDirectionFindingWorkspaceBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic base accepted only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDirectionFindingWorkspaceModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothDirectionFindingWorkspaceModelAddress {
    /// Validate one synthetic Controller-SRAM base.
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }

    const fn address(self) -> u32 {
        self.0.address()
    }
}

/// Opaque link from an ordinary BLE link state to the global DF environment.
///
/// This value carries reviewed address geometry only. It grants neither CPU
/// access to the allocation nor CTE-register publication authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDirectionFindingWorkspaceLink {
    link_state_configuration: BluetoothControllerSramLinkAddress,
}

/// Immutable geometry retained with one workspace allocation.
pub struct BluetoothDirectionFindingWorkspaceBinding {
    base: BluetoothControllerSramAddress,
    link_state_configuration: BluetoothControllerSramLinkAddress,
    disabled_cte_descriptor: BluetoothControllerSramAddress,
    end_exclusive: u32,
}

impl BluetoothDirectionFindingWorkspaceBinding {
    fn new(base: u32) -> Result<Self, BluetoothDirectionFindingWorkspaceBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothDirectionFindingWorkspaceBindError::InvalidBase)?;
        let end_exclusive = base
            .checked_add(BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES as u32)
            .ok_or(BluetoothDirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BluetoothDirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram);
        }

        let link_state_configuration = BluetoothControllerSramLinkAddress::new(
            base.checked_add(LINK_STATE_CONFIGURATION_OFFSET)
                .ok_or(BluetoothDirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram)?,
        )
        .map_err(|_| BluetoothDirectionFindingWorkspaceBindError::InvalidMember)?;
        let disabled_cte_descriptor = BluetoothControllerSramAddress::new(
            base.checked_add(DISABLED_CTE_DESCRIPTOR_OFFSET)
                .ok_or(BluetoothDirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram)?,
        )
        .map_err(|_| BluetoothDirectionFindingWorkspaceBindError::InvalidMember)?;

        Ok(Self {
            base: base_address,
            link_state_configuration,
            disabled_cte_descriptor,
            end_exclusive,
        })
    }

    /// Complete physical range retained by this owner.
    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    /// Descriptor published into CTE hardware buffer zero.
    pub const fn disabled_cte_descriptor_address(&self) -> BluetoothControllerSramAddress {
        self.disabled_cte_descriptor
    }

    /// Opaque ordinary-role link into the controller-global environment.
    pub const fn link(&self) -> BluetoothDirectionFindingWorkspaceLink {
        BluetoothDirectionFindingWorkspaceLink {
            link_state_configuration: self.link_state_configuration,
        }
    }
}

/// Unique CPU owner of one initialized, address-bound DF workspace.
#[must_use = "the direction-finding workspace must be retained or published"]
pub struct BluetoothDirectionFindingWorkspaceCpuOwned {
    storage: Pin<&'static mut BluetoothDirectionFindingWorkspaceStorage>,
    binding: BluetoothDirectionFindingWorkspaceBinding,
}

impl BluetoothDirectionFindingWorkspaceCpuOwned {
    /// Borrow the validated address geometry without changing ownership.
    pub const fn binding(&self) -> &BluetoothDirectionFindingWorkspaceBinding {
        &self.binding
    }

    /// Confirm the source-owned disabled-CTE initialization is still present.
    pub fn is_disabled_baseline_initialized(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        storage.words[LINK_STATE_CONFIGURATION_WORD].get() == 1
            && storage.words[DISABLED_CTE_DESCRIPTOR_WORD].get() == 1
    }
}

impl BluetoothDirectionFindingWorkspaceStorage {
    /// Reserve zeroed controller-global storage before address binding.
    pub const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; WORKSPACE_WORDS],
            _pin: PhantomPinned,
        }
    }

    /// Bind the real location of one unique static target allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        BluetoothDirectionFindingWorkspaceCpuOwned,
        BluetoothDirectionFindingWorkspaceBindFailure,
    > {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothDirectionFindingWorkspaceBindFailure::new(
                    storage,
                    BluetoothDirectionFindingWorkspaceBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    /// Bind a deterministic physical-SRAM base to a native ownership model.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothDirectionFindingWorkspaceModelAddress,
    ) -> Result<
        BluetoothDirectionFindingWorkspaceCpuOwned,
        BluetoothDirectionFindingWorkspaceBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        BluetoothDirectionFindingWorkspaceCpuOwned,
        BluetoothDirectionFindingWorkspaceBindFailure,
    > {
        let binding = match BluetoothDirectionFindingWorkspaceBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothDirectionFindingWorkspaceBindFailure::new(
                    storage, error,
                ));
            }
        };
        for word in &storage.words {
            word.set(0);
        }
        storage.words[LINK_STATE_CONFIGURATION_WORD].set(1);
        storage.words[DISABLED_CTE_DESCRIPTOR_WORD].set(1);
        Ok(BluetoothDirectionFindingWorkspaceCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        })
    }
}

impl Default for BluetoothDirectionFindingWorkspaceStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothDirectionFindingWorkspaceBindError,
        BluetoothDirectionFindingWorkspaceModelAddress, BluetoothDirectionFindingWorkspaceStorage,
    };

    fn storage() -> &'static mut BluetoothDirectionFindingWorkspaceStorage {
        std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothDirectionFindingWorkspaceStorage::new(),
        ))
    }

    #[test]
    fn model_binding_initializes_the_disabled_cte_workspace() {
        let base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f00_1000)
            .expect("model base is encodable");
        let owner = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(storage(), base)
            .expect("the complete workspace fits physical SRAM");

        assert!(owner.is_disabled_baseline_initialized());
    }

    #[test]
    fn model_binding_rejects_a_crossing_extent() {
        let base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f07_fff0)
            .expect("crossing base itself is encodable");
        let failure =
            match BluetoothDirectionFindingWorkspaceStorage::pin_static_model(storage(), base) {
                Ok(_) => panic!("the complete workspace must fit physical SRAM"),
                Err(failure) => failure,
            };

        assert_eq!(
            failure.error(),
            BluetoothDirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram
        );
    }
}
