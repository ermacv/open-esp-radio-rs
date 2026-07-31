//! Embassy wake adapter for the executor-neutral S31 MAC interrupt state.
//!
//! The hard ISR only publishes finite MAC work. The driver-owned
//! [`IrqState`] then applies the recovered vendor priority before separate
//! coalescing Embassy signals become visible to the radio task.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_embassy_net::{RawMutex, Signal};
use open_esp_radio_esp32s31_wifi_mac::irq::{IrqSink, IrqState, IrqWork};

/// Driver-owned S31 MAC interrupt handoff for one Embassy radio task.
///
/// `publish` drains one hardware snapshot through
/// [`IrqState::try_take_next`], which orders RX success before TX completion,
/// timeout and collision. Consumers that wait for RX and TX concurrently must
/// poll [`Self::wait_rx`] first; this is the same ordering contract as the
/// vendor FIQ and is intentionally visible at the adapter boundary.
///
/// Duplicate work of one kind coalesces in `Signal`, matching the vendor
/// worker latch. Descriptor and completion rings remain the durable source of
/// multiplicity.
///
/// SOURCE: complete `_oracles/libpp.a[wdev.o]::wDev_ProcessFiq` services
/// `RX_SUCCESS` before `TX_COMPLETE`, `TX_TIMEOUT` and `COLLISION`. Complete
/// `_oracles/libpp.a[pp.o]::{pp_post,ppTask}` coalesces the corresponding
/// worker wake while the descriptor/completion state remains hardware-owned.
pub struct EmbassyMacIrqRuntime<M: RawMutex> {
    state: IrqState,
    rx: Signal<M, ()>,
    tx: Signal<M, ()>,
    rx_post_count: AtomicU32,
}

impl<M: RawMutex> EmbassyMacIrqRuntime<M> {
    pub const fn new() -> Self {
        Self {
            state: IrqState::new(),
            rx: Signal::new(),
            tx: Signal::new(),
            rx_post_count: AtomicU32::new(0),
        }
    }

    /// Publish one acknowledged MAC interrupt snapshot.
    ///
    /// The executor signals are emitted only after the executor-neutral state
    /// has selected each work item in recovered vendor priority order.
    #[inline]
    pub fn publish(&self, mac_pending: u32) {
        self.state.post(mac_pending);
        while let Some(work) = self.state.try_take_next() {
            match work {
                IrqWork::RxSuccess => {
                    self.rx_post_count.fetch_add(1, Ordering::Relaxed);
                    self.rx.signal(());
                }
                IrqWork::TxComplete | IrqWork::TxTimeout | IrqWork::Collision => {
                    self.tx.signal(());
                }
            }
        }
    }

    /// Wait for a coalesced RX-success bottom-half edge.
    pub async fn wait_rx(&self) {
        self.rx.wait().await;
    }

    /// Wait for any TX completion, timeout or collision edge.
    pub async fn wait_tx(&self) {
        self.tx.wait().await;
    }

    /// Whether the RX bottom half has durable pending work.
    #[inline]
    pub fn rx_signaled(&self) -> bool {
        self.rx.signaled()
    }

    /// Consume a stale TX wake before publishing a new transaction.
    #[inline]
    pub fn try_take_tx(&self) -> Option<()> {
        self.tx.try_take()
    }

    /// Number of RX-success work publications, with wrapping semantics.
    #[inline]
    pub fn rx_post_count(&self) -> u32 {
        self.rx_post_count.load(Ordering::Relaxed)
    }

    /// Unsupported MAC bits observed by the shared hard-ISR handler.
    #[inline]
    pub fn observed_unhandled(&self) -> u32 {
        self.state.observed_unhandled()
    }
}

impl<M: RawMutex> IrqSink for EmbassyMacIrqRuntime<M> {
    #[inline]
    fn post(&self, mac_pending: u32) {
        self.publish(mac_pending);
    }

    #[inline]
    fn record_unhandled(&self, bits: u32) {
        self.state.record_unhandled(bits);
    }
}

impl<M: RawMutex> Default for EmbassyMacIrqRuntime<M> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_mac::irq::{
        IrqSink, MAC_INT_COLLISION, MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
    };

    use super::EmbassyMacIrqRuntime;

    #[test]
    fn maps_one_combined_snapshot_to_bounded_rx_and_tx_wakes() {
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

        runtime.publish(
            MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS,
        );

        assert_eq!(runtime.rx_post_count(), 1);
        assert!(runtime.rx_signaled());
        assert_eq!(runtime.try_take_tx(), Some(()));
        // Three TX causes intentionally coalesce into one worker wake.
        assert_eq!(runtime.try_take_tx(), None);
    }

    #[test]
    fn retains_unhandled_evidence_through_the_irq_sink_contract() {
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        IrqSink::record_unhandled(&runtime, 0x8000_0000);
        assert_eq!(runtime.observed_unhandled(), 0x8000_0000);
    }
}
