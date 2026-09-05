use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

use crate::{
    BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanPrimaryChannel,
    BluetoothPassiveScanResetConfig, BluetoothPassiveScanSchedulerWindow,
    BluetoothPassiveScanStartSelection,
    le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
    passive_scanning_event_image::BluetoothPassiveScanRxHeadProjection,
};

use super::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothPassiveScanMemoryGraphBindError,
    BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanMemoryGraphStorage,
    BluetoothPassiveScanSchedulerAllocationConfig,
};

fn reset_config() -> BluetoothPassiveScanResetConfig {
    BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
        BluetoothPassiveScanDefaultTxPowerDbm::new(0),
        BluetoothControllerLatchedTime::from_bits(0x1234_5678),
    )
}

fn model_graph(base: u32) -> super::BluetoothPassiveScanMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPassiveScanMemoryGraphStorage::new(),
    ));
    let address = BluetoothPassiveScanMemoryGraphModelAddress::new(base)
        .expect("the model address is controller-encodable");
    BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
        storage,
        address,
        reset_config(),
        BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the restricted product limits fit every scanner item"),
    )
    .expect("the graph fits physical controller SRAM")
}

#[test]
fn initialized_graph_contains_the_scanner_link_state_and_receive_chain() {
    let owner = model_graph(0x2f00_0100);
    let bindings = owner.binding.nodes;
    let storage = owner.storage.as_ref().get_ref();

    assert!(storage.nodes[0].header.retains_packet(bindings[0].packet));
    assert!(storage.nodes[1].header.retains_packet(bindings[1].packet));
    assert_eq!(
        storage.nodes[0].header.successor(),
        Some(bindings[1].header.compressed_image())
    );
    assert_eq!(storage.nodes[1].header.successor(), None);
    assert_eq!(storage.nodes[0].header.predecessor(), None);
    assert_eq!(
        storage.nodes[1].header.predecessor(),
        Some(bindings[0].header.controller_address().address())
    );
    assert!(storage.nodes[0].header.rotates_into_successor());
    assert!(!storage.nodes[1].header.rotates_into_successor());
    assert!(storage.nodes.iter().all(|node| node.packet.is_armed()));
    let link_state = storage.link_state.image();
    assert!(
        link_state.retains_rx_head(BluetoothPassiveScanRxHeadProjection::from_bound(
            bindings[0].header
        ))
    );
    assert_eq!(link_state.crc_init(), BluetoothLeCrcInit::LE_PRESET);
    assert_eq!(
        link_state.access_address(),
        BluetoothLeAccessAddress::PRIMARY_ADVERTISING
    );
    assert_eq!(
        link_state.controller_time(),
        reset_config().controller_time().bits()
    );
    assert_eq!(
        storage.link_state.receive_graph(),
        (
            bindings[0].header.controller_address(),
            bindings[1].header.controller_address(),
            None,
        )
    );
    assert_eq!(
        storage.link_state.scheduler_head(),
        owner.binding.scheduler_head()
    );
    assert!(
        storage
            .scheduler_items
            .iter()
            .enumerate()
            .all(|(index, item)| item.retains_graph(
                index
                    .checked_sub(1)
                    .map(|index| owner.binding.scheduler_items[index]),
                owner.binding.scheduler_context,
                owner.binding.link_state,
            ))
    );

    let link_state_address = owner.binding.link_state();
    let scheduler_head = owner.binding.scheduler_head();
    let window = BluetoothPassiveScanSchedulerWindow::from_controller_ticks(500, 1_500)
        .expect("the first scan window is non-empty");
    let event = owner.prepare_first_event(
        BluetoothPassiveScanPrimaryChannel::Channel37,
        window,
        BluetoothPassiveScanStartSelection::Requested,
        BluetoothControllerLatchedTime::from_bits(0x2345_6789),
    );
    assert_eq!(
        event.channel(),
        BluetoothPassiveScanPrimaryChannel::Channel37
    );
    assert_eq!(event.window(), window);
    assert_eq!(
        event
            .storage
            .as_ref()
            .get_ref()
            .link_state
            .image()
            .controller_time(),
        0x2345_6789
    );
    let event = event.prepare_scheduler_admission().cancel();
    let prepared = event.prepare_scheduler_admission().prepare_publication();
    assert_eq!(prepared.head(), bindings[0].header.controller_address());
    assert_eq!(prepared.link_state(), link_state_address);
    assert_eq!(prepared.scheduler_head(), scheduler_head);
    assert_eq!(
        prepared.selector(),
        crate::BluetoothRxMemoryListClass::Scanning.selector()
    );
}

#[test]
fn failed_extent_binding_retains_the_exact_static_allocation() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPassiveScanMemoryGraphStorage::new(),
    ));
    let identity = core::ptr::addr_of!(*storage).addr();
    let address = BluetoothPassiveScanMemoryGraphModelAddress::new(
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 4,
    )
    .expect("the aligned address remains controller-encodable");

    let failure = match BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
        storage,
        address,
        reset_config(),
        BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the restricted product limits fit every scanner item"),
    ) {
        Ok(_) => panic!("the complete graph must not cross physical SRAM"),
        Err(failure) => failure,
    };

    assert_eq!(
        failure.error(),
        BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram
    );
    let (storage, _) = failure.into_parts();
    assert_eq!(core::ptr::addr_of!(*storage).addr(), identity);
}

#[test]
fn completed_receive_nodes_copy_only_bounded_pdu_and_signed_rssi() {
    let owner = model_graph(0x2f00_1000);
    let storage = owner.storage.as_ref().get_ref();
    let pdu = [0x02, 6, 1, 2, 3, 4, 5, 6];
    storage.nodes[0]
        .packet
        .emulate_hardware_receive(&pdu, -47, 0x1234_5678);
    storage.nodes[0].header.emulate_hardware_completion();

    let batch = storage
        .extract_received_batch()
        .expect("one completed prefix node is a valid receive batch");
    assert_eq!(batch.len(), 1);
    let packet = batch.packet(0).expect("the completed node is retained");
    assert_eq!(packet.as_bytes(), &pdu);
    assert_eq!(packet.rssi_dbm(), -47);
    assert!(batch.packet(1).is_none());
}

#[test]
fn completed_receive_chain_rejects_a_gap_before_packet_access() {
    let owner = model_graph(0x2f00_2000);
    let storage = owner.storage.as_ref().get_ref();
    storage.nodes[1].packet.emulate_hardware_receive(
        &[0x02, 6, 1, 2, 3, 4, 5, 6],
        -20,
        0x2345_6789,
    );
    storage.nodes[1].header.emulate_hardware_completion();

    assert_eq!(
        storage.extract_received_batch(),
        Err(super::BluetoothLeRxError::CompletionChainGap)
    );
}

#[test]
fn scheduler_limits_must_fit_every_retained_item() {
    assert!(BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0).is_some());
    assert!(BluetoothPassiveScanSchedulerAllocationConfig::new(u16::MAX, u16::MAX).is_none());
}
