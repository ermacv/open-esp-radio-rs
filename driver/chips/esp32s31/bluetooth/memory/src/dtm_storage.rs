//! Static CPU-owned storage for the recovered ESP32-S31 DTM memory graph.
//!
//! The types in this module reserve the complete finite per-event link-graph
//! footprint and reproduce only reviewed CPU-side initialization transforms.
//! The separate `0x28`-byte DTM environment remains LLL state above this
//! boundary. A target-only binding derives the real addresses of one static
//! allocation, rejects storage outside physical internal SRAM and retains the
//! allocation behind one movable CPU owner. It exposes no controller
//! publication or hardware-owned state. Four-byte alignment is the minimum
//! proven by compressed controller links; it is not a cache-coherency claim.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;

use crate::sram_link::BluetoothDtmBoundSramLinkAddress;

/// Bytes allocated for one DTM link-state object.
pub const BLUETOOTH_DTM_LINK_STATE_BYTES: usize = 0x84;
/// Bytes allocated for one DTM scheduler item.
pub const BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Bytes allocated for the separate DTM scheduler context.
pub const BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES: usize = 0x48;
/// Bytes in every common RX/TX buffer header.
pub const BLUETOOTH_DTM_BUFFER_HEADER_BYTES: usize = 0x18;
/// Bytes preceding the maximum DTM transmitter payload.
pub const BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES: usize = 0x12;
/// Bytes preceding the maximum DTM receiver capacity.
pub const BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum packet capacity supplied by the complete DTM allocator.
pub const BLUETOOTH_DTM_MAX_PACKET_CAPACITY: usize = u8::MAX as usize;
/// Logical bytes in the complete DTM TX packet allocation.
pub const BLUETOOTH_DTM_TX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;
/// Logical bytes in the complete DTM RX packet allocation.
pub const BLUETOOTH_DTM_RX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;

/// First byte in the physical ESP32-S31 internal SRAM window.
pub const BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW: u32 = 0x2f00_0000;
/// First byte after the physical ESP32-S31 internal SRAM window.
///
/// This is deliberately narrower than the controller's 20-bit compressed
/// pointer encoding domain. Linker policy may reserve an additional suffix
/// below this architectural boundary and must place the static allocation in
/// its available `.dma.bss` range.
pub const BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH: u32 = 0x2f08_0000;

const RX_PACKET_LAST_ALIGNED_OFFSET: u32 = 0x11c;
const TX_PACKET_LAST_ALIGNED_OFFSET: u32 = 0x110;
const TX_HEADER_PACKET_TARGET_OFFSET: u32 = 0x10;
const LINK_STATE_RX_HEAD_OFFSET: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD_OFFSET: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL_OFFSET: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL_OFFSET: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE_OFFSET: usize = 0x78 / 4;
const SCHEDULER_ITEM_LINK_STATE_OFFSET: usize = 0x08 / 4;

/// Why a complete DTM TX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTxPacketAddressError {
    /// The proposed base is not a reviewed compressed controller-SRAM address.
    InvalidBase(BluetoothControllerSramAddressError),
    /// The base encodes as the zero link used for an unbound header.
    ZeroCompressedBase,
    /// The aligned packet tail crosses the reviewed controller-SRAM window.
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM geometry for one complete DTM TX packet.
///
/// This value proves only address range and alignment. It does not derive an
/// address from Rust storage, dereference it or grant publication authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxPacketAddress {
    base: BluetoothControllerSramAddress,
    header_packet_target: BluetoothControllerSramAddress,
}

impl BluetoothDtmTxPacketAddress {
    /// Validate the base, header target and final aligned word of the
    /// `0x111`-byte allocation.
    pub const fn new(address: u32) -> Result<Self, BluetoothDtmTxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(BluetoothDtmTxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(BluetoothDtmTxPacketAddressError::ZeroCompressedBase);
        }
        let header_packet_target_address = match address.checked_add(TX_HEADER_PACKET_TARGET_OFFSET)
        {
            Some(address) => address,
            None => return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let last_aligned_address = match address.checked_add(TX_PACKET_LAST_ALIGNED_OFFSET) {
            Some(address) => address,
            None => return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let header_packet_target =
            match BluetoothControllerSramAddress::new(header_packet_target_address) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram);
                }
            };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self {
            base,
            header_packet_target,
        })
    }

    const fn base_compressed_image(self) -> u32 {
        self.base.compressed_image()
    }

    const fn header_packet_target_compressed_image(self) -> u32 {
        self.header_packet_target.compressed_image()
    }
}

/// Complete six-word allocation-time image of one DTM TX buffer header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxBufferHeaderImage {
    words: [u32; 6],
}

impl BluetoothDtmTxBufferHeaderImage {
    /// Build the exact header image used by the full-capacity DTM allocation.
    pub const fn new(packet: BluetoothDtmTxPacketAddress) -> Self {
        Self {
            words: [
                0,
                packet.base_compressed_image(),
                0x80a0_0000 | packet.header_packet_target_compressed_image(),
                0,
                0x0000_07f8,
                0,
            ],
        }
    }

