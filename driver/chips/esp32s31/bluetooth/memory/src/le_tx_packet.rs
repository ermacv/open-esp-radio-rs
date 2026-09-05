//! Common ESP32-S31 LE controller TX-packet allocation.
//!
//! Named same-chip `r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr` supplies this
//! allocation to both DTM and advertising. The role-specific producer owns
//! the Link Layer header and payload; this module alone owns their placement
//! inside controller SRAM. Preparing bytes here does not publish an address or
//! grant scheduler ownership.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use vcell::VolatileCell;

use crate::sram_link::BluetoothControllerSramLinkAddress;

/// Bytes in one common controller RX/TX buffer header.
pub const BLUETOOTH_LE_BUFFER_HEADER_BYTES: usize = 0x18;
/// Bytes preceding the maximum Link Layer payload in a controller TX allocation.
pub const BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES: usize = 0x12;

const CONTROLLER_METADATA_BYTES: usize = 0x10;
const PDU_HEADER_BYTE: usize = CONTROLLER_METADATA_BYTES;
const PDU_LENGTH_BYTE: usize = CONTROLLER_METADATA_BYTES + 1;
const ALLOCATION_CLASS_BYTE: usize = 0x05;
const ALLOCATION_STATE_BYTE: usize = 0x06;
const HEADER_PACKET_TARGET_OFFSET: u32 = CONTROLLER_METADATA_BYTES as u32;

/// Why a complete LE TX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BluetoothLeTxPacketAddressError {
    InvalidBase(BluetoothControllerSramAddressError),
    ZeroCompressedBase,
    UnsupportedAllocationSize,
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM geometry for one complete LE TX packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothLeTxPacketAddress<const ALLOCATION_BYTES: usize> {
    base: BluetoothControllerSramAddress,
    header_packet_target: BluetoothControllerSramAddress,
}

impl<const ALLOCATION_BYTES: usize> BluetoothLeTxPacketAddress<ALLOCATION_BYTES> {
    pub(super) const fn new(address: u32) -> Result<Self, BluetoothLeTxPacketAddressError> {
        if ALLOCATION_BYTES < BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES
            || ALLOCATION_BYTES - BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES > u8::MAX as usize
            || ALLOCATION_BYTES > u32::MAX as usize
        {
            return Err(BluetoothLeTxPacketAddressError::UnsupportedAllocationSize);
        }
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(BluetoothLeTxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(BluetoothLeTxPacketAddressError::ZeroCompressedBase);
        }
        let header_packet_target_address = match address.checked_add(HEADER_PACKET_TARGET_OFFSET) {
            Some(address) => address,
            None => return Err(BluetoothLeTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let last_aligned_offset = ((ALLOCATION_BYTES as u32 - 1) / 4) * 4;
        let last_aligned_address = match address.checked_add(last_aligned_offset) {
            Some(address) => address,
            None => return Err(BluetoothLeTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let header_packet_target =
            match BluetoothControllerSramAddress::new(header_packet_target_address) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothLeTxPacketAddressError::ExtentOutsideControllerSram);
                }
            };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(BluetoothLeTxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self {
            base,
            header_packet_target,
        })
    }

    pub(super) const fn base_link(self) -> BluetoothLeTxPacketBaseLink {
        BluetoothLeTxPacketBaseLink(self.base.compressed_image())
    }

    pub(super) const fn pdu_target_link(self) -> BluetoothLeTxPduTargetLink {
        BluetoothLeTxPduTargetLink(self.header_packet_target.compressed_image())
    }

    const fn allocation_extent_image(self) -> u32 {
        ((ALLOCATION_BYTES - BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES) as u32) << 3
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothLeTxPacketBaseLink(u32);

impl BluetoothLeTxPacketBaseLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn image(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothLeTxPduTargetLink(u32);

impl BluetoothLeTxPduTargetLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn image(self) -> u32 {
        self.0
    }
}

/// CPU-owned common TX buffer header before graph publication.
#[repr(C, align(4))]
pub(super) struct BluetoothLeTxBufferHeaderStorage {
    words: [VolatileCell<u32>; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
}

impl BluetoothLeTxBufferHeaderStorage {
    const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
    const ALLOCATION_EXTENT_MASK: u32 = 0x0000_07f8;

    pub(super) const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        }
    }

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    #[cfg(test)]
    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn install(&self, words: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4]) {
        for (cell, word) in self.words.iter().zip(words) {
            cell.set(word);
        }
    }

    pub(super) fn initialize_bound_tx<const ALLOCATION_BYTES: usize>(
        &self,
        packet: BluetoothLeTxPacketAddress<ALLOCATION_BYTES>,
    ) {
        self.initialize_bound_tx_with_successor(packet, None);
    }

    pub(super) fn initialize_bound_tx_with_successor<const ALLOCATION_BYTES: usize>(
        &self,
        packet: BluetoothLeTxPacketAddress<ALLOCATION_BYTES>,
        successor: Option<BluetoothControllerSramLinkAddress>,
    ) {
        self.install([
            successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image),
            packet.base_link().image(),
            0x80a0_0000 | packet.pdu_target_link().image(),
            0,
            packet.allocation_extent_image(),
            0,
        ]);
    }

