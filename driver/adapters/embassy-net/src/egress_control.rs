//! Affine cross-core mirror of stack-owned egress demand.
//!
//! Packet payload and DMA credits never enter this control plane. Core1
//! publishes only bounded, generation-bound queue lifecycle snapshots. Core0
//! owns the corresponding radio-side mirror and services it synchronously at
//! datapath scheduling boundaries. A future authoritative radio scheduler can
//! derive burst/airtime grants from that mirror without a per-packet
//! cross-core request/reply protocol.

#[cfg(feature = "tx-phase-telemetry")]
use core::sync::atomic::AtomicU32;
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use embassy_net_driver::EgressDemandUpdate;
use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal, waitqueue::GenericAtomicWaker};
use open_esp_radio_dma::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError,
};
use pin_project_lite::pin_project;

use crate::egress_demand::{EgressDemandOutbox, EgressDemandStateError, EgressRadioDemandState};
#[cfg(feature = "tx-phase-telemetry")]
use crate::tx_performance::TxPerformanceSample;

#[cfg(feature = "tx-egress-diagnostic-switch")]
static EGRESS_CONTROL_ENABLED: AtomicBool = AtomicBool::new(true);

/// Select the disabled same-ELF control for intrusive HIL accounting.
///
/// This is a startup policy. It must be configured before the network- and
/// radio-side demand owners are attached to their permanent cores; both
/// snapshot the value so ordinary packet admission needs no inter-core atomic
/// load merely to observe an immutable diagnostic mode.
#[cfg(feature = "tx-egress-diagnostic-switch")]
pub fn configure_egress_control_for_diagnostics(enabled: bool) {
    EGRESS_CONTROL_ENABLED.store(enabled, Ordering::Release);
}

pub(crate) fn egress_control_enabled() -> bool {
    #[cfg(feature = "tx-egress-diagnostic-switch")]
    return EGRESS_CONTROL_ENABLED.load(Ordering::Acquire);
    #[cfg(not(feature = "tx-egress-diagnostic-switch"))]
    true
}

/// Maximum number of simultaneously visible egress lifetimes per interface.
///
/// This is a control-plane horizon, not packet storage and not a promise that
/// every active key owns a DMA credit.
pub const DEFAULT_EGRESS_CONTROL_DEPTH: usize = 16;
/// Maximum lifecycle transitions consumed in one Core0 turn.
pub const DEFAULT_EGRESS_RADIO_SERVICE_BUDGET: usize = 4;
/// Maximum lifecycle transitions published in one Core1 callback.
pub const DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET: usize = 4;

pub type DefaultEgressControlPlane<M> = EgressControlPlane<M, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressNetworkPort<'control, M> =
    EgressNetworkPort<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressRadioPort<'control, M> =
    EgressRadioPort<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressRadioScheduler<'control, M> =
    EgressRadioScheduler<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressRadioOwner<'control, M> =
    EgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultDualEgressRadioOwner<'control, M> =
    DualEgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressControlledNetwork<'control, M, N, P = ()> =
    EgressControlledNetwork<N, EgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>, P>;
pub type DefaultDualEgressControlledNetwork<'control, M, N, P = ()> =
    EgressControlledNetwork<N, DualEgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH>, P>;
pub type DefaultEgressRadioWake<'control, M> = EgressRadioWake<'control, M>;
pub type DefaultEgressNetworkState = EgressNetworkState<DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressNetworkScheduler<'resources, M> =
    EgressNetworkScheduler<'resources, 'resources, M, DEFAULT_EGRESS_CONTROL_DEPTH>;

#[cfg(feature = "tx-phase-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressControlSnapshot {
    pub demand_publications: u32,
    pub demand_full: u32,
    pub radio_demand_updates: u32,
    pub radio_demand_rejected: u32,
    pub radio_wakes: u32,
    pub radio_service_calls: u32,
    pub radio_service_progressed: u32,
    pub radio_service_cycles: u32,
    pub radio_service_instructions: u32,
}

#[cfg(feature = "tx-phase-telemetry")]
impl EgressControlSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            demand_publications: self
                .demand_publications
                .wrapping_sub(earlier.demand_publications),
            demand_full: self.demand_full.wrapping_sub(earlier.demand_full),
            radio_demand_updates: self
                .radio_demand_updates
                .wrapping_sub(earlier.radio_demand_updates),
            radio_demand_rejected: self
                .radio_demand_rejected
                .wrapping_sub(earlier.radio_demand_rejected),
            radio_wakes: self.radio_wakes.wrapping_sub(earlier.radio_wakes),
            radio_service_calls: self
                .radio_service_calls
                .wrapping_sub(earlier.radio_service_calls),
            radio_service_progressed: self
                .radio_service_progressed
                .wrapping_sub(earlier.radio_service_progressed),
            radio_service_cycles: self
                .radio_service_cycles
                .wrapping_sub(earlier.radio_service_cycles),
            radio_service_instructions: self
                .radio_service_instructions
                .wrapping_sub(earlier.radio_service_instructions),
        }
    }
}