    /// Return all six little-endian positional words without publication.
    pub const fn words(self) -> [u32; 6] {
        self.words
    }
}

/// Why a complete DTM RX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRxPacketAddressError {
    /// The proposed base is not a reviewed compressed controller-SRAM address.
    InvalidBase(BluetoothControllerSramAddressError),
    /// The base encodes as the zero link used by the unbound swap reserve.
    ZeroCompressedBase,
    /// The aligned packet tail crosses the reviewed controller-SRAM window.
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM geometry for one complete DTM RX packet.
///
/// This value proves only address range and alignment. It does not derive an
/// address from Rust storage, dereference it or grant publication authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxPacketAddress {
    base: BluetoothControllerSramAddress,
}

impl BluetoothDtmRxPacketAddress {
    /// Validate the base and final aligned word of the `0x11d`-byte allocation.
    pub const fn new(address: u32) -> Result<Self, BluetoothDtmRxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(BluetoothDtmRxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(BluetoothDtmRxPacketAddressError::ZeroCompressedBase);
        }
        let last_aligned_address = match address.checked_add(RX_PACKET_LAST_ALIGNED_OFFSET) {
            Some(address) => address,
            None => return Err(BluetoothDtmRxPacketAddressError::ExtentOutsideControllerSram),
        };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(BluetoothDtmRxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self { base })
    }

    const fn compressed_image(self) -> u32 {
        self.base.compressed_image()
    }
}

/// Complete zero-based allocation-time image of the bound DTM RX header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxBufferHeaderImage {
    words: [u32; 6],
}

impl BluetoothDtmRxBufferHeaderImage {
    /// Bind the RX packet while preserving the exact positional high image.
    pub const fn new(packet: BluetoothDtmRxPacketAddress) -> Self {
        Self {
            words: [0, packet.compressed_image(), 0x8080_0000, 0, 0, 0],
        }
    }

    /// Return all six little-endian positional words without publication.
    pub const fn words(self) -> [u32; 6] {
        self.words
    }

    /// Initial image of the separately allocated software swap reserve.
    pub const fn unbound_swap_reserve() -> Self {
        Self {
            words: [0, 0, 0x8080_0000, 0, 0, 0],
        }
    }
}

/// Opaque CPU-owned link-state allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmLinkStateStorage {
    words: [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
}

/// Opaque CPU-owned scheduler-item allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmSchedulerItemStorage {
    words: [u32; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
}

/// Opaque CPU-owned scheduler-context allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmSchedulerContextStorage {
    words: [u32; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
}

/// One zero-based common buffer-header allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmBufferHeaderStorage {
    words: [u32; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
}

impl BluetoothDtmBufferHeaderStorage {
    fn clear_rx_completion_bit(&mut self) {
        self.words[3] &= !0x8000_0000;
    }
}

/// CPU-owned TX packet slot reserved by the complete DTM link graph.
///
/// This is the sole backing-storage type used by both the memory graph and the
/// LLL packet-pattern preparation. It exposes no raw address or publication
/// operation.
#[repr(C, align(4))]
pub struct BluetoothDtmTxPacketStorage {
    bytes: [u8; BLUETOOTH_DTM_TX_PACKET_BYTES],
}

impl BluetoothDtmTxPacketStorage {
    /// Create zeroed, CPU-owned storage without allocating.
    pub const fn new() -> Self {
        Self {
            bytes: [0; BLUETOOTH_DTM_TX_PACKET_BYTES],
        }
    }

    /// Install the four positional DTM prefix bytes and exclusively borrow the
    /// declared payload for an upper-layer pattern generator.
    pub fn begin_prepare(
        &mut self,
        pattern_selector: u8,
        payload_length: u8,
    ) -> BluetoothDtmTxPacketPreparation<'_> {
        self.bytes[0x05] = 2;
        self.bytes[0x06] = 0;
        self.bytes[0x10] = pattern_selector;
        self.bytes[0x11] = payload_length;

        BluetoothDtmTxPacketPreparation {
            payload_length: payload_length as usize,
            storage: self,
        }
    }

    /// Borrow the complete CPU-owned allocation image for review and tests.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Default for BluetoothDtmTxPacketStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Exclusive CPU-owned TX packet preparation in progress.
///
/// The memory layer deliberately treats the selector and payload bytes as
/// positional. Standard DTM pattern semantics remain in the LLL/portable
/// Controller above this type.
pub struct BluetoothDtmTxPacketPreparation<'storage> {
    payload_length: usize,
    storage: &'storage mut BluetoothDtmTxPacketStorage,
}

impl<'storage> BluetoothDtmTxPacketPreparation<'storage> {
    /// Borrow exactly the declared payload bytes for bounded pattern filling.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.storage.bytes[BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES..][..self.payload_length]
    }

    /// Finish the CPU-only transform without publishing the packet.
    pub fn finish(self) -> BluetoothDtmPreparedTxPacketStorage<'storage> {
        BluetoothDtmPreparedTxPacketStorage {
            payload_length: self.payload_length,
            storage: self.storage,
        }
    }
}

