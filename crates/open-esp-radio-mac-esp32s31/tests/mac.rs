use std::collections::BTreeMap;

use open_esp_radio_mac_esp32s31::{
    crypto::{install_sta_group_ccmp, install_sta_pairwise_ccmp, CryptoKeyError},
    descriptor::{
        descriptor_address_valid, dma_range_valid, length, rx_armed_word, rx_rearm_word, size,
        tx_owned_word, Descriptor, BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT,
    },
    init::{configure_sta_link_receive_policy, initialize_promiscuous_receive, MacClockControl},
    irq::{handle_mac_irq, IrqDisposition, IrqState},
    registers::{
        Mmio, MAC_INT_CLEAR, MAC_INT_ENABLE, MAC_INT_RX_SUCCESS, MAC_INT_STATUS,
        MAC_INT_TX_COMPLETE, RX_CONTROL, RX_DESCRIPTOR_BASE, RX_ENABLE, RX_LAST_DESCRIPTOR,
        RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR, RX_RELOAD, TX_CCA_CONTROL, TX_CCA_FORCE_MASK,
        TX_COMPLETE_ALTERNATE_Q0, TX_COMPLETE_AUX_A_Q0, TX_COMPLETE_AUX_B_Q0, TX_COMPLETE_AUX_C_Q0,
        TX_COMPLETE_CLEAR, TX_COMPLETE_PRIMARY_Q0, TX_COMPLETE_Q0, TX_COMPLETE_STATE,
        TX_Q0_CONTROL, TX_Q_ENABLE_VALID, TX_STATE, TX_STATE_CLEAR, TX_TIMEOUT_SHIFT,
    },
    rx::{
        build_cold_ring, disable_receive, enable_receive, extract_ccmp_data, extract_data,
        extract_management, first_segment_layout, prepare_recycled_buffer, publish_cold_ring,
        rearm_descriptor, RxError, RxIngressConfig, RxRingError, RxRingStopped, RxSegment,
        INGRESS_STRICT_DUMP, INGRESS_STRICT_RXEND, RX_BUFFER_SENTINEL,
    },
    tx::{legacy_q0_image, LegacyTxConfig, TxError, TxSlot, TxSlotState},
};
use open_esp_radio_pac_esp32s31::{
    mac::{self, init as mac_init},
    Register32,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Read(Register32),
    Write(Register32, u32),
    Fence,
}

#[derive(Default)]
struct MockMmio {
    words: BTreeMap<Register32, u32>,
    operations: Vec<Operation>,
}

impl MockMmio {
    fn set(&mut self, register: Register32, value: u32) {
        self.words.insert(register, value);
    }

    fn operations(&self) -> &[Operation] {
        &self.operations
    }
}

impl Mmio for MockMmio {
    fn read32(&mut self, register: Register32) -> u32 {
        self.operations.push(Operation::Read(register));
        self.words.get(&register).copied().unwrap_or(0)
    }

    fn write32(&mut self, register: Register32, value: u32) {
        self.operations.push(Operation::Write(register, value));
        self.words.insert(register, value);
    }

