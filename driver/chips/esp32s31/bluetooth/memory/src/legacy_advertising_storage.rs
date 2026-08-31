//! Pinned CPU-owned storage for the first ESP32-S31 legacy advertising role.
//!
//! This module closes allocation and packet/header binding only. The private
//! link-state and scheduler-item capacities are reserved in the same stable
//! graph, but no unknown descriptor image is synthesized and no hardware list
//! can be published from these states.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothLeTxBufferHeaderStorage,
        BluetoothLeTxPacketAddress, BluetoothLeTxPacketPrepareError,
        BluetoothLeTxPacketPreparedLength, BluetoothLeTxPacketStorage,
    },
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Bytes reserved for the advertising link-state object.
pub const BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES: usize = 0x84;
/// Bytes reserved for one advertising scheduler item.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Maximum payload after the two-byte legacy advertising PDU header.
pub const BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
/// Complete controller TX allocation for the maximum legacy advertising PDU.
pub const BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

const LINK_STATE_WORDS: usize = BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES / 4;

type AdvertisingTxPacketAddress =
    BluetoothLeTxPacketAddress<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketLength =
    BluetoothLeTxPacketPreparedLength<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Stable storage for one restricted legacy advertising event graph.
#[pin_project]
#[repr(C)]
pub struct BluetoothLegacyAdvertisingMemoryGraphStorage {
    link_state: [VolatileCell<u32>; LINK_STATE_WORDS],
    scheduler_item: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
    tx_header: BluetoothLeTxBufferHeaderStorage,
    tx_packet: BluetoothLeTxPacketStorage<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>,
    #[pin]
    _pin: PhantomPinned,
}

const GRAPH_BYTES: u32 =
    core::mem::size_of::<BluetoothLegacyAdvertisingMemoryGraphStorage>() as u32;
const LINK_STATE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, link_state) as u32;
const SCHEDULER_ITEM_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, scheduler_item) as u32;
const TX_HEADER_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, tx_header) as u32;
const TX_PACKET_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, tx_packet) as u32;

/// Why static advertising storage cannot become an address-bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
    InvalidPacketExtent,
}

/// Failed binding that returns the exact unchanged static allocation.
pub struct BluetoothLegacyAdvertisingMemoryGraphBindFailure {
    storage: &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
    error: BluetoothLegacyAdvertisingMemoryGraphBindError,
}

impl BluetoothLegacyAdvertisingMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        error: BluetoothLegacyAdvertisingMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> BluetoothLegacyAdvertisingMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        BluetoothLegacyAdvertisingMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyAdvertisingMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothLegacyAdvertisingMemoryGraphModelAddress {
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

/// Opaque identity of one exact pinned advertising graph allocation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BluetoothLegacyAdvertisingMemoryGraphIdentity(usize);

impl BluetoothLegacyAdvertisingMemoryGraphIdentity {
    fn for_storage(storage: &BluetoothLegacyAdvertisingMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Immutable geometry retained by every advertising graph typestate.
pub struct BluetoothLegacyAdvertisingMemoryGraphBinding {
    #[cfg(not(target_arch = "riscv32"))]
    identity: BluetoothLegacyAdvertisingMemoryGraphIdentity,
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_item: BluetoothControllerSramLinkAddress,
    tx_header: BluetoothControllerSramLinkAddress,
    tx_packet: AdvertisingTxPacketAddress,
}

impl BluetoothLegacyAdvertisingMemoryGraphBinding {
    fn new(
        identity: BluetoothLegacyAdvertisingMemoryGraphIdentity,
        base: u32,
    ) -> Result<Self, BluetoothLegacyAdvertisingMemoryGraphBindError> {
        #[cfg(target_arch = "riscv32")]
        let _ = identity;
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothLegacyAdvertisingMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || GRAPH_BYTES > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(BluetoothLegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(BluetoothLegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let link = |offset: u32| {
            BluetoothControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| BluetoothLegacyAdvertisingMemoryGraphBindError::ZeroCompressedLink)
        };

        let link_state = link(LINK_STATE_OFFSET)?;
        let scheduler_item = link(SCHEDULER_ITEM_OFFSET)?;
        let tx_header = link(TX_HEADER_OFFSET)?;
        let tx_packet = AdvertisingTxPacketAddress::new(address(TX_PACKET_OFFSET)?)
            .map_err(|_| BluetoothLegacyAdvertisingMemoryGraphBindError::InvalidPacketExtent)?;

        Ok(Self {
            #[cfg(not(target_arch = "riscv32"))]
            identity,
            base: base_address,
            end_exclusive: base + GRAPH_BYTES,
            link_state,
            scheduler_item,
            tx_header,
            tx_packet,
        })
    }

    pub const fn identity(&self) -> BluetoothLegacyAdvertisingMemoryGraphIdentity {
        #[cfg(target_arch = "riscv32")]
        {
            BluetoothLegacyAdvertisingMemoryGraphIdentity(self.base.address() as usize)
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            self.identity
        }
    }

    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    pub const fn link_state_address(&self) -> BluetoothControllerSramLinkAddress {
        self.link_state
    }

    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramLinkAddress {
        self.scheduler_item
    }

    pub const fn tx_header_address(&self) -> BluetoothControllerSramLinkAddress {
        self.tx_header
    }
}

/// Unique CPU owner of one bound advertising graph before descriptor reset.
#[must_use = "the bound advertising graph must be retained"]
pub struct BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothLegacyAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyAdvertisingMemoryGraphBinding,
}

impl BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    pub const fn binding(&self) -> &BluetoothLegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    /// Install one complete legacy advertising PDU without publishing the graph.
    pub fn prepare_packet(
        mut self,
        pdu: &[u8],
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
        BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure,
    > {
        let packet = self.storage.as_mut().project().tx_packet;
        let packet_length = match packet.prepare_encoded_pdu(pdu) {
            Ok(length) => length,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
                    owner: self,
                    error,
                });
            }
        };
        Ok(BluetoothLegacyAdvertisingMemoryGraphPacketPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length,
        })
    }
}