#[cfg(feature = "tx-phase-telemetry")]
struct EgressControlTelemetry {
    demand_publications: AtomicU32,
    demand_full: AtomicU32,
    radio_demand_updates: AtomicU32,
    radio_demand_rejected: AtomicU32,
    radio_wakes: AtomicU32,
    radio_service_calls: AtomicU32,
    radio_service_progressed: AtomicU32,
    radio_service_cycles: AtomicU32,
    radio_service_instructions: AtomicU32,
}

#[cfg(feature = "tx-phase-telemetry")]
impl EgressControlTelemetry {
    const fn new() -> Self {
        Self {
            demand_publications: AtomicU32::new(0),
            demand_full: AtomicU32::new(0),
            radio_demand_updates: AtomicU32::new(0),
            radio_demand_rejected: AtomicU32::new(0),
            radio_wakes: AtomicU32::new(0),
            radio_service_calls: AtomicU32::new(0),
            radio_service_progressed: AtomicU32::new(0),
            radio_service_cycles: AtomicU32::new(0),
            radio_service_instructions: AtomicU32::new(0),
        }
    }

    fn snapshot(&self) -> EgressControlSnapshot {
        EgressControlSnapshot {
            demand_publications: self.demand_publications.load(Ordering::Acquire),
            demand_full: self.demand_full.load(Ordering::Acquire),
            radio_demand_updates: self.radio_demand_updates.load(Ordering::Acquire),
            radio_demand_rejected: self.radio_demand_rejected.load(Ordering::Acquire),
            radio_wakes: self.radio_wakes.load(Ordering::Acquire),
            radio_service_calls: self.radio_service_calls.load(Ordering::Acquire),
            radio_service_progressed: self.radio_service_progressed.load(Ordering::Acquire),
            radio_service_cycles: self.radio_service_cycles.load(Ordering::Acquire),
            radio_service_instructions: self.radio_service_instructions.load(Ordering::Acquire),
        }
    }
}

/// Core1-only lifecycle state kept outside recoverable async device owners.
pub struct EgressNetworkState<const DEMAND_DEPTH: usize> {
    demands: EgressDemandOutbox<DEMAND_DEPTH>,
    flush_pending: bool,
}

impl<const DEMAND_DEPTH: usize> EgressNetworkState<DEMAND_DEPTH> {
    pub const fn new() -> Self {
        Self {
            demands: EgressDemandOutbox::new(),
            flush_pending: false,
        }
    }
}

impl<const DEMAND_DEPTH: usize> Default for EgressNetworkState<DEMAND_DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// Static storage for one lifecycle stream and its cross-core wake state.
pub struct EgressControlPlane<M: RawMutex, const DEMAND_DEPTH: usize> {
    demands: AffineSpscQueue<EgressDemandUpdate, DEMAND_DEPTH>,
    radio_progress: Signal<M, ()>,
    radio_progress_pending: AtomicBool,
    radio_waiting: AtomicBool,
    demand_send_blocked: AtomicBool,
    network_progress: GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: EgressControlTelemetry,
}

/// Wake shared by all logical-interface streams of one physical radio owner.
pub struct EgressSharedRadioWake<'control, M: RawMutex> {
    progress: &'control AtomicBool,
    signal: &'control Signal<M, ()>,
    waiting: &'control AtomicBool,
}

impl<M: RawMutex> Clone for EgressSharedRadioWake<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for EgressSharedRadioWake<'_, M> {}

impl<M: RawMutex, const DEMAND_DEPTH: usize> EgressControlPlane<M, DEMAND_DEPTH> {
    pub const fn new() -> Self {
        Self {
            demands: AffineSpscQueue::new(),
            radio_progress: Signal::new(),
            radio_progress_pending: AtomicBool::new(false),
            radio_waiting: AtomicBool::new(false),
            demand_send_blocked: AtomicBool::new(false),
            network_progress: GenericAtomicWaker::new(M::INIT),
            #[cfg(feature = "tx-phase-telemetry")]
            telemetry: EgressControlTelemetry::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        EgressNetworkPort<'_, M, DEMAND_DEPTH>,
        EgressRadioPort<'_, M, DEMAND_DEPTH>,
    ) {
        self.split_with_radio_wake(self.shared_radio_wake())
    }

    pub const fn shared_radio_wake(&self) -> EgressSharedRadioWake<'_, M> {
        EgressSharedRadioWake {
            progress: &self.radio_progress_pending,
            signal: &self.radio_progress,
            waiting: &self.radio_waiting,
        }
    }