    fn fence(&mut self) {
        self.operations.push(Operation::Fence);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlatformOperation {
    EnableWifiMacClocks,
    EnableCoexistenceClock,
    ConfigureModemSourceClocks,
    SetWifiMacReset(bool),
}

#[derive(Default)]
struct MockPlatform {
    operations: Vec<PlatformOperation>,
}

impl MacClockControl for MockPlatform {
    fn enable_wifi_mac_clocks(&mut self) {
        self.operations.push(PlatformOperation::EnableWifiMacClocks);
    }

    fn enable_coexistence_clock(&mut self) {
        self.operations
            .push(PlatformOperation::EnableCoexistenceClock);
    }

    fn configure_modem_source_clocks(&mut self) {
        self.operations
            .push(PlatformOperation::ConfigureModemSourceClocks);
    }

    fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.operations
            .push(PlatformOperation::SetWifiMacReset(asserted));
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
fn cold_mac_init_uses_only_pac_registers_and_publishes_both_interfaces() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio::default();
    mmio.set(mac_init::HANDSHAKE, 1);

    let station = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    let access_point = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
    let outcome =
        initialize_promiscuous_receive(&mut platform, &mut mmio, 4, station, access_point).unwrap();

    assert_eq!(outcome.handshake_samples, 0);
    assert_eq!(outcome.handshake_value, 3);
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_LOW[0]),
        Some(&0x3322_1102)
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_HIGH[0]),
        Some(&0x0001_5544)
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_LOW[1]),
        Some(&0xccbb_aa02)
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_HIGH[1]),
        Some(&0x0001_eedd)
    );
    assert_eq!(mmio.words.get(&mac_init::CONTROL), Some(&0));
    assert_eq!(mmio.words.get(&mac::INT_ENABLE), Some(&0x19a8_79e0));
    assert_eq!(mmio.words.get(&mac_init::R_4C60), Some(&0xffff_0000));
    assert_eq!(mmio.words.get(&mac_init::R_4400), Some(&0x0002_0350));
    assert_eq!(mmio.words.get(&mac_init::R_4404), Some(&0x8080_8080));
    assert_eq!(mmio.words.get(&mac_init::R_4C7C), Some(&0x0000_0400));
    assert_eq!(mmio.words.get(&mac_init::R_4E04), Some(&0));
    assert_eq!(
        platform.operations,
        [
            PlatformOperation::EnableWifiMacClocks,
            PlatformOperation::EnableCoexistenceClock,
            PlatformOperation::ConfigureModemSourceClocks,
            PlatformOperation::SetWifiMacReset(true),
            PlatformOperation::SetWifiMacReset(false),
        ]
    );
    assert_eq!(mmio.operations().last(), Some(&Operation::Fence));
}

