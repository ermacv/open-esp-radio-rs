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
/// SOURCE: complete `libpp.a[wdev.o]::wDev_ProcessFiq` services
/// `RX_SUCCESS` before `TX_COMPLETE`, `TX_TIMEOUT` and `COLLISION`. Complete
/// `libpp.a[pp.o]::{pp_post,ppTask}` coalesces the corresponding
/// worker wake while the descriptor/completion state remains hardware-owned.
pub struct EmbassyMacIrqRuntime<M: RawMutex> {
    state: IrqState,
    rx: Signal<M, ()>,
    rx_capacity: Signal<M, ()>,
    tx: Signal<M, ()>,
    tx_pending: AtomicU32,
    rx_post_count: AtomicU32,
}

/// Coalesced executor work discarded after an interrupt epoch is quiesced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbassyMacIrqDrain {
    pub rx: bool,
    pub rx_capacity: bool,
    pub tx_events: u32,
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

    /// Remove every coalesced executor wake after hardware publication stops.
    ///
    /// Descriptor and transaction owners remain the durable source of truth;
    /// this only prevents one epoch's already-acknowledged wake from being
    /// interpreted as work in a later connected epoch. The caller must first
    /// mask the peripheral and CPU interrupt routes.
    pub fn drain_pending(&self) -> EmbassyMacIrqDrain {
        let rx = self.rx.try_take().is_some();
        let rx_capacity = self.rx_capacity.try_take().is_some();
        let _tx_wake = self.tx.try_take();
        let tx_events = self.tx_pending.swap(0, Ordering::Acquire);
        EmbassyMacIrqDrain {
            rx,
            rx_capacity,
            tx_events,
        }
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