    /// Split an interface stream which wakes an existing physical owner.
    pub fn split_with_radio_wake<'control>(
        &'control self,
        wake: EgressSharedRadioWake<'control, M>,
    ) -> (
        EgressNetworkPort<'control, M, DEMAND_DEPTH>,
        EgressRadioPort<'control, M, DEMAND_DEPTH>,
    ) {
        let (demand_tx, demand_rx) = self.demands.split();
        (
            EgressNetworkPort {
                demand_tx,
                radio_progress: wake.signal,
                radio_progress_pending: wake.progress,
                radio_waiting: wake.waiting,
                demand_send_blocked: &self.demand_send_blocked,
                network_progress: &self.network_progress,
                #[cfg(feature = "tx-phase-telemetry")]
                telemetry: &self.telemetry,
            },
            EgressRadioPort {
                demand_rx,
                radio_progress: wake.signal,
                radio_progress_pending: wake.progress,
                radio_waiting: wake.waiting,
                demand_send_blocked: &self.demand_send_blocked,
                network_progress: &self.network_progress,
                #[cfg(feature = "tx-phase-telemetry")]
                telemetry: &self.telemetry,
            },
        )
    }

    #[cfg(feature = "tx-phase-telemetry")]
    pub fn snapshot(&self) -> EgressControlSnapshot {
        self.telemetry.snapshot()
    }
}

impl<M: RawMutex, const DEMAND_DEPTH: usize> Default for EgressControlPlane<M, DEMAND_DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sole Core1 endpoint for lifecycle demand publication.
pub struct EgressNetworkPort<'control, M: RawMutex, const DEMAND_DEPTH: usize> {
    demand_tx: AffineSpscSender<'control, EgressDemandUpdate, DEMAND_DEPTH>,
    radio_progress: &'control Signal<M, ()>,
    radio_progress_pending: &'control AtomicBool,
    radio_waiting: &'control AtomicBool,
    demand_send_blocked: &'control AtomicBool,
    network_progress: &'control GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: &'control EgressControlTelemetry,
}

impl<M: RawMutex, const DEMAND_DEPTH: usize> EgressNetworkPort<'_, M, DEMAND_DEPTH> {
    pub fn register_network_waker(&self, context: &Context<'_>) {
        self.network_progress.register(context.waker());
    }

    fn try_send_demand(&mut self, update: EgressDemandUpdate) -> Result<(), EgressDemandUpdate> {
        if let Err(AffineSpscTrySendError(update)) = self.demand_tx.try_send(update) {
            self.demand_send_blocked.store(true, Ordering::Release);
            #[cfg(feature = "tx-phase-telemetry")]
            self.telemetry.demand_full.fetch_add(1, Ordering::Relaxed);
            return Err(update);
        }
        #[cfg(feature = "tx-phase-telemetry")]
        self.telemetry
            .demand_publications
            .fetch_add(1, Ordering::Relaxed);
        self.request_radio_progress();
        Ok(())
    }

    fn request_radio_progress(&self) {
        self.radio_progress_pending.store(true, Ordering::Release);
        if self.radio_waiting.swap(false, Ordering::AcqRel) {
            #[cfg(feature = "tx-phase-telemetry")]
            self.telemetry.radio_wakes.fetch_add(1, Ordering::Relaxed);
            self.radio_progress.signal(());
        }
    }
}

/// Core1 owner of the demand outbox and its affine transport endpoint.
pub struct EgressNetworkScheduler<'control, 'state, M: RawMutex, const DEMAND_DEPTH: usize> {
    port: EgressNetworkPort<'control, M, DEMAND_DEPTH>,
    state: &'state mut EgressNetworkState<DEMAND_DEPTH>,
}

impl<'control, 'state, M: RawMutex, const DEMAND_DEPTH: usize>
    EgressNetworkScheduler<'control, 'state, M, DEMAND_DEPTH>
{
    pub fn new(
        port: EgressNetworkPort<'control, M, DEMAND_DEPTH>,
        state: &'state mut EgressNetworkState<DEMAND_DEPTH>,
    ) -> Self {
        Self { port, state }
    }

    pub(crate) fn update_egress_demand(
        &mut self,
        context: &Context<'_>,
        update: EgressDemandUpdate,
    ) -> Result<bool, EgressDemandStateError> {
        self.port.register_network_waker(context);
        self.state.demands.record(update)?;
        self.state.flush_pending = true;
        Ok(self.flush_egress_demand())
    }

    /// Advance an already-dirty outbox and return whether a retry is needed.
    ///
    /// The explicit bit keeps the ordinary `egress_schedule` observation O(1)
    /// after lifecycle state is synchronized. In particular it avoids
    /// rescanning every key in cached external state at network-poll frequency.
    pub(crate) fn flush_egress_demand(&mut self) -> bool {
        if !self.state.flush_pending {
            return false;
        }
        for _ in 0..DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET {
            let Some(update) = self.state.demands.next() else {
                self.state.flush_pending = false;
                return false;
            };
            if self.port.try_send_demand(update).is_err() {
                return true;
            }
            self.state.demands.commit(update);
        }
        self.state.flush_pending = self.state.demands.next().is_some();
        if self.state.flush_pending {
            self.port.network_progress.wake();
        }
        self.state.flush_pending
    }

    pub(crate) fn egress_demand_flush_pending(&self) -> bool {
        self.state.flush_pending
    }
}