#[test]
fn sta_link_rx_policy_matches_migration_policy_five() {
    let mut mmio = MockMmio::default();
    mmio.set(mac_init::RX_FILTER[0], u32::MAX);
    mmio.set(mac_init::BSSID_HIGH[0], u32::MAX);
    mmio.set(mac_init::INTERFACE_ADDRESS_HIGH[0], 0x0000_5544);

    configure_sta_link_receive_policy(&mut mmio);

    assert_eq!(
        mmio.words.get(&mac_init::RX_FILTER[0]),
        Some(&(u32::MAX & !((1 << 10) | (1 << 8) | (1 << 6) | (1 << 4) | (1 << 1))))
    );
    assert_eq!(mmio.words.get(&mac_init::BSSID_HIGH[0]), Some(&0xbfff_ffff));
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_HIGH[0]),
        Some(&0x0001_5544)
    );
    assert_eq!(
        mmio.operations()
            .iter()
            .filter(|operation| **operation == Operation::Fence)
            .count(),
        1
    );
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

    let mut mmio = MockMmio::default();
    mmio.set(RX_LAST_DESCRIPTOR_HIGH, 0x0005_4321);
    mmio.set(RX_CONTROL, 0x1234);
    publish_cold_ring(&mut mmio, 0x2f00_1000, true).unwrap();

    assert_eq!(
        mmio.operations(),
        &[
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
fn recycled_rx_buffer_restores_both_migration_sentinels() {
    let mut storage = [0x5a; 20];
    prepare_recycled_buffer(&mut storage, 16).unwrap();
    assert_eq!(&storage[..4], &RX_BUFFER_SENTINEL.to_le_bytes());
    assert_eq!(&storage[4..16], &[0x5a; 12]);
    assert_eq!(&storage[16..20], &RX_BUFFER_SENTINEL.to_le_bytes());
    assert_eq!(
        prepare_recycled_buffer(&mut storage[..16], 16),
        Err(RxRingError::Size)
    );
}

#[test]
fn live_rx_ring_owns_rotated_handoff_reload_and_rom_base_repair() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut prepared = Vec::new();
    let mut mmio = MockMmio::default();
    // The previous walker retained descriptor one, so the Rust owner must
    // rotate the cold list to begin at descriptor two.
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_CONTROL, RX_ENABLE);

    let stopped = RxRingStopped::prepare(
        &mut mmio,
        &descriptors,
        BASE,
        &buffers,
        BUFFER_SIZE,
        |index| {
            prepared.push(index);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(prepared, [0, 1, 2, 3]);
    assert_eq!(stopped.initial_start(), 2);
    assert_eq!(stopped.accepted_tail(), 1);
    assert_eq!(descriptors[2].next_address(), BASE + 3 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[1].next_address(), 0);
    assert_eq!(
        mmio.words.get(&RX_DESCRIPTOR_BASE),
        Some(&(BASE + 2 * DESCRIPTOR_BYTES))
    );

    let mut live = stopped.start(&mut mmio).unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptors[2].write_word0(completed);
    descriptors[3].write_word0(completed);
    assert_eq!(live.take_completed(2).unwrap().index, 2);
    assert_eq!(live.take_completed(2), None);
    assert_eq!(live.take_completed(3).unwrap().index, 3);

    let mut recycled = Vec::new();
    let first = live
        .recycle_completed_half(&mut mmio, |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [2, 3]);
    assert_eq!(first.head_index, 2);
    assert_eq!(first.tail_index, 3);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert_ne!(mmio.words[&RX_CONTROL] & RX_RELOAD, 0);
    assert!(live.reload_pending());
    assert_eq!(live.accepted_tail(), 1);

    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    assert!(live.take_completed(0).is_some());
    assert!(live.take_completed(1).is_some());

    // Model bit-0 self-clear at a terminal frontier. ROM repairs BASE from the
    // last accepted descriptor's now-published next link before accepting the
    // pending tail and appending the following group.
    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, 0);
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    recycled.clear();
    let second = live
        .recycle_completed_half(&mut mmio, |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [0, 1]);
    assert_eq!(second.head_index, 0);
    assert_eq!(second.tail_index, 1);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert!(mmio.operations().contains(&Operation::Write(
        RX_DESCRIPTOR_BASE,
        BASE + 2 * DESCRIPTOR_BYTES,
    )));
    assert_eq!(live.accepted_tail(), 3);
    assert!(live.reload_pending());
}

#[test]
fn receive_disable_confirms_the_ring_ownership_edge() {
    let mut mmio = MockMmio::default();
    mmio.set(RX_CONTROL, RX_ENABLE | 0x1234);
    disable_receive(&mut mmio).unwrap();
    assert_eq!(mmio.words.get(&RX_CONTROL), Some(&0x1234));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, 0x1234),
            Operation::Fence,
            Operation::Read(RX_CONTROL),
        ]
    );
}

#[test]
fn receive_enable_is_a_separate_confirmed_hardware_edge() {
    let mut mmio = MockMmio::default();
    mmio.set(RX_CONTROL, 0x1234);
    enable_receive(&mut mmio).unwrap();
    assert_eq!(mmio.words.get(&RX_CONTROL), Some(&(RX_ENABLE | 0x1234)));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, RX_ENABLE | 0x1234),
            Operation::Fence,
            Operation::Read(RX_CONTROL),
        ]
    );

    let mut already_enabled = MockMmio::default();
    already_enabled.set(RX_CONTROL, RX_ENABLE | 0x1234);
    assert_eq!(enable_receive(&mut already_enabled), Err(RxRingError::Busy));
    assert_eq!(already_enabled.operations(), &[Operation::Read(RX_CONTROL)]);
}

