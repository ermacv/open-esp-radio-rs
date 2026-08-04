//! Embassy wake adapter for the executor-neutral S31 MAC interrupt state.
//!
//! The hard ISR only publishes finite MAC work. The driver-owned
//! [`IrqState`] then applies the recovered vendor priority before separate
//! coalescing Embassy signals become visible to the radio task.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_embassy_net::{RawMutex, Signal};
use open_esp_radio_esp32s31_wifi_mac::irq::{
    IrqSink, IrqState, IrqWork, MacInterruptRoute, PowerIrqSink,
};

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

/// Coalesced executor work discarded after an interrupt epoch is quiesced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbassyMacIrqDrain {
    pub rx: bool,
    pub rx_capacity: bool,
    pub tx_events: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacInterruptEpochStateError {
    Active,
    Quiesced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacInterruptEpochActivateError<E> {
    AlreadyActive,
    Route(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacInterruptEpochQuiesceError<E> {
    AlreadyQuiesced,
    Route(E),
}

/// Complete stale executor publication removed after a hardware interrupt
/// epoch is closed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31MacInterruptEpochDrain {
    pub mac: EmbassyMacIrqDrain,
    pub power_events: u32,
}

/// Persistent setup owner for repeated connected MAC interrupt epochs.
///
/// The inactive state contains the task-side setup token. The active state
/// lends that token to a platform route. Quiescence first recovers the exact
/// token and only then drains coalesced Embassy publications, preventing one
/// epoch's wake from becoming work in the next epoch.
pub struct Esp32s31MacInterruptEpoch<'runtime, R, M: RawMutex>
where
    R: MacInterruptRoute,
{
    route: R,
    setup: Option<R::Setup>,
    mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
}

impl<'runtime, R, M> Esp32s31MacInterruptEpoch<'runtime, R, M>
where
    R: MacInterruptRoute,
    M: RawMutex,
{
    pub const fn new(
        route: R,
        setup: R::Setup,
        mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
        power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    ) -> Self {
        Self {
            route,
            setup: Some(setup),
            mac_runtime,
            power_runtime,
        }
    }

    pub const fn is_active(&self) -> bool {
        self.setup.is_none()
    }

    /// Borrow the task-side capability for polling-only scan/auth phases.
    pub fn setup(&self) -> Result<&R::Setup, Esp32s31MacInterruptEpochStateError> {
        self.setup
            .as_ref()
            .ok_or(Esp32s31MacInterruptEpochStateError::Active)
    }

    pub fn activate(
        &mut self,
        platform: &R::Platform,
        event_mask: u32,
    ) -> Result<(), Esp32s31MacInterruptEpochActivateError<R::Error>> {
        let setup = self
            .setup
            .take()
            .ok_or(Esp32s31MacInterruptEpochActivateError::AlreadyActive)?;
        match self.route.activate(platform, setup, event_mask) {
            Ok(()) => Ok(()),
            Err((error, setup)) => {
                self.setup = Some(setup);
                Err(Esp32s31MacInterruptEpochActivateError::Route(error))
            }
        }
    }

    pub fn quiesce(
        &mut self,
        platform: &R::Platform,
    ) -> Result<Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError<R::Error>>
    {
        if self.setup.is_some() {
            return Err(Esp32s31MacInterruptEpochQuiesceError::AlreadyQuiesced);
        }
        let setup = self
            .route
            .quiesce(platform)
            .map_err(Esp32s31MacInterruptEpochQuiesceError::Route)?;
        self.setup = Some(setup);
        Ok(Esp32s31MacInterruptEpochDrain {
            mac: self.mac_runtime.drain_pending(),
            power_events: self.power_runtime.drain_pending(),
        })
    }
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

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_mac::irq::{
        IrqDisposition, IrqSink, MAC_INT_COLLISION, MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE,
        MAC_INT_TX_TIMEOUT, MacInterrupt, MacInterruptRoute, MacPowerInterrupt,
        PowerIrqDisposition, handle_mac_irq, handle_power_irq,
    };

    use super::{
        EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch,
        Esp32s31MacInterruptEpochActivateError, Esp32s31MacInterruptEpochQuiesceError,
        Esp32s31MacInterruptEpochStateError,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RouteError {
        Activation,
        Quiescence,
    }

    struct Route {
        active: bool,
    }

    impl MacInterruptRoute for Route {
        type Platform = Cell<u8>;
        type Setup = u8;
        type Error = RouteError;

        fn activate(
            &mut self,
            platform: &Self::Platform,
            setup: Self::Setup,
            _event_mask: u32,
        ) -> Result<(), (Self::Error, Self::Setup)> {
            if platform.get() == 10 {
                return Err((RouteError::Activation, setup));
            }
            self.active = true;
            platform.set(1);
            Ok(())
        }

        fn quiesce(&mut self, platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
            if platform.get() == 20 {
                return Err(RouteError::Quiescence);
            }
            assert!(self.active);
            self.active = false;
            platform.set(2);
            Ok(7)
        }
    }

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
    fn quiesced_epoch_drain_removes_every_coalesced_wake() {
        let runtime = EmbassyMacIrqRuntime::<NoopRawMutex>::new();

        runtime.publish(MAC_INT_RX_SUCCESS | MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT);
        runtime.notify_rx_capacity();

        assert_eq!(
            runtime.drain_pending(),
            super::EmbassyMacIrqDrain {
                rx: true,
                rx_capacity: true,
                tx_events: MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT,
            }
        );
        assert_eq!(runtime.drain_pending(), Default::default());
        assert!(!runtime.rx_signaled());
        assert_eq!(runtime.try_take_tx(), None);
    }

    #[test]
    fn irq_epoch_recovers_setup_before_draining_every_executor_wake() {
        let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
        let platform = Cell::new(0);
        let mut epoch = Esp32s31MacInterruptEpoch::new(Route { active: false }, 7, &mac, &power);

        assert_eq!(epoch.setup(), Ok(&7));
        epoch.activate(&platform, 0x1234).unwrap();
        assert!(epoch.is_active());
        assert_eq!(
            epoch.setup(),
            Err(Esp32s31MacInterruptEpochStateError::Active)
        );
        mac.publish(MAC_INT_RX_SUCCESS | MAC_INT_TX_COMPLETE);
        mac.notify_rx_capacity();
        power.publish(0x55);

        let drained = epoch.quiesce(&platform).unwrap();
        assert_eq!(platform.get(), 2);
        assert_eq!(drained.mac.rx, true);
        assert_eq!(drained.mac.rx_capacity, true);
        assert_eq!(drained.mac.tx_events, MAC_INT_TX_COMPLETE);
        assert_eq!(drained.power_events, 0x55);
        assert_eq!(epoch.setup(), Ok(&7));
        assert_eq!(mac.drain_pending(), Default::default());
        assert_eq!(power.drain_pending(), 0);
    }

    #[test]
    fn irq_epoch_retains_the_exact_frontier_on_each_route_failure() {
        let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
        let platform = Cell::new(10);
        let mut epoch = Esp32s31MacInterruptEpoch::new(Route { active: false }, 7, &mac, &power);

        assert_eq!(
            epoch.activate(&platform, 0x1234),
            Err(Esp32s31MacInterruptEpochActivateError::Route(
                RouteError::Activation
            ))
        );
        assert_eq!(epoch.setup(), Ok(&7));
        platform.set(0);
        epoch.activate(&platform, 0x1234).unwrap();
        platform.set(20);
        assert_eq!(
            epoch.quiesce(&platform),
            Err(Esp32s31MacInterruptEpochQuiesceError::Route(
                RouteError::Quiescence
            ))
        );
        assert!(epoch.is_active());
        platform.set(0);
        epoch.quiesce(&platform).unwrap();
        assert_eq!(epoch.setup(), Ok(&7));
        assert_eq!(
            epoch.quiesce(&platform),
            Err(Esp32s31MacInterruptEpochQuiesceError::AlreadyQuiesced)
        );
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