/// Affine CPU-owned view of one positionally prepared TX packet slot.
pub struct BluetoothDtmPreparedTxPacketStorage<'storage> {
    payload_length: usize,
    storage: &'storage mut BluetoothDtmTxPacketStorage,
}

impl<'storage> BluetoothDtmPreparedTxPacketStorage<'storage> {
    /// Borrow the reviewed prefix and declared payload only.
    pub fn prepared_bytes(&self) -> &[u8] {
        &self.storage.bytes[..BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + self.payload_length]
    }

    /// Borrow only the declared payload bytes.
    pub fn payload(&self) -> &[u8] {
        &self.storage.bytes[BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES..][..self.payload_length]
    }

    /// Return the sole CPU-owned backing slot.
    pub fn release(self) -> &'storage mut BluetoothDtmTxPacketStorage {
        self.storage
    }
}

/// CPU-owned RX packet allocation with only the reviewed DTM defaults exposed.
#[repr(C, align(4))]
pub struct BluetoothDtmRxPacketStorage {
    bytes: [u8; BLUETOOTH_DTM_RX_PACKET_BYTES],
}

impl BluetoothDtmRxPacketStorage {
    /// Create one zero-based slot and apply the reviewed maximum-capacity
    /// allocation and re-arm images.
    pub const fn new() -> Self {
        let mut storage = Self {
            bytes: [0; BLUETOOTH_DTM_RX_PACKET_BYTES],
        };
        storage.bytes[0x05] = 1;
        storage.bytes[0x06] = 1;
        storage.bytes[0x0c] = 0xff;
        storage.bytes[0x0d] = 0xff;
        storage.bytes[0x0e] = 0xff;
        storage.bytes[0x18] = 0xff;
        storage.bytes[0x19] = 0xff;
        storage
    }

    /// Reapply the exact returned-buffer default values before a future append.
    ///
    /// The low three bytes of word `+0x0c` become `0xff`; byte `+0x0f` is
    /// deliberately retained. Halfword `+0x18` becomes `0xffff`.
    fn rearm_reviewed_packet_fields(&mut self) {
        self.bytes[0x0c] = 0xff;
        self.bytes[0x0d] = 0xff;
        self.bytes[0x0e] = 0xff;
        self.bytes[0x18] = 0xff;
        self.bytes[0x19] = 0xff;
    }

    /// Read the positional result word without assigning field semantics.
    pub fn result_word(&self) -> u32 {
        u32::from_le_bytes([
            self.bytes[0x0c],
            self.bytes[0x0d],
            self.bytes[0x0e],
            self.bytes[0x0f],
        ])
    }

    /// Borrow the CPU-owned allocation for reviewed preparation and tests.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Exclusive CPU-owned view of the ordinary RX packet and header allocations.
///
/// The append path mutates both allocations as one transition, so the public
/// memory API does not expose packet re-arm separately from clearing the
/// returned header's completion bit.
pub struct BluetoothDtmRxBufferStorage<'storage> {
    packet: &'storage mut BluetoothDtmRxPacketStorage,
    header: &'storage mut BluetoothDtmBufferHeaderStorage,
}

/// Why the ordinary returned-buffer re-arm must stop without mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRxRearmError {
    /// Header bit `+0x10.0` selects the unresolved swap-reserve branch.
    SwapReserveDecisionRequired,
}

impl BluetoothDtmRxBufferStorage<'_> {
    /// Apply the complete ordinary returned-buffer re-arm before append.
    ///
    /// This covers the packet sentinels and completion bit. The special header
    /// `+0x10.0` swap-reserve branch fails closed before either allocation is
    /// mutated.
    pub fn rearm_reviewed_fields(&mut self) -> Result<(), BluetoothDtmRxRearmError> {
        if self.header.words[4] & 1 != 0 {
            return Err(BluetoothDtmRxRearmError::SwapReserveDecisionRequired);
        }
        self.packet.rearm_reviewed_packet_fields();
        self.header.clear_rx_completion_bit();
        Ok(())
    }

    /// Read the positional packet result word before re-arm.
    pub fn result_word(&self) -> u32 {
        self.packet.result_word()
    }
}

impl Default for BluetoothDtmRxPacketStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete no-heap allocation capacity for one reviewed DTM per-event graph.
///
/// Fields are private because the link relationships and publishable images
/// belong to an address-bound owner. In particular, this value has no method
/// that publishes a raw address or creates a `HardwareOwned` state.
#[pin_project]
#[repr(C)]
pub struct BluetoothDtmMemoryGraphStorage {
    link_state: BluetoothDtmLinkStateStorage,
    scheduler_context: BluetoothDtmSchedulerContextStorage,
    scheduler_item: BluetoothDtmSchedulerItemStorage,
    rx_header: BluetoothDtmBufferHeaderStorage,
    rx_swap_reserve: BluetoothDtmBufferHeaderStorage,
    tx_header: BluetoothDtmBufferHeaderStorage,
    tx_packet: BluetoothDtmTxPacketStorage,
    rx_packet: BluetoothDtmRxPacketStorage,
    #[pin]
    _pin: PhantomPinned,
}

