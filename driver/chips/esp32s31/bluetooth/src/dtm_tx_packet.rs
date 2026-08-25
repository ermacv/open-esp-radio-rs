//! Reviewed Direct Test Mode transmitter packet and header images.
//!
//! Current `r_sym_ble_4FZFpypyQDtGoyqc084f` and named same-chip
//! `r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr` construct the complete 24-byte
//! allocation-time header below. The DTM allocator always requests the full
//! eight-bit payload capacity. Current and named `dtm_tx_create_ctx` bodies
//! then write the four positional packet bytes and bounded payload modeled by
//! this module. No type here exposes a pointer or hardware publication token.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

use crate::{BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern};

/// Bytes preceding the DTM payload in the reviewed TX buffer.
pub const BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES: usize = 0x12;
/// Maximum payload bytes represented by the reviewed eight-bit HCI length.
pub const BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES: usize = u8::MAX as usize;
/// Complete caller-owned storage required by the reviewed DTM TX allocation.
pub const BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES: usize =
    BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES;

const PACKET_LAST_ALIGNED_OFFSET: u32 = 0x110;
const HEADER_PACKET_TARGET_OFFSET: u32 = 0x10;

/// Why a complete DTM TX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTxPacketAddressError {
    /// The proposed base is not a reviewed compressed controller-SRAM address.
    InvalidBase(BluetoothControllerSramAddressError),
    /// The aligned packet tail crosses the reviewed controller-SRAM window.
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM addresses for one complete DTM TX packet.
///
/// The value proves only address geometry for a 273-byte packet allocation. It
/// does not dereference, allocate, publish or establish a hardware lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxPacketAddress {
    base: BluetoothControllerSramAddress,
    header_packet_target: BluetoothControllerSramAddress,
}

impl BluetoothDtmTxPacketAddress {
    /// Validate the base, payload prefix and complete aligned allocation tail.
    pub const fn new(address: u32) -> Result<Self, BluetoothDtmTxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(BluetoothDtmTxPacketAddressError::InvalidBase(error)),
        };
        let header_packet_target_address = match address.checked_add(HEADER_PACKET_TARGET_OFFSET) {
            Some(address) => address,
            None => return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let last_aligned_address = match address.checked_add(PACKET_LAST_ALIGNED_OFFSET) {
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
///
/// The named allocator zeroes all 24 bytes before installing the fields. Names
/// remain positional because the packet-engine consumer and field semantics
/// above the two pointer images have not been independently established.
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

/// Statically sized CPU storage for one maximum-length DTM TX packet.
///
/// The inline storage may later be placed by a platform owner, but this type
/// deliberately has no address or publication API. Four-byte alignment only
/// matches the reviewed compressed-pointer geometry; it is not a DMA claim.
#[repr(C, align(4))]
pub struct BluetoothDtmTxPacketStorage {
    bytes: [u8; BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES],
}

impl BluetoothDtmTxPacketStorage {
    /// Create zeroed, CPU-owned storage without allocating.
    pub const fn new() -> Self {
        Self {
            bytes: [0; BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES],
        }
    }

    /// Apply every complete packet-buffer write performed before scheduling.
    pub fn prepare(
        &mut self,
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    ) -> BluetoothDtmPreparedTxPacket<'_> {
        self.bytes[0x05] = 2;
        self.bytes[0x06] = 0;
        self.bytes[0x10] = pattern.hci_selector();
        self.bytes[0x11] = length.hci_image();
        pattern.fill_reviewed(
            &mut self.bytes[BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES..][..length.as_usize()],
        );

        BluetoothDtmPreparedTxPacket {
            pattern,
            length,
            storage: self,
        }
    }
}

impl Default for BluetoothDtmTxPacketStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Affine CPU-owned view of one prepared DTM TX packet.
///
/// It exposes the reviewed prefix and declared payload only. No hardware state
/// can be reached from this value.
pub struct BluetoothDtmPreparedTxPacket<'storage> {
    pattern: BluetoothDtmPayloadPattern,
    length: BluetoothDtmPayloadLength,
    storage: &'storage mut BluetoothDtmTxPacketStorage,
}

