use core::{
    future::ready,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_sync::channel::TryReceiveError;
use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    connected_rx::{ConnectedRxConfig, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink},
    descriptor::{BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT},
    rx::{PUBLIC_HEADER_SIZE, RxDmaBinding, RxIngressConfig, RxRingStopped},
};

use super::*;
use crate::{
    connected_rx_protocol::{Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue},
    embassy_irq::EmbassyMacIrqRuntime,
    rx_reorder::{RxReorderCommand, RxReorderCommandResources, try_send_rx_reorder_command},
};

const BASE: u32 = 0x2f00_1000;

#[derive(Default)]
struct RecordingRxObserver {
    stage_too_long_discards: AtomicU32,
}

impl RxPipelineObserver for RecordingRxObserver {
    fn now_micros(&self) -> u64 {
        0
    }

    fn observe(&self, observation: RxPipelineObservation) {
        if matches!(
            observation,
            RxPipelineObservation::StageDiscarded(RxStageDiscard::TooLong)
        ) {
            self.stage_too_long_discards.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct MockRxDma {
    walker: bool,
    descriptor_base: u32,
    fail_enable: bool,
}

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        0
    }
    fn next_descriptor_low(&mut self) -> u32 {
        BASE + DESCRIPTOR_BYTES
    }
    fn walker_enabled(&mut self) -> bool {
        self.walker
    }
    fn reload_pending(&mut self) -> bool {
        false
    }
    fn set_descriptor_high_window(&mut self, _: &RxDmaBinding, _address_high: u16) {}
    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.descriptor_base = address;
    }
    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.walker = true;
    }
    fn request_reload(&mut self, _: &RxDmaBinding) {}
    fn try_enable_walker(&mut self, _: &RxDmaBinding) -> bool {
        if self.fail_enable {
            return false;
        }
        self.walker = true;
        true
    }
    fn try_disable_walker(&mut self) -> bool {
        self.walker = false;
        true
    }
    fn fence(&mut self) {}
}

struct NoDelay;

impl RxReloadDelay for NoDelay {
    fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

#[derive(Default)]
struct Observer(u32);

impl ConnectedRxSink for Observer {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
        self.0 += 1;
    }
}

#[derive(Default)]
struct OrderObserver(std::vec::Vec<u16>);

impl ConnectedRxSink for OrderObserver {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { raw, .. } = event {
            let sequence_control =
                u16::from_le_bytes([raw[PUBLIC_HEADER_SIZE + 22], raw[PUBLIC_HEADER_SIZE + 23]]);
            self.0.push(sequence_control >> 4);
        }
    }
}

fn dispatcher() -> ConnectedRxDispatcher {
    ConnectedRxDispatcher::new(ConnectedRxConfig {
        station_address: [2, 3, 4, 5, 6, 7],
        bssid: [8, 9, 10, 11, 12, 13],
        association_id: 1,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    })
}

#[test]
fn finite_service_uses_queue_credits_and_protocol_dispatch_returns_ownership() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    let pool = RxStagePool::new();
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0; ESP32S31_RX_BUFFER_SIZE];
    let mut ethernet = [0; ESP32S31_RX_BUFFER_SIZE];
    let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);
    let mut protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher(),
        crate::connected_rx_protocol::AlwaysReadyConnectedRxSink(Observer::default()),
        &mut mpdu,
        &mut ethernet,
    );

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Backpressured),
    );
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(pool.network_slots(), 1);
    assert_eq!(protocol.queue_len(), 1);
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(pool.network_slots(), 0);
    assert_eq!(service.ring().recycle_start(), 1);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Drained),
    );
    assert_eq!(service.ring().recycle_start(), 0);
    assert_eq!(protocol.queue_len(), 1);
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(pool.network_slots(), 0);
}