const BLUETOOTH_DTM_MEMORY_GRAPH_BYTES: u32 = 0x3a8;
const LINK_STATE_STORAGE_OFFSET: u32 = 0x000;
const SCHEDULER_CONTEXT_STORAGE_OFFSET: u32 = 0x084;
const SCHEDULER_ITEM_STORAGE_OFFSET: u32 = 0x0cc;
const RX_HEADER_STORAGE_OFFSET: u32 = 0x12c;
const RX_SWAP_RESERVE_STORAGE_OFFSET: u32 = 0x144;
const TX_HEADER_STORAGE_OFFSET: u32 = 0x15c;
const TX_PACKET_STORAGE_OFFSET: u32 = 0x174;
const RX_PACKET_STORAGE_OFFSET: u32 = 0x288;

const _: () = {
    assert!(core::mem::size_of::<BluetoothDtmMemoryGraphStorage>() == 0x3a8);
    assert!(core::mem::align_of::<BluetoothDtmMemoryGraphStorage>() == 4);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, link_state) == 0x000);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_context) == 0x084);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_item) == 0x0cc);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_header) == 0x12c);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_swap_reserve) == 0x144);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, tx_header) == 0x15c);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, tx_packet) == 0x174);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_packet) == 0x288);
};

/// Why static DTM graph storage cannot become an address-bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphBindError {
    /// A target pointer cannot be represented by the 32-bit S31 address space.
    AddressWidth,
    /// The proposed base is outside the compressed controller-pointer domain.
    InvalidBase(BluetoothControllerSramAddressError),
    /// Some byte of the complete graph is outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
    /// A required graph link would encode as the unbound zero image.
    ZeroCompressedLink,
    /// A reviewed packet extent is inconsistent with the bound graph layout.
    InvalidPacketExtent,
}

/// Failed static binding that retains the exact, unchanged allocation.
///
/// Address validation completes before any allocation-time field is written.
/// The failure is linear and cannot be duplicated:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphBindFailure;
///
/// fn duplicate(failure: BluetoothDtmMemoryGraphBindFailure) {
///     let moved = failure;
///     let _ = failure.error();
///     drop(moved);
/// }
/// ```
pub struct BluetoothDtmMemoryGraphBindFailure {
    storage: &'static mut BluetoothDtmMemoryGraphStorage,
    error: BluetoothDtmMemoryGraphBindError,
}

impl BluetoothDtmMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothDtmMemoryGraphStorage,
        error: BluetoothDtmMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    /// Return the finite reason without releasing the allocation.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphBindError {
        self.error
    }

    /// Recover the unchanged static allocation and binding error.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothDtmMemoryGraphStorage,
        BluetoothDtmMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
///
/// Construction proves the compressed-pointer syntax only. The graph binding
/// still validates the complete extent against physical S31 SRAM. Production
/// RISC-V code has no access to this type and must derive real field addresses
/// from its static allocation.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothDtmMemoryGraphModelAddress {
    /// Validate one synthetic base against the controller encoding domain.
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

/// Non-forgeable address binding retained by one CPU-owned static graph.
///
/// The value proves that every contained allocation lies in physical internal
/// SRAM and matches this crate's exact `repr(C)` layout. It is intentionally
/// not `Clone` or `Copy`; obtaining compressed component addresses does not
/// grant controller publication authority.
pub struct BluetoothDtmMemoryGraphBinding {
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothDtmBoundSramLinkAddress,
    scheduler_context: BluetoothControllerSramAddress,
    scheduler_item: BluetoothDtmBoundSramLinkAddress,
    rx_header: BluetoothDtmBoundSramLinkAddress,
    rx_swap_reserve: BluetoothDtmBoundSramLinkAddress,
    tx_header: BluetoothDtmBoundSramLinkAddress,
    tx_packet: BluetoothDtmTxPacketAddress,
    rx_packet: BluetoothDtmRxPacketAddress,
}

impl BluetoothDtmMemoryGraphBinding {
    fn new(base: u32) -> Result<Self, BluetoothDtmMemoryGraphBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothDtmMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || BLUETOOTH_DTM_MEMORY_GRAPH_BYTES
                > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let bound_link = |offset: u32| {
            BluetoothDtmBoundSramLinkAddress::new(address(offset)?)
                .map_err(|_| BluetoothDtmMemoryGraphBindError::ZeroCompressedLink)
        };

