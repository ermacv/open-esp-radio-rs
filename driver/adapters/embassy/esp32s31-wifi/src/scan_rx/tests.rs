use super::*;
use crate::{connected_rx_protocol::Esp32s31StagedRxQueue, rx_dma_service::Esp32s31ConnectedRx};
use core::{
    future::{Future, ready},
    pin::pin,
    task::{Context, Poll},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::{rx::RxDmaBinding, rx_pool::RxStagePool};

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(core::task::Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

const RX_TEST_COUNT: usize = 2;
const RX_TEST_BUFFER_SIZE: usize = 128;
const RX_TEST_STORAGE_SIZE: usize = RX_TEST_BUFFER_SIZE + 4;
const RX_TEST_BASE: u32 = 0x2f00_1000;
const RX_TEST_BUFFERS: [u32; RX_TEST_COUNT] = [0x2f00_2000, 0x2f00_2080];

#[derive(Default)]
struct MockRxDma {
    walker: bool,
    fail_enable: bool,
    fail_disable: bool,
    descriptor_base: u32,
    reload_requests: u32,
}

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        0
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RX_TEST_BASE + 12
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

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reload_requests = self.reload_requests.saturating_add(1);
    }

    fn try_enable_walker(&mut self, _: &RxDmaBinding) -> bool {
        if self.fail_enable {
            false
        } else {
            self.walker = true;
            true
        }
    }

    fn try_disable_walker(&mut self) -> bool {
        if self.fail_disable {
            false
        } else {
            self.walker = false;
            true
        }
    }

    fn fence(&mut self) {}
}

#[derive(Default)]
struct FrameObserver {
    frames: u32,
}

impl Esp32s31ScanFrameObserver for FrameObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {
        self.frames = self.frames.saturating_add(1);
    }
}

fn write_test_beacon(
    storage: &mut Esp32s31RxDmaStorage<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
) {
    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;

    let mut bytes = [0_u8; RX_TEST_BUFFER_SIZE];
    bytes[0] = (-42_i8) as u8;
    bytes[0x38..0x3c].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    let frame = &mut bytes[FRAME_OFFSET..FRAME_OFFSET + FRAME_LENGTH];
    frame[0] = 0x80;
    frame[10..16].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[36..40].copy_from_slice(&[0, 2, b'a', b'p']);
    frame[40..43].copy_from_slice(&[3, 1, 6]);
    storage
        .buffer_mut(0)
        .expect("test RX buffer exists")
        .copy_from_slice(&bytes);
}

fn complete_test_beacon(
    storage: &Esp32s31RxDmaStorage<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
) {
    use open_esp_radio_esp32s31_wifi_mac::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;
    const RECEIVED_LENGTH: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    storage.descriptors()[0].write_word0(
        RX_TEST_BUFFER_SIZE as u32 | (RECEIVED_LENGTH as u32) << LENGTH_SHIFT | BIT_30 | BIT_31,
    );
}

#[test]
fn scan_rx_hands_the_exact_halted_ring_to_the_next_phase() {
    let mut storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut rx =
        Esp32s31ScanRx::prepare_initial(&mut hardware, &storage, RX_TEST_BASE, &RX_TEST_BUFFERS)
            .unwrap();
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);

    rx.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let mut table = ScanTable::<4>::new();
    let mut frame = [0_u8; 64];
    let mut observer = FrameObserver::default();
    let mut context = Esp32s31ScanObservationContext::new(6, &mut frame, &mut table, &mut observer);
    let progress = rx.observe_management(&mut hardware, &mut context).unwrap();

    assert_eq!(progress.completed_descriptors, 1);
    assert_eq!(progress.parsed_management_frames, 1);
    assert_eq!(progress.inserted_records, 1);
    assert_eq!(progress.recycled_descriptors, 1);
    assert_eq!(observer.frames, 1);
    assert_eq!(table.records()[0].ssid_bytes(), b"ap");
    assert_eq!(table.records()[0].channel, 6);

    rx.stop(&mut hardware).unwrap();
    let halted = match rx.into_halted() {
        Ok(halted) => halted,
        Err(_) => panic!("completed scan must expose its halted owner"),
    };
    assert_eq!(halted.descriptor_base(), RX_TEST_BASE);
    assert_eq!(halted.buffer_addresses(), &RX_TEST_BUFFERS);
}

