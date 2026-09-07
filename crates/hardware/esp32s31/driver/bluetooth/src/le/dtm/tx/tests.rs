use crate::le::dtm::{DtmPayloadLength, DtmPayloadPattern};

use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphModelAddress, DtmMemoryGraphStorage, DtmSchedulerAllocationConfig,
};

use super::DtmTxGraphPrepare;

#[test]
fn bound_graph_preparation_retains_the_typed_packet_identity() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(DtmMemoryGraphStorage::new()));
    let base = DtmMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("test base has valid compressed-pointer syntax");
    let owner = DtmMemoryGraphStorage::pin_static_model(
        storage,
        base,
        DtmSchedulerAllocationConfig::new(2, 3, 4),
    )
    .expect("test graph fits physical controller SRAM");

    let prepared = owner.prepare_dtm_tx_packet(
        DtmPayloadPattern::Repeated11110000,
        DtmPayloadLength::from_hci_image(3),
    );

    assert_eq!(prepared.pattern(), DtmPayloadPattern::Repeated11110000);
    assert_eq!(prepared.length().hci_image(), 3);
}
