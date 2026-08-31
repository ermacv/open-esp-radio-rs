//! Reviewed Direct Test Mode transmitter packet and header images.
//!
//! Current `r_sym_ble_4FZFpypyQDtGoyqc084f` and named same-chip
//! `r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr` construct the complete 24-byte
//! allocation-time header now owned by the controller-memory layer. The DTM
//! allocator always requests the full eight-bit payload capacity. Current and
//! named `dtm_tx_create_ctx` bodies then write the standard no-CTE PDU header,
//! payload length, two positional allocator bytes and bounded payload modeled
//! by this LLL extension. Both layers remain CPU-only and expose no hardware
//! publication token.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_DTM_MAX_PACKET_CAPACITY as BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES,
    BLUETOOTH_DTM_TX_PACKET_BYTES as BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES,
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphTxPacketPrepared,
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
        let memory =
            match self.prepare_tx_packet(pattern.hci_selector(), length.hci_image(), &payload) {
                Ok(memory) => memory,
                Err(_) => {
                    unreachable!("a typed DTM payload pattern always has a standard PDU Type")
                }
            };
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

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
        BluetoothDtmSchedulerAllocationConfig,
    };

    use super::BluetoothDtmTxGraphPrepare;
    use crate::{BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern};

    #[test]
    fn bound_graph_preparation_retains_the_typed_packet_identity() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("test base has valid compressed-pointer syntax");
        let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(
            storage,
            base,
            BluetoothDtmSchedulerAllocationConfig::new(2, 3, 4),
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
    }
}
