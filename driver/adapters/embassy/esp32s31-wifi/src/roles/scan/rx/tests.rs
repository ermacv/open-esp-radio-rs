use super::*;
use crate::{
    datapath::rx::{dma::Esp32s31StagedRxProducer, staging::Esp32s31StagedRxQueue},
    roles::monitor::rx::Esp32s31MonitorRx,
};
use core::{
    future::{Future, ready},
    pin::pin,
    task::{Context, Poll},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDmaBinding, RxDmaWalkerStopped, RxRingStopped},
    rx_pool::RxStagePool,
};
use open_esp_radio_wifi_embassy::{MonitorCapturePool, MonitorCaptureResources};
use open_esp_radio_wifi_softmac::{
    MonitorDropReason, MonitorFrame, MonitorPublishOutcome, MonitorSink, WifiConfig,
    WifiMonitorConfig,
    interface::{ChannelContextId, MonitorTapPoint},
};
use std::boxed::Box;

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
        if self.walker {
            RX_TEST_BASE & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor_low(&mut self) -> u32 {
        if self.walker {
            (RX_TEST_BASE + 12) & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor(&mut self) -> open_esp_radio_esp32s31_wifi_dma::rx_dma::RxDmaNextDescriptor {
        open_esp_radio_esp32s31_wifi_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low(),
            false,
        )
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaCursorObservation::validation(last, next),
        )
    }

    fn walker_enabled(&mut self) -> bool {
        self.walker
    }

    fn reload_pending(&mut self) -> bool {
        false
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending()).then(|| {
            settled(open_esp_radio_esp32s31_wifi_mac::rx::RxDmaReloadSettled::validation())
        })
    }

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding) {}

    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.descriptor_base = address;
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.walker = true;
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reload_requests = self.reload_requests.saturating_add(1);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        if self.fail_enable {
            None
        } else {
            self.walker = true;
            Some(enabled(
                open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled::validation(),
            ))
        }
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        if self.fail_disable {
            None
        } else {
            self.walker = false;
            Some(stopped(RxDmaWalkerStopped::validation()))
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

#[derive(Default)]
struct MonitorObserver {
    frames: u32,
    first_byte: Option<u8>,
    drop_all: bool,
}

impl MonitorSink<open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo> for MonitorObserver {
    fn try_publish(
        &mut self,
        frame: MonitorFrame<'_, open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo>,
    ) -> MonitorPublishOutcome {
        assert_eq!(frame.tap, MonitorTapPoint::Normalized);
        assert_eq!(frame.channel_context, ChannelContextId::PRIMARY);
        self.frames = self.frames.saturating_add(1);
        self.first_byte = frame.bytes.first().copied();
        if self.drop_all {
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
        } else {
            MonitorPublishOutcome::Published
        }
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
    use open_esp_radio_esp32s31_wifi_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;
    const RECEIVED_LENGTH: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    storage.descriptors()[0].write_word0(
        RX_TEST_BUFFER_SIZE as u32 | (RECEIVED_LENGTH as u32) << LENGTH_SHIFT | BIT_30 | BIT_31,
    );
}

fn monitor_plan() -> open_esp_radio_wifi_softmac::WifiStandaloneMonitorPlan {
    WifiConfig::monitor(WifiMonitorConfig::normalized())
        .validate(open_esp_radio_esp32s31_wifi_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
        .unwrap()
        .standalone_monitor()
        .unwrap()
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
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Prepared);

    rx.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let mut table = ScanTable::<4>::new();
    let mut frame = [0_u8; 64];
    let mut observer = FrameObserver::default();
    let mut context = Esp32s31ScanObservationContext::new(6, &mut frame, &mut table, &mut observer);
    let progress = rx.observe_management(&mut hardware, &mut context).unwrap();
    let release = rx.observe_management(&mut hardware, &mut context).unwrap();

    assert_eq!(progress.completed_descriptors, 1);
    assert_eq!(progress.parsed_management_frames, 1);
    assert_eq!(progress.inserted_records, 1);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.recycled_descriptors, 0);
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
fn normalized_monitor_borrows_the_mpdu_and_retains_current_last_until_stop() {
    let mut storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = Esp32s31MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);

    let mut sink = MonitorObserver::default();
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    let release = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.completed_descriptors, 1);
    assert_eq!(progress.published_frames, 1);
    assert_eq!(progress.dropped_frames, 0);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.published_frames, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(sink.frames, 1);
    assert_eq!(sink.first_byte, Some(0x80));
    monitor.stop(&mut hardware).unwrap();
}

#[test]
fn halted_scan_ring_is_prepared_before_monitor_restarts_the_walker() {
    let mut storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut scan =
        Esp32s31ScanRx::prepare_initial(&mut hardware, &storage, RX_TEST_BASE, &RX_TEST_BUFFERS)
            .unwrap();
    scan.start(&mut hardware).unwrap();
    scan.stop(&mut hardware).unwrap();
    let halted = scan
        .into_halted()
        .unwrap_or_else(|_| panic!("stopped scan must return its halted ring"));

    let mut monitor =
        Esp32s31MonitorRx::prepare_halted(monitor_plan(), halted, &mut hardware, &storage)
            .unwrap_or_else(|_| panic!("monitor must accept the exact stopped scan ring"));
    assert_eq!(monitor.phase(), Esp32s31RxFrontierPhase::Prepared);
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);

    let mut sink = MonitorObserver::default();
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.published_frames, 1);
    assert_eq!(sink.frames, 1);
    monitor.stop(&mut hardware).unwrap();
}

