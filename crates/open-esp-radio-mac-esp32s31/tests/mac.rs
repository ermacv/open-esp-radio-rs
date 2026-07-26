use std::{cell::RefCell, collections::BTreeMap};

use open_esp_radio_mac_esp32s31::{
    descriptor::{
        descriptor_address_valid, dma_range_valid, length, rx_armed_word, rx_rearm_word, size,
        tx_owned_word, Descriptor, BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT,
    },
    irq::{handle_mac_irq, IrqDisposition, IrqState},
    registers::{
        Mmio, MAC_INT_CLEAR, MAC_INT_ENABLE, MAC_INT_RX_SUCCESS, MAC_INT_STATUS,
        MAC_INT_TX_COMPLETE, RX_CONTROL, RX_DESCRIPTOR_BASE, RX_ENABLE, RX_LAST_DESCRIPTOR_HIGH,
        TX_COMPLETE_ALTERNATE_Q0, TX_COMPLETE_AUX_A_Q0, TX_COMPLETE_AUX_B_Q0, TX_COMPLETE_AUX_C_Q0,
        TX_COMPLETE_CLEAR, TX_COMPLETE_PRIMARY_Q0, TX_COMPLETE_Q0, TX_COMPLETE_STATE,
        TX_Q0_CONTROL, TX_Q_ENABLE_VALID, TX_STATE,
    },
    rx::{
        build_cold_ring, disable_receive, extract_management, publish_cold_ring, rearm_descriptor,
        RxError, RxIngressConfig, RxSegment, INGRESS_STRICT_DUMP, INGRESS_STRICT_RXEND,
    },
    tx::{legacy_q0_image, LegacyTxConfig, TxError, TxSlot, TxSlotState},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Read(u32),
    Write(u32, u32),
    Fence,
}

#[derive(Default)]
struct MockMmio {
    words: RefCell<BTreeMap<u32, u32>>,
    operations: RefCell<Vec<Operation>>,
}

impl MockMmio {
    fn set(&self, address: u32, value: u32) {
        self.words.borrow_mut().insert(address, value);
    }

    fn operations(&self) -> Vec<Operation> {
        self.operations.borrow().clone()
    }
}

impl Mmio for MockMmio {
    fn read32(&self, address: u32) -> u32 {
        self.operations.borrow_mut().push(Operation::Read(address));
        self.words.borrow().get(&address).copied().unwrap_or(0)
    }

    fn write32(&self, address: u32, value: u32) {
        self.operations
            .borrow_mut()
            .push(Operation::Write(address, value));
        self.words.borrow_mut().insert(address, value);
    }

    fn fence(&self) {
        self.operations.borrow_mut().push(Operation::Fence);
    }
}

#[test]
fn descriptor_words_preserve_the_recovered_geometry() {
    assert_eq!(
        core::mem::size_of::<Descriptor>(),
        DESCRIPTOR_BYTES as usize
    );
    assert!(descriptor_address_valid(0x2f00_0000));
    assert!(!descriptor_address_valid(0x2f00_0002));
    assert!(dma_range_valid(0x2f00_0100, 0x100));
    assert!(!dma_range_valid(0x2f07_fff0, 0x20));

    let rx = rx_armed_word(1700).unwrap();
    assert_eq!(size(rx), 1700);
    assert_eq!(length(rx), 1700);
    assert_ne!(rx & BIT_31, 0);
    assert_eq!(rx & BIT_30, 0);

    let completed = 1700 | (96 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    let recycled = rx_rearm_word(completed).unwrap();
    assert_eq!(size(recycled), 1700);
    assert_eq!(length(recycled), 1700);
    assert_ne!(recycled & BIT_31, 0);
    assert_eq!(recycled & BIT_30, 0);

    let tx = tx_owned_word(512, 123).unwrap();
    assert_eq!(size(tx), 512);
    assert_eq!(length(tx), 123);
    assert_eq!(tx & (BIT_30 | BIT_31), BIT_30 | BIT_31);
    assert_eq!(tx_owned_word(64, 65), None);
}

#[test]
fn cold_rx_ring_publishes_links_and_hardware_in_order() {
    let descriptors = [Descriptor::new(), Descriptor::new()];
    build_cold_ring(&descriptors, 0x2f00_1000, &[0x2f00_2000, 0x2f00_2800], 1700).unwrap();
    assert_eq!(
        descriptors[0].next_address(),
        0x2f00_1000 + DESCRIPTOR_BYTES
    );
    assert_eq!(descriptors[1].next_address(), 0);

    let mmio = MockMmio::default();
    mmio.set(RX_LAST_DESCRIPTOR_HIGH, 0x0005_4321);
    mmio.set(RX_CONTROL, 0x1234);
    publish_cold_ring(&mmio, 0x2f00_1000, true).unwrap();

    assert_eq!(
        mmio.operations(),
        vec![
            Operation::Fence,
            Operation::Read(RX_LAST_DESCRIPTOR_HIGH),
            Operation::Write(RX_LAST_DESCRIPTOR_HIGH, 0x2f05_4321),
            Operation::Write(RX_DESCRIPTOR_BASE, 0x2f00_1000),
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, 0x1234 | RX_ENABLE),
            Operation::Fence,
        ]
    );
}