        let link_state = bound_link(LINK_STATE_STORAGE_OFFSET)?;
        let scheduler_context =
            BluetoothControllerSramAddress::new(address(SCHEDULER_CONTEXT_STORAGE_OFFSET)?)
                .map_err(BluetoothDtmMemoryGraphBindError::InvalidBase)?;
        let scheduler_item = bound_link(SCHEDULER_ITEM_STORAGE_OFFSET)?;
        let rx_header = bound_link(RX_HEADER_STORAGE_OFFSET)?;
        let rx_swap_reserve = bound_link(RX_SWAP_RESERVE_STORAGE_OFFSET)?;
        let tx_header = bound_link(TX_HEADER_STORAGE_OFFSET)?;
        let tx_packet = BluetoothDtmTxPacketAddress::new(address(TX_PACKET_STORAGE_OFFSET)?)
            .map_err(|_| BluetoothDtmMemoryGraphBindError::InvalidPacketExtent)?;
        let rx_packet = BluetoothDtmRxPacketAddress::new(address(RX_PACKET_STORAGE_OFFSET)?)
            .map_err(|_| BluetoothDtmMemoryGraphBindError::InvalidPacketExtent)?;

        Ok(Self {
            base: base_address,
            end_exclusive: base + BLUETOOTH_DTM_MEMORY_GRAPH_BYTES,
            link_state,
            scheduler_context,
            scheduler_item,
            rx_header,
            rx_swap_reserve,
            tx_header,
            tx_packet,
            rx_packet,
        })
    }

    /// Return the complete physical SRAM range occupied by this graph.
    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    /// Return the address of the private DTM link-state allocation.
    pub const fn link_state_address(&self) -> BluetoothDtmBoundSramLinkAddress {
        self.link_state
    }

    /// Return the address of the separate CPU-owned scheduler context.
    pub const fn scheduler_context_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_context
    }

    /// Return the address of the private DTM scheduler item.
    pub const fn scheduler_item_address(&self) -> BluetoothDtmBoundSramLinkAddress {
        self.scheduler_item
    }
}

/// Unique CPU owner of one statically located and allocation-initialized graph.
///
/// This state is still unreachable by hardware. It contains no `arm`,
/// `publish`, raw-pointer or `HardwareOwned` transition; the future LLL must
/// first prove the private packet-engine latch and visibility fences.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::{
///     BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
/// };
///
/// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
/// let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100).unwrap();
/// let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base).unwrap();
/// let moved = owner;
/// let _binding = owner.binding();
/// drop(moved);
/// ```
pub struct BluetoothDtmMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphCpuOwned {
    /// Borrow the location proof without granting publication authority.
    pub const fn binding(&self) -> &BluetoothDtmMemoryGraphBinding {
        &self.binding
    }

    /// Borrow the sole TX packet slot for CPU-only LLL pattern preparation.
    pub fn tx_packet_mut(&mut self) -> &mut BluetoothDtmTxPacketStorage {
        self.storage.as_mut().project().tx_packet
    }

    /// Borrow the ordinary RX packet/header allocations as one CPU transition.
    ///
    /// This does not claim that hardware has completed or released a header.
    pub fn rx_buffer_mut(&mut self) -> BluetoothDtmRxBufferStorage<'_> {
        let storage = self.storage.as_mut().project();
        BluetoothDtmRxBufferStorage {
            packet: storage.rx_packet,
            header: storage.rx_header,
        }
    }

    fn initialize_reviewed_allocation(&mut self) {
        let rx_header = BluetoothDtmRxBufferHeaderImage::new(self.binding.rx_packet).words();
        let rx_swap_reserve = BluetoothDtmRxBufferHeaderImage::unbound_swap_reserve().words();
        let tx_header = BluetoothDtmTxBufferHeaderImage::new(self.binding.tx_packet).words();
        let link_state = self.binding.link_state.compressed_image();
        let rx_header_link = self.binding.rx_header.compressed_image();
        let rx_swap_link = self.binding.rx_swap_reserve.compressed_image();
        let tx_header_link = self.binding.tx_header.compressed_image();

        let storage = self.storage.as_mut().project();
        storage.link_state.words.fill(0);
        storage.scheduler_context.words.fill(0);
        storage.scheduler_item.words.fill(0);
        storage.rx_header.words = rx_header;
        storage.rx_swap_reserve.words = rx_swap_reserve;
        storage.tx_header.words = tx_header;
        storage.tx_packet.bytes.fill(0);
        *storage.rx_packet = BluetoothDtmRxPacketStorage::new();

        storage.link_state.words[LINK_STATE_RX_HEAD_OFFSET] = rx_header_link;
        storage.link_state.words[LINK_STATE_TX_HEAD_OFFSET] = tx_header_link;
        storage.link_state.words[LINK_STATE_RX_TAIL_OFFSET] = rx_header_link;
        storage.link_state.words[LINK_STATE_TX_TAIL_OFFSET] = tx_header_link;
        storage.link_state.words[LINK_STATE_RX_SWAP_RESERVE_OFFSET] = rx_swap_link;
        storage.scheduler_item.words[SCHEDULER_ITEM_LINK_STATE_OFFSET] = link_state;
    }
}

