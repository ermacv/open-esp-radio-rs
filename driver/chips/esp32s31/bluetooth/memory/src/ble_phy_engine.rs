//! Static storage retained by the ESP32-S31 BLE PHY engine.
//!
//! The recovered register-initialization transaction publishes two addresses:
//! a `0x68`-byte BLE environment, three allocations referenced by its
//! positional pointer fields, and a controller-SRAM resolving-list object
//! allocated in `0x40`-byte units. The register transaction publishes one
//! environment member at `+0x2c` and the start of a subregion at `+0x40`.
//! This module owns the complete recovered allocation graph and its stable
//! location. It deliberately does not assign semantics to still-unrecovered
//! words or grant MMIO publication authority.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothPhyEnvironmentAddress, BluetoothPhyEnvironmentAddressError,
};

use crate::{BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW};

/// Bytes in the complete recovered BLE PHY environment allocation.
pub const BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES: usize = 0x68;
/// Bytes in one recovered resolving-list hardware allocation.
pub const BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES: usize = 0x40;

const ENVIRONMENT_AUXILIARY_30_BYTES: usize = 0x28;
const ENVIRONMENT_AUXILIARY_34_BYTES: usize = 0x08;
const ENVIRONMENT_AUXILIARY_38_BYTES: usize = 0x04;
const ENVIRONMENT_AUXILIARY_30_POINTER_OFFSET: usize = 0x30;
const ENVIRONMENT_AUXILIARY_34_POINTER_OFFSET: usize = 0x34;
const ENVIRONMENT_AUXILIARY_38_POINTER_OFFSET: usize = 0x38;
const RESOLVING_LIST_INITIAL_HEAD_IMAGE: u32 = 0x8000_0000;

/// Complete static backing storage required by BLE PHY register publication.
///
/// Fields remain opaque until individual controller consumers establish their
/// semantics. Zero initialization reserves storage; it does not claim Link
/// Layer, privacy, advertising, scanning, or connection readiness.
#[repr(C, align(4))]
pub struct BluetoothBlePhyEngineStorage {
    environment: [u8; BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES],
    environment_auxiliary_30: [u8; ENVIRONMENT_AUXILIARY_30_BYTES],
    environment_auxiliary_34: [u8; ENVIRONMENT_AUXILIARY_34_BYTES],
    environment_auxiliary_38: [u8; ENVIRONMENT_AUXILIARY_38_BYTES],
    resolving_list: [u8; BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES],
    _pin: PhantomPinned,
}

const _: () = {
    assert!(core::mem::align_of::<BluetoothBlePhyEngineStorage>() == 4);
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, environment_auxiliary_30)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
    );
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, environment_auxiliary_34)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES + ENVIRONMENT_AUXILIARY_30_BYTES
    );
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, environment_auxiliary_38)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + ENVIRONMENT_AUXILIARY_30_BYTES
                + ENVIRONMENT_AUXILIARY_34_BYTES
    );
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, resolving_list)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + ENVIRONMENT_AUXILIARY_30_BYTES
                + ENVIRONMENT_AUXILIARY_34_BYTES
                + ENVIRONMENT_AUXILIARY_38_BYTES
    );
};

/// Why static BLE PHY storage could not acquire a physical address binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothBlePhyEngineBindError {
    /// A target pointer cannot be represented by the 32-bit S31 address space.
    AddressWidth,
    /// The environment address cannot represent every published member.
    InvalidEnvironment(BluetoothPhyEnvironmentAddressError),
    /// The resolving-list base is outside the controller-pointer domain.
    InvalidResolvingList(BluetoothControllerSramAddressError),
    /// An environment-owned auxiliary base is outside the controller domain.
    InvalidAuxiliary(BluetoothControllerSramAddressError),
    /// Some byte of either object lies outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
}

/// Failed storage binding that returns the exact unchanged allocation.
#[must_use = "failed BLE PHY storage remains available for corrected placement"]
pub struct BluetoothBlePhyEngineBindFailure {
    storage: &'static mut BluetoothBlePhyEngineStorage,
    error: BluetoothBlePhyEngineBindError,
}