/// Sole Core0 endpoint for demand consumption.
pub struct EgressRadioPort<'control, M: RawMutex, const DEMAND_DEPTH: usize> {
    demand_rx: AffineSpscReceiver<'control, EgressDemandUpdate, DEMAND_DEPTH>,
    radio_progress: &'control Signal<M, ()>,
    radio_progress_pending: &'control AtomicBool,
    radio_waiting: &'control AtomicBool,
    demand_send_blocked: &'control AtomicBool,
    network_progress: &'control GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: &'control EgressControlTelemetry,
}

/// Copyable level/wake edge shared with the immutable TX publication frontier.
pub struct EgressRadioWake<'control, M: RawMutex> {
    progress: &'control AtomicBool,
    signal: &'control Signal<M, ()>,
    waiting: &'control AtomicBool,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: &'control EgressControlTelemetry,
}

impl<M: RawMutex> Clone for EgressRadioWake<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for EgressRadioWake<'_, M> {}

impl<'control, M: RawMutex> EgressRadioWake<'control, M> {
    fn progress_pending(self) -> bool {
        self.progress.load(Ordering::Acquire)
    }

    fn progress_flag(self) -> &'control AtomicBool {
        self.progress
    }

    fn progress_signal(self) -> &'control Signal<M, ()> {
        self.signal
    }

    fn waiting_flag(self) -> &'control AtomicBool {
        self.waiting
    }

    #[cfg(feature = "tx-phase-telemetry")]
    fn record_service_cost(self, cost: TxPerformanceSample) {
        self.telemetry
            .radio_service_cycles
            .fetch_add(cost.cycles, Ordering::Relaxed);
        self.telemetry
            .radio_service_instructions
            .fetch_add(cost.instructions, Ordering::Relaxed);
    }
}

impl<'control, M: RawMutex, const DEMAND_DEPTH: usize> EgressRadioPort<'control, M, DEMAND_DEPTH> {
    pub const fn wake_handle(&self) -> EgressRadioWake<'control, M> {
        EgressRadioWake {
            progress: self.radio_progress_pending,
            signal: self.radio_progress,
            waiting: self.radio_waiting,
            #[cfg(feature = "tx-phase-telemetry")]
            telemetry: self.telemetry,
        }
    }

    fn try_receive_demand(&mut self) -> Option<EgressDemandUpdate> {
        let update = match self.demand_rx.try_receive() {
            Ok(update) => update,
            Err(AffineSpscTryReceiveError::Empty) => return None,
        };
        #[cfg(feature = "tx-phase-telemetry")]
        self.telemetry
            .radio_demand_updates
            .fetch_add(1, Ordering::Relaxed);
        if self.demand_send_blocked.swap(false, Ordering::AcqRel) {
            self.network_progress.wake();
        }
        Some(update)
    }

    pub async fn wait_progress(&self) {
        self.radio_progress.wait().await;
    }
}

/// Unique Core0 owner of one interface's mirrored egress demand.
pub struct EgressRadioScheduler<'control, M: RawMutex, const DEMAND_DEPTH: usize> {
    port: EgressRadioPort<'control, M, DEMAND_DEPTH>,
    demands: EgressRadioDemandState<DEMAND_DEPTH>,
}