#[test]
fn connected_rx_stop_confirms_walker_off_and_preserves_static_resources() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    let pool = RxStagePool::<STAGED_DEPTH, ESP32S31_RX_BUFFER_SIZE>::new();
    let queue = Esp32s31StagedRxQueue::<
        NoopRawMutex,
        STAGED_DEPTH,
        ESP32S31_RX_BUFFER_SIZE,
        STAGED_DEPTH,
    >::new();
    let (sender, _receiver) = queue.split();
    let service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);
    assert!(hardware.walker);

    let stopped = match service.try_stop(&mut hardware) {
        Ok(stopped) => stopped,
        Err(_) => panic!("mock walker must confirm the stop edge"),
    };

    assert!(!hardware.walker);
    assert_eq!(stopped.ring().descriptor_base(), BASE);
    assert_eq!(stopped.ring().buffer_addresses(), &addresses);
    assert_eq!(stopped.queued_frames(), 0);

    let (ring, epoch_resources) = stopped.into_epoch_parts();
    assert_eq!(ring.descriptor_base(), BASE);
    assert_eq!(epoch_resources.queued_frames(), 0);
    let stopped = epoch_resources.with_halted_ring(ring);

    let prepared = match stopped.prepare(&mut hardware) {
        Ok(prepared) => prepared,
        Err(_) => panic!("halted owner must rebuild the next descriptor epoch"),
    };
    assert!(!hardware.walker);
    assert_eq!(prepared.ring().initial_start(), 0);
    hardware.fail_enable = true;
    let prepared = match embassy_futures::block_on(prepared.start(&mut hardware)) {
        Ok(_) => panic!("rejected walker enable must not create a live owner"),
        Err((prepared, error)) => {
            assert_eq!(error, RxRingError::Busy);
            prepared
        }
    };
    assert!(!hardware.walker);
    assert_eq!(prepared.ring().initial_start(), 0);
    assert_eq!(prepared.queued_frames(), 0);
    hardware.fail_enable = false;
    let restarted = match embassy_futures::block_on(prepared.start(&mut hardware)) {
        Ok(restarted) => restarted,
        Err(_) => panic!("prepared owner must reopen the mock walker"),
    };
    assert!(hardware.walker);
    assert_eq!(restarted.ring().descriptor_base(), BASE);
    assert_eq!(restarted.queued_frames(), 0);

    let stopped = match restarted.try_stop(&mut hardware) {
        Ok(stopped) => stopped,
        Err(_) => panic!("restarted owner must stop before the split test"),
    };
    let (ring, epoch_resources) = stopped.into_epoch_parts();
    let prepared = match storage.prepare_halted(ring, &mut hardware) {
        Ok(prepared) => prepared,
        Err(_) => panic!("split halted ring must rebuild"),
    };
    let ring = prepared.start(&mut hardware).unwrap();
    let restarted = epoch_resources.with_live_ring(ring);
    assert!(hardware.walker);
    assert_eq!(restarted.ring().descriptor_base(), BASE);
    assert_eq!(restarted.queued_frames(), 0);
}