impl BluetoothBlePhyEngineBindFailure {
    fn new(
        storage: &'static mut BluetoothBlePhyEngineStorage,
        error: BluetoothBlePhyEngineBindError,
    ) -> Self {
        Self { storage, error }
    }

    /// Inspect the finite binding failure.
    pub const fn error(&self) -> BluetoothBlePhyEngineBindError {
        self.error
    }

    /// Recover the unchanged allocation and failure reason.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothBlePhyEngineStorage,
        BluetoothBlePhyEngineBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothBlePhyEngineBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothBlePhyEngineBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic base accepted only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothBlePhyEngineModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothBlePhyEngineModelAddress {
    /// Validate one synthetic base without deriving it from a host pointer.
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

/// Address proof for the complete contiguous allocation graph.
pub struct BluetoothBlePhyEngineBinding {
    environment: BluetoothPhyEnvironmentAddress,
    environment_auxiliary_30: BluetoothControllerSramAddress,
    environment_auxiliary_34: BluetoothControllerSramAddress,
    environment_auxiliary_38: BluetoothControllerSramAddress,
    resolving_list: BluetoothControllerSramAddress,
    end_exclusive: u32,
}

impl BluetoothBlePhyEngineBinding {
    fn new(
        environment: u32,
        environment_auxiliary_30: u32,
        environment_auxiliary_34: u32,
        environment_auxiliary_38: u32,
        resolving_list: u32,
    ) -> Result<Self, BluetoothBlePhyEngineBindError> {
        let environment = BluetoothPhyEnvironmentAddress::new(environment)
            .map_err(BluetoothBlePhyEngineBindError::InvalidEnvironment)?;
        let environment_auxiliary_30 =
            BluetoothControllerSramAddress::new(environment_auxiliary_30)
                .map_err(BluetoothBlePhyEngineBindError::InvalidAuxiliary)?;
        let environment_auxiliary_34 =
            BluetoothControllerSramAddress::new(environment_auxiliary_34)
                .map_err(BluetoothBlePhyEngineBindError::InvalidAuxiliary)?;
        let environment_auxiliary_38 =
            BluetoothControllerSramAddress::new(environment_auxiliary_38)
                .map_err(BluetoothBlePhyEngineBindError::InvalidAuxiliary)?;
        let resolving_list = BluetoothControllerSramAddress::new(resolving_list)
            .map_err(BluetoothBlePhyEngineBindError::InvalidResolvingList)?;
        let end_exclusive = resolving_list
            .address()
            .checked_add(BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_auxiliary_30 = environment
            .address()
            .checked_add(BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_auxiliary_34 = environment_auxiliary_30
            .address()
            .checked_add(ENVIRONMENT_AUXILIARY_30_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_auxiliary_38 = environment_auxiliary_34
            .address()
            .checked_add(ENVIRONMENT_AUXILIARY_34_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_resolving_list = environment_auxiliary_38
            .address()
            .checked_add(ENVIRONMENT_AUXILIARY_38_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        if environment.address() < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || environment_auxiliary_30.address() != expected_auxiliary_30
            || environment_auxiliary_34.address() != expected_auxiliary_34
            || environment_auxiliary_38.address() != expected_auxiliary_38
            || resolving_list.address() != expected_resolving_list
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram);
        }
        Ok(Self {
            environment,
            environment_auxiliary_30,
            environment_auxiliary_34,
            environment_auxiliary_38,
            resolving_list,
            end_exclusive,
        })
    }

    /// Return the complete physical SRAM range retained by this owner.
    pub const fn range(&self) -> (u32, u32) {
        (self.environment.address(), self.end_exclusive)
    }

    /// Return the typed environment base without granting publication.
    pub const fn environment_address(&self) -> BluetoothPhyEnvironmentAddress {
        self.environment
    }

    /// Return the typed resolving-list base without granting publication.
    pub const fn resolving_list_address(&self) -> BluetoothControllerSramAddress {
        self.resolving_list
    }
}

/// Unique CPU owner of address-bound BLE PHY engine storage.
///
/// The owner intentionally has no mutable-storage or hardware-publication API.
/// A higher lifecycle must consume and retain it while the PAC transaction
/// makes the environment and resolving-list addresses visible to the
/// controller.
#[must_use = "BLE PHY storage must outlive every controller consumer"]
pub struct BluetoothBlePhyEngineCpuOwned {
    _storage: Pin<&'static mut BluetoothBlePhyEngineStorage>,
    binding: BluetoothBlePhyEngineBinding,
}

impl BluetoothBlePhyEngineCpuOwned {
    /// Borrow the stable address proof without changing ownership.
    pub const fn binding(&self) -> &BluetoothBlePhyEngineBinding {
        &self.binding
    }

    #[cfg(test)]
    fn storage_for_test(&self) -> &BluetoothBlePhyEngineStorage {
        self._storage.as_ref().get_ref()
    }
}

impl BluetoothBlePhyEngineStorage {
    /// Reserve zero-based opaque storage for the BLE PHY engine.
    pub const fn new() -> Self {
        Self {
            environment: [0; BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES],
            environment_auxiliary_30: [0; ENVIRONMENT_AUXILIARY_30_BYTES],
            environment_auxiliary_34: [0; ENVIRONMENT_AUXILIARY_34_BYTES],
            environment_auxiliary_38: [0; ENVIRONMENT_AUXILIARY_38_BYTES],
            resolving_list: [0; BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES],
            _pin: PhantomPinned,
        }
    }

    /// Bind the real locations of one unique static target allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineBindFailure> {
        let environment = match u32::try_from(core::ptr::addr_of!(storage.environment).addr()) {
            Ok(address) => address,
            Err(_) => {
                return Err(BluetoothBlePhyEngineBindFailure::new(
                    storage,
                    BluetoothBlePhyEngineBindError::AddressWidth,
                ));
            }
        };
        let resolving_list = match u32::try_from(core::ptr::addr_of!(storage.resolving_list).addr())
        {
            Ok(address) => address,
            Err(_) => {
                return Err(BluetoothBlePhyEngineBindFailure::new(
                    storage,
                    BluetoothBlePhyEngineBindError::AddressWidth,
                ));
            }
        };
        let environment_auxiliary_30 =
            match u32::try_from(core::ptr::addr_of!(storage.environment_auxiliary_30).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothBlePhyEngineBindFailure::new(
                        storage,
                        BluetoothBlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        let environment_auxiliary_34 =
            match u32::try_from(core::ptr::addr_of!(storage.environment_auxiliary_34).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothBlePhyEngineBindFailure::new(
                        storage,
                        BluetoothBlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        let environment_auxiliary_38 =
            match u32::try_from(core::ptr::addr_of!(storage.environment_auxiliary_38).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothBlePhyEngineBindFailure::new(
                        storage,
                        BluetoothBlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        Self::pin_static_inner(
            storage,
            environment,
            environment_auxiliary_30,
            environment_auxiliary_34,
            environment_auxiliary_38,
            resolving_list,
        )
    }

    /// Bind a deterministic physical-SRAM base to a native ownership model.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothBlePhyEngineModelAddress,
    ) -> Result<BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineBindFailure> {
        let environment = base.address();
        let environment_auxiliary_30 = environment + BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32;
        let environment_auxiliary_34 =
            environment_auxiliary_30 + ENVIRONMENT_AUXILIARY_30_BYTES as u32;
        let environment_auxiliary_38 =
            environment_auxiliary_34 + ENVIRONMENT_AUXILIARY_34_BYTES as u32;
        let resolving_list = environment_auxiliary_38 + ENVIRONMENT_AUXILIARY_38_BYTES as u32;
        Self::pin_static_inner(
            storage,
            environment,
            environment_auxiliary_30,
            environment_auxiliary_34,
            environment_auxiliary_38,
            resolving_list,
        )
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        environment: u32,
        environment_auxiliary_30: u32,
        environment_auxiliary_34: u32,
        environment_auxiliary_38: u32,
        resolving_list: u32,
    ) -> Result<BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineBindFailure> {
        let binding = match BluetoothBlePhyEngineBinding::new(
            environment,
            environment_auxiliary_30,
            environment_auxiliary_34,
            environment_auxiliary_38,
            resolving_list,
        ) {
            Ok(binding) => binding,
            Err(error) => return Err(BluetoothBlePhyEngineBindFailure::new(storage, error)),
        };
        storage.initialize_reviewed_allocation(&binding);
        Ok(BluetoothBlePhyEngineCpuOwned {
            _storage: Pin::static_mut(storage),
            binding,
        })
    }

    fn initialize_reviewed_allocation(&mut self, binding: &BluetoothBlePhyEngineBinding) {
        let publish_pointer = |environment: &mut [u8], offset: usize, address: u32| {
            environment[offset..offset + 4].copy_from_slice(&address.to_le_bytes());
        };
        publish_pointer(
            &mut self.environment,
            ENVIRONMENT_AUXILIARY_30_POINTER_OFFSET,
            binding.environment_auxiliary_30.address(),
        );
        publish_pointer(
            &mut self.environment,
            ENVIRONMENT_AUXILIARY_34_POINTER_OFFSET,
            binding.environment_auxiliary_34.address(),
        );
        publish_pointer(
            &mut self.environment,
            ENVIRONMENT_AUXILIARY_38_POINTER_OFFSET,
            binding.environment_auxiliary_38.address(),
        );
        self.resolving_list[..4].copy_from_slice(&RESOLVING_LIST_INITIAL_HEAD_IMAGE.to_le_bytes());
    }
}

impl Default for BluetoothBlePhyEngineStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES, BluetoothBlePhyEngineBindError,
        BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage,
        ENVIRONMENT_AUXILIARY_30_BYTES, ENVIRONMENT_AUXILIARY_30_POINTER_OFFSET,
        ENVIRONMENT_AUXILIARY_34_BYTES, ENVIRONMENT_AUXILIARY_38_BYTES,
        RESOLVING_LIST_INITIAL_HEAD_IMAGE,
    };

    #[test]
    fn failed_binding_returns_the_same_opaque_allocation() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let original = core::ptr::from_mut(storage);
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f07_fffc)
            .expect("model base uses the controller-SRAM encoding");
        let failure = match BluetoothBlePhyEngineStorage::pin_static_model(storage, base) {
            Ok(_) => panic!("both retained extents cross physical SRAM"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram
        );
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::from_mut(storage), original);
    }

    #[test]
    fn successful_binding_initializes_the_complete_allocation_graph() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_0100)
            .expect("model base uses the controller-SRAM encoding");
        let owner = BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .expect("both complete extents fit physical SRAM");
        let binding = owner.binding();
        assert_eq!(
            binding.resolving_list_address().address() - binding.environment_address().address(),
            (BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + ENVIRONMENT_AUXILIARY_30_BYTES
                + ENVIRONMENT_AUXILIARY_34_BYTES
                + ENVIRONMENT_AUXILIARY_38_BYTES) as u32
        );
        assert!(binding.range().1 > binding.resolving_list_address().address());
        assert_eq!(
            &owner.storage_for_test().environment[ENVIRONMENT_AUXILIARY_30_POINTER_OFFSET
                ..ENVIRONMENT_AUXILIARY_30_POINTER_OFFSET + 4],
            &(binding.environment_address().address() + BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32)
                .to_le_bytes()
        );
        assert_eq!(
            &owner.storage_for_test().resolving_list[..4],
            &RESOLVING_LIST_INITIAL_HEAD_IMAGE.to_le_bytes()
        );
    }
}