impl<'storage> BluetoothDtmPreparedTxPacket<'storage> {
    /// Return the selected HCI test pattern.
    pub const fn pattern(&self) -> BluetoothDtmPayloadPattern {
        self.pattern
    }

    /// Return the declared eight-bit HCI length.
    pub const fn length(&self) -> BluetoothDtmPayloadLength {
        self.length
    }

    /// Borrow the open packet prefix and declared payload.
    ///
    /// Only bytes `+0x05`, `+0x06`, `+0x10` and `+0x11` have reviewed writes;
    /// the other prefix bytes retain their CPU-owned slot images.
    pub fn prepared_bytes(&self) -> &[u8] {
        &self.storage.bytes[..BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + self.length.as_usize()]
    }

    /// Borrow only the declared payload bytes.
    pub fn payload(&self) -> &[u8] {
        &self.storage.bytes[BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES..][..self.length.as_usize()]
    }

    /// Return the CPU-owned slot for reuse or later verified composition.
    pub fn release(self) -> &'storage mut BluetoothDtmTxPacketStorage {
        self.storage
    }
}

#[cfg(test)]
mod tests {
    use core::mem::{align_of, size_of};

    use open_esp_radio_esp32s31_pac::BluetoothControllerSramAddressError;

    use super::{
        BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES, BluetoothDtmTxBufferHeaderImage,
        BluetoothDtmTxPacketAddress, BluetoothDtmTxPacketAddressError, BluetoothDtmTxPacketStorage,
    };
    use crate::{BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern};

    #[test]
    fn packet_address_validates_base_header_target_and_complete_aligned_tail() {
        let packet = BluetoothDtmTxPacketAddress::new(0x2f00_0100)
            .expect("complete packet extent fits controller SRAM");
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
            BluetoothDtmTxPacketAddress::new(0x2f00_0001),
            Err(BluetoothDtmTxPacketAddressError::InvalidBase(
                BluetoothControllerSramAddressError::Unaligned
            ))
        );
    }

    #[test]
    fn static_packet_slot_has_the_reviewed_capacity_and_alignment() {
        assert_eq!(BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES, 0x111);
        assert_eq!(size_of::<BluetoothDtmTxPacketStorage>(), 0x114);
        assert_eq!(align_of::<BluetoothDtmTxPacketStorage>(), 4);
    }

    #[test]
    fn packet_preparation_writes_every_complete_positional_byte() {
        let mut storage = BluetoothDtmTxPacketStorage::new();
        let prepared = storage.prepare(
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadLength::from_hci_image(3),
        );

        assert_eq!(
            prepared.pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(prepared.length().hci_image(), 3);
        assert_eq!(prepared.payload(), [0x0f; 3]);
        assert_eq!(prepared.prepared_bytes().len(), 0x15);
        assert_eq!(prepared.prepared_bytes()[0x05], 2);
        assert_eq!(prepared.prepared_bytes()[0x06], 0);
        assert_eq!(prepared.prepared_bytes()[0x10], 1);
        assert_eq!(prepared.prepared_bytes()[0x11], 3);
    }

    #[test]
    fn reprepare_preserves_bytes_outside_the_new_declared_packet() {
        let mut storage = BluetoothDtmTxPacketStorage::new();
        let prepared = storage.prepare(
            BluetoothDtmPayloadPattern::RepeatedAllOnes,
            BluetoothDtmPayloadLength::from_hci_image(4),
        );
        let storage = prepared.release();
        let prepared = storage.prepare(
            BluetoothDtmPayloadPattern::RepeatedAllZeros,
            BluetoothDtmPayloadLength::from_hci_image(2),
        );
        assert_eq!(prepared.payload(), [0, 0]);
        let storage = prepared.release();
        assert_eq!(storage.bytes[0x14], 0xff);
        assert_eq!(storage.bytes[0x15], 0xff);
    }
}