impl<'control, M: RawMutex, const DEMAND_DEPTH: usize>
    EgressRadioScheduler<'control, M, DEMAND_DEPTH>
{
    pub const fn new(port: EgressRadioPort<'control, M, DEMAND_DEPTH>) -> Self {
        Self {
            port,
            demands: EgressRadioDemandState::new(),
        }
    }

    pub const fn wake_handle(&self) -> EgressRadioWake<'control, M> {
        self.port.wake_handle()
    }

    /// Consume one finite shadow demand turn without influencing TX admission.
    pub fn service_shadow(&mut self) -> bool {
        self.service_shadow_observed(|_| {})
    }

    /// Consume one finite turn and expose only transitions accepted by the
    /// radio-side lifecycle mirror.
    pub fn service_shadow_observed(&mut self, mut observe: impl FnMut(EgressDemandUpdate)) -> bool {
        let wake = self.port.wake_handle();
        let _ = wake.progress_signal().try_take();
        if !wake.progress_flag().swap(false, Ordering::AcqRel) {
            return false;
        }
        let (progressed, revisit) = self.service_shadow_turn(&mut observe);
        if revisit {
            wake.progress_flag().store(true, Ordering::Release);
        }
        progressed
    }

    fn service_shadow_turn(
        &mut self,
        observe: &mut impl FnMut(EgressDemandUpdate),
    ) -> (bool, bool) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.port
            .telemetry
            .radio_service_calls
            .fetch_add(1, Ordering::Relaxed);
        let mut progressed = false;
        for _ in 0..DEFAULT_EGRESS_RADIO_SERVICE_BUDGET {
            let Some(update) = self.port.try_receive_demand() else {
                return (progressed, false);
            };
            if !progressed {
                #[cfg(feature = "tx-phase-telemetry")]
                self.port
                    .telemetry
                    .radio_service_progressed
                    .fetch_add(1, Ordering::Relaxed);
            }
            progressed = true;
            if self.demands.apply(update).is_err() {
                #[cfg(feature = "tx-phase-telemetry")]
                self.port
                    .telemetry
                    .radio_demand_rejected
                    .fetch_add(1, Ordering::Relaxed);
            } else {
                observe(update);
            }
        }
        (progressed, true)
    }

    #[cfg(test)]
    fn active_demand_count(&self) -> usize {
        self.demands.active_count()
    }
}

/// Movable Core0 owner of radio egress policy and its idle wake state.
pub struct EgressRadioOwner<'control, M: RawMutex, const DEMAND_DEPTH: usize> {
    scheduler: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
    active: bool,
}

pin_project! {
    /// Wait for ordinary payload work or a level-latched demand edge.
    pub struct EgressWaitOr<'control, M: RawMutex, F> {
        wake: EgressRadioWake<'control, M>,
        active: bool,
        armed: bool,
        #[pin]
        payload: F,
    }

    impl<M: RawMutex, F> PinnedDrop for EgressWaitOr<'_, M, F> {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            if *this.armed {
                this.wake.waiting_flag().store(false, Ordering::Release);
            }
        }
    }
}

impl<M: RawMutex, F: Future<Output = ()>> Future for EgressWaitOr<'_, M, F> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if !*this.active {
            return this.payload.poll(context);
        }

        let wake = *this.wake;
        let disarm = |armed: &mut bool| {
            if *armed {
                wake.waiting_flag().store(false, Ordering::Release);
                *armed = false;
            }
        };

        if wake.progress_pending() {
            disarm(this.armed);
            return Poll::Ready(());
        }

        if !*this.armed {
            wake.waiting_flag().store(true, Ordering::Release);
            *this.armed = true;
            if wake.progress_pending() {
                disarm(this.armed);
                return Poll::Ready(());
            }
        }

        if this.payload.poll(context).is_ready() {
            disarm(this.armed);
            return Poll::Ready(());
        }

        let signal = wake.progress_signal().wait();
        let mut signal = core::pin::pin!(signal);
        if signal.as_mut().poll(context).is_ready() {
            disarm(this.armed);
            return Poll::Ready(());
        }

        Poll::Pending
    }
}

impl<'control, M: RawMutex, const DEMAND_DEPTH: usize> EgressRadioOwner<'control, M, DEMAND_DEPTH> {
    pub fn new(scheduler: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>) -> Self {
        Self {
            scheduler,
            active: egress_control_enabled(),
        }
    }

    pub fn service(&mut self) -> bool {
        self.service_observed(|_, _| {})
    }

    pub fn service_observed(&mut self, mut observe: impl FnMut(u8, EgressDemandUpdate)) -> bool {
        if !self.active {
            return false;
        }
        let wake = self.scheduler.wake_handle();
        if !wake.progress_pending() {
            return false;
        }
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let progressed = self
            .scheduler
            .service_shadow_observed(|update| observe(0, update));
        #[cfg(feature = "tx-phase-telemetry")]
        wake.record_service_cost(TxPerformanceSample::read().wrapping_delta_since(started));
        progressed
    }

    pub fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> EgressWaitOr<'control, M, F> {
        EgressWaitOr {
            wake: self.scheduler.wake_handle(),
            active: self.active,
            armed: false,
            payload,
        }
    }
}

/// One physical Core0 owner for two independent interface demand streams.
pub struct DualEgressRadioOwner<'control, M: RawMutex, const DEMAND_DEPTH: usize> {
    first: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
    second: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
    active: bool,
    second_first: bool,
}