    pub(super) fn packet_base_link(&self) -> Option<BluetoothLeTxPacketBaseLink> {
        BluetoothLeTxPacketBaseLink::from_image(self.read_word(1) & Self::COMPRESSED_LINK_MASK)
    }

    pub(super) fn pdu_target_link(&self) -> Option<BluetoothLeTxPduTargetLink> {
        BluetoothLeTxPduTargetLink::from_image(self.read_word(2) & Self::COMPRESSED_LINK_MASK)
    }

    pub(super) fn retains_allocation_extent<const ALLOCATION_BYTES: usize>(
        &self,
        packet: BluetoothLeTxPacketAddress<ALLOCATION_BYTES>,
    ) -> bool {
        self.read_word(4) & Self::ALLOCATION_EXTENT_MASK == packet.allocation_extent_image()
    }

    pub(super) fn retains_bound_tx_with_successor<const ALLOCATION_BYTES: usize>(
        &self,
        packet: BluetoothLeTxPacketAddress<ALLOCATION_BYTES>,
        successor: Option<BluetoothControllerSramLinkAddress>,
    ) -> bool {
        let expected_successor =
            successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        self.read_word(0) & Self::COMPRESSED_LINK_MASK == expected_successor
            && self.packet_base_link() == Some(packet.base_link())
            && self.pdu_target_link() == Some(packet.pdu_target_link())
            && self.retains_allocation_extent(packet)
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4] {
        core::array::from_fn(|index| self.read_word(index))
    }

    #[cfg(test)]
    pub(super) fn model_retarget_packet_base<const ALLOCATION_BYTES: usize>(
        &self,
        packet: BluetoothLeTxPacketAddress<ALLOCATION_BYTES>,
    ) {
        let current = self.read_word(1);
        self.write_word(
            1,
            (current & !Self::COMPRESSED_LINK_MASK) | packet.base_link().image(),
        );
    }

    #[cfg(test)]
    pub(super) fn model_retarget_pdu<const ALLOCATION_BYTES: usize>(
        &self,
        packet: BluetoothLeTxPacketAddress<ALLOCATION_BYTES>,
    ) {
        let current = self.read_word(2);
        self.write_word(
            2,
            (current & !Self::COMPRESSED_LINK_MASK) | packet.pdu_target_link().image(),
        );
    }

    #[cfg(test)]
    pub(super) fn model_drop_allocation_extent(&self) {
        let current = self.read_word(4);
        self.write_word(4, current & !Self::ALLOCATION_EXTENT_MASK);
    }
}

/// Why a Link Layer PDU cannot be installed in one controller TX allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLeTxPacketPrepareError {
    /// The storage type cannot contain even the reviewed controller prefix.
    AllocationTooSmall { available: usize },
    /// The supplied payload exceeds this allocation or the LE length field.
    PayloadTooLong { length: usize, capacity: usize },
    /// An encoded Link Layer PDU omitted its two-byte header.
    EncodedPduTooShort { available: usize },
    /// The encoded length field does not describe all supplied payload bytes.
    EncodedPduLengthMismatch { declared: u8, actual: usize },
}

/// Complete encoded PDU proved to fit one controller allocation class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothLeTxPacketPreparedInput<'a, const ALLOCATION_BYTES: usize> {
    pdu: &'a [u8],
    payload_length: u8,
}

impl<'a, const ALLOCATION_BYTES: usize> BluetoothLeTxPacketPreparedInput<'a, ALLOCATION_BYTES> {
    pub(super) fn new(pdu: &'a [u8]) -> Result<Self, BluetoothLeTxPacketPrepareError> {
        if ALLOCATION_BYTES < BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES {
            return Err(BluetoothLeTxPacketPrepareError::AllocationTooSmall {
                available: ALLOCATION_BYTES,
            });
        }
        if pdu.len() < 2 {
            return Err(BluetoothLeTxPacketPrepareError::EncodedPduTooShort {
                available: pdu.len(),
            });
        }
        let actual = pdu.len() - 2;
        let declared = pdu[1];
        if usize::from(declared) != actual {
            return Err(BluetoothLeTxPacketPrepareError::EncodedPduLengthMismatch {
                declared,
                actual,
            });
        }
        let capacity = ALLOCATION_BYTES
            .saturating_sub(BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES)
            .min(u8::MAX as usize);
        if actual > capacity {
            return Err(BluetoothLeTxPacketPrepareError::PayloadTooLong {
                length: actual,
                capacity,
            });
        }
        Ok(Self {
            pdu,
            payload_length: declared,
        })
    }

    pub(super) const fn from_validated_encoded_pdu(pdu: &'a [u8], payload_length: u8) -> Self {
        Self {
            pdu,
            payload_length,
        }
    }

    pub(super) const fn as_bytes(self) -> &'a [u8] {
        self.pdu
    }

    pub(super) const fn payload_bytes(self) -> u8 {
        self.payload_length
    }
}