#[test]
fn sta_pairwise_ccmp_install_owns_one_bounded_hardware_slot() {
    let mut mmio = MockMmio::default();
    mmio.set(mac::CRYPTO_POLICY_CONTROL, u32::MAX);
    let peer = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let temporal_key = core::array::from_fn(|index| index as u8);

    let mut slot = install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 4);
    assert_eq!(slot.peer(), &peer);
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP),
        Some(&(1 << 4))
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 0).unwrap()),
        Some(&0x54c8_15dc)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 1).unwrap()),
        Some(&0x086c_1ebc)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 2).unwrap()),
        Some(&0x0302_0100)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 5).unwrap()),
        Some(&0x0f0e_0d0c)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 6).unwrap()),
        Some(&0)
    );
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_INTERFACE_CONTROL[0]),
        Some(&0x0003_0103)
    );
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_POLICY_CONTROL),
        Some(&0xffc0_003f)
    );
    assert_eq!(slot.next_tx_ccmp_header(), [3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(slot.next_tx_ccmp_header(), [6, 0, 0, 0x20, 0, 0, 0, 0]);

    slot.clear(&mut mmio);
    assert_eq!(mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP), Some(&0));
    for word in 0..mac::CRYPTO_KEY_ENTRY_WORDS {
        assert_eq!(
            mmio.words
                .get(&mac::crypto_key_entry_word(4, word).unwrap()),
            Some(&0)
        );
    }

    mmio.set(mac::CRYPTO_KEY_VALID_BITMAP, 1 << 4);
    assert_eq!(
        install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).err(),
        Some(CryptoKeyError::Occupied)
    );
}

#[test]
fn sta_group_ccmp_install_matches_the_migration_slot_and_control_word() {
    let mut mmio = MockMmio::default();
    mmio.set(mac::CRYPTO_POLICY_CONTROL, u32::MAX);
    let temporal_key = core::array::from_fn(|index| 0xf0 | index as u8);

    let slot = install_sta_group_ccmp(&mut mmio, 1, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 1);
    assert_eq!(slot.key_id(), 1);
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP),
        Some(&(1 << 1))
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 0).unwrap()),
        Some(&u32::MAX)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 1).unwrap()),
        Some(&0x48cc_ffff)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 2).unwrap()),
        Some(&0xf3f2_f1f0)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 5).unwrap()),
        Some(&0xfffe_fdfc)
    );

    slot.clear(&mut mmio);
    assert_eq!(mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP), Some(&0));
    assert_eq!(
        install_sta_group_ccmp(&mut mmio, 4, &temporal_key).err(),
        Some(CryptoKeyError::InvalidGroupKeyId)
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
fn data_rx_reports_qos_llc_payload_offset() {
    const SIGNAL_LENGTH: usize = 26 + 8 + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x02;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + 26..FRAME_OFFSET + 34]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 64];
    let frame = extract_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, SIGNAL_LENGTH - 4);
    assert_eq!(frame.payload_offset, 26);
    assert_eq!(
        &output[frame.payload_offset..frame.payload_offset + 8],
        &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]
    );
}

#[test]
fn ccmp_data_rx_reproduces_the_oracle_header_and_mic_adjustment() {
    const HEADER_LENGTH: usize = 26;
    const LLC_LENGTH: usize = 8;
    const PAYLOAD_LENGTH: usize = 4;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + PAYLOAD_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 16..FRAME_OFFSET + HEADER_LENGTH + 20]
        .copy_from_slice(&[1, 2, 3, 4]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 80];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, MPDU_LENGTH);
    assert_eq!(frame.ccmp_header_offset, HEADER_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + PAYLOAD_LENGTH);
    assert_eq!(frame.mic_offset, MPDU_LENGTH - 8);
    assert_eq!(frame.mic_bytes_in_dma, 8);
    assert!(frame.mic_present_in_dma);
    assert_eq!(
        &output[frame.payload_offset..frame.payload_offset + LLC_LENGTH],
        &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]
    );
}