impl<'control, M: RawMutex, const DEMAND_DEPTH: usize>
    DualEgressRadioOwner<'control, M, DEMAND_DEPTH>
{
    pub fn new(
        first: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
        second: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
    ) -> Self {
        let first_wake = first.wake_handle();
        let second_wake = second.wake_handle();
        assert!(
            core::ptr::eq(first_wake.progress_flag(), second_wake.progress_flag())
                && core::ptr::eq(first_wake.progress_signal(), second_wake.progress_signal())
                && core::ptr::eq(first_wake.waiting_flag(), second_wake.waiting_flag()),
            "dual egress owner requires one physical-radio wake domain"
        );
        Self {
            first,
            second,
            active: egress_control_enabled(),
            second_first: false,
        }
    }

    fn service_one(
        scheduler: &mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
        vif: u8,
        observe: &mut impl FnMut(u8, EgressDemandUpdate),
    ) -> (bool, bool) {
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let result = scheduler.service_shadow_turn(&mut |update| observe(vif, update));
        #[cfg(feature = "tx-phase-telemetry")]
        scheduler
            .wake_handle()
            .record_service_cost(TxPerformanceSample::read().wrapping_delta_since(started));
        result
    }

    pub fn service(&mut self) -> bool {
        self.service_observed(|_, _| {})
    }

    pub fn service_observed(&mut self, mut observe: impl FnMut(u8, EgressDemandUpdate)) -> bool {
        if !self.active {
            return false;
        }
        let wake = self.first.wake_handle();
        if !wake.progress_pending() {
            return false;
        }
        let _ = wake.progress_signal().try_take();
        if !wake.progress_flag().swap(false, Ordering::AcqRel) {
            return false;
        }

        let ((first_progressed, first_revisit), (second_progressed, second_revisit)) =
            if self.second_first {
                let second = Self::service_one(self.second, 1, &mut observe);
                let first = Self::service_one(self.first, 0, &mut observe);
                (first, second)
            } else {
                let first = Self::service_one(self.first, 0, &mut observe);
                let second = Self::service_one(self.second, 1, &mut observe);
                (first, second)
            };
        self.second_first = !self.second_first;
        if first_revisit || second_revisit {
            wake.progress_flag().store(true, Ordering::Release);
        }
        first_progressed || second_progressed
    }

    pub fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> EgressWaitOr<'control, M, F> {
        EgressWaitOr {
            wake: self.first.wake_handle(),
            active: self.active,
            armed: false,
            payload,
        }
    }
}

/// A permanent network frontier paired with the unique movable Core0 policy.
pub struct EgressControlledNetwork<N, R, P = ()> {
    inner: N,
    radio: R,
    policy: P,
}

pub trait EgressRadioControlOwner {
    fn service(&mut self) -> bool;

    fn service_observed(&mut self, observe: impl FnMut(u8, EgressDemandUpdate)) -> bool;

    fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()>;
}

impl<M: RawMutex, const DEMAND_DEPTH: usize> EgressRadioControlOwner
    for EgressRadioOwner<'_, M, DEMAND_DEPTH>
{
    fn service(&mut self) -> bool {
        EgressRadioOwner::service(self)
    }

    fn service_observed(&mut self, observe: impl FnMut(u8, EgressDemandUpdate)) -> bool {
        EgressRadioOwner::service_observed(self, observe)
    }

    fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()> {
        EgressRadioOwner::wait_or(self, payload)
    }
}

impl<M: RawMutex, const DEMAND_DEPTH: usize> EgressRadioControlOwner
    for DualEgressRadioOwner<'_, M, DEMAND_DEPTH>
{
    fn service(&mut self) -> bool {
        DualEgressRadioOwner::service(self)
    }

    fn service_observed(&mut self, observe: impl FnMut(u8, EgressDemandUpdate)) -> bool {
        DualEgressRadioOwner::service_observed(self, observe)
    }

    fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()> {
        DualEgressRadioOwner::wait_or(self, payload)
    }
}

impl<N, R> EgressControlledNetwork<N, R> {
    pub const fn new(inner: N, radio: R) -> Self {
        Self {
            inner,
            radio,
            policy: (),
        }
    }
}

impl<N, R, P> EgressControlledNetwork<N, R, P> {
    pub fn with_policy<Q>(self, policy: Q) -> EgressControlledNetwork<N, R, Q> {
        EgressControlledNetwork {
            inner: self.inner,
            radio: self.radio,
            policy,
        }
    }

    pub const fn inner(&self) -> &N {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut N {
        &mut self.inner
    }

    pub fn parts_mut(&mut self) -> (&mut N, &mut R, &mut P) {
        (&mut self.inner, &mut self.radio, &mut self.policy)
    }

    pub fn into_parts(self) -> (N, R, P) {
        (self.inner, self.radio, self.policy)
    }
}

impl<N, R: EgressRadioControlOwner, P> EgressControlledNetwork<N, R, P> {
    pub fn service_egress_control(&mut self) -> bool {
        self.radio.service()
    }

    pub fn service_egress_control_observed(
        &mut self,
        observe: impl FnMut(u8, EgressDemandUpdate),
    ) -> bool {
        self.radio.service_observed(observe)
    }