#[test]
fn completed_rx_descriptor_rearms_only_for_the_expected_storage() {
    let descriptor = Descriptor::new();
    let completed = 256 | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptor.publish(completed, 0x2f00_3000, 0);
    rearm_descriptor(&descriptor, 0x2f00_3000, 0).unwrap();
    assert_eq!(length(descriptor.word0()), 256);
    assert_ne!(descriptor.word0() & BIT_31, 0);

    descriptor.publish(completed, 0x2f00_3000, 0);
    assert!(rearm_descriptor(&descriptor, 0x2f00_3400, 0).is_err());
}

#[test]
fn receive_disable_confirms_the_ring_ownership_edge() {
    let mmio = MockMmio::default();
    mmio.set(RX_CONTROL, RX_ENABLE | 0x1234);
    disable_receive(&mmio).unwrap();
    assert_eq!(mmio.words.borrow().get(&RX_CONTROL), Some(&0x1234));
    assert_eq!(
        mmio.operations(),
        vec![
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, 0x1234),
            Operation::Fence,
            Operation::Read(RX_CONTROL),
        ]
    );
}

fn management_segment<'a>(storage: &'a mut [u8; 128]) -> RxSegment<'a> {
    const SIGNAL_LENGTH: usize = 34;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0xb0;
    storage[FRAME_OFFSET + 1] = 0;
    storage[FRAME_OFFSET + 22] = 0;

    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: storage,
        next_descriptor_address: 0,
    }
}

#[test]
fn management_rx_extracts_one_bounded_mpdu_and_strips_fcs() {
    let mut storage = [0_u8; 128];
    let segment = management_segment(&mut storage);
    let mut output = [0_u8; 64];
    let frame = extract_management(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 4,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.length, 30);
    assert_eq!(frame.signal_length, 34);
    assert_eq!(frame.dump_length, 38);
    assert!(frame.dump_length_matches);
    assert_eq!(output[0], 0xb0);
}

#[test]
fn management_rx_rejects_failed_hardware_status() {
    let mut storage = [0_u8; 128];
    let mut segment = management_segment(&mut storage);
    let mut failed = [0_u8; 128];
    failed.copy_from_slice(segment.buffer);
    failed[0x38 + 4] = 0xf5;
    segment.buffer = &failed;
    let mut output = [0_u8; 64];
    assert_eq!(
        extract_management(
            &[segment],
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            &mut output,
        ),
        Err(RxError::MicFailure)
    );
}

#[test]
fn irq_state_coalesces_known_bits_and_records_unknown_bits() {
    let mmio = MockMmio::default();
    mmio.set(MAC_INT_ENABLE, u32::MAX);
    mmio.set(
        MAC_INT_STATUS,
        MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS | 0x20,
    );
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mmio, &state);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert_eq!(snapshot.unhandled, 0x20);
    assert_eq!(state.observed_unhandled(), 0x20);
    let event = state.try_take().unwrap();
    assert_eq!(event.mac_pending, MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS);
    assert_eq!(mmio.operations().last(), Some(&Operation::Fence));
    assert!(mmio
        .operations()
        .contains(&Operation::Write(MAC_INT_CLEAR, snapshot.status)));
}

#[test]
fn tx_slot_rejects_stale_cookie_and_completes_one_generation() {
    let mut slot = TxSlot::new();
    let cookie = slot.reserve(0x2f00_5000, 0x2f00_6000, 512, 100).unwrap();
    assert_eq!(slot.state(), TxSlotState::Reserved);
    assert_eq!(slot.mark_hardware_owned(cookie), Ok(()));
    assert_eq!(slot.mark_hardware_owned(cookie), Err(TxError::Stale));

    let mmio = MockMmio::default();
    mmio.set(TX_COMPLETE_STATE, TX_COMPLETE_Q0);
    mmio.set(TX_COMPLETE_AUX_A_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_B_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_C_Q0, 0);
    mmio.set(TX_COMPLETE_PRIMARY_Q0, 3 << 12);
    mmio.set(TX_COMPLETE_ALTERNATE_Q0, 7 << 12);
    mmio.set(TX_STATE, 1 << 24);
    mmio.set(TX_COMPLETE_CLEAR, 0x100);

    let completion = slot.acknowledge_q0_completion(&mmio).unwrap().unwrap();
    assert_eq!(completion.cookie, cookie);
    assert_eq!(completion.status, 3);
    assert!(completion.trigger_flow);
    assert!(!completion.used_alternate);
    assert_eq!(slot.state(), TxSlotState::Completed);

    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID | 0x100);
    slot.detach_completed(&mmio, cookie).unwrap();
    assert_eq!(slot.state(), TxSlotState::Free);
}

#[test]
fn legacy_q0_image_reproduces_the_recovered_management_profile() {
    let image = legacy_q0_image(0x2f00_5000, LegacyTxConfig::management_1m(0x40)).unwrap();
    assert_eq!(image.plcp0, 0x0060_5000);
    assert_eq!(image.plcp1, 0x0000_0040);
    assert_eq!(image.power, 0x0808_0008);
    assert_eq!(image.length_control, 0x0040_0004);
}