/// Bound advertising graph carrying one complete CPU-owned TX packet.
#[must_use = "the prepared advertising graph must be retained or cancelled"]
pub struct BluetoothLegacyAdvertisingMemoryGraphPacketPrepared {
    storage: Pin<&'static mut BluetoothLegacyAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
}

impl BluetoothLegacyAdvertisingMemoryGraphPacketPrepared {
    pub const fn binding(&self) -> &BluetoothLegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    pub fn pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .tx_packet
            .prepared_pdu(self.packet_length)
    }

    pub fn cancel(self) -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// Failed packet preparation retaining the exact byte-unchanged graph owner.
pub struct BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
    owner: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    error: BluetoothLeTxPacketPrepareError,
}

impl BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
    pub const fn error(&self) -> BluetoothLeTxPacketPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLeTxPacketPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothLegacyAdvertisingMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            link_state: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
            scheduler_item: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
            tx_header: BluetoothLeTxBufferHeaderStorage::new(),
            tx_packet: BluetoothLeTxPacketStorage::new(),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    > {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphBindFailure::new(
                    storage,
                    BluetoothLegacyAdvertisingMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothLegacyAdvertisingMemoryGraphModelAddress,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    > {
        let identity = BluetoothLegacyAdvertisingMemoryGraphIdentity::for_storage(storage);
        let binding = match BluetoothLegacyAdvertisingMemoryGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphBindFailure::new(
                    storage, error,
                ));
            }
        };
        let owner = BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner
            .storage
            .as_ref()
            .get_ref()
            .tx_header
            .initialize_bound_tx(owner.binding.tx_packet);
        Ok(owner)
    }
}

impl Default for BluetoothLegacyAdvertisingMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyAdvertisingMemoryGraphStorage,
    };

    fn owner() -> super::BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
        ));
        let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
            .expect("the complete graph fits physical controller SRAM")
    }

    #[test]
    fn bound_graph_prepares_and_cancels_one_complete_advertising_pdu() {
        let owner = owner();
        let identity = owner.binding().identity();
        let range = owner.binding().range();
        let prepared = owner
            .prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6])
            .expect("the complete legacy advertising PDU fits");

        assert_eq!(prepared.pdu(), &[0x02, 6, 1, 2, 3, 4, 5, 6]);
        assert_eq!(prepared.binding().identity(), identity);

        let owner = prepared.cancel();
        assert_eq!(owner.binding().identity(), identity);
        assert_eq!(owner.binding().range(), range);
    }

    #[test]
    fn malformed_packet_returns_the_same_graph_for_retry() {
        let owner = owner();
        let identity = owner.binding().identity();
        let failure = match owner.prepare_packet(&[0x02, 7, 1, 2, 3]) {
            Ok(_) => panic!("a mismatched encoded length must fail closed"),
            Err(failure) => failure,
        };
        let (owner, _) = failure.into_parts();
        assert_eq!(owner.binding().identity(), identity);
        assert!(owner.prepare_packet(&[0x02, 3, 1, 2, 3]).is_ok());
    }
}