/// Opaque length accepted by the controller TX-packet codec.
///
/// This value proves only that the length fits its allocation-size class;
/// hardware visibility and allocation identity remain owned by the enclosing
/// graph typestate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLeTxPacketPreparedLength<const ALLOCATION_BYTES: usize>(u8);

impl<const ALLOCATION_BYTES: usize> BluetoothLeTxPacketPreparedLength<ALLOCATION_BYTES> {
    /// Number of Link Layer payload bytes following the two-byte PDU header.
    pub const fn payload_bytes(self) -> u8 {
        self.0
    }

    /// Complete Link Layer PDU size, including its two-byte header.
    pub const fn pdu_bytes(self) -> usize {
        2 + self.0 as usize
    }
}

/// CPU-owned storage for one common ESP32-S31 LE controller TX allocation.
///
/// `ALLOCATION_BYTES` includes the complete controller prefix. A role chooses
/// the smallest capacity it needs; DTM uses all 255 LE payload octets while
/// legacy advertising needs at most 37.
#[repr(C, align(4))]
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothLeTxPacketStorage<const ALLOCATION_BYTES: usize> {
    bytes: [u8; ALLOCATION_BYTES],
}

impl<const ALLOCATION_BYTES: usize> BluetoothLeTxPacketStorage<ALLOCATION_BYTES> {
    /// Reserve a zero-based, CPU-owned allocation.
    pub const fn new() -> Self {
        Self {
            bytes: [0; ALLOCATION_BYTES],
        }
    }

    /// Maximum Link Layer payload accepted by this allocation.
    pub const fn payload_capacity() -> usize {
        ALLOCATION_BYTES.saturating_sub(BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES)
    }

    /// Install a role-produced PDU header and its complete payload.
    pub fn prepare_pdu(
        &mut self,
        header: u8,
        payload: &[u8],
    ) -> Result<BluetoothLeTxPacketPreparedLength<ALLOCATION_BYTES>, BluetoothLeTxPacketPrepareError>
    {
        if ALLOCATION_BYTES < BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES {
            return Err(BluetoothLeTxPacketPrepareError::AllocationTooSmall {
                available: ALLOCATION_BYTES,
            });
        }

        let capacity = Self::payload_capacity().min(u8::MAX as usize);
        if payload.len() > capacity {
            return Err(BluetoothLeTxPacketPrepareError::PayloadTooLong {
                length: payload.len(),
                capacity,
            });
        }

        let length = payload.len() as u8;
        self.bytes[ALLOCATION_CLASS_BYTE] = 2;
        self.bytes[ALLOCATION_STATE_BYTE] = 0;
        self.bytes[PDU_HEADER_BYTE] = header;
        self.bytes[PDU_LENGTH_BYTE] = length;
        self.bytes[BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES..][..payload.len()].copy_from_slice(payload);
        Ok(BluetoothLeTxPacketPreparedLength(length))
    }

    /// Install one complete, already encoded two-byte-header Link Layer PDU.
    pub fn prepare_encoded_pdu(
        &mut self,
        pdu: &[u8],
    ) -> Result<BluetoothLeTxPacketPreparedLength<ALLOCATION_BYTES>, BluetoothLeTxPacketPrepareError>
    {
        let packet = BluetoothLeTxPacketPreparedInput::new(pdu)?;
        Ok(self.prepare_validated_encoded_pdu(packet))
    }

    pub(super) fn prepare_validated_encoded_pdu(
        &mut self,
        packet: BluetoothLeTxPacketPreparedInput<'_, ALLOCATION_BYTES>,
    ) -> BluetoothLeTxPacketPreparedLength<ALLOCATION_BYTES> {
        let pdu = packet.as_bytes();
        let length = packet.payload_bytes();
        self.bytes[ALLOCATION_CLASS_BYTE] = 2;
        self.bytes[ALLOCATION_STATE_BYTE] = 0;
        self.bytes[PDU_HEADER_BYTE] = pdu[0];
        self.bytes[PDU_LENGTH_BYTE] = length;
        self.bytes[BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES..][..usize::from(length)]
            .copy_from_slice(&pdu[2..]);
        BluetoothLeTxPacketPreparedLength(length)
    }

    /// Borrow the semantic Link Layer PDU installed by the enclosing owner.
    pub fn prepared_pdu(
        &self,
        length: BluetoothLeTxPacketPreparedLength<ALLOCATION_BYTES>,
    ) -> &[u8] {
        &self.bytes[CONTROLLER_METADATA_BYTES..][..length.pdu_bytes()]
    }

    pub(super) fn prepared_allocation(
        &self,
        length: BluetoothLeTxPacketPreparedLength<ALLOCATION_BYTES>,
    ) -> &[u8] {
        &self.bytes[..BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + usize::from(length.payload_bytes())]
    }

    pub(super) fn clear(&mut self) {
        self.bytes.fill(0);
    }

    #[cfg(test)]
    pub(super) const fn snapshot(&self) -> [u8; ALLOCATION_BYTES] {
        self.bytes
    }
}

impl<const ALLOCATION_BYTES: usize> Default for BluetoothLeTxPacketStorage<ALLOCATION_BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
