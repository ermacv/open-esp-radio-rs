//! Reviewed Direct Test Mode transmitter packet and header images.
//!
//! Current `r_sym_ble_4FZFpypyQDtGoyqc084f` and named same-chip
//! `r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr` construct the complete 24-byte
//! allocation-time header now owned by the controller-memory layer. The DTM
//! allocator always requests the full eight-bit payload capacity. Current and
//! named `dtm_tx_create_ctx` bodies then write the four positional packet bytes
//! and bounded payload modeled by this LLL extension. Both layers remain
//! CPU-only and expose no hardware publication token.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmPreparedTxPacketStorage;
pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_DTM_MAX_PACKET_CAPACITY as BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES,
    BLUETOOTH_DTM_TX_PACKET_BYTES as BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES,
    BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES, BluetoothDtmTxBufferHeaderImage,
    BluetoothDtmTxPacketAddress, BluetoothDtmTxPacketAddressError, BluetoothDtmTxPacketStorage,
};

use crate::{BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern};

/// LLL extension that fills standard DTM pattern bytes in the sole memory-layer
/// TX backing slot.
pub trait BluetoothDtmTxPacketPrepare {
    /// Apply every complete packet-buffer write performed before scheduling.
    fn prepare(
        &mut self,
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    ) -> BluetoothDtmPreparedTxPacket<'_>;
}

impl BluetoothDtmTxPacketPrepare for BluetoothDtmTxPacketStorage {
    fn prepare(
        &mut self,
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    ) -> BluetoothDtmPreparedTxPacket<'_> {
        let mut preparation = self.begin_prepare(pattern.hci_selector(), length.hci_image());
        pattern.fill_reviewed(preparation.payload_mut());
        BluetoothDtmPreparedTxPacket {
            pattern,
            length,
            storage: preparation.finish(),
        }
    }
}

/// Affine CPU-owned view of one prepared DTM TX packet.
///
/// It exposes the reviewed prefix and declared payload only. No hardware state
/// can be reached from this value.
pub struct BluetoothDtmPreparedTxPacket<'storage> {
    pattern: BluetoothDtmPayloadPattern,
    length: BluetoothDtmPayloadLength,
    storage: BluetoothDtmPreparedTxPacketStorage<'storage>,
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
        self.storage.prepared_bytes()
    }

    /// Borrow only the declared payload bytes.
    pub fn payload(&self) -> &[u8] {
        self.storage.payload()
    }

    /// Return the CPU-owned slot for reuse or later verified composition.
    pub fn release(self) -> &'storage mut BluetoothDtmTxPacketStorage {
        self.storage.release()
    }
}

#[cfg(test)]
mod tests {
    use super::{BluetoothDtmTxPacketPrepare, BluetoothDtmTxPacketStorage};
    use crate::{BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern};

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
        assert_eq!(storage.bytes()[0x14], 0xff);
        assert_eq!(storage.bytes()[0x15], 0xff);
    }
}