#[test]
fn first_segment_layout_exposes_a_consumed_ccmp_mic_shortfall() {
    const MPDU_LENGTH: usize = 26 + 8 + 8 + 4 + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 8;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let layout = first_segment_layout(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
    )
    .unwrap();

    assert_eq!(layout.received_length, RECEIVED);
    assert_eq!(layout.expected_frame_length, MPDU_LENGTH);
    assert_eq!(layout.available_frame_bytes, DMA_FRAME_LENGTH);
    assert_eq!(layout.frame_shortfall, 8);
}

#[test]
fn ccmp_data_rx_accepts_a_hardware_consumed_mic() {
    const HEADER_LENGTH: usize = 26;
    const LLC_LENGTH: usize = 8;
    const PAYLOAD_LENGTH: usize = 4;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + PAYLOAD_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 8;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 16..FRAME_OFFSET + HEADER_LENGTH + 20]
        .copy_from_slice(&[1, 2, 3, 4]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 80];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, DMA_FRAME_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + PAYLOAD_LENGTH);
    assert_eq!(frame.mic_offset, DMA_FRAME_LENGTH);
    assert_eq!(frame.mic_bytes_in_dma, 0);
    assert!(!frame.mic_present_in_dma);
}

#[test]
fn ccmp_data_rx_accepts_a_dma_view_ending_inside_the_verified_mic() {
    const HEADER_LENGTH: usize = 24;
    const LLC_LENGTH: usize = 8;
    const ARP_AND_PADDING_LENGTH: usize = 46;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + ARP_AND_PADDING_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    // The external-LAN ARP HIL frame retained the first two MIC bytes.
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 6;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 192];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x08;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 192 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 128];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, DMA_FRAME_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + ARP_AND_PADDING_LENGTH);
    assert_eq!(frame.mic_offset, MPDU_LENGTH - 8);
    assert_eq!(frame.mic_bytes_in_dma, 2);
    assert!(!frame.mic_present_in_dma);
}

#[test]
fn ccmp_data_rx_rejects_missing_extiv_and_hardware_mic_failure() {
    const SIGNAL_LENGTH: usize = 26 + 8 + 8 + 8 + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    let config = RxIngressConfig {
        ring_entry_limit: 1,
        csi_config: 0,
        flags: 0,
    };
    let mut output = [0_u8; 80];
    {
        let segment = RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            buffer: &storage,
            next_descriptor_address: 0,
        };
        assert_eq!(
            extract_ccmp_data(&[segment], config, &mut output),
            Err(RxError::Unsupported)
        );
    }

    storage[FRAME_OFFSET + 26 + 3] = 0x20;
    storage[TAIL_OFFSET + 4] = 0xf5;
    let failed = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    assert_eq!(
        extract_ccmp_data(&[failed], config, &mut output),
        Err(RxError::MicFailure)
    );
}

#[test]
fn irq_state_coalesces_known_bits_and_records_unknown_bits() {
    let mut mmio = MockMmio::default();
    mmio.set(MAC_INT_ENABLE, u32::MAX);
    mmio.set(
        MAC_INT_STATUS,
        MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS | 0x20,
    );
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

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
    assert_eq!(size(slot.descriptor.word0()), 512);
    assert_eq!(length(slot.descriptor.word0()), 100);
    assert_eq!(slot.state(), TxSlotState::Reserved);
    assert_eq!(slot.mark_hardware_owned(cookie), Ok(()));
    assert_eq!(slot.mark_hardware_owned(cookie), Err(TxError::Stale));

    let mut mmio = MockMmio::default();
    mmio.set(TX_COMPLETE_STATE, TX_COMPLETE_Q0);
    mmio.set(TX_COMPLETE_AUX_A_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_B_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_C_Q0, 0);
    mmio.set(TX_COMPLETE_PRIMARY_Q0, 3 << 12);
    mmio.set(TX_COMPLETE_ALTERNATE_Q0, 7 << 12);
    mmio.set(TX_STATE, 1 << 24);
    mmio.set(TX_COMPLETE_CLEAR, 0x100);

    let completion = slot.acknowledge_q0_completion(&mut mmio).unwrap().unwrap();
    assert_eq!(completion.cookie, cookie);
    assert_eq!(completion.status, 3);
    assert!(completion.trigger_flow);
    assert!(!completion.used_alternate);
    assert_eq!(slot.state(), TxSlotState::Completed);

    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID | 0x100);
    slot.detach_completed(&mut mmio, cookie).unwrap();
    assert_eq!(slot.state(), TxSlotState::Free);
}

