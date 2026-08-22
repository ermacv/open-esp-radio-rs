use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_embassy_net::{RawMutex, Signal};
use open_esp_radio_esp32s31_wifi_mac::irq::PowerIrqSink;

/// Embassy handoff for acknowledged DATAPATHPWR snapshots.
///
/// The event remains an opaque bit image here. A later power-policy slice may
/// decode only causes whose hardware meaning and lifecycle have their own
/// qualification evidence.
pub struct EmbassyPowerIrqRuntime<M: RawMutex> {
    signal: Signal<M, ()>,
    pending: AtomicU32,
}

impl<M: RawMutex> EmbassyPowerIrqRuntime<M> {
    pub const fn new() -> Self {
        Self {
            signal: Signal::new(),
            pending: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn publish(&self, pending: u32) {
        if pending != 0 {
            self.pending.fetch_or(pending, Ordering::Release);
            self.signal.signal(());
        }
    }

    /// Wait for and consume the complete coalesced DATAPATHPWR image.
    pub async fn wait(&self) -> u32 {
        self.signal.wait().await;
        self.pending.swap(0, Ordering::Acquire)
    }

    /// Consume a pending image without blocking the executor.
    pub fn try_take(&self) -> Option<u32> {
        self.signal.try_take()?;
        Some(self.pending.swap(0, Ordering::Acquire))
    }

    /// Remove one stale power wake and its complete coalesced event image
    /// after the platform interrupt route has been quiesced.
    pub fn drain_pending(&self) -> u32 {
        let _wake = self.signal.try_take();
        self.pending.swap(0, Ordering::Acquire)
    }
}

impl<M: RawMutex> PowerIrqSink for EmbassyPowerIrqRuntime<M> {
    #[inline]
    fn post_power(&self, pending: u32) {
        self.publish(pending);
    }
}

impl<M: RawMutex> Default for EmbassyPowerIrqRuntime<M> {
    fn default() -> Self {
        Self::new()
    }
}
