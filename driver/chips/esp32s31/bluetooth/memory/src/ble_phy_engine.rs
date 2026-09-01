//! Static storage retained by the ESP32-S31 BLE PHY engine.
//!
//! The recovered register-initialization transaction publishes two addresses:
//! a `0x68`-byte BLE environment, three tables referenced by its positional
//! pointer fields, and a controller-SRAM resolving-list object allocated in
//! `0x40`-byte units. The BLE PHY module initialization copies the reviewed
//! channel-frequency and receive packet-start calibration tables into those
//! allocations. The register transaction publishes one environment member at
//! `+0x2c` and the start of a subregion at `+0x40`. This module owns the
//! complete recovered allocation graph and its stable location. It
//! deliberately does not assign semantics to still-unrecovered words or grant
//! MMIO publication authority.

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

const CHANNEL_FREQUENCY_OFFSETS_BYTES: usize = 0x28;
const PACKET_START_OFFSETS_BYTES: usize = 0x08;
const RX_ADDRESS_DELAYS_BYTES: usize = 0x04;
const CHANNEL_FREQUENCY_OFFSETS_POINTER_OFFSET: usize = 0x30;
const PACKET_START_OFFSETS_POINTER_OFFSET: usize = 0x34;
const RX_ADDRESS_DELAYS_POINTER_OFFSET: usize = 0x38;
const RESOLVING_LIST_INITIAL_HEAD_IMAGE: u32 = 0x8000_0000;

const LE_1M_PHY_MODE_INDEX: usize = 1;

#[derive(Clone, Copy)]
enum BluetoothBlePhyPacketMode {
    LeCodedS2 = 0,
    Le1M = 1,
    Le2M = 2,
    LeCodedS8 = 3,
}

const fn channel_frequency_offsets_mhz() -> [u8; 40] {
    let mut offsets = [0; 40];
    let mut channel = 0;
    while channel < offsets.len() {
        offsets[channel] = match channel {
            0..=10 => 2 + 2 * channel as u8,
            11..=36 => 4 + 2 * channel as u8,
            37 => 0,
            38 => 24,
            39 => 78,
            _ => unreachable!(),
        };
        channel += 1;
    }
    offsets
}

const fn preamble_and_access_address_airtime_micros(mode: BluetoothBlePhyPacketMode) -> u16 {
    match mode {
        BluetoothBlePhyPacketMode::Le1M => 40,
        BluetoothBlePhyPacketMode::Le2M => 24,
        BluetoothBlePhyPacketMode::LeCodedS2 | BluetoothBlePhyPacketMode::LeCodedS8 => 336,
    }
}

const fn rx_address_capture_delay_micros(mode: BluetoothBlePhyPacketMode) -> u8 {
    match mode {
        BluetoothBlePhyPacketMode::Le1M | BluetoothBlePhyPacketMode::Le2M => 3,
        BluetoothBlePhyPacketMode::LeCodedS2 | BluetoothBlePhyPacketMode::LeCodedS8 => 68,
    }
}

const fn packet_start_offsets_micros() -> [u16; 4] {
    [
        preamble_and_access_address_airtime_micros(BluetoothBlePhyPacketMode::LeCodedS2),
        preamble_and_access_address_airtime_micros(BluetoothBlePhyPacketMode::Le1M),
        preamble_and_access_address_airtime_micros(BluetoothBlePhyPacketMode::Le2M),
        preamble_and_access_address_airtime_micros(BluetoothBlePhyPacketMode::LeCodedS8),
    ]
}

const fn rx_address_delays_micros() -> [u8; 4] {
    [
        rx_address_capture_delay_micros(BluetoothBlePhyPacketMode::LeCodedS2),
        rx_address_capture_delay_micros(BluetoothBlePhyPacketMode::Le1M),
        rx_address_capture_delay_micros(BluetoothBlePhyPacketMode::Le2M),
        rx_address_capture_delay_micros(BluetoothBlePhyPacketMode::LeCodedS8),
    ]
}

/// Complete static backing storage required by BLE PHY register publication.
///
/// Unknown environment fields remain opaque until individual controller
/// consumers establish their semantics. Construction installs the reviewed
/// immutable PHY lookup data, but does not claim Link Layer, privacy,
/// advertising, scanning, or connection readiness.
#[repr(C, align(4))]
pub struct BluetoothBlePhyEngineStorage {
    environment: [u8; BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES],
    channel_frequency_offsets_mhz: [u8; CHANNEL_FREQUENCY_OFFSETS_BYTES],
    packet_start_offsets_micros: [u16; 4],
    rx_address_delays_micros: [u8; RX_ADDRESS_DELAYS_BYTES],
    resolving_list: [u8; BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES],
    _pin: PhantomPinned,
}

