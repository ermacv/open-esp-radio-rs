//! Embassy scheduling boundary for a pending ESP32-S31 RX append.

use core::future::Future;

use embassy_time::Timer;

use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDma, RxRingLive},
    rx_pool::{NetworkRxFrame, RxStageReloadPending, RxStageTransactionError},
};

/// Timer capability used between two live reload observations.
pub trait RxReloadDelay {
    fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_;
}

/// Executor timer used by a normal ESP32-S31 connected RX owner.
///
/// HIL compositions may wrap the same edge to collect timing evidence, but
/// the driver itself only requires a finite asynchronous delay between PAC
/// reload observations.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31RxReloadDelay;

impl RxReloadDelay for EmbassyEsp32s31RxReloadDelay {
    fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_ {
        Timer::after_micros(u64::from(micros))
    }
}

/// Complete one typed staged-frame transaction without blocking the executor.
///
/// The MAC owner performs exactly one MMIO observation per `poll_reload` call.
/// Every pending result crosses this adapter's awaited one-microsecond edge
/// before another observation. The transaction itself owns the recovered ROM
/// attempt bound and does not release a network-visible frame early.
pub async fn await_staged_rx_reload<
    'pool,
    const SLOTS: usize,
    const CAPACITY: usize,
    const COUNT: usize,
    M: RxDma,
    D: RxReloadDelay,
>(
    mut pending: RxStageReloadPending<'pool, SLOTS, CAPACITY>,
    mmio: &mut M,
    ring: &mut RxRingLive<'_, COUNT>,
    delay: &mut D,
) -> Result<NetworkRxFrame<'pool, SLOTS, CAPACITY>, RxStageTransactionError> {
    loop {
        match pending.poll_reload(mmio, ring)? {
            Some(frame) => return Ok(frame),
            None => delay.after_micros(1).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::future::{Future, ready};
    use std::task::{Context, Poll, Waker};

    use open_esp_radio_esp32s31_wifi_dma::descriptor::{
        BIT_30, BIT_31, DESCRIPTOR_BYTES, Descriptor, LENGTH_SHIFT,
    };
    use open_esp_radio_esp32s31_wifi_dma::{
        rx_ring::{RxDmaArenaState, RxRingError},
        rx_storage::RxDmaStorage,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        rx::{RxDma, RxDmaBinding, RxRingStopped},
        rx_pool::RxStagePool,
    };

    use super::{RxReloadDelay, await_staged_rx_reload};

    const BASE: u32 = 0x2f00_1000;

    #[derive(Default)]
    struct MockRxDma {
        walker_enabled: bool,
        reload_requested: bool,
        pending_samples: u32,
        reload_reads: u32,
        descriptor_base: u32,
    }

    impl RxDma for MockRxDma {
        fn last_descriptor_low(&mut self) -> u32 {
            0
        }

        fn next_descriptor_low(&mut self) -> u32 {
            BASE + DESCRIPTOR_BYTES
        }

        fn walker_enabled(&mut self) -> bool {
            self.walker_enabled
        }

        fn reload_pending(&mut self) -> bool {
            self.reload_reads = self.reload_reads.saturating_add(1);
            if self.reload_requested && self.pending_samples != 0 {
                self.pending_samples -= 1;
                true
            } else {
                false
            }
        }

        fn set_descriptor_high_window(&mut self, _: &RxDmaBinding, _address_high: u16) {}

        fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
            self.descriptor_base = address;
        }

        fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
            self.walker_enabled = true;
        }

        fn request_reload(&mut self, _: &RxDmaBinding) {
            self.reload_requested = true;
        }

        fn try_enable_walker(&mut self, _: &RxDmaBinding) -> bool {
            if self.walker_enabled {
                false
            } else {
                self.walker_enabled = true;
                true
            }
        }

        fn try_disable_walker(&mut self) -> bool {
            self.walker_enabled = false;
            true
        }

        fn fence(&mut self) {}
    }

    #[derive(Default)]
    struct CountingDelay {
        calls: u32,
    }

    impl RxReloadDelay for CountingDelay {
        fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_ {
            assert_eq!(micros, 1);
            self.calls = self.calls.saturating_add(1);
            ready(())
        }
    }

    fn run_ready<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("ready-only test future unexpectedly suspended"),
        }
    }

    #[test]
    fn awaits_once_for_every_pending_reload_observation() {
        const COUNT: usize = 2;
        const BUFFER_SIZE: u32 = 256;
        let descriptors = [const { Descriptor::new() }; COUNT];
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let mut mmio = MockRxDma {
            pending_samples: 2,
            ..MockRxDma::default()
        };
        let stopped =
            RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
                Ok(())
            })
            .unwrap();
        let mut ring = stopped.start(&mut mmio).unwrap();
        descriptors[0].write_word0(BUFFER_SIZE | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        let completed = ring.take_completed(0).unwrap();
        let pool = RxStagePool::<1, 16>::new();
        let pending = pool
            .stage_recycle(completed, &[1, 2, 3, 4], &mut mmio, &mut ring, |_| Ok(()))
            .unwrap();
        let mut delay = CountingDelay::default();
        let reload_reads_before_await = mmio.reload_reads;

        let network = run_ready(await_staged_rx_reload(
            pending, &mut mmio, &mut ring, &mut delay,
        ))
        .unwrap();
        assert_eq!(delay.calls, 2);
        assert_eq!(mmio.reload_reads - reload_reads_before_await, 3);
        assert_eq!(network.segment().buffer, &[1, 2, 3, 4]);
        assert_eq!(pool.network_slots(), 1);
        drop(network);
        ring.try_stop(&mut mmio)
            .unwrap_or_else(|_| panic!("test RX ring must stop"));
    }

    #[test]
    fn dropped_live_ring_poison_is_sticky_in_the_static_dma_arena() {
        const COUNT: usize = 2;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 256, 260>::new();
        let mut mmio = MockRxDma::default();
        let stopped = storage.prepare_ring(&mut mmio, BASE, &buffers).unwrap();
        let ring = stopped.start(&mut mmio).unwrap();
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);

        drop(ring);

        assert!(mmio.walker_enabled);
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
        assert!(matches!(
            storage.prepare_ring(&mut mmio, BASE, &buffers),
            Err(RxRingError::ResetRequired)
        ));
    }
}