#[test]
fn full_monitor_sink_drops_observation_without_backpressuring_the_ring() {
    let mut storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = Esp32s31MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);

    let mut sink = MonitorObserver {
        drop_all: true,
        ..MonitorObserver::default()
    };
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    let release = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.published_frames, 0);
    assert_eq!(progress.dropped_frames, 1);
    assert_eq!(progress.full_drops, 1);
    assert_eq!(progress.oversized_drops, 0);
    assert_eq!(progress.filtered_drops, 0);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.dropped_frames, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(hardware.reload_requests, 0);
    monitor.stop(&mut hardware).unwrap();
}

#[test]
fn monitor_capture_owns_its_copy_while_current_last_remains_dma_visible() {
    let mut storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = Esp32s31MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    let pool = MonitorCapturePool::<64, 2>::new();
    let resources = MonitorCaptureResources::<
        NoopRawMutex,
        open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo,
        2,
        64,
        2,
    >::new(&pool);
    let (mut sink, receiver) = resources.split();

    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    let release = monitor.service(&mut hardware, &mut sink).unwrap();

    assert_eq!(progress.published_frames, 1);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.published_frames, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(hardware.reload_requests, 0);
    assert_eq!(pool.claimed_slots(), 1);
    let captured = receiver.try_receive().expect("async capture owns its copy");
    assert_eq!(captured.bytes().first(), Some(&0x80));
    assert_eq!(
        captured.metadata().channel_context,
        ChannelContextId::PRIMARY
    );
    drop(captured);
    assert_eq!(pool.claimed_slots(), 0);
    monitor.stop(&mut hardware).unwrap();
}

#[test]
fn monitor_only_wifi_plan_materializes_the_promiscuous_ring_owner() {
    let mut storage =
        Esp32s31RxDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = Esp32s31MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    assert_eq!(monitor.channel_context(), ChannelContextId::PRIMARY);
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let mut sink = MonitorObserver::default();
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.published_frames, 1);
    monitor.stop(&mut hardware).unwrap();
    assert!(monitor.into_halted().is_ok());
}

#[test]
fn complete_initial_scan_can_prepare_the_same_ring_for_a_retry() {
    let storage = Box::leak(Box::new(Esp32s31RxDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
    let mut hardware = MockRxDma::default();
    let mut rx =
        Esp32s31ScanRx::prepare_initial(&mut hardware, storage, RX_TEST_BASE, &RX_TEST_BUFFERS)
            .unwrap();

    rx.prepare_initial_or_retry(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Prepared);
    rx.start(&mut hardware).unwrap();
    rx.stop(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Halted);

    rx.prepare_initial_or_retry(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Prepared);
}

#[test]
fn scan_rx_retains_its_typed_phase_across_enable_and_disable_failure() {
    let storage = Box::leak(Box::new(Esp32s31RxDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
    let mut hardware = MockRxDma::default();
    let mut rx =
        Esp32s31ScanRx::prepare_initial(&mut hardware, storage, RX_TEST_BASE, &RX_TEST_BUFFERS)
            .unwrap();

    hardware.fail_enable = true;
    assert_eq!(
        rx.start(&mut hardware),
        Err(Esp32s31RxFrontierError::Ring(RxRingError::Busy))
    );
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Prepared);

    hardware.fail_enable = false;
    rx.start(&mut hardware).unwrap();
    hardware.fail_disable = true;
    assert_eq!(
        rx.stop(&mut hardware),
        Err(Esp32s31RxFrontierError::Ring(RxRingError::Busy))
    );
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Live);

    hardware.fail_disable = false;
    rx.stop(&mut hardware).unwrap();
    assert_eq!(rx.phase(), Esp32s31RxFrontierPhase::Halted);
}

#[test]
fn running_scan_rx_returns_the_exact_connected_epoch_resources() {
    const STAGE_SLOTS: usize = 1;
    const STAGE_CAPACITY: usize = 64;
    struct TestDelay;

    impl RxDmaObservationDelay for TestDelay {
        fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    let storage = Box::leak(Box::new(Esp32s31RxDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
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
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let pool = RxStagePool::<STAGE_SLOTS, STAGE_CAPACITY>::new();
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGE_SLOTS, STAGE_CAPACITY, STAGE_SLOTS>::new();
    let (sender, _receiver) = queue.split();
    let connected = Esp32s31StagedRxProducer::new(ring, storage, &pool, TestDelay, sender);
    let stopped = connected
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("mock connected ring must stop"));
    let pool_address = stopped.pool() as *const _;

    let mut running = Esp32s31RunningScanRx::from_stopped(stopped);
    assert_eq!(running.phase(), Esp32s31RxFrontierPhase::Halted);
    running.prepare_initial(&mut hardware).unwrap();
    assert_eq!(running.phase(), Esp32s31RxFrontierPhase::Prepared);

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