const _: () = {
    assert!(core::mem::align_of::<BluetoothBlePhyEngineStorage>() == 4);
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, channel_frequency_offsets_mhz)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
    );
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, packet_start_offsets_micros)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES + CHANNEL_FREQUENCY_OFFSETS_BYTES
    );
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, rx_address_delays_micros)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + CHANNEL_FREQUENCY_OFFSETS_BYTES
                + PACKET_START_OFFSETS_BYTES
    );
    assert!(
        core::mem::offset_of!(BluetoothBlePhyEngineStorage, resolving_list)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + CHANNEL_FREQUENCY_OFFSETS_BYTES
                + PACKET_START_OFFSETS_BYTES
                + RX_ADDRESS_DELAYS_BYTES
    );
};

/// Source-owned calibration for one received LE 1M packet timestamp.
///
/// The terms remain private to the BLE PHY SRAM codec. Consumers can normalize
/// a controller-microsecond observation but cannot address or reinterpret the
/// underlying hardware tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothBlePhyLe1MPacketStartCalibration {
    packet_start_offset_micros: u16,
    rx_address_delay_micros: u8,
}

impl BluetoothBlePhyLe1MPacketStartCalibration {
    /// Recover the on-air packet-start time from a converted receive timestamp.
    pub fn normalize_controller_micros(self, captured_micros: u32) -> u32 {
        captured_micros.wrapping_sub(
            u32::from(self.packet_start_offset_micros) + u32::from(self.rx_address_delay_micros),
        )
    }
}

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
    channel_frequency_offsets: BluetoothControllerSramAddress,
    packet_start_offsets: BluetoothControllerSramAddress,
    rx_address_delays: BluetoothControllerSramAddress,
    resolving_list: BluetoothControllerSramAddress,
    end_exclusive: u32,
}