#[test]
fn complete_cold_scan_can_prepare_the_same_ring_for_a_retry() {
    let storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    let mut hardware = MockRxDma::default();
    let mut rx =
        Esp32s31ScanRx::prepare_initial(&mut hardware, &storage, RX_TEST_BASE, &RX_TEST_BUFFERS)
            .unwrap();

    rx.prepare_initial_or_retry(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);
    rx.start(&mut hardware).unwrap();
    rx.stop(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Halted);

    rx.prepare_initial_or_retry(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);
}

#[test]
fn scan_rx_retains_its_typed_phase_across_enable_and_disable_failure() {
    let storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    let mut hardware = MockRxDma::default();
    let mut rx =
        Esp32s31ScanRx::prepare_initial(&mut hardware, &storage, RX_TEST_BASE, &RX_TEST_BUFFERS)
            .unwrap();

    hardware.fail_enable = true;
    assert_eq!(
        rx.start(&mut hardware),
        Err(Esp32s31ScanRxError::Ring(RxRingError::Busy))
    );
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Prepared);

    hardware.fail_enable = false;
    rx.start(&mut hardware).unwrap();
    hardware.fail_disable = true;
    assert_eq!(
        rx.stop(&mut hardware),
        Err(Esp32s31ScanRxError::Ring(RxRingError::Busy))
    );
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Live);

    hardware.fail_disable = false;
    rx.stop(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31ScanRxPhase::Halted);
}

#[test]
fn running_scan_rx_returns_the_exact_connected_epoch_resources() {
    const STAGE_SLOTS: usize = 1;
    const STAGE_CAPACITY: usize = 64;
    struct TestDelay;

    impl RxReloadDelay for TestDelay {
        fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    let storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
        RX_TEST_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped.start(&mut hardware).unwrap();
    let pool = RxStagePool::<STAGE_SLOTS, STAGE_CAPACITY>::new();
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGE_SLOTS, STAGE_CAPACITY, STAGE_SLOTS>::new();
    let (sender, _receiver) = queue.split();
    let connected = Esp32s31ConnectedRx::new(ring, &storage, &pool, TestDelay, sender);
    let stopped = connected
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("mock connected ring must stop"));
    let pool_address = stopped.pool() as *const _;

    let mut running = Esp32s31RunningScanRx::from_stopped(stopped);
    assert_eq!(running.phase(), Esp32s31ScanRxPhase::Halted);
    running.prepare_initial(&mut hardware).unwrap();
    assert_eq!(running.phase(), Esp32s31ScanRxPhase::Prepared);

    let stopped = running
        .into_stopped()
        .unwrap_or_else(|_| panic!("prepared running scan must discard its unstarted epoch"));
    assert_eq!(stopped.pool() as *const _, pool_address);
    assert_eq!(stopped.ring().descriptor_base(), RX_TEST_BASE);

    let mut running = Esp32s31RunningScanRx::from_stopped(stopped);
    running.prepare_initial(&mut hardware).unwrap();
    block_on(running.start(&mut hardware)).unwrap();
    running.stop(&mut hardware).unwrap();

    let stopped = running
        .into_stopped()
        .unwrap_or_else(|_| panic!("halted running scan must restore connected resources"));
    assert_eq!(stopped.pool() as *const _, pool_address);
    assert_eq!(stopped.ring().descriptor_base(), RX_TEST_BASE);
    assert_eq!(stopped.ring().buffer_addresses(), &RX_TEST_BUFFERS);
    assert_eq!(stopped.buffers().as_ptr(), storage.buffers().as_ptr());
    assert_eq!(stopped.queued_frames(), 0);
}
