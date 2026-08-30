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

pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_DTM_MAX_PACKET_CAPACITY as BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES,
    BLUETOOTH_DTM_TX_PACKET_BYTES as BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES,
    BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES, BluetoothDtmTxBufferHeaderImage,
    BluetoothDtmTxPacketAddress, BluetoothDtmTxPacketAddressError, BluetoothDtmTxPacketStorage,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphTxPacketPrepared,
    BluetoothDtmPreparedTxPacketStorage,
};

use crate::{BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern};

/// LLL extension that consumes a bound graph into a standard DTM TX packet.
pub trait BluetoothDtmTxGraphPrepare {
    /// Fill every declared payload byte and retain its semantic pattern proof.
    fn prepare_dtm_tx_packet(
        self,
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    ) -> BluetoothDtmPreparedTxGraph;
}

impl BluetoothDtmTxGraphPrepare for BluetoothDtmMemoryGraphCpuOwned {
    fn prepare_dtm_tx_packet(
        self,
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    ) -> BluetoothDtmPreparedTxGraph {
        let mut payload = [0; BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES];
        pattern.fill_reviewed(&mut payload[..usize::from(length.hci_image())]);
        let memory = self.prepare_tx_packet(pattern.hci_selector(), length.hci_image(), &payload);
        BluetoothDtmPreparedTxGraph {
            memory,
            pattern,
            length,
        }
    }
}

/// Bound CPU-owned graph carrying one complete standard DTM TX packet.
///
/// This state proves only packet construction. The graph remains unreachable
/// by hardware and has no scheduler, fence or publication authority.
#[must_use = "the prepared TX graph must be composed or explicitly discarded"]
pub struct BluetoothDtmPreparedTxGraph {
    memory: BluetoothDtmMemoryGraphTxPacketPrepared,
    pattern: BluetoothDtmPayloadPattern,
    length: BluetoothDtmPayloadLength,
}

impl BluetoothDtmPreparedTxGraph {
    /// Return the validated HCI test pattern represented by the packet.
    pub const fn pattern(&self) -> BluetoothDtmPayloadPattern {
        self.pattern
    }

    /// Return the validated HCI payload length represented by the packet.
    pub const fn length(&self) -> BluetoothDtmPayloadLength {
        self.length
    }

    /// Borrow the complete reviewed prefix and declared payload.
    pub fn prepared_bytes(&self) -> &[u8] {
        self.memory.prepared_packet_bytes()
    }

    /// Discard packet readiness and recover the ordinary CPU-owned graph.
    pub fn discard(self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.memory.discard_packet_readiness()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphTxPacketPrepared,
        BluetoothDtmPayloadPattern,
        BluetoothDtmPayloadLength,
    ) {
        (self.memory, self.pattern, self.length)
    }
}

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
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
        BluetoothDtmSchedulerAllocationConfig,
    };

    use super::{
        BluetoothDtmTxGraphPrepare, BluetoothDtmTxPacketPrepare, BluetoothDtmTxPacketStorage,
    };
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

    #[test]
    fn bound_graph_preparation_retains_the_typed_packet_identity() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("test base has valid compressed-pointer syntax");
        let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(
            storage,
            base,
            BluetoothDtmSchedulerAllocationConfig::new(2, 3, 5, 4),
        )
        .expect("test graph fits physical controller SRAM");

        let prepared = owner.prepare_dtm_tx_packet(
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadLength::from_hci_image(3),
        );

        assert_eq!(
            prepared.pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(prepared.length().hci_image(), 3);
        assert_eq!(&prepared.prepared_bytes()[0x12..], &[0x0f; 3]);
    }
}