    pub fn wait_egress_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()> {
        self.radio.wait_or(payload)
    }
}

impl<'control, M: RawMutex, N, const DEMAND_DEPTH: usize>
    EgressControlledNetwork<N, EgressRadioOwner<'control, M, DEMAND_DEPTH>, ()>
{
    pub fn with_egress_control(
        inner: N,
        scheduler: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
    ) -> Self {
        Self::new(inner, EgressRadioOwner::new(scheduler))
    }
}

impl<'control, M: RawMutex, N, const DEMAND_DEPTH: usize>
    EgressControlledNetwork<N, DualEgressRadioOwner<'control, M, DEMAND_DEPTH>, ()>
{
    pub fn with_dual_egress_control(
        inner: N,
        first: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
        second: &'control mut EgressRadioScheduler<'control, M, DEMAND_DEPTH>,
    ) -> Self {
        Self::new(inner, DualEgressRadioOwner::new(first, second))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::{
        future::{pending, poll_fn, ready},
        num::{NonZeroU16, NonZeroU32},
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
    };
    use std::{boxed::Box, sync::Arc, task::Wake};

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;

    fn demand(epoch: u32, activation: u32, ready: u16) -> embassy_net_driver::EgressDemand {
        embassy_net_driver::EgressDemand::new(
            embassy_net_driver::EgressDemandId::new(epoch, NonZeroU32::new(activation).unwrap()),
            embassy_net_driver::EgressKey::from_words([1, 2, 3, 4]),
            embassy_net_driver::EgressDemandLevel::new(
                NonZeroU16::new(ready).unwrap(),
                ready >= 32,
            ),
        )
    }

    #[test]
    fn blocked_transport_replays_the_latest_demand_snapshot() {
        let control = EgressControlPlane::<NoopRawMutex, 1>::new();
        let (network, radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut radio = EgressRadioScheduler::new(radio);
        let context = Context::from_waker(Waker::noop());

        assert!(
            !network
                .update_egress_demand(&context, EgressDemandUpdate::Reset { schedule_epoch: 5 })
                .unwrap()
        );
        assert!(
            network
                .update_egress_demand(&context, EgressDemandUpdate::Active(demand(5, 1, 1)))
                .unwrap()
        );
        assert!(radio.service_shadow());
        assert_eq!(radio.active_demand_count(), 0);

        assert!(!network.flush_egress_demand());
        assert!(radio.service_shadow());
        assert_eq!(radio.active_demand_count(), 1);
        #[cfg(feature = "tx-phase-telemetry")]
        assert_eq!(
            control.snapshot(),
            EgressControlSnapshot {
                demand_publications: 2,
                demand_full: 1,
                radio_demand_updates: 2,
                radio_service_calls: 2,
                radio_service_progressed: 2,
                ..EgressControlSnapshot::default()
            }
        );
    }

    #[test]
    fn dual_owner_services_independent_vifs_from_one_physical_wake() {
        let first_control = EgressControlPlane::<NoopRawMutex, 4>::new();
        let second_control = EgressControlPlane::<NoopRawMutex, 4>::new();
        let shared_wake = first_control.shared_radio_wake();
        let (first_network, first_radio) = first_control.split();
        let (second_network, second_radio) = second_control.split_with_radio_wake(shared_wake);
        let mut first_state = EgressNetworkState::new();
        let mut second_state = EgressNetworkState::new();
        let mut first_network = EgressNetworkScheduler::new(first_network, &mut first_state);
        let mut second_network = EgressNetworkScheduler::new(second_network, &mut second_state);
        let first_radio = Box::leak(Box::new(EgressRadioScheduler::new(first_radio)));
        let second_radio = Box::leak(Box::new(EgressRadioScheduler::new(second_radio)));
        let context = Context::from_waker(Waker::noop());

        for network in [&mut first_network, &mut second_network] {
            network
                .update_egress_demand(&context, EgressDemandUpdate::Reset { schedule_epoch: 6 })
                .unwrap();
            network
                .update_egress_demand(&context, EgressDemandUpdate::Active(demand(6, 1, 1)))
                .unwrap();
        }

        let mut owner = DualEgressRadioOwner::new(first_radio, second_radio);
        let mut observed = std::vec::Vec::new();
        assert!(owner.service_observed(|vif, update| observed.push((vif, update))));
        assert_eq!(
            observed
                .iter()
                .map(|(vif, _)| *vif)
                .collect::<std::vec::Vec<_>>(),
            [0, 0, 1, 1]
        );
        assert!(matches!(
            observed[0].1,
            EgressDemandUpdate::Reset { schedule_epoch: 6 }
        ));
        assert!(matches!(observed[1].1, EgressDemandUpdate::Active(_)));
        assert!(matches!(
            observed[2].1,
            EgressDemandUpdate::Reset { schedule_epoch: 6 }
        ));
        assert!(matches!(observed[3].1, EgressDemandUpdate::Active(_)));
        assert!(!owner.service());
        #[cfg(feature = "tx-phase-telemetry")]
        {
            assert_eq!(first_control.snapshot().radio_demand_updates, 2);
            assert_eq!(second_control.snapshot().radio_demand_updates, 2);
            assert_eq!(first_control.snapshot().radio_service_progressed, 1);
            assert_eq!(second_control.snapshot().radio_service_progressed, 1);
        }
    }

    #[test]
    fn observer_never_sees_a_transition_rejected_by_the_radio_mirror() {
        let control = EgressControlPlane::<NoopRawMutex, 4>::new();
        let (mut network, radio) = control.split();
        let mut radio = EgressRadioScheduler::new(radio);

        network
            .try_send_demand(EgressDemandUpdate::Active(demand(7, 1, 1)))
            .unwrap();
        let mut observed = std::vec::Vec::new();
        assert!(radio.service_shadow_observed(|update| observed.push(update)));
        assert!(observed.is_empty());
        assert_eq!(radio.active_demand_count(), 0);

        network
            .try_send_demand(EgressDemandUpdate::Reset { schedule_epoch: 7 })
            .unwrap();
        network
            .try_send_demand(EgressDemandUpdate::Active(demand(7, 2, 32)))
            .unwrap();
        assert!(radio.service_shadow_observed(|update| observed.push(update)));
        assert_eq!(observed.len(), 2);
        assert!(matches!(
            observed[0],
            EgressDemandUpdate::Reset { schedule_epoch: 7 }
        ));
        assert!(matches!(observed[1], EgressDemandUpdate::Active(_)));
        assert_eq!(radio.active_demand_count(), 1);
    }

    #[derive(Default)]
    struct WakeCount(AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn radio_wait_observes_a_level_published_before_its_first_poll() {
        let control = Box::leak(Box::new(EgressControlPlane::<NoopRawMutex, 1>::new()));
        let (mut network, radio) = control.split();
        network
            .try_send_demand(EgressDemandUpdate::Reset { schedule_epoch: 1 })
            .unwrap();
        let scheduler = Box::leak(Box::new(EgressRadioScheduler::new(radio)));
        let owner = EgressRadioOwner::new(scheduler);
        let mut wait = pin!(owner.wait_or(pending::<()>()));

        assert!(
            wait.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready()
        );
        assert!(!control.radio_waiting.load(Ordering::Acquire));
    }

    #[test]
    fn radio_wait_cannot_lose_a_publication_after_arming() {
        let control = Box::leak(Box::new(EgressControlPlane::<NoopRawMutex, 1>::new()));
        let (mut network, radio) = control.split();
        let scheduler = Box::leak(Box::new(EgressRadioScheduler::new(radio)));
        let owner = EgressRadioOwner::new(scheduler);
        let wakes = Arc::new(WakeCount::default());
        let waker = Waker::from(wakes.clone());
        let mut published = false;
        let publish_after_arm = poll_fn(|_| {
            if !published {
                network
                    .try_send_demand(EgressDemandUpdate::Reset { schedule_epoch: 1 })
                    .unwrap();
                published = true;
            }
            Poll::<()>::Pending
        });
        let mut wait = pin!(owner.wait_or(publish_after_arm));

        assert!(
            wait.as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_ready()
        );
        assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
        assert!(!control.radio_waiting.load(Ordering::Acquire));
    }

    #[test]
    fn radio_wait_disarms_on_payload_completion_and_cancellation() {
        let control = Box::leak(Box::new(EgressControlPlane::<NoopRawMutex, 1>::new()));
        let (_network, radio) = control.split();
        let scheduler = Box::leak(Box::new(EgressRadioScheduler::new(radio)));
        let owner = EgressRadioOwner::new(scheduler);
        let context = &mut Context::from_waker(Waker::noop());

        {
            let mut payload_ready = pin!(owner.wait_or(ready(())));
            assert!(payload_ready.as_mut().poll(context).is_ready());
            assert!(!control.radio_waiting.load(Ordering::Acquire));
        }

        {
            let mut cancelled = pin!(owner.wait_or(pending::<()>()));
            assert!(cancelled.as_mut().poll(context).is_pending());
            assert!(control.radio_waiting.load(Ordering::Acquire));
        }
        assert!(!control.radio_waiting.load(Ordering::Acquire));
    }

    #[cfg(feature = "tx-phase-telemetry")]
    #[test]
    fn snapshot_delta_wraps_each_counter() {
        let earlier = EgressControlSnapshot {
            demand_publications: u32::MAX,
            radio_service_cycles: u32::MAX - 1,
            ..EgressControlSnapshot::default()
        };
        let current = EgressControlSnapshot {
            demand_publications: 2,
            radio_service_cycles: 3,
            ..EgressControlSnapshot::default()
        };
        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.demand_publications, 3);
        assert_eq!(delta.radio_service_cycles, 5);
    }
}