impl BluetoothBlePhyEngineBinding {
    fn new(
        environment: u32,
        channel_frequency_offsets: u32,
        packet_start_offsets: u32,
        rx_address_delays: u32,
        resolving_list: u32,
    ) -> Result<Self, BluetoothBlePhyEngineBindError> {
        let environment = BluetoothPhyEnvironmentAddress::new(environment)
            .map_err(BluetoothBlePhyEngineBindError::InvalidEnvironment)?;
        let channel_frequency_offsets =
            BluetoothControllerSramAddress::new(channel_frequency_offsets)
                .map_err(BluetoothBlePhyEngineBindError::InvalidAuxiliary)?;
        let packet_start_offsets = BluetoothControllerSramAddress::new(packet_start_offsets)
            .map_err(BluetoothBlePhyEngineBindError::InvalidAuxiliary)?;
        let rx_address_delays = BluetoothControllerSramAddress::new(rx_address_delays)
            .map_err(BluetoothBlePhyEngineBindError::InvalidAuxiliary)?;
        let resolving_list = BluetoothControllerSramAddress::new(resolving_list)
            .map_err(BluetoothBlePhyEngineBindError::InvalidResolvingList)?;
        let end_exclusive = resolving_list
            .address()
            .checked_add(BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_channel_frequency_offsets = environment
            .address()
            .checked_add(BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_packet_start_offsets = channel_frequency_offsets
            .address()
            .checked_add(CHANNEL_FREQUENCY_OFFSETS_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_rx_address_delays = packet_start_offsets
            .address()
            .checked_add(PACKET_START_OFFSETS_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_resolving_list = rx_address_delays
            .address()
            .checked_add(RX_ADDRESS_DELAYS_BYTES as u32)
            .ok_or(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        if environment.address() < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || channel_frequency_offsets.address() != expected_channel_frequency_offsets
            || packet_start_offsets.address() != expected_packet_start_offsets
            || rx_address_delays.address() != expected_rx_address_delays
            || resolving_list.address() != expected_resolving_list
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram);
        }
        Ok(Self {
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
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
    storage: Pin<&'static mut BluetoothBlePhyEngineStorage>,
    binding: BluetoothBlePhyEngineBinding,
}

impl BluetoothBlePhyEngineCpuOwned {
    /// Borrow the stable address proof without changing ownership.
    pub const fn binding(&self) -> &BluetoothBlePhyEngineBinding {
        &self.binding
    }

    /// Borrow the initialized LE 1M packet-start calibration by value.
    pub fn le_1m_packet_start_calibration(&self) -> BluetoothBlePhyLe1MPacketStartCalibration {
        let storage = self.storage.as_ref().get_ref();
        BluetoothBlePhyLe1MPacketStartCalibration {
            packet_start_offset_micros: storage.packet_start_offsets_micros[LE_1M_PHY_MODE_INDEX],
            rx_address_delay_micros: storage.rx_address_delays_micros[LE_1M_PHY_MODE_INDEX],
        }
    }
}

impl BluetoothBlePhyEngineStorage {
    /// Reserve storage for the BLE PHY engine and its reviewed immutable data.
    pub const fn new() -> Self {
        Self {
            environment: [0; BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES],
            channel_frequency_offsets_mhz: channel_frequency_offsets_mhz(),
            packet_start_offsets_micros: packet_start_offsets_micros(),
            rx_address_delays_micros: rx_address_delays_micros(),
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
        let channel_frequency_offsets = match u32::try_from(
            core::ptr::addr_of!(storage.channel_frequency_offsets_mhz).addr(),
        ) {
            Ok(address) => address,
            Err(_) => {
                return Err(BluetoothBlePhyEngineBindFailure::new(
                    storage,
                    BluetoothBlePhyEngineBindError::AddressWidth,
                ));
            }
        };
        let packet_start_offsets =
            match u32::try_from(core::ptr::addr_of!(storage.packet_start_offsets_micros).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothBlePhyEngineBindFailure::new(
                        storage,
                        BluetoothBlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        let rx_address_delays =
            match u32::try_from(core::ptr::addr_of!(storage.rx_address_delays_micros).addr()) {
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
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
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
        let channel_frequency_offsets = environment + BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32;
        let packet_start_offsets =
            channel_frequency_offsets + CHANNEL_FREQUENCY_OFFSETS_BYTES as u32;
        let rx_address_delays = packet_start_offsets + PACKET_START_OFFSETS_BYTES as u32;
        let resolving_list = rx_address_delays + RX_ADDRESS_DELAYS_BYTES as u32;
        Self::pin_static_inner(
            storage,
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
            resolving_list,
        )
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        environment: u32,
        channel_frequency_offsets: u32,
        packet_start_offsets: u32,
        rx_address_delays: u32,
        resolving_list: u32,
    ) -> Result<BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineBindFailure> {
        let binding = match BluetoothBlePhyEngineBinding::new(
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
            resolving_list,
        ) {
            Ok(binding) => binding,
            Err(error) => return Err(BluetoothBlePhyEngineBindFailure::new(storage, error)),
        };
        storage.initialize_reviewed_allocation(&binding);
        Ok(BluetoothBlePhyEngineCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        })
    }

    fn initialize_reviewed_allocation(&mut self, binding: &BluetoothBlePhyEngineBinding) {
        let publish_pointer = |environment: &mut [u8], offset: usize, address: u32| {
            environment[offset..offset + 4].copy_from_slice(&address.to_le_bytes());
        };
        publish_pointer(
            &mut self.environment,
            CHANNEL_FREQUENCY_OFFSETS_POINTER_OFFSET,
            binding.channel_frequency_offsets.address(),
        );
        publish_pointer(
            &mut self.environment,
            PACKET_START_OFFSETS_POINTER_OFFSET,
            binding.packet_start_offsets.address(),
        );
        publish_pointer(
            &mut self.environment,
            RX_ADDRESS_DELAYS_POINTER_OFFSET,
            binding.rx_address_delays.address(),
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
    use super::{BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage};

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
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::from_mut(storage), original);
    }

    #[test]
    fn le_1m_calibration_preserves_elapsed_controller_time() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_0100)
            .expect("model base uses the controller-SRAM encoding");
        let owner = BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .expect("complete model storage fits physical SRAM");
        let calibration = owner.le_1m_packet_start_calibration();

        let first = calibration.normalize_controller_micros(1_000);
        let second = calibration.normalize_controller_micros(1_001);

        assert_ne!(first, 1_000, "the initialized calibration is not zero");
        assert_eq!(second.wrapping_sub(first), 1);
    }
}
