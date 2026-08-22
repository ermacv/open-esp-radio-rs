//! ESP32-S31 RX append completion and independent link-release delays.

use core::future::Future;

use embassy_time::Timer;

use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDma, RxRingLive},
    rx_pool::{NetworkRxFrame, RxStageReloadPending, RxStageTransactionError},
};

/// Timer capability for cooperative RX ownership probes and walker settle.
pub trait RxDmaObservationDelay {
    fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_;
}

/// Executor timer used by a normal ESP32-S31 connected RX owner.
///
/// HIL compositions may wrap the same edge to collect timing evidence, but
/// the driver itself only requires finite asynchronous delays at explicitly
/// schedulable ownership edges. The reload suffix is intentionally not one.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31RxDmaObservationDelay;

impl RxDmaObservationDelay for EmbassyEsp32s31RxDmaObservationDelay {
    fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_ {
        Timer::after_micros(u64::from(micros))
    }
}

/// Complete one typed staged-frame transaction without a scheduler boundary.
///
/// The recovered vendor transaction spins on the doorbell and immediately
/// performs its NEXT/LAST suffix. Splitting those observations across an
/// executor yield permits unrelated RX progress to replace the cursor epoch.
pub fn complete_staged_rx_reload<
    'pool,
    const SLOTS: usize,
    const CAPACITY: usize,
    const COUNT: usize,
    M: RxDma,
>(
    mut pending: RxStageReloadPending<'pool, SLOTS, CAPACITY>,
    mmio: &mut M,
    ring: &mut RxRingLive<'_, COUNT>,
) -> Result<NetworkRxFrame<'pool, SLOTS, CAPACITY>, RxStageTransactionError> {
    pending.complete_reload(mmio, ring)
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_wifi_dma::descriptor::{
        BIT_30, BIT_31, DESCRIPTOR_BYTES, Descriptor, LENGTH_SHIFT,
    };
    use open_esp_radio_esp32s31_wifi_dma::{
        rx_ring::{RxDmaArenaState, RxRingError},
        rx_storage::RxDmaStorage,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        rx::{RxDma, RxDmaBinding, RxDmaWalkerStopped, RxRingStopped},
        rx_pool::RxStagePool,
    };

    use super::complete_staged_rx_reload;

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
            if self.walker_enabled {
                (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff
            } else {
                0
            }
        }

        fn next_descriptor_low(&mut self) -> u32 {
            if self.walker_enabled {
                (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff
            } else {
                0
            }
        }

        fn next_descriptor_word(&mut self) -> u32 {
            self.next_descriptor_low()
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
                open_esp_radio_esp32s31_wifi_mac::rx::RxDmaCursorObservation::validation(
                    last, next,
                ),
            )
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

        fn try_with_walker_enabled<R>(
            &mut self,
            _: &RxDmaBinding,
            enabled: impl for<'confirmation> FnOnce(
                open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled<'confirmation>,
            ) -> R,
        ) -> Option<R> {
            if self.walker_enabled {
                None
            } else {
                self.walker_enabled = true;
                Some(enabled(
                    open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled::validation(),
                ))
            }
        }

        fn try_with_walker_stopped<R>(
            &mut self,
            stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
        ) -> Option<R> {
            self.walker_enabled = false;
            Some(stopped(RxDmaWalkerStopped::validation()))
        }

        fn fence(&mut self) {}
    }

    #[test]
    fn completes_pending_reload_without_a_scheduling_boundary() {
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
        let mut ring = stopped
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .unwrap();
        descriptors[0].write_word0(BUFFER_SIZE | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        let completed = ring.take_completed(0).unwrap();
        assert!(!ring.observe_completed_unit_link_release(&mut mmio, BASE & 0x000f_ffff, 1));
        descriptors[1].write_word0(descriptors[1].word0() | BIT_30);
        let later_low = (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff;
        assert!(ring.observe_completed_unit_link_release(&mut mmio, later_low, 1));
        let pool = RxStagePool::<1, 16>::new();
        let pending = pool
            .stage_recycle(completed, &[1, 2, 3, 4], &mut mmio, &mut ring, |_| Ok(()))
            .unwrap();
        let reload_reads_before_completion = mmio.reload_reads;

        let network = complete_staged_rx_reload(pending, &mut mmio, &mut ring).unwrap();
        assert_eq!(mmio.reload_reads - reload_reads_before_completion, 3);
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
        let ring = stopped
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .unwrap();
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
