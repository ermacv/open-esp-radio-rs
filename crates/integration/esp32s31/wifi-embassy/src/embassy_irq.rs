//! Embassy wake adapter for the executor-neutral S31 MAC interrupt state.
//!
//! The hard ISR only publishes finite MAC work. The driver-owned
//! [`IrqState`] then applies the recovered vendor priority before separate
//! coalescing Embassy signals become visible to the radio task.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_embassy_net::{RawMutex, Signal};
use open_esp_radio_esp32s31_wifi_mac::irq::{IrqSink, IrqState, IrqWork, PowerIrqSink};

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
    rx_capacity: Signal<M, ()>,
    tx: Signal<M, ()>,
    tx_pending: AtomicU32,
    rx_post_count: AtomicU32,
}

impl<M: RawMutex> EmbassyMacIrqRuntime<M> {
    pub const fn new() -> Self {
        Self {
            state: IrqState::new(),
            rx: Signal::new(),
            rx_capacity: Signal::new(),
            tx: Signal::new(),
            tx_pending: AtomicU32::new(0),
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
                    self.tx_pending.fetch_or(work.mac_bit(), Ordering::Release);
                    self.tx.signal(());
                }
            }
        }
    }

    /// Wait for a coalesced RX-success bottom-half edge.
    pub async fn wait_rx(&self) {
        self.rx.wait().await;
    }

    /// Wake a radio actor stopped by staging ownership backpressure.
    ///
    /// This is distinct from a hardware RX edge: while backpressured, new RX
    /// interrupts must not repeatedly win ordered arbitration over a pending TX
    /// completion. The protocol consumer emits this only after it has actually
    /// returned one staging credit.
    #[inline]
    pub fn notify_rx_capacity(&self) {
        self.rx_capacity.signal(());
    }

    /// Wait until protocol processing returns at least one staging credit.
    pub async fn wait_rx_capacity(&self) {
        self.rx_capacity.wait().await;
    }

    /// Wait for and consume coalesced TX completion, timeout or collision bits.
    pub async fn wait_tx(&self) -> u32 {
        self.tx.wait().await;
        self.tx_pending.swap(0, Ordering::Acquire)
    }

    /// Whether the RX bottom half has durable pending work.
    #[inline]
    pub fn rx_signaled(&self) -> bool {
        self.rx.signaled()
    }

    /// Consume a stale TX wake before publishing a new transaction.
    #[inline]
    pub fn try_take_tx(&self) -> Option<u32> {
        self.tx.try_take()?;
        Some(self.tx_pending.swap(0, Ordering::Acquire))
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

/// Embassy handoff for acknowledged WDEVPWR snapshots.
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

    /// Wait for and consume the complete coalesced WDEVPWR image.
    pub async fn wait(&self) -> u32 {
        self.signal.wait().await;
        self.pending.swap(0, Ordering::Acquire)
    }

    /// Consume a pending image without blocking the executor.
    pub fn try_take(&self) -> Option<u32> {
        self.signal.try_take()?;
        Some(self.pending.swap(0, Ordering::Acquire))
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

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_mac::irq::{
        IrqDisposition, IrqSink, MAC_INT_COLLISION, MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE,
        MAC_INT_TX_TIMEOUT, MacInterrupt, MacPowerInterrupt, PowerIrqDisposition, handle_mac_irq,
        handle_power_irq,
    };

    use super::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime};

    #[test]
    fn maps_one_combined_snapshot_to_bounded_rx_and_tx_wakes() {
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

        runtime.publish(
            MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS,
        );

        assert_eq!(runtime.rx_post_count(), 1);
        assert!(runtime.rx_signaled());
        assert_eq!(
            runtime.try_take_tx(),
            Some(MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_TX_COMPLETE)
        );
        // Three TX causes coalesce into one wake without losing their bits.
        assert_eq!(runtime.try_take_tx(), None);
    }

    #[test]
    fn staging_capacity_wake_does_not_forge_interrupt_evidence() {
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

        runtime.notify_rx_capacity();
        embassy_futures::block_on(runtime.wait_rx_capacity());

        assert!(!runtime.rx_signaled());
        assert_eq!(runtime.rx_post_count(), 0);
    }

    #[test]
    fn retains_unhandled_evidence_through_the_irq_sink_contract() {
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        IrqSink::record_unhandled(&runtime, 0x8000_0000);
        assert_eq!(runtime.observed_unhandled(), 0x8000_0000);
    }

    struct Interrupt {
        status: u32,
        acknowledged: Cell<Option<u32>>,
    }

    impl MacInterrupt for Interrupt {
        fn status(&mut self) -> u32 {
            self.status
        }

        fn acknowledge(&mut self, events: u32) {
            self.acknowledged.set(Some(events));
        }
    }

    #[test]
    fn production_handler_acknowledges_before_publishing_embassy_work() {
        let status = MAC_INT_RX_SUCCESS | MAC_INT_TX_COMPLETE;
        let mut interrupt = Interrupt {
            status,
            acknowledged: Cell::new(None),
        };
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

        let (disposition, snapshot) = handle_mac_irq(&mut interrupt, &runtime);

        assert_eq!(disposition, IrqDisposition::Posted);
        assert_eq!(snapshot.status, status);
        assert_eq!(interrupt.acknowledged.get(), Some(status));
        assert!(runtime.rx_signaled());
        assert_eq!(runtime.try_take_tx(), Some(MAC_INT_TX_COMPLETE));
    }

    #[test]
    fn spurious_status_neither_acknowledges_nor_wakes_embassy() {
        let mut interrupt = Interrupt {
            status: 0,
            acknowledged: Cell::new(None),
        };
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

        assert_eq!(
            handle_mac_irq(&mut interrupt, &runtime).0,
            IrqDisposition::Spurious
        );
        assert_eq!(interrupt.acknowledged.get(), None);
        assert!(!runtime.rx_signaled());
        assert_eq!(runtime.try_take_tx(), None);
    }

    struct PowerInterrupt {
        status: u32,
        acknowledged: Cell<Option<u32>>,
    }

    impl MacPowerInterrupt for PowerInterrupt {
        fn status(&mut self) -> u32 {
            self.status
        }

        fn acknowledge(&mut self, events: u32) {
            self.acknowledged.set(Some(events));
        }
    }

    #[test]
    fn power_irq_retains_the_complete_acknowledged_image_without_decoding_it() {
        let status = 0x8040_0010;
        let mut interrupt = PowerInterrupt {
            status,
            acknowledged: Cell::new(None),
        };
        let runtime = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();

        let (disposition, snapshot) = handle_power_irq(&mut interrupt, &runtime);

        assert_eq!(disposition, PowerIrqDisposition::Posted);
        assert_eq!(snapshot.status, status);
        assert_eq!(interrupt.acknowledged.get(), Some(status));
        assert_eq!(runtime.try_take(), Some(status));
        assert_eq!(runtime.try_take(), None);
    }

    #[test]
    fn spurious_power_irq_neither_acknowledges_nor_wakes_embassy() {
        let mut interrupt = PowerInterrupt {
            status: 0,
            acknowledged: Cell::new(None),
        };
        let runtime = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();

        assert_eq!(
            handle_power_irq(&mut interrupt, &runtime).0,
            PowerIrqDisposition::Spurious
        );
        assert_eq!(interrupt.acknowledged.get(), None);
        assert_eq!(runtime.try_take(), None);
    }
}
