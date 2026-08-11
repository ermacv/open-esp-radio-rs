use core::future::{Future, ready};

use open_esp_radio_esp32s31_wifi_dma::descriptor::{BIT_30, DESCRIPTOR_BYTES};
use open_esp_radio_esp32s31_wifi_mac::rx::{RxDma, RxDmaBinding, RxRingHalted, RxRingStopped};

use super::*;
use crate::rx_dma_service::Esp32s31RxDmaStorage;

const COUNT: usize = 2;
const BUFFER_SIZE: usize = 64;
const STORAGE_SIZE: usize = 128;
const BASE: u32 = 0x2f00_1000;
const BUFFERS: [u32; COUNT] = [0x2f00_2000, 0x2f00_2100];

struct ReadyDelay;

impl Esp32s31PreconnectedRxDelay for ReadyDelay {
    fn after_micros(_micros: u32) -> impl Future<Output = ()> {
        ready(())
    }
}

#[derive(Default)]
struct Hardware {
    walker: bool,
    descriptor_base: u32,
    enable_count: u32,
    disable_count: u32,
    reload_count: u32,
}

impl RxDma for Hardware {
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
    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reload_count += 1;
    }
    fn try_enable_walker(&mut self, _: &RxDmaBinding) -> bool {
        if self.walker {
            false
        } else {
            self.walker = true;
            self.enable_count += 1;
            true
        }
    }
    fn try_disable_walker(&mut self) -> bool {
        if self.walker {
            self.walker = false;
            self.disable_count += 1;
            true
        } else {
            false
        }
    }
    fn fence(&mut self) {}
}

fn halted_ring<'a>(
    hardware: &mut Hardware,
    storage: &'a Esp32s31RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>,
) -> RxRingHalted<'a, COUNT> {
    RxRingStopped::prepare(
        hardware,
        storage.descriptors(),
        BASE,
        &BUFFERS,
        BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap()
    .into_halted()
}

#[test]
fn owner_services_a_terminal_descriptor_and_round_trips_between_phases() {
    let storage = Esp32s31RxDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let mut rx = Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(
        halted_ring(&mut hardware, &storage),
    );
    embassy_futures::block_on(rx.start_with_storage(&mut hardware, &storage)).unwrap();
    assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Live);

    storage.descriptors()[0].write_word0(storage.descriptors()[0].word0() | BIT_30);
    let progress = rx
        .service_completed(&mut hardware, &storage, |segment| {
            assert_eq!(segment.descriptor_address, BASE);
            assert_eq!(segment.buffer.len(), BUFFER_SIZE);
            Esp32s31PreconnectedRxDirective::Stop
        })
        .unwrap();
    assert_eq!(
        progress,
        Esp32s31PreconnectedRxProgress {
            completed: 1,
            stopped: true,
        }
    );

    let mut moved = rx.take().unwrap();
    assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Vacant);
    assert_eq!(moved.phase(), Esp32s31PreconnectedRxPhase::Live);
    moved.stop(&mut hardware).unwrap();
    assert_eq!(moved.phase(), Esp32s31PreconnectedRxPhase::Halted);
    rx = moved;
    assert_eq!(rx.phase(), Esp32s31PreconnectedRxPhase::Halted);
}

#[test]
fn consuming_connected_promotion_returns_live_ring_or_exact_owner() {
    let storage = Esp32s31RxDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let rx = Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(halted_ring(
        &mut hardware,
        &storage,
    ));
    let live = embassy_futures::block_on(rx.try_into_live_with_storage(&mut hardware, &storage))
        .unwrap_or_else(|_| panic!("fresh halted owner must become live"));
    assert_eq!(live.descriptor_base(), BASE);

    let halted = match live.try_stop(&mut hardware) {
        Ok(halted) => halted,
        Err(_) => panic!("mock walker must stop before the failure case"),
    };
    let mut already_live =
        Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(halted);
    embassy_futures::block_on(already_live.start_with_storage(&mut hardware, &storage))
        .expect("finite protocol phase starts the ring");
    storage.descriptors()[0].write_word0(storage.descriptors()[0].word0() | BIT_30);
    already_live
        .service_completed(&mut hardware, &storage, |_| {
            Esp32s31PreconnectedRxDirective::Stop
        })
        .expect("the final protocol frame remains observed before promotion");
    let enable_count = hardware.enable_count;
    let disable_count = hardware.disable_count;
    let reload_count = hardware.reload_count;
    let live =
        embassy_futures::block_on(already_live.try_into_live_with_storage(&mut hardware, &storage))
            .unwrap_or_else(|_| panic!("an existing live frontier must transfer cleanly"));
    assert_eq!(hardware.enable_count, enable_count + 1);
    assert_eq!(hardware.disable_count, disable_count + 1);
    assert_eq!(hardware.reload_count, reload_count);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_eq!(live.observed_mask(), 0);
    let halted = match live.try_stop(&mut hardware) {
        Ok(halted) => halted,
        Err(_) => panic!("mock walker must stop after the live handoff"),
    };
    let mut vacant = Esp32s31PreconnectedRx::<ReadyDelay, COUNT, BUFFER_SIZE>::from_halted(halted);
    let retained = vacant.take().expect("test retains the exact halted owner");
    let failure =
        embassy_futures::block_on(vacant.try_into_live_with_storage(&mut hardware, &storage))
            .err()
            .expect("vacant frontier must return its placeholder owner");
    assert_eq!(failure.error, Esp32s31PreconnectedRxError::OwnerUnavailable);
    assert_eq!(failure.owner.phase(), Esp32s31PreconnectedRxPhase::Vacant);
    assert_eq!(retained.phase(), Esp32s31PreconnectedRxPhase::Halted);
}