impl BluetoothDtmMemoryGraphStorage {
    /// Reserve the graph and install the reviewed RX allocation defaults.
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothDtmLinkStateStorage {
                words: [0; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
            },
            scheduler_context: BluetoothDtmSchedulerContextStorage {
                words: [0; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
            },
            scheduler_item: BluetoothDtmSchedulerItemStorage {
                words: [0; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
            },
            rx_header: BluetoothDtmBufferHeaderStorage {
                words: [0; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
            },
            rx_swap_reserve: BluetoothDtmBufferHeaderStorage {
                words: [0; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
            },
            tx_header: BluetoothDtmBufferHeaderStorage {
                words: [0; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
            },
            tx_packet: BluetoothDtmTxPacketStorage::new(),
            rx_packet: BluetoothDtmRxPacketStorage::new(),
            _pin: PhantomPinned,
        }
    }

    /// Bind the real addresses of one unique static S31 allocation.
    ///
    /// All address and extent checks finish before the first mutation. The
    /// returned CPU owner installs only reviewed allocation-time headers and
    /// private-chain anchors; the graph remains unreachable by hardware.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothDtmMemoryGraphBindFailure::new(
                    storage,
                    BluetoothDtmMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    /// Bind a deterministic physical-SRAM address to a native ownership model.
    ///
    /// The model never derives an address from its host pointer. Passing a raw
    /// integer is rejected by the type system:
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphStorage;
    ///
    /// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
    /// let _ = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, 0x2f00_0100);
    /// ```
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothDtmMemoryGraphModelAddress,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        let binding = match BluetoothDtmMemoryGraphBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => return Err(BluetoothDtmMemoryGraphBindFailure::new(storage, error)),
        };
        let mut owner = BluetoothDtmMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.initialize_reviewed_allocation();
        Ok(owner)
    }

    /// Borrow the sole TX packet backing slot for CPU-only LLL preparation.
    pub fn tx_packet_mut(&mut self) -> &mut BluetoothDtmTxPacketStorage {
        &mut self.tx_packet
    }

    /// Borrow the ordinary RX packet/header allocations as one re-arm unit.
    ///
    /// A future completed-header owner must quarantine the special swap bit
    /// before choosing this ordinary transition.
    pub fn rx_buffer_mut(&mut self) -> BluetoothDtmRxBufferStorage<'_> {
        BluetoothDtmRxBufferStorage {
            packet: &mut self.rx_packet,
            header: &mut self.rx_header,
        }
    }
}

impl Default for BluetoothDtmMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use core::mem::{align_of, offset_of, size_of};

    use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddressError;