#[test]
fn tx_slot_reproduces_the_migration_timeout_abort_order() {
    let mut slot = TxSlot::new();
    let cookie = slot.reserve(0x2f00_5000, 0x2f00_6000, 512, 100).unwrap();
    slot.mark_hardware_owned(cookie).unwrap();

    let timeout_mask = 1 << TX_TIMEOUT_SHIFT;
    let mut mmio = MockMmio::default();
    mmio.set(TX_STATE, timeout_mask);
    mmio.set(TX_CCA_CONTROL, 0x1234_5678);
    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID | 0x100);

    assert_eq!(slot.begin_timeout_abort(&mut mmio, cookie), Ok(true));
    assert_eq!(
        mmio.words.get(&TX_CCA_CONTROL).copied().unwrap() & TX_CCA_FORCE_MASK,
        TX_CCA_FORCE_MASK,
    );
    slot.finish_timeout_abort(&mut mmio, cookie).unwrap();

    assert_eq!(slot.state(), TxSlotState::Free);
    assert_eq!(
        mmio.words.get(&TX_Q0_CONTROL).copied().unwrap() & TX_Q_ENABLE_VALID,
        0,
    );
    assert_eq!(
        mmio.words.get(&TX_CCA_CONTROL).copied().unwrap() & TX_CCA_FORCE_MASK,
        0,
    );
    assert!(mmio
        .operations()
        .contains(&Operation::Write(TX_STATE_CLEAR, timeout_mask)));

    let invalidation = mmio
        .operations()
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Write(register, value)
                if *register == TX_Q0_CONTROL && value & (1 << 30) == 0)
        })
        .unwrap();
    let cca_release = mmio
        .operations()
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Write(register, value)
                if *register == TX_CCA_CONTROL && value & TX_CCA_FORCE_MASK == 0)
        })
        .unwrap();
    let timeout_clear = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::Write(TX_STATE_CLEAR, timeout_mask))
        .unwrap();
    assert!(invalidation < cca_release);
    assert!(cca_release < timeout_clear);
}

#[test]
fn legacy_q0_image_reproduces_the_recovered_management_profile() {
    let image = legacy_q0_image(0x2f00_5000, LegacyTxConfig::management_1m(0x40)).unwrap();
    assert_eq!(image.plcp0, 0x0060_5000);
    assert_eq!(image.plcp1, 0x0000_0040);
    assert_eq!(image.power, 0x0808_0008);
    assert_eq!(image.length_control, 0x0040_0004);
    assert_eq!(LegacyTxConfig::management_1m(0x40).timeout, 100);
}

#[test]
fn management_profile_derives_plcp1_from_mpdu_plus_fcs() {
    let config = LegacyTxConfig::management_1m_from_mpdu_length(30).unwrap();
    assert_eq!(config.signal, 0x22);
    assert!(LegacyTxConfig::management_1m_from_mpdu_length(0x0ffc).is_none());
}

#[test]
fn protected_legacy_profile_publishes_sta_pairwise_slot_in_plcp1() {
    let mut config = LegacyTxConfig::management_1m(0x99);
    config.no_ack = false;
    config.hardware_key_selector = 4;
    let image = legacy_q0_image(0x2f00_0100, config).unwrap();
    assert_eq!(image.plcp1, 0x0008_0099);
    assert_eq!(image.length_control, 0x0040_0004);
}
