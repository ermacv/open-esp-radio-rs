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
        self.install([
            0,
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
        self.prepare_pdu(pdu[0], &pdu[2..])
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
mod tests {
    use super::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothLeTxBufferHeaderStorage,
        BluetoothLeTxPacketAddress, BluetoothLeTxPacketPrepareError, BluetoothLeTxPacketStorage,
    };

    #[test]
    fn encoded_pdu_round_trips_at_the_role_boundary() {
        let mut packet = BluetoothLeTxPacketStorage::<32>::new();
        let length = packet
            .prepare_encoded_pdu(&[0x42, 3, 1, 2, 3])
            .expect("the encoded PDU fits the controller allocation");

        assert_eq!(packet.prepared_pdu(length), &[0x42, 3, 1, 2, 3]);
        assert_eq!(length.payload_bytes(), 3);
    }

    #[test]
    fn malformed_or_oversized_pdu_is_rejected_before_mutation() {
        let mut packet =
            BluetoothLeTxPacketStorage::<{ BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 2 }>::new();
        let before = packet.snapshot();

        assert_eq!(
            packet.prepare_encoded_pdu(&[0x02]),
            Err(BluetoothLeTxPacketPrepareError::EncodedPduTooShort { available: 1 })
        );
        assert_eq!(packet.snapshot(), before);
        assert_eq!(
            packet.prepare_encoded_pdu(&[0x02, 2, 0]),
            Err(BluetoothLeTxPacketPrepareError::EncodedPduLengthMismatch {
                declared: 2,
                actual: 1,
            })
        );
        assert_eq!(packet.snapshot(), before);
        assert_eq!(
            packet.prepare_encoded_pdu(&[0x02, 3, 0, 1, 2]),
            Err(BluetoothLeTxPacketPrepareError::PayloadTooLong {
                length: 3,
                capacity: 2,
            })
        );
        assert_eq!(packet.snapshot(), before);
    }

    #[test]
    fn buffer_header_retains_the_bound_packet_and_its_role_capacity() {
        const LEGACY_ADVERTISING_ALLOCATION_BYTES: usize = BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 37;
        const DTM_ALLOCATION_BYTES: usize = BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 255;

        let advertising =
            BluetoothLeTxPacketAddress::<LEGACY_ADVERTISING_ALLOCATION_BYTES>::new(0x2f00_0100)
                .expect("the advertising allocation fits controller SRAM");
        let dtm = BluetoothLeTxPacketAddress::<DTM_ALLOCATION_BYTES>::new(0x2f00_0100)
            .expect("the DTM allocation fits controller SRAM");
        let header = BluetoothLeTxBufferHeaderStorage::new();

        header.initialize_bound_tx(advertising);

        assert_eq!(header.packet_base_link(), Some(advertising.base_link()));
        assert_eq!(
            header.pdu_target_link(),
            Some(advertising.pdu_target_link())
        );
        assert!(header.retains_allocation_extent(advertising));
        assert!(!header.retains_allocation_extent(dtm));
    }
}