#[test]
fn finite_service_stages_a_descriptor_chain_as_one_contiguous_unit() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    const STAGE_CAPACITY: usize = 16;
    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
    storage.buffer_mut(0).unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    storage.buffer_mut(1).unwrap()[..4].copy_from_slice(&[5, 6, 7, 8]);
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Drained),
    );
    let frame = receiver.try_receive().expect("one chained staged unit");
    assert_eq!(frame.segment().buffer, &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(service.ring().recycle_start(), 0);
    drop(frame);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn negotiated_rx_block_ack_releases_staged_leases_in_sequence_order() {
    const COUNT: usize = 4;
    const STAGED_DEPTH: usize = 3;
    const STAGE_CAPACITY: usize = 192;
    const MPDU: usize = 26 + 8 + 8 + 4 + 8;
    const SIGNAL: usize = MPDU + 4;
    const RECEIVED: usize = PUBLIC_HEADER_SIZE + SIGNAL;

    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let addresses = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut hardware = MockRxDma::default();

    let mut buffer = [0_u8; ESP32S31_RX_BUFFER_SIZE];
    for (index, sequence) in [102_u16, 100, 101].into_iter().enumerate() {
        buffer.fill(0);
        buffer[0x38..0x3c]
            .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
        let frame = &mut buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
        frame[..2].copy_from_slice(&0x4288_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
        frame[10..16].copy_from_slice(&[8, 9, 10, 11, 12, 13]);
        frame[16..22].copy_from_slice(&[14, 15, 16, 17, 18, 19]);
        frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
        frame[24] = 0;
        frame[26..34].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        frame[34..42].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame[42..46].copy_from_slice(&sequence.to_be_bytes().repeat(2));
        storage
            .buffer_mut(index)
            .expect("test RX buffer exists")
            .copy_from_slice(&buffer);
    }

    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    for index in 0..3 {
        storage.descriptors()[index].write_word0(
            STAGE_CAPACITY as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        );
    }

    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    // Declared before the queue because protocol frame types borrow both
    // pools and Rust drops local owners in reverse declaration order.
    let reorder_storage = crate::rx_reorder::RxReorderFrameStorage::<STAGE_CAPACITY>::new();
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_resources.split();
    try_send_rx_reorder_command(
        &reorder_sender,
        RxReorderCommand::Start {
            tid: 0,
            starting_sequence: 100,
            window: 8,
        },
    )
    .unwrap();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0; STAGE_CAPACITY];
    let mut ethernet = [0; STAGE_CAPACITY];
    let mut reorder_scratch = [0; STAGE_CAPACITY];
    let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);
    let mut protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher(),
        crate::connected_rx_protocol::AlwaysReadyConnectedRxSink(OrderObserver::default()),
        &mut mpdu,
        &mut ethernet,
    )
    .with_rx_reorder_commands(reorder_receiver)
    .with_rx_reorder_storage(&reorder_storage)
    .with_rx_reorder_scratch(&mut reorder_scratch);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Drained),
    );
    assert_eq!(pool.claimed_slots(), 3);
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(protocol.sink().0.0, [100]);
    // Sequence 102 is retained in the cold reorder backing while 100 has
    // been dispatched. The former implementation retained both staging
    // leases here and therefore reported two claimed SRAM slots.
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(
        reorder_storage.available_slots(),
        crate::rx_reorder::RX_REORDER_BACKING_SLOT_COUNT - 1
    );
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(protocol.sink().0.0, [100, 101, 102]);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        reorder_storage.available_slots(),
        crate::rx_reorder::RX_REORDER_BACKING_SLOT_COUNT
    );
}

#[test]
fn finite_service_discards_oversize_unit_and_keeps_the_ring_live() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    storage.descriptors()[0].write_word0(
        ESP32S31_RX_BUFFER_SIZE as u32
            | ((VENDOR_LARGE_RX_PAYLOAD_CAPACITY as u32 + 1) << LENGTH_SHIFT)
            | BIT_30
            | BIT_31,
    );
    let pool = RxStagePool::new();
    let observer = RecordingRxObserver::default();
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Drained),
    );
    assert_eq!(service.ring().recycle_start(), 1);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);
    assert_eq!(pool.claimed_slots(), 0);
    assert!(matches!(
        receiver.try_receive(),
        Err(TryReceiveError::Empty)
    ));
    assert_eq!(observer.stage_too_long_discards.load(Ordering::Relaxed), 1);

    // The recovered discard path is not a reset frontier: the following
    // descriptor is still accepted, staged and returned to the caller.
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Drained),
    );
    let next = receiver.try_receive().expect("post-discard frame");
    assert_eq!(next.length(), 4);
    drop(next);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn finite_service_accepts_a_unit_within_a_wider_negotiated_stage() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    const WIDE_STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY + 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    storage.descriptors()[0].write_word0(
        ESP32S31_RX_BUFFER_SIZE as u32
            | ((WIDE_STAGE_CAPACITY as u32) << LENGTH_SHIFT)
            | BIT_30
            | BIT_31,
    );
    let pool = RxStagePool::<VENDOR_LARGE_RX_SLOT_COUNT, WIDE_STAGE_CAPACITY>::new();
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, WIDE_STAGE_CAPACITY>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31ConnectedRx::new(ring, &storage, &pool, NoDelay, sender);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(WifiRxProgress::Drained),
    );
    let frame = receiver.try_receive().expect("wide staged frame");
    assert_eq!(frame.length(), WIDE_STAGE_CAPACITY);
    drop(frame);
    assert_eq!(pool.claimed_slots(), 0);
}
