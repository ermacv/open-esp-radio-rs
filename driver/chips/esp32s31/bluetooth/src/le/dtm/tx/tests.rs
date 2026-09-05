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