    use super::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_DTM_BUFFER_HEADER_BYTES,
        BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_RX_PACKET_BYTES,
        BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES, BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES,
        BLUETOOTH_DTM_TX_PACKET_BYTES, BluetoothDtmBufferHeaderStorage,
        BluetoothDtmLinkStateStorage, BluetoothDtmMemoryGraphBindError,
        BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
        BluetoothDtmRxBufferHeaderImage, BluetoothDtmRxBufferStorage, BluetoothDtmRxPacketAddress,
        BluetoothDtmRxPacketAddressError, BluetoothDtmRxPacketStorage, BluetoothDtmRxRearmError,
        BluetoothDtmSchedulerContextStorage, BluetoothDtmSchedulerItemStorage,
        BluetoothDtmTxBufferHeaderImage, BluetoothDtmTxPacketAddress,
        BluetoothDtmTxPacketAddressError, BluetoothDtmTxPacketStorage,
    };

    #[test]
    fn static_link_graph_has_every_reviewed_graph_allocation_footprint() {
        assert_eq!(size_of::<BluetoothDtmLinkStateStorage>(), 0x84);
        assert_eq!(size_of::<BluetoothDtmSchedulerItemStorage>(), 0x60);
        assert_eq!(size_of::<BluetoothDtmSchedulerContextStorage>(), 0x48);
        assert_eq!(size_of::<BluetoothDtmBufferHeaderStorage>(), 0x18);
        assert_eq!(size_of::<BluetoothDtmTxPacketStorage>(), 0x114);
        assert_eq!(size_of::<BluetoothDtmRxPacketStorage>(), 0x120);
        assert_eq!(size_of::<BluetoothDtmMemoryGraphStorage>(), 0x3a8);

        assert_eq!(BLUETOOTH_DTM_LINK_STATE_BYTES, 0x84);
        assert_eq!(BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, 0x60);
        assert_eq!(BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES, 0x48);
        assert_eq!(BLUETOOTH_DTM_BUFFER_HEADER_BYTES, 0x18);
        assert_eq!(BLUETOOTH_DTM_TX_PACKET_BYTES, 0x111);
        assert_eq!(BLUETOOTH_DTM_RX_PACKET_BYTES, 0x11d);

        assert_eq!(align_of::<BluetoothDtmMemoryGraphStorage>(), 4);
        assert_eq!(align_of::<BluetoothDtmRxPacketStorage>(), 4);

        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, link_state),
            0x000
        );
        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_context),
            0x084
        );
        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_item),
            0x0cc
        );
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, rx_header), 0x12c);
        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, rx_swap_reserve),
            0x144
        );
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, tx_header), 0x15c);
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, tx_packet), 0x174);
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, rx_packet), 0x288);
    }

    #[test]
    fn model_binding_initializes_every_reviewed_header_and_private_anchor() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("model base has valid compressed-pointer syntax");
        let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base)
            .expect("complete model graph fits physical controller SRAM");
        assert_eq!(owner.binding().range(), (0x2f00_0100, 0x2f00_04a8));
        assert_eq!(
            owner.binding().link_state_address().compressed_image(),
            0x040
        );
        assert_eq!(
            owner.binding().scheduler_context_address().address(),
            0x2f00_0184
        );
        assert_eq!(
            owner.binding().scheduler_item_address().compressed_image(),
            0x073
        );

        let graph = owner.storage.as_ref().get_ref();
        assert_eq!(graph.rx_header.words, [0, 0x0e2, 0x8080_0000, 0, 0, 0]);
        assert_eq!(graph.rx_swap_reserve.words, [0, 0, 0x8080_0000, 0, 0, 0]);
        assert_eq!(
            graph.tx_header.words,
            [0, 0x09d, 0x80a0_00a1, 0, 0x0000_07f8, 0]
        );
        assert_eq!(
            &graph.link_state.words[0x68 / 4..=0x78 / 4],
            &[0x08b, 0x097, 0x08b, 0x097, 0x091]
        );
        assert_eq!(graph.scheduler_item.words[0x08 / 4], 0x040);
        assert_eq!(graph.rx_packet.bytes[0x05], 1);
        assert_eq!(graph.rx_packet.bytes[0x06], 1);
        assert_eq!(graph.rx_packet.result_word(), 0x00ff_ffff);
        assert_eq!(&graph.rx_packet.bytes[0x18..0x1a], &[0xff, 0xff]);
    }

    #[test]
    fn failed_binding_returns_the_unchanged_storage_for_retry() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let mut preparation = storage.tx_packet_mut().begin_prepare(7, 3);
        preparation
            .payload_mut()
            .copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        preparation.finish().release();
        let original_address = core::ptr::addr_of!(*storage).addr();
        let crossing = BluetoothDtmMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8 + 4,
        )
        .expect("crossing base still has valid compressed-pointer syntax");

        let failure = match BluetoothDtmMemoryGraphStorage::pin_static_model(storage, crossing) {
            Ok(_) => panic!("a graph crossing physical SRAM must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        let (storage, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        assert_eq!(core::ptr::addr_of!(*storage).addr(), original_address);
        assert_eq!(
            &storage.tx_packet.bytes[0x10..0x15],
            &[7, 3, 0xaa, 0xbb, 0xcc]
        );
        assert_eq!(storage.rx_header.words, [0; 6]);

        let valid = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("retry base has valid compressed-pointer syntax");
        let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, valid)
            .expect("returned allocation can be bound exactly once");
        assert_eq!(owner.binding().range().0, 0x2f00_0100);
        assert_eq!(owner.storage.as_ref().get_ref().tx_packet.bytes, [0; 0x111]);
    }

    #[test]
    fn binding_rejects_zero_link_and_accepts_the_exact_physical_tail() {
        assert_eq!(
            BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0101),
            Err(BluetoothControllerSramAddressError::Unaligned)
        );

        let zero_storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let zero = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0000)
            .expect("physical base has valid encoding syntax");
        let zero_failure =
            match BluetoothDtmMemoryGraphStorage::pin_static_model(zero_storage, zero) {
                Ok(_) => panic!("link state must not collapse onto the unbound zero image"),
                Err(failure) => failure,
            };
        assert_eq!(
            zero_failure.error(),
            BluetoothDtmMemoryGraphBindError::ZeroCompressedLink
        );

        let tail_storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let tail = BluetoothDtmMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8,
        )
        .expect("tail base has valid compressed-pointer syntax");
        let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(tail_storage, tail)
            .expect("graph whose exclusive end equals SRAM high is valid");
        assert_eq!(
            owner.binding().range(),
            (
                BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8,
                BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH,
            )
        );
    }

    #[test]
    fn tx_packet_extent_and_bound_header_match_the_complete_allocator() {
        let packet = BluetoothDtmTxPacketAddress::new(0x2f00_0100)
            .expect("complete TX packet extent fits controller SRAM");
        assert_eq!(
            BluetoothDtmTxBufferHeaderImage::new(packet).words(),
            [0, 0x0000_0040, 0x80a0_0044, 0, 0x0000_07f8, 0]
        );

        assert!(BluetoothDtmTxPacketAddress::new(0x2f3f_feec).is_ok());
        assert_eq!(
            BluetoothDtmTxPacketAddress::new(0x2f3f_fef0),
            Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram)
        );
        assert_eq!(
            BluetoothDtmTxPacketAddress::new(0x2f00_0000),
            Err(BluetoothDtmTxPacketAddressError::ZeroCompressedBase)
        );
        assert_eq!(
            BluetoothDtmTxPacketAddress::new(0x2f00_0001),
            Err(BluetoothDtmTxPacketAddressError::InvalidBase(
                BluetoothControllerSramAddressError::Unaligned
            ))
        );
    }

    #[test]
    fn rx_packet_extent_and_bound_header_match_the_complete_allocator() {
        let packet = BluetoothDtmRxPacketAddress::new(0x2f00_0100)
            .expect("complete RX packet extent fits controller SRAM");
        assert_eq!(
            BluetoothDtmRxBufferHeaderImage::new(packet).words(),
            [0, 0x0000_0040, 0x8080_0000, 0, 0, 0]
        );
        assert_eq!(
            BluetoothDtmRxBufferHeaderImage::unbound_swap_reserve().words(),
            [0, 0, 0x8080_0000, 0, 0, 0]
        );

        assert!(BluetoothDtmRxPacketAddress::new(0x2f3f_fee0).is_ok());
        assert_eq!(
            BluetoothDtmRxPacketAddress::new(0x2f00_0000),
            Err(BluetoothDtmRxPacketAddressError::ZeroCompressedBase)
        );
        assert_eq!(
            BluetoothDtmRxPacketAddress::new(0x2f3f_fee4),
            Err(BluetoothDtmRxPacketAddressError::ExtentOutsideControllerSram)
        );
        assert_eq!(
            BluetoothDtmRxPacketAddress::new(0x2f00_0001),
            Err(BluetoothDtmRxPacketAddressError::InvalidBase(
                BluetoothControllerSramAddressError::Unaligned
            ))
        );
    }

    #[test]
    fn rx_rearm_updates_packet_and_header_as_one_transition() {
        let mut packet = BluetoothDtmRxPacketStorage::new();
        let mut header = BluetoothDtmBufferHeaderStorage {
            words: [0, 0, 0x8080_0000, 0x8123_4567, 0, 0],
        };
        assert_eq!(packet.bytes()[0x05], 1);
        assert_eq!(packet.bytes()[0x06], 1);
        assert_eq!(packet.result_word(), 0x00ff_ffff);
        assert_eq!(&packet.bytes()[0x18..0x1a], &[0xff, 0xff]);

        packet.bytes[0x0c] = 0;
        packet.bytes[0x0d] = 0;
        packet.bytes[0x0e] = 0;
        packet.bytes[0x0f] = 0xa5;
        packet.bytes[0x18] = 0;
        packet.bytes[0x19] = 0;
        BluetoothDtmRxBufferStorage {
            packet: &mut packet,
            header: &mut header,
        }
        .rearm_reviewed_fields()
        .expect("ordinary header does not request the swap reserve");

        assert_eq!(packet.result_word(), 0xa5ff_ffff);
        assert_eq!(&packet.bytes()[0x18..0x1a], &[0xff, 0xff]);
        assert_eq!(header.words[3], 0x0123_4567);
    }

    #[test]
    fn rx_rearm_quarantines_swap_reserve_bit_without_mutation() {
        let mut packet = BluetoothDtmRxPacketStorage::new();
        packet.bytes[0x0c] = 0;
        packet.bytes[0x0d] = 0;
        packet.bytes[0x0e] = 0;
        packet.bytes[0x0f] = 0xa5;
        packet.bytes[0x18] = 0;
        packet.bytes[0x19] = 0;
        let before_packet = packet.bytes;
        let mut header = BluetoothDtmBufferHeaderStorage {
            words: [0, 0x40, 0x8080_0000, 0x8123_4567, 1, 0],
        };
        let before_header = header.words;

        assert_eq!(
            BluetoothDtmRxBufferStorage {
                packet: &mut packet,
                header: &mut header,
            }
            .rearm_reviewed_fields(),
            Err(BluetoothDtmRxRearmError::SwapReserveDecisionRequired)
        );
        assert_eq!(packet.bytes, before_packet);
        assert_eq!(header.words, before_header);
    }

    #[test]
    fn graph_tx_slot_is_the_same_backing_used_for_packet_preparation() {
        let mut graph = BluetoothDtmMemoryGraphStorage::new();
        let mut preparation = graph.tx_packet_mut().begin_prepare(7, 3);
        preparation
            .payload_mut()
            .copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        let prepared = preparation.finish();

        assert_eq!(prepared.prepared_bytes()[0x05], 2);
        assert_eq!(prepared.prepared_bytes()[0x06], 0);
        assert_eq!(prepared.prepared_bytes()[0x10], 7);
        assert_eq!(prepared.prepared_bytes()[0x11], 3);
        assert_eq!(prepared.payload(), [0xaa, 0xbb, 0xcc]);
    }
}
