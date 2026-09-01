//! Affine cross-core control plane for radio egress scheduling.
//!
//! Packet payload and DMA credits never enter these queues. Core1 publishes a
//! bounded, generation-bound demand value before final SRAM admission. Core0
//! returns a grant for that exact candidate. The two SPSC streams have
//! independent capacity and wake edges so neither core has to poll a shared
//! allocator or wait while holding a hardware obligation.

#[cfg(feature = "tx-phase-telemetry")]
use core::sync::atomic::AtomicU32;
use core::{
    future::Future,
    num::NonZeroU8,
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

#[cfg(feature = "tx-phase-telemetry")]
use crate::tx_performance::TxPerformanceSample;
use crate::{
    EgressGrantKey,
    egress_demand::{EgressDemandOutbox, EgressDemandStateError, EgressRadioDemandState},
};

#[cfg(feature = "tx-egress-diagnostic-switch")]
static EGRESS_CONTROL_ENABLED: AtomicBool = AtomicBool::new(true);

/// Select the disabled same-ELF control for intrusive HIL accounting.
///
/// This is a startup policy. It must be configured before the network- and
/// radio-side egress schedulers are attached to their permanent owners; both
/// snapshot the value so neither packet admission nor radio scheduling needs
/// an inter-core atomic load or memory fence merely to observe an immutable
/// diagnostic mode.
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

/// One bounded control entry per currently visible AP peer.
///
/// This is a control-plane horizon, not packet storage and not a promise that
/// every associated peer owns a DMA credit. A full candidate set applies
/// backpressure to further publications without changing packet admission.
pub const DEFAULT_EGRESS_CONTROL_DEPTH: usize = 16;
/// Maximum candidate decisions in one Core0 hardware-idle turn.
///
/// The producer runs concurrently on Core1. A drain-until-empty loop can
/// therefore livelock under saturated traffic even though both SPSC queues
/// are bounded: each returned BA-sized grant lets Core1 publish a successor
/// before Core0 observes an empty frontier.
pub const DEFAULT_EGRESS_RADIO_SERVICE_BUDGET: usize = 4;
/// Maximum grant decisions consumed in one Core1 admission call.
pub const DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET: usize = 4;

pub type DefaultEgressControlPlane<M> =
    EgressControlPlane<M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressNetworkPort<'control, M> =
    EgressNetworkPort<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressRadioPort<'control, M> =
    EgressRadioPort<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressRadioScheduler<'control, M> =
    EgressRadioScheduler<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressRadioOwner<'control, M> =
    EgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultDualEgressRadioOwner<'control, M> =
    DualEgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressControlledNetwork<'control, M, N> = EgressControlledNetwork<
    N,
    EgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>,
>;
pub type DefaultDualEgressControlledNetwork<'control, M, N> = EgressControlledNetwork<
    N,
    DualEgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>,
>;
pub type DefaultEgressRadioWake<'control, M> = EgressRadioWake<'control, M>;
pub type DefaultEgressNetworkState =
    EgressNetworkState<DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>;
pub type DefaultEgressNetworkScheduler<'resources, M> = EgressNetworkScheduler<
    'resources,
    'resources,
    M,
    DEFAULT_EGRESS_CONTROL_DEPTH,
    DEFAULT_EGRESS_CONTROL_DEPTH,
>;

#[cfg(feature = "tx-phase-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressControlSnapshot {
    pub demand_publications: u32,
    pub demand_full: u32,
    pub radio_demand_updates: u32,
    pub radio_demand_rejected: u32,
    pub candidate_publications: u32,
    pub candidate_full: u32,
    pub grants_received: u32,
    pub grants_accepted: u32,
    pub grants_rejected: u32,
    pub grant_credits_spent: u32,
    pub admissions_without_grant: u32,
    pub radio_wakes: u32,
    pub radio_candidates: u32,
    pub grant_publications: u32,
    pub grant_full: u32,
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
            candidate_publications: self
                .candidate_publications
                .wrapping_sub(earlier.candidate_publications),
            candidate_full: self.candidate_full.wrapping_sub(earlier.candidate_full),
            grants_received: self.grants_received.wrapping_sub(earlier.grants_received),
            grants_accepted: self.grants_accepted.wrapping_sub(earlier.grants_accepted),
            grants_rejected: self.grants_rejected.wrapping_sub(earlier.grants_rejected),
            grant_credits_spent: self
                .grant_credits_spent
                .wrapping_sub(earlier.grant_credits_spent),
            admissions_without_grant: self
                .admissions_without_grant
                .wrapping_sub(earlier.admissions_without_grant),
            radio_wakes: self.radio_wakes.wrapping_sub(earlier.radio_wakes),
            radio_candidates: self.radio_candidates.wrapping_sub(earlier.radio_candidates),
            grant_publications: self
                .grant_publications
                .wrapping_sub(earlier.grant_publications),
            grant_full: self.grant_full.wrapping_sub(earlier.grant_full),
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
    candidate_publications: AtomicU32,
    candidate_full: AtomicU32,
    grants_received: AtomicU32,
    grants_accepted: AtomicU32,
    grants_rejected: AtomicU32,
    grant_credits_spent: AtomicU32,
    admissions_without_grant: AtomicU32,
    radio_wakes: AtomicU32,
    radio_candidates: AtomicU32,
    grant_publications: AtomicU32,
    grant_full: AtomicU32,
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
            candidate_publications: AtomicU32::new(0),
            candidate_full: AtomicU32::new(0),
            grants_received: AtomicU32::new(0),
            grants_accepted: AtomicU32::new(0),
            grants_rejected: AtomicU32::new(0),
            grant_credits_spent: AtomicU32::new(0),
            admissions_without_grant: AtomicU32::new(0),
            radio_wakes: AtomicU32::new(0),
            radio_candidates: AtomicU32::new(0),
            grant_publications: AtomicU32::new(0),
            grant_full: AtomicU32::new(0),
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
            candidate_publications: self.candidate_publications.load(Ordering::Acquire),
            candidate_full: self.candidate_full.load(Ordering::Acquire),
            grants_received: self.grants_received.load(Ordering::Acquire),
            grants_accepted: self.grants_accepted.load(Ordering::Acquire),
            grants_rejected: self.grants_rejected.load(Ordering::Acquire),
            grant_credits_spent: self.grant_credits_spent.load(Ordering::Acquire),
            admissions_without_grant: self.admissions_without_grant.load(Ordering::Acquire),
            radio_wakes: self.radio_wakes.load(Ordering::Acquire),
            radio_candidates: self.radio_candidates.load(Ordering::Acquire),
            grant_publications: self.grant_publications.load(Ordering::Acquire),
            grant_full: self.grant_full.load(Ordering::Acquire),
            radio_service_calls: self.radio_service_calls.load(Ordering::Acquire),
            radio_service_progressed: self.radio_service_progressed.load(Ordering::Acquire),
            radio_service_cycles: self.radio_service_cycles.load(Ordering::Acquire),
            radio_service_instructions: self.radio_service_instructions.load(Ordering::Acquire),
        }
    }
}

/// One early stack-side demand publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressCandidate {
    serial: u32,
    key: EgressGrantKey,
    requested_frames: NonZeroU8,
    immediate_progress: bool,
}

impl EgressCandidate {
    pub const fn new(serial: u32, key: EgressGrantKey, requested_frames: NonZeroU8) -> Self {
        assert!(serial != 0, "egress candidate serial must be non-zero");
        Self {
            serial,
            key,
            requested_frames,
            immediate_progress: true,
        }
    }

    pub const fn with_immediate_progress(mut self, immediate_progress: bool) -> Self {
        self.immediate_progress = immediate_progress;
        self
    }

    pub const fn serial(self) -> u32 {
        self.serial
    }

    pub const fn key(self) -> EgressGrantKey {
        self.key
    }

    pub const fn requested_frames(self) -> NonZeroU8 {
        self.requested_frames
    }

    pub const fn requires_immediate_progress(self) -> bool {
        self.immediate_progress
    }
}

/// One Core0 decision bound to an exact candidate publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressGrant {
    candidate_serial: u32,
    key: EgressGrantKey,
    frame_credits: NonZeroU8,
}

impl EgressGrant {
    pub const fn new(candidate_serial: u32, key: EgressGrantKey, frame_credits: NonZeroU8) -> Self {
        assert!(
            candidate_serial != 0,
            "egress grant candidate serial must be non-zero"
        );
        Self {
            candidate_serial,
            key,
            frame_credits,
        }
    }

    pub const fn candidate_serial(self) -> u32 {
        self.candidate_serial
    }

    pub const fn key(self) -> EgressGrantKey {
        self.key
    }

    pub const fn frame_credits(self) -> NonZeroU8 {
        self.frame_credits
    }
}

#[derive(Debug)]
pub struct EgressCandidateFull(pub EgressCandidate);

#[derive(Debug)]
pub struct EgressGrantFull(pub EgressGrant);

#[derive(Clone, Copy)]
struct NetworkGrant {
    grant: EgressGrant,
    remaining: u16,
}

/// Core1-local credit for one contiguous Xarxa egress run.
///
/// The bounded tables and cross-core transport stay in
/// [`EgressNetworkScheduler`], but the packet-frequency state lives beside the
/// network device's current egress key. Spending a healthy lease therefore
/// neither follows the scheduler's external-state pointer nor touches either
/// SPSC queue. Only run changes and refill boundaries enter the cold control
/// plane.
pub(crate) struct EgressBurstLease {
    key: Option<EgressGrantKey>,
    grant_slot: Option<u8>,
    pending_slot: Option<u8>,
    direct_grant: Option<EgressGrant>,
    remaining: u16,
    maintenance_at: u16,
    requested_frames: u8,
    refill_pending: bool,
    #[cfg(feature = "tx-phase-telemetry")]
    accounted_remaining: u16,
    #[cfg(feature = "tx-phase-telemetry")]
    unreported_admissions_without_grant: u8,
}

impl EgressBurstLease {
    pub(crate) const fn new() -> Self {
        Self {
            key: None,
            grant_slot: None,
            pending_slot: None,
            direct_grant: None,
            remaining: 0,
            maintenance_at: 0,
            requested_frames: 0,
            refill_pending: false,
            #[cfg(feature = "tx-phase-telemetry")]
            accounted_remaining: 0,
            #[cfg(feature = "tx-phase-telemetry")]
            unreported_admissions_without_grant: 0,
        }
    }

    /// Spend one credit after the corresponding SRAM slot was acquired.
    ///
    /// `true` requests a cold maintenance pass before this synchronous driver
    /// call returns. It is not an admission decision while the control plane
    /// remains in shadow mode.
    #[inline(always)]
    pub(crate) fn commit_admission(&mut self) -> bool {
        if self.remaining == 0 {
            #[cfg(feature = "tx-phase-telemetry")]
            {
                self.unreported_admissions_without_grant = self
                    .unreported_admissions_without_grant
                    .saturating_add(u8::from(self.key.is_some()));
            }
            return self.key.is_some();
        }

        self.remaining -= 1;
        self.remaining == self.maintenance_at
    }

    #[cfg(test)]
    const fn remaining(&self) -> u16 {
        self.remaining
    }

    #[cfg(test)]
    fn needs_maintenance(&self) -> bool {
        self.key.is_some() && self.remaining == self.maintenance_at
    }

    fn maintain_at_remaining(&mut self, remaining: u16) {
        debug_assert!(remaining <= self.remaining);
        self.maintenance_at = remaining;
    }

    fn arm_retry(&mut self) {
        self.maintenance_at = self.remaining.saturating_sub(1);
    }
}

/// Core1-only protocol state kept outside recoverable async device owners.
///
/// The sole [`EgressNetworkScheduler`] holds a unique mutable reference, so
/// ordinary packet admission uses no lock or atomic state transition. Static
/// allocation prevents these bounded tables from being copied into every
/// enum/future which temporarily owns the permanent network device.
pub struct EgressNetworkState<const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> {
    demands: EgressDemandOutbox<CANDIDATE_DEPTH>,
    next_candidate_serial: u32,
    pending: [Option<EgressCandidate>; CANDIDATE_DEPTH],
    granted: [Option<NetworkGrant>; GRANT_DEPTH],
    cached_grant_slot: Option<u8>,
}

impl<const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressNetworkState<CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            demands: EgressDemandOutbox::new(),
            next_candidate_serial: 1,
            pending: [None; CANDIDATE_DEPTH],
            granted: [None; GRANT_DEPTH],
            cached_grant_slot: None,
        }
    }
}

impl<const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> Default
    for EgressNetworkState<CANDIDATE_DEPTH, GRANT_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Static storage for lifecycle demand, burst candidates, grants and wakes.
pub struct EgressControlPlane<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> {
    demands: AffineSpscQueue<EgressDemandUpdate, CANDIDATE_DEPTH>,
    candidates: AffineSpscQueue<EgressCandidate, CANDIDATE_DEPTH>,
    grants: AffineSpscQueue<EgressGrant, GRANT_DEPTH>,
    radio_progress: Signal<M, ()>,
    radio_progress_pending: AtomicBool,
    radio_waiting: AtomicBool,
    demand_send_blocked: AtomicBool,
    grant_send_blocked: AtomicBool,
    network_progress: GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: EgressControlTelemetry,
}

/// Radio wake shared by all logical-interface control streams owned by one
/// physical Wi-Fi datapath.
///
/// Every interface retains independent affine demand/grant queues. Sharing
/// only this level-triggered edge lets the sole Core0 owner wait once and then
/// service all VIFs fairly without introducing an MPSC queue or shared mutable
/// scheduler state.
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

impl<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressControlPlane<M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            demands: AffineSpscQueue::new(),
            candidates: AffineSpscQueue::new(),
            grants: AffineSpscQueue::new(),
            radio_progress: Signal::new(),
            radio_progress_pending: AtomicBool::new(false),
            radio_waiting: AtomicBool::new(false),
            demand_send_blocked: AtomicBool::new(false),
            grant_send_blocked: AtomicBool::new(false),
            network_progress: GenericAtomicWaker::new(M::INIT),
            #[cfg(feature = "tx-phase-telemetry")]
            telemetry: EgressControlTelemetry::new(),
        }
    }

    /// Acquire the sole Core1 and Core0 endpoints for the physical radio.
    pub fn split(
        &self,
    ) -> (
        EgressNetworkPort<'_, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
        EgressRadioPort<'_, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
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

    /// Split an independent interface stream which wakes an existing physical
    /// radio owner.
    pub fn split_with_radio_wake<'control>(
        &'control self,
        wake: EgressSharedRadioWake<'control, M>,
    ) -> (
        EgressNetworkPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
        EgressRadioPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) {
        let (demand_tx, demand_rx) = self.demands.split();
        let (candidate_tx, candidate_rx) = self.candidates.split();
        let (grant_tx, grant_rx) = self.grants.split();
        (
            EgressNetworkPort {
                demand_tx,
                candidate_tx,
                grant_rx,
                radio_progress: wake.signal,
                radio_progress_pending: wake.progress,
                radio_waiting: wake.waiting,
                demand_send_blocked: &self.demand_send_blocked,
                grant_send_blocked: &self.grant_send_blocked,
                network_progress: &self.network_progress,
                #[cfg(feature = "tx-phase-telemetry")]
                telemetry: &self.telemetry,
            },
            EgressRadioPort {
                demand_rx,
                candidate_rx,
                grant_tx,
                radio_progress: wake.signal,
                radio_progress_pending: wake.progress,
                radio_waiting: wake.waiting,
                demand_send_blocked: &self.demand_send_blocked,
                grant_send_blocked: &self.grant_send_blocked,
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

impl<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> Default
    for EgressControlPlane<M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Sole Core1 endpoint: publish candidates and consume grants.
pub struct EgressNetworkPort<
    'control,
    M: RawMutex,
    const CANDIDATE_DEPTH: usize,
    const GRANT_DEPTH: usize,
> {
    demand_tx: AffineSpscSender<'control, EgressDemandUpdate, CANDIDATE_DEPTH>,
    candidate_tx: AffineSpscSender<'control, EgressCandidate, CANDIDATE_DEPTH>,
    grant_rx: AffineSpscReceiver<'control, EgressGrant, GRANT_DEPTH>,
    radio_progress: &'control Signal<M, ()>,
    radio_progress_pending: &'control AtomicBool,
    radio_waiting: &'control AtomicBool,
    demand_send_blocked: &'control AtomicBool,
    grant_send_blocked: &'control AtomicBool,
    network_progress: &'control GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: &'control EgressControlTelemetry,
}

impl<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressNetworkPort<'_, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    /// Register the executor wake used for a new grant or candidate capacity.
    pub fn register_network_waker(&self, context: &Context<'_>) {
        self.network_progress.register(context.waker());
    }

    pub fn try_send_candidate(
        &mut self,
        candidate: EgressCandidate,
    ) -> Result<(), EgressCandidateFull> {
        if let Err(AffineSpscTrySendError(candidate)) = self.candidate_tx.try_send(candidate) {
            #[cfg(feature = "tx-phase-telemetry")]
            self.telemetry
                .candidate_full
                .fetch_add(1, Ordering::Relaxed);
            return Err(EgressCandidateFull(candidate));
        }
        #[cfg(feature = "tx-phase-telemetry")]
        self.telemetry
            .candidate_publications
            .fetch_add(1, Ordering::Relaxed);
        self.radio_progress_pending.store(true, Ordering::Release);
        if candidate.requires_immediate_progress() {
            self.request_radio_progress();
        }
        Ok(())
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

    pub fn try_receive_grant(&mut self) -> Option<EgressGrant> {
        let grant = self.grant_rx.try_receive().ok()?;
        #[cfg(feature = "tx-phase-telemetry")]
        self.telemetry
            .grants_received
            .fetch_add(1, Ordering::Relaxed);
        // Only a producer which actually observed a full reverse queue needs
        // another Core0 turn. Ordinary grant consumption must not create an
        // empty radio service pass for every granted burst.
        if self.grant_send_blocked.swap(false, Ordering::AcqRel) {
            self.request_radio_progress();
        }
        Some(grant)
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

/// Core1 egress protocol owner: one affine transport plus external state.
pub struct EgressNetworkScheduler<
    'control,
    'state,
    M: RawMutex,
    const CANDIDATE_DEPTH: usize,
    const GRANT_DEPTH: usize,
> {
    port: EgressNetworkPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    state: &'state mut EgressNetworkState<CANDIDATE_DEPTH, GRANT_DEPTH>,
}

impl<'control, 'state, M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressNetworkScheduler<'control, 'state, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub fn new(
        port: EgressNetworkPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
        state: &'state mut EgressNetworkState<CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) -> Self {
        Self { port, state }
    }

    /// Record one stack-owned lifecycle transition and advance its lossless
    /// bounded stream towards Core0.
    pub(crate) fn update_egress_demand(
        &mut self,
        context: &Context<'_>,
        update: EgressDemandUpdate,
    ) -> Result<(), EgressDemandStateError> {
        self.port.register_network_waker(context);
        self.state.demands.record(update)?;
        self.flush_egress_demand();
        Ok(())
    }

    /// Retry a previously blocked lifecycle suffix without touching packet
    /// admission or the run/refill lease.
    pub(crate) fn flush_egress_demand(&mut self) {
        for _ in 0..DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET {
            let Some(update) = self.state.demands.next() else {
                return;
            };
            if self.port.try_send_demand(update).is_err() {
                return;
            }
            self.state.demands.commit(update);
        }
        // A successful finite turn can leave a long reset/rekey suffix while
        // the SPSC still has capacity. Schedule another Core1 poll explicitly;
        // a genuinely full queue returns above and is woken only when Core0
        // frees a slot, so this cannot spin against radio backpressure.
        if self.state.demands.next().is_some() {
            self.port.network_progress.wake();
        }
    }

    /// Select one affine Core1-local grant at a Xarxa egress-run boundary.
    ///
    /// The selected slot remains active until the stack changes key or spends
    /// its final credit. Ordinary frames in the same run therefore do not
    /// search or revalidate the bounded grant table. This call is synchronous:
    /// no scheduler or epoch transition can occur between selecting the slot,
    /// claiming SRAM and [`EgressBurstLease::commit_admission`].
    pub(crate) fn begin_active_run(
        &mut self,
        lease: &mut EgressBurstLease,
        context: &Context<'_>,
        key: EgressGrantKey,
        requested_frames: NonZeroU8,
    ) {
        self.release_active_grant(lease);
        lease.key = Some(key);
        lease.grant_slot = None;
        lease.direct_grant = None;
        lease.remaining = 0;
        lease.maintenance_at = 0;
        lease.requested_frames = requested_frames.get();
        lease.pending_slot = self
            .matching_pending_slot(key)
            .and_then(|slot| u8::try_from(slot).ok());
        lease.refill_pending = lease.pending_slot.is_some();
        self.maintain_active_run(lease, context);
    }

    /// Stop spending the prior key while retaining its unused bounded grant.
    pub(crate) fn end_active_run(&mut self, lease: &mut EgressBurstLease) {
        self.release_active_grant(lease);
        lease.key = None;
        lease.grant_slot = None;
        lease.pending_slot = None;
        lease.direct_grant = None;
        lease.remaining = 0;
        lease.maintenance_at = 0;
        lease.requested_frames = 0;
        lease.refill_pending = false;
    }

    /// Refresh grants and publish at most one successor for the active burst.
    #[inline(never)]
    pub(crate) fn maintain_active_run(
        &mut self,
        lease: &mut EgressBurstLease,
        context: &Context<'_>,
    ) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.flush_active_telemetry(lease);
        let Some(key) = lease.key else {
            return;
        };
        let requested_frames = NonZeroU8::new(lease.requested_frames)
            .expect("active egress run retains a non-zero request");
        let refill_threshold = u16::from(requested_frames.get() / 4).max(1);

        self.port.register_network_waker(context);
        self.drain_grants(Some(lease));
        let matching_grant = if lease.direct_grant.is_some() && lease.grant_slot.is_none() {
            // A directly accepted active grant proves that this key did not
            // need an inactive table slot. Keep its subsequent packet and
            // refill boundaries independent of the cold multi-key table.
            None
        } else {
            self.matching_grant(key)
        };
        if let Some((slot, grant)) = matching_grant {
            lease.grant_slot = u8::try_from(slot).ok();
            lease.remaining = lease.remaining.saturating_add(grant.remaining);
            #[cfg(feature = "tx-phase-telemetry")]
            {
                lease.accounted_remaining =
                    lease.accounted_remaining.saturating_add(grant.remaining);
            }
            self.state.granted[slot] = Some(NetworkGrant {
                grant: grant.grant,
                remaining: 0,
            });
            if grant.remaining != 0 {
                lease.refill_pending = false;
                lease.pending_slot = None;
            }
        }
        let remaining = lease.remaining;

        if remaining > refill_threshold {
            lease.maintain_at_remaining(refill_threshold);
            return;
        }
        if lease.refill_pending {
            lease.maintain_at_remaining(0);
            if remaining == 0 {
                self.port.request_radio_progress();
            }
            return;
        }
        if let Some(slot) = self.matching_pending_slot(key) {
            lease.refill_pending = true;
            lease.pending_slot = u8::try_from(slot).ok();
            lease.maintain_at_remaining(0);
            if remaining == 0 {
                self.port.request_radio_progress();
            }
            return;
        }
        let Some(slot) = self.state.pending.iter().position(Option::is_none) else {
            lease.arm_retry();
            return;
        };
        let serial = self.state.next_candidate_serial;
        let candidate = EgressCandidate::new(serial, key, requested_frames)
            .with_immediate_progress(remaining == 0);
        if self.port.try_send_candidate(candidate).is_ok() {
            self.state.pending[slot] = Some(candidate);
            lease.refill_pending = true;
            lease.pending_slot = u8::try_from(slot).ok();
            self.state.next_candidate_serial = serial
                .checked_add(1)
                .filter(|serial| *serial != 0)
                .expect("egress candidate serial is not reusable");
        }
        if lease.refill_pending {
            lease.maintain_at_remaining(0);
        } else {
            lease.arm_retry();
        }
    }

    /// Revoke local epoch state while retaining the affine queue endpoints.
    ///
    /// Grants already in flight are harmless: without a matching pending
    /// serial they are discarded by the next drain.
    pub(crate) fn reset_epoch(&mut self, lease: &mut EgressBurstLease) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.flush_active_telemetry(lease);
        *lease = EgressBurstLease::new();
        self.state.pending.fill(None);
        self.state.granted.fill(None);
        self.state.cached_grant_slot = None;
        self.drain_grants(None);
    }

    pub fn pending_candidates(&self) -> usize {
        self.state.pending.iter().flatten().count()
    }

    #[cfg(test)]
    fn granted_keys(&self, lease: &EgressBurstLease) -> usize {
        let active_slot = lease.grant_slot.map(usize::from);
        let inactive = self
            .state
            .granted
            .iter()
            .enumerate()
            .filter_map(|(slot, grant)| grant.as_ref().map(|grant| (slot, grant)))
            .filter(|(slot, grant)| Some(*slot) != active_slot && grant.remaining != 0)
            .count();
        let active = lease.key.is_some()
            && (lease.remaining != 0
                || active_slot.is_some_and(|slot| {
                    self.state.granted[slot].is_some_and(|grant| grant.remaining != 0)
                }));
        inactive + usize::from(active)
    }

    /// Return a partially spent active lease to its bounded key slot.
    #[inline(never)]
    fn release_active_grant(&mut self, lease: &mut EgressBurstLease) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.flush_active_telemetry(lease);
        if let Some(slot) = lease.grant_slot.map(usize::from) {
            if let Some(mut grant) = self.state.granted.get(slot).copied().flatten() {
                grant.remaining = grant.remaining.saturating_add(lease.remaining);
                if grant.remaining == 0 {
                    self.state.granted[slot] = None;
                    if self.state.cached_grant_slot == u8::try_from(slot).ok() {
                        self.state.cached_grant_slot = None;
                    }
                } else {
                    self.state.granted[slot] = Some(grant);
                }
            }
        } else if lease.remaining != 0
            && let Some(grant) = lease.direct_grant
        {
            // A grant accepted directly into the active run deliberately has
            // no table slot on the saturated path. Preserve its unused credit
            // only when the stack changes key, where one bounded table search
            // is both cold and semantically necessary.
            if let Some((slot, existing)) = self.matching_grant(grant.key()) {
                self.state.granted[slot] = Some(NetworkGrant {
                    grant,
                    remaining: existing.remaining.saturating_add(lease.remaining),
                });
            } else if let Some(slot) = self.state.granted.iter().position(Option::is_none) {
                self.state.granted[slot] = Some(NetworkGrant {
                    grant,
                    remaining: lease.remaining,
                });
                self.state.cached_grant_slot = u8::try_from(slot).ok();
            }
        }
        lease.grant_slot = None;
        lease.pending_slot = None;
        lease.direct_grant = None;
        lease.remaining = 0;
        lease.maintenance_at = 0;
        #[cfg(feature = "tx-phase-telemetry")]
        {
            lease.accounted_remaining = 0;
        }
    }

    fn drain_grants(&mut self, mut active: Option<&mut EgressBurstLease>) {
        for _ in 0..DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET {
            let Some(grant) = self.port.try_receive_grant() else {
                return;
            };

            // The common saturated path already knows the exact candidate
            // slot. Accept its reply directly into the Core1-local burst lease
            // instead of scanning pending, occupied-grant and free-grant
            // tables once per aggregate. Inactive keys retain the bounded
            // table path below.
            if let Some(lease) = active.as_deref_mut()
                && lease.key == Some(grant.key())
                && let Some(pending_slot) = lease.pending_slot.map(usize::from)
                && self
                    .state
                    .pending
                    .get(pending_slot)
                    .is_some_and(|candidate| {
                        candidate.is_some_and(|candidate| {
                            candidate.key() == grant.key()
                                && candidate.serial() == grant.candidate_serial()
                        })
                    })
            {
                self.state.pending[pending_slot] = None;
                lease.pending_slot = None;
                lease.refill_pending = false;
                lease.direct_grant = Some(grant);
                let credits = u16::from(grant.frame_credits().get());
                lease.remaining = lease.remaining.saturating_add(credits);
                #[cfg(feature = "tx-phase-telemetry")]
                {
                    lease.accounted_remaining = lease.accounted_remaining.saturating_add(credits);
                    self.port
                        .telemetry
                        .grants_accepted
                        .fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            let Some(pending_slot) = self.matching_pending_slot(grant.key()).filter(|slot| {
                self.state.pending[*slot]
                    .is_some_and(|candidate| candidate.serial() == grant.candidate_serial())
            }) else {
                #[cfg(feature = "tx-phase-telemetry")]
                self.port
                    .telemetry
                    .grants_rejected
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            };
            self.state.pending[pending_slot] = None;
            #[cfg(feature = "tx-phase-telemetry")]
            self.port
                .telemetry
                .grants_accepted
                .fetch_add(1, Ordering::Relaxed);

            if let Some((slot, _)) = self.matching_grant(grant.key()) {
                let existing = self.state.granted[slot].expect("cached occupied grant slot");
                self.state.granted[slot] = Some(NetworkGrant {
                    grant,
                    remaining: existing
                        .remaining
                        .saturating_add(u16::from(grant.frame_credits().get())),
                });
                continue;
            }
            if let Some(slot) = self.state.granted.iter().position(Option::is_none) {
                self.state.granted[slot] = Some(NetworkGrant {
                    grant,
                    remaining: u16::from(grant.frame_credits().get()),
                });
                self.state.cached_grant_slot = u8::try_from(slot).ok();
            }
        }
    }

    fn matching_pending_slot(&self, key: EgressGrantKey) -> Option<usize> {
        self.state
            .pending
            .iter()
            .position(|candidate| candidate.is_some_and(|candidate| candidate.key() == key))
    }

    /// Return one coherent local copy of the matching grant.
    ///
    /// The table may live in cached external memory. Returning the copied
    /// record prevents the healthy per-frame path from repeatedly loading the
    /// same slot merely to read its remaining credit and publication serial.
    #[inline(always)]
    fn matching_grant(&mut self, key: EgressGrantKey) -> Option<(usize, NetworkGrant)> {
        if let Some(slot) = self.state.cached_grant_slot.map(usize::from)
            && let Some(grant) = self.state.granted.get(slot).copied().flatten()
            && grant.grant.key() == key
        {
            return Some((slot, grant));
        }
        self.find_and_cache_matching_grant(key)
    }

    /// Scan the inactive-key table only after the one-entry hot cache misses.
    ///
    /// Keeping this cold path out of [`Self::matching_grant`] is material on a
    /// target which executes ordinary text from cached PSRAM: a saturated
    /// single-peer run must not fetch or retire the 16-entry search machinery
    /// for every packet merely because the compiler declined to inline the
    /// combined helper.
    #[inline(never)]
    fn find_and_cache_matching_grant(
        &mut self,
        key: EgressGrantKey,
    ) -> Option<(usize, NetworkGrant)> {
        let match_ = self
            .state
            .granted
            .iter()
            .copied()
            .enumerate()
            .find_map(|(slot, grant)| {
                grant
                    .filter(|grant| grant.grant.key() == key)
                    .map(|grant| (slot, grant))
            });
        self.state.cached_grant_slot = match_.and_then(|(slot, _)| u8::try_from(slot).ok());
        match_
    }

    /// Publish packet-frequency diagnostic accounting only on a cold control
    /// boundary. The hot lease uses ordinary Core1-local bytes and never
    /// updates a cross-core-visible atomic per packet.
    #[cfg(feature = "tx-phase-telemetry")]
    #[inline(never)]
    fn flush_active_telemetry(&mut self, lease: &mut EgressBurstLease) {
        let credits = lease.accounted_remaining.saturating_sub(lease.remaining);
        if credits != 0 {
            self.port
                .telemetry
                .grant_credits_spent
                .fetch_add(u32::from(credits), Ordering::Relaxed);
            lease.accounted_remaining = lease.remaining;
        }
        let ungranted = lease.unreported_admissions_without_grant;
        if ungranted != 0 {
            self.port
                .telemetry
                .admissions_without_grant
                .fetch_add(u32::from(ungranted), Ordering::Relaxed);
            lease.unreported_admissions_without_grant = 0;
        }
    }
}

/// Sole Core0 endpoint: consume candidates and publish grants.
pub struct EgressRadioPort<
    'control,
    M: RawMutex,
    const CANDIDATE_DEPTH: usize,
    const GRANT_DEPTH: usize,
> {
    demand_rx: AffineSpscReceiver<'control, EgressDemandUpdate, CANDIDATE_DEPTH>,
    candidate_rx: AffineSpscReceiver<'control, EgressCandidate, CANDIDATE_DEPTH>,
    grant_tx: AffineSpscSender<'control, EgressGrant, GRANT_DEPTH>,
    radio_progress: &'control Signal<M, ()>,
    radio_progress_pending: &'control AtomicBool,
    radio_waiting: &'control AtomicBool,
    demand_send_blocked: &'control AtomicBool,
    grant_send_blocked: &'control AtomicBool,
    network_progress: &'control GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: &'control EgressControlTelemetry,
}

/// Copyable level/wake edge shared with the immutable network TX frontier.
///
/// The handle cannot consume a candidate or publish a grant. It only exposes
/// whether finite Core0 service is required and the event used while the
/// datapath is otherwise idle.
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
    pub(crate) fn progress_pending(self) -> bool {
        self.progress.load(Ordering::Acquire)
    }

    pub(crate) fn progress_flag(self) -> &'control AtomicBool {
        self.progress
    }

    pub(crate) fn progress_signal(self) -> &'control Signal<M, ()> {
        self.signal
    }

    pub(crate) fn waiting_flag(self) -> &'control AtomicBool {
        self.waiting
    }

    #[cfg(feature = "tx-phase-telemetry")]
    pub(crate) fn record_service_cost(self, cost: TxPerformanceSample) {
        self.telemetry
            .radio_service_cycles
            .fetch_add(cost.cycles, Ordering::Relaxed);
        self.telemetry
            .radio_service_instructions
            .fetch_add(cost.instructions, Ordering::Relaxed);
    }
}

/// Unique Core0 owner of the affine candidate/grant endpoint.
///
/// This diagnostic scheduler deliberately echoes the requested bounded
/// credit and cannot yet authorize real packet admission. Keeping it outside
/// the shared network runner is the ownership shape required by the eventual
/// role-aware peer-generation and airtime policy.
pub struct EgressRadioScheduler<
    'control,
    M: RawMutex,
    const CANDIDATE_DEPTH: usize,
    const GRANT_DEPTH: usize,
> {
    port: EgressRadioPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    demands: EgressRadioDemandState<CANDIDATE_DEPTH>,
    deferred: Option<EgressCandidate>,
}

impl<'control, M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub const fn new(port: EgressRadioPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>) -> Self {
        Self {
            port,
            demands: EgressRadioDemandState::new(),
            deferred: None,
        }
    }

    pub const fn wake_handle(&self) -> EgressRadioWake<'control, M> {
        self.port.wake_handle()
    }

    /// Mirror lifecycle demand and echo every currently visible candidate
    /// without influencing TX.
    ///
    /// The deferred slot preserves the exact candidate if the reverse SPSC
    /// is full. No radio or DMA ownership survives this synchronous call.
    pub fn service_shadow(&mut self) -> bool {
        let wake = self.port.wake_handle();
        // Consume the coalesced edge before the flag. A publication racing
        // after this point sets both again and remains visible to the next
        // hardware-idle scheduler boundary.
        let _ = wake.progress_signal().try_take();
        if !wake.progress_flag().swap(false, Ordering::AcqRel) {
            return false;
        }
        let (progressed, revisit) = self.service_shadow_turn();
        if revisit {
            wake.progress_flag().store(true, Ordering::Release);
        }
        progressed
    }

    /// Execute one finite interface-local turn after the physical owner has
    /// consumed the shared wake level.
    fn service_shadow_turn(&mut self) -> (bool, bool) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.port
            .telemetry
            .radio_service_calls
            .fetch_add(1, Ordering::Relaxed);
        let mut progressed = false;
        for _ in 0..DEFAULT_EGRESS_RADIO_SERVICE_BUDGET {
            if let Some(update) = self.port.try_receive_demand() {
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
                }
                continue;
            }
            let candidate = match self.deferred.take() {
                Some(candidate) => candidate,
                None => match self.port.try_receive_candidate() {
                    Some(candidate) => candidate,
                    None => return (progressed, false),
                },
            };
            if !progressed {
                #[cfg(feature = "tx-phase-telemetry")]
                self.port
                    .telemetry
                    .radio_service_progressed
                    .fetch_add(1, Ordering::Relaxed);
            }
            progressed = true;
            let grant = EgressGrant::new(
                candidate.serial(),
                candidate.key(),
                candidate.requested_frames(),
            );
            if self.port.try_send_grant(grant).is_err() {
                self.deferred = Some(candidate);
                return (progressed, false);
            }
        }
        // The budget was exhausted. The caller must revisit even if the last
        // consumed item happened to empty the queue; one bounded empty pass is
        // preferable to an unsynchronized frontier probe racing Core1.
        (progressed, true)
    }

    #[cfg(test)]
    fn active_demand_count(&self) -> usize {
        self.demands.active_count()
    }
}

/// Sole movable Core0 owner of radio egress policy and its idle wake state.
///
/// The scheduler is held through one affine mutable reference rather than
/// behind interior mutability. Consequently a radio control decision requires
/// `&mut self` and cannot be made by a shared network handle or on Core1. The
/// immutable wake handle is only a level-triggered input to the ordinary
/// network-publication wait.
pub struct EgressRadioOwner<
    'control,
    M: RawMutex,
    const CANDIDATE_DEPTH: usize,
    const GRANT_DEPTH: usize,
> {
    scheduler: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    active: bool,
}

pin_project! {
    /// Compact wait for either ordinary payload work or a latched Core0
    /// egress-control edge.
    ///
    /// The payload future remains structurally pinned, while the control side
    /// stores only its three-pointer wake capability and two state bits. A
    /// custom future avoids retaining the generic `select` state machine in
    /// every connected datapath branch.
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

        // A publication before the first poll is a level, not an edge. Avoid
        // arming the signal waiter when the owner already owes service.
        if wake.progress_pending() {
            disarm(this.armed);
            return Poll::Ready(());
        }

        if !*this.armed {
            wake.waiting_flag().store(true, Ordering::Release);
            *this.armed = true;

            // A producer may have observed the old false waiter state between
            // the first check and arm. Rechecking closes that lost-wake window.
            if wake.progress_pending() {
                disarm(this.armed);
                return Poll::Ready(());
            }
        }

        if this.payload.poll(context).is_ready() {
            disarm(this.armed);
            return Poll::Ready(());
        }

        // Signal::wait is cancel-safe and stores the current task waker in the
        // signal itself. The temporary future therefore needs no retained
        // state beyond this poll call.
        let signal = wake.progress_signal().wait();
        let mut signal = core::pin::pin!(signal);
        if signal.as_mut().poll(context).is_ready() {
            disarm(this.armed);
            return Poll::Ready(());
        }

        Poll::Pending
    }
}

impl<'control, M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressRadioOwner<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub fn new(
        scheduler: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) -> Self {
        Self {
            scheduler,
            active: egress_control_enabled(),
        }
    }

    /// Execute one finite control turn at a Core0 scheduling boundary.
    pub fn service(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let wake = self.scheduler.wake_handle();
        if !wake.progress_pending() {
            return false;
        }
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let progressed = self.scheduler.service_shadow();
        #[cfg(feature = "tx-phase-telemetry")]
        wake.record_service_cost(TxPerformanceSample::read().wrapping_delta_since(started));
        progressed
    }

    /// Wait for either payload publication or a level-latched control edge.
    ///
    /// This function never services policy. The unique Core0 caller does so
    /// after the wait returns, preserving one obvious mutable owner while the
    /// check/arm/recheck sequence prevents a candidate wake from being lost.
    pub fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> EgressWaitOr<'control, M, F> {
        EgressWaitOr {
            wake: self.scheduler.wake_handle(),
            active: self.active,
            armed: false,
            payload,
        }
    }
}

/// One physical Core0 owner for two independent logical-interface streams.
///
/// STA and AP keep separate SPSC queues, outboxes, grants and telemetry. The
/// physical owner shares only their level-triggered wake and alternates the
/// first serviced VIF at every turn so a continuously active interface cannot
/// starve sparse traffic on the other one.
pub struct DualEgressRadioOwner<
    'control,
    M: RawMutex,
    const CANDIDATE_DEPTH: usize,
    const GRANT_DEPTH: usize,
> {
    first: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    second: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    active: bool,
    second_first: bool,
}

impl<'control, M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    DualEgressRadioOwner<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub fn new(
        first: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
        second: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
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
        scheduler: &mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) -> (bool, bool) {
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let result = scheduler.service_shadow_turn();
        #[cfg(feature = "tx-phase-telemetry")]
        scheduler
            .wake_handle()
            .record_service_cost(TxPerformanceSample::read().wrapping_delta_since(started));
        result
    }

    pub fn service(&mut self) -> bool {
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
                let second = Self::service_one(self.second);
                let first = Self::service_one(self.first);
                (first, second)
            } else {
                let first = Self::service_one(self.first);
                let second = Self::service_one(self.second);
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
///
/// `inner` may itself be a shared reference to static DMA/network resources;
/// only the radio policy is affine and follows the active datapath lifecycle.
pub struct EgressControlledNetwork<N, R> {
    inner: N,
    radio: R,
}

/// Core0 policy capability required by a controlled physical network.
pub trait EgressRadioControlOwner {
    fn service(&mut self) -> bool;

    fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()>;
}

impl<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> EgressRadioControlOwner
    for EgressRadioOwner<'_, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    fn service(&mut self) -> bool {
        EgressRadioOwner::service(self)
    }

    fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()> {
        EgressRadioOwner::wait_or(self, payload)
    }
}

impl<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> EgressRadioControlOwner
    for DualEgressRadioOwner<'_, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    fn service(&mut self) -> bool {
        DualEgressRadioOwner::service(self)
    }

    fn wait_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()> {
        DualEgressRadioOwner::wait_or(self, payload)
    }
}

impl<N, R> EgressControlledNetwork<N, R> {
    pub const fn new(inner: N, radio: R) -> Self {
        Self { inner, radio }
    }

    pub const fn inner(&self) -> &N {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut N {
        &mut self.inner
    }

    pub fn into_parts(self) -> (N, R) {
        (self.inner, self.radio)
    }
}

impl<N, R: EgressRadioControlOwner> EgressControlledNetwork<N, R> {
    pub fn service_egress_control(&mut self) -> bool {
        self.radio.service()
    }

    pub fn wait_egress_or<F: Future<Output = ()>>(&self, payload: F) -> impl Future<Output = ()> {
        self.radio.wait_or(payload)
    }
}

impl<'control, M: RawMutex, N, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressControlledNetwork<N, EgressRadioOwner<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>>
{
    pub fn with_egress_control(
        inner: N,
        scheduler: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) -> Self {
        Self::new(inner, EgressRadioOwner::new(scheduler))
    }
}

impl<'control, M: RawMutex, N, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressControlledNetwork<N, DualEgressRadioOwner<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>>
{
    pub fn with_dual_egress_control(
        inner: N,
        first: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
        second: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) -> Self {
        Self::new(inner, DualEgressRadioOwner::new(first, second))
    }
}

impl<'control, M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressRadioPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub const fn wake_handle(&self) -> EgressRadioWake<'control, M> {
        EgressRadioWake {
            progress: self.radio_progress_pending,
            signal: self.radio_progress,
            waiting: self.radio_waiting,
            #[cfg(feature = "tx-phase-telemetry")]
            telemetry: self.telemetry,
        }
    }

    pub fn try_receive_candidate(&mut self) -> Option<EgressCandidate> {
        let candidate = match self.candidate_rx.try_receive() {
            Ok(candidate) => candidate,
            Err(AffineSpscTryReceiveError::Empty) => return None,
        };
        #[cfg(feature = "tx-phase-telemetry")]
        self.telemetry
            .radio_candidates
            .fetch_add(1, Ordering::Relaxed);
        // A full candidate queue makes Core1 temporarily unable to publish a
        // new active key. Freeing one slot must wake the network poll.
        self.network_progress.wake();
        Some(candidate)
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

    pub fn try_send_grant(&mut self, grant: EgressGrant) -> Result<(), EgressGrantFull> {
        if let Err(AffineSpscTrySendError(grant)) = self.grant_tx.try_send(grant) {
            self.grant_send_blocked.store(true, Ordering::Release);
            #[cfg(feature = "tx-phase-telemetry")]
            self.telemetry.grant_full.fetch_add(1, Ordering::Relaxed);
            return Err(EgressGrantFull(grant));
        }
        #[cfg(feature = "tx-phase-telemetry")]
        self.telemetry
            .grant_publications
            .fetch_add(1, Ordering::Relaxed);
        self.network_progress.wake();
        Ok(())
    }

    /// Wait for a candidate publication or newly available grant capacity.
    pub async fn wait_progress(&self) {
        self.radio_progress.wait().await;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::{
        future::{pending, poll_fn, ready},
        num::{NonZeroU8, NonZeroU16, NonZeroU32},
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
    };
    use std::{boxed::Box, sync::Arc, task::Wake};

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;

    fn key(slot: u8) -> EgressGrantKey {
        EgressGrantKey::new(
            1,
            NonZeroU8::new(slot).unwrap(),
            NonZeroU32::new(7).unwrap(),
            0,
        )
    }

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
    fn blocked_demand_transport_replays_the_latest_snapshot() {
        let control = EgressControlPlane::<NoopRawMutex, 1, 1>::new();
        let (network, radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut radio = EgressRadioScheduler::new(radio);
        let context = Context::from_waker(Waker::noop());

        network
            .update_egress_demand(&context, EgressDemandUpdate::Reset { schedule_epoch: 5 })
            .unwrap();
        network
            .update_egress_demand(&context, EgressDemandUpdate::Active(demand(5, 1, 1)))
            .unwrap();
        // The one-entry transport contains Reset. The Active observation is
        // retained in the desired/sent mirror rather than dropped.
        assert!(radio.service_shadow());
        assert_eq!(radio.active_demand_count(), 0);

        network.flush_egress_demand();
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
        let first_control = EgressControlPlane::<NoopRawMutex, 4, 4>::new();
        let second_control = EgressControlPlane::<NoopRawMutex, 4, 4>::new();
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
        assert!(owner.service());
        assert!(!owner.service());
        #[cfg(feature = "tx-phase-telemetry")]
        {
            assert_eq!(first_control.snapshot().radio_demand_updates, 2);
            assert_eq!(second_control.snapshot().radio_demand_updates, 2);
            assert_eq!(first_control.snapshot().radio_service_progressed, 1);
            assert_eq!(second_control.snapshot().radio_service_progressed, 1);
        }
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
        let control = Box::leak(Box::new(EgressControlPlane::<NoopRawMutex, 1, 1>::new()));
        let (mut network, radio) = control.split();
        network
            .try_send_candidate(EgressCandidate::new(1, key(1), NonZeroU8::new(1).unwrap()))
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
        let control = Box::leak(Box::new(EgressControlPlane::<NoopRawMutex, 1, 1>::new()));
        let (mut network, radio) = control.split();
        let scheduler = Box::leak(Box::new(EgressRadioScheduler::new(radio)));
        let mut owner = EgressRadioOwner::new(scheduler);
        let wakes = Arc::new(WakeCount::default());
        let waker = Waker::from(wakes.clone());
        let mut published = false;
        let publish_after_arm = poll_fn(|_| {
            if !published {
                network
                    .try_send_candidate(EgressCandidate::new(1, key(1), NonZeroU8::new(1).unwrap()))
                    .unwrap();
                published = true;
            }
            Poll::<()>::Pending
        });
        {
            let mut wait = pin!(owner.wait_or(publish_after_arm));
            assert!(
                wait.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_ready()
            );
        }
        assert_eq!(
            wakes.0.load(Ordering::Relaxed),
            0,
            "a signal consumed inside the publishing poll needs no executor wake"
        );
        assert!(!control.radio_waiting.load(Ordering::Acquire));

        assert!(owner.service());
        assert!(network.try_receive_grant().is_some());
        let mut wait = pin!(owner.wait_or(pending::<()>()));
        assert!(
            wait.as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        network
            .try_send_candidate(EgressCandidate::new(2, key(1), NonZeroU8::new(1).unwrap()))
            .unwrap();
        assert_eq!(
            wakes.0.load(Ordering::Relaxed),
            1,
            "a publication after Pending must wake the registered Core0 task"
        );
        assert!(
            wait.as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_ready()
        );
        assert!(!control.radio_waiting.load(Ordering::Acquire));
    }

    #[test]
    fn radio_wait_disarms_on_payload_completion_and_cancellation() {
        let control = Box::leak(Box::new(EgressControlPlane::<NoopRawMutex, 1, 1>::new()));
        let (_network, radio) = control.split();
        let scheduler = Box::leak(Box::new(EgressRadioScheduler::new(radio)));
        let owner = EgressRadioOwner::new(scheduler);
        let context = &mut Context::from_waker(Waker::noop());

        let mut payload_ready = pin!(owner.wait_or(ready(())));
        assert!(payload_ready.as_mut().poll(context).is_ready());
        assert!(!control.radio_waiting.load(Ordering::Acquire));
        drop(payload_ready);

        {
            let mut cancelled = pin!(owner.wait_or(pending::<()>()));
            assert!(cancelled.as_mut().poll(context).is_pending());
            assert!(control.radio_waiting.load(Ordering::Acquire));
        }
        assert!(!control.radio_waiting.load(Ordering::Acquire));
    }

    #[test]
    fn two_affine_streams_preserve_candidate_and_grant_identity() {
        let control = EgressControlPlane::<NoopRawMutex, 2, 1>::new();
        let (mut network, mut radio) = control.split();
        let candidate = EgressCandidate::new(1, key(2), NonZeroU8::new(32).unwrap());
        network.try_send_candidate(candidate).unwrap();
        assert_eq!(radio.try_receive_candidate(), Some(candidate));

        let grant = EgressGrant::new(
            candidate.serial(),
            candidate.key(),
            NonZeroU8::new(8).unwrap(),
        );
        radio.try_send_grant(grant).unwrap();
        assert_eq!(network.try_receive_grant(), Some(grant));
        assert_eq!(network.try_receive_grant(), None);
    }

    #[test]
    fn radio_service_is_finite_while_core1_keeps_candidates_visible() {
        const DEPTH: usize = DEFAULT_EGRESS_RADIO_SERVICE_BUDGET + 2;
        let control = EgressControlPlane::<NoopRawMutex, DEPTH, DEPTH>::new();
        let (mut network, radio) = control.split();
        let mut radio = EgressRadioScheduler::new(radio);
        for index in 0..DEPTH {
            let serial = u32::try_from(index + 1).unwrap();
            network
                .try_send_candidate(EgressCandidate::new(
                    serial,
                    key(u8::try_from(index + 1).unwrap()),
                    NonZeroU8::new(1).unwrap(),
                ))
                .unwrap();
        }

        assert!(radio.service_shadow());
        for serial in 1..=DEFAULT_EGRESS_RADIO_SERVICE_BUDGET {
            assert_eq!(
                network
                    .try_receive_grant()
                    .map(EgressGrant::candidate_serial),
                Some(u32::try_from(serial).unwrap())
            );
        }
        assert_eq!(network.try_receive_grant(), None);

        assert!(radio.service_shadow());
        for serial in (DEFAULT_EGRESS_RADIO_SERVICE_BUDGET + 1)..=DEPTH {
            assert_eq!(
                network
                    .try_receive_grant()
                    .map(EgressGrant::candidate_serial),
                Some(u32::try_from(serial).unwrap())
            );
        }
        assert_eq!(network.try_receive_grant(), None);
    }

    #[test]
    fn network_grant_drain_is_finite_while_core0_keeps_grants_visible() {
        const DEPTH: usize = DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET + 2;
        let control = EgressControlPlane::<NoopRawMutex, DEPTH, DEPTH>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());
        for index in 0..DEPTH {
            network.begin_active_run(
                &mut lease,
                &context,
                key(u8::try_from(index + 1).unwrap()),
                NonZeroU8::new(1).unwrap(),
            );
        }
        for _ in 0..DEPTH {
            let candidate = radio.try_receive_candidate().unwrap();
            radio
                .try_send_grant(EgressGrant::new(
                    candidate.serial(),
                    candidate.key(),
                    candidate.requested_frames(),
                ))
                .unwrap();
        }

        network.drain_grants(None);
        assert_eq!(
            network.granted_keys(&lease),
            DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET
        );
        network.drain_grants(None);
        assert_eq!(network.granted_keys(&lease), DEPTH);
    }

    #[test]
    fn bounded_full_paths_return_the_exact_unsent_value() {
        let control = EgressControlPlane::<NoopRawMutex, 1, 1>::new();
        let (mut network, mut radio) = control.split();
        let first = EgressCandidate::new(1, key(1), NonZeroU8::new(1).unwrap());
        let second = EgressCandidate::new(2, key(2), NonZeroU8::new(1).unwrap());
        network.try_send_candidate(first).unwrap();
        assert_eq!(network.try_send_candidate(second).unwrap_err().0, second);
        assert_eq!(radio.try_receive_candidate(), Some(first));

        let first = EgressGrant::new(1, key(1), NonZeroU8::new(1).unwrap());
        let second = EgressGrant::new(2, key(2), NonZeroU8::new(1).unwrap());
        radio.try_send_grant(first).unwrap();
        assert_eq!(radio.try_send_grant(second).unwrap_err().0, second);
        assert_eq!(network.try_receive_grant(), Some(first));
    }

    #[test]
    fn grant_consumption_requests_radio_progress_only_after_real_backpressure() {
        let control = EgressControlPlane::<NoopRawMutex, 1, 1>::new();
        let (mut network, mut radio) = control.split();
        let radio_wake = radio.wake_handle();
        let first = EgressGrant::new(1, key(1), NonZeroU8::new(1).unwrap());
        let blocked = EgressGrant::new(2, key(2), NonZeroU8::new(1).unwrap());

        radio.try_send_grant(first).unwrap();
        assert_eq!(network.try_receive_grant(), Some(first));
        assert!(!radio_wake.progress_pending());

        radio.try_send_grant(first).unwrap();
        assert_eq!(radio.try_send_grant(blocked).unwrap_err().0, blocked);
        assert!(!radio_wake.progress_pending());
        assert_eq!(network.try_receive_grant(), Some(first));
        assert!(radio_wake.progress_pending());

        assert!(radio.try_send_grant(blocked).is_ok());
        assert_eq!(network.try_receive_grant(), Some(blocked));
    }

    #[test]
    fn grant_and_candidate_capacity_edges_wake_core1() {
        let control = EgressControlPlane::<NoopRawMutex, 1, 1>::new();
        let (mut network, mut radio) = control.split();
        let wake_count = Arc::new(WakeCount::default());
        let waker = Waker::from(wake_count.clone());
        let context = Context::from_waker(&waker);
        network.register_network_waker(&context);

        network
            .try_send_candidate(EgressCandidate::new(1, key(1), NonZeroU8::new(1).unwrap()))
            .unwrap();
        assert!(radio.try_receive_candidate().is_some());
        radio
            .try_send_grant(EgressGrant::new(1, key(1), NonZeroU8::new(1).unwrap()))
            .unwrap();
        assert_eq!(wake_count.0.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn diagnostic_protocol_publishes_once_and_spends_only_after_success() {
        let control = EgressControlPlane::<NoopRawMutex, 2, 2>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());

        network.begin_active_run(&mut lease, &context, key(1), NonZeroU8::new(2).unwrap());
        network.begin_active_run(&mut lease, &context, key(1), NonZeroU8::new(2).unwrap());
        assert_eq!(network.pending_candidates(), 1);
        let candidate = radio.try_receive_candidate().unwrap();
        assert_eq!(candidate.key(), key(1));
        assert_eq!(candidate.requested_frames().get(), 2);
        assert_eq!(radio.try_receive_candidate(), None);

        radio
            .try_send_grant(EgressGrant::new(
                candidate.serial(),
                candidate.key(),
                candidate.requested_frames(),
            ))
            .unwrap();
        network.maintain_active_run(&mut lease, &context);
        assert_eq!(network.pending_candidates(), 0);
        assert_eq!(network.granted_keys(&lease), 1);
        assert_eq!(lease.remaining(), 2);
        assert!(lease.commit_admission());
        assert_eq!(lease.remaining(), 1);
        assert!(lease.needs_maintenance());
        network.maintain_active_run(&mut lease, &context);
        assert!(lease.commit_admission());
        assert_eq!(lease.remaining(), 0);
        assert!(lease.needs_maintenance());
        network.maintain_active_run(&mut lease, &context);
        assert!(lease.commit_admission());
        assert_eq!(network.granted_keys(&lease), 0);
        #[cfg(feature = "tx-phase-telemetry")]
        {
            network.reset_epoch(&mut lease);
            assert_eq!(
                control.snapshot(),
                EgressControlSnapshot {
                    candidate_publications: 2,
                    grants_received: 1,
                    grants_accepted: 1,
                    grant_credits_spent: 2,
                    admissions_without_grant: 1,
                    radio_candidates: 1,
                    grant_publications: 1,
                    ..EgressControlSnapshot::default()
                }
            );
        }
    }

    #[test]
    fn healthy_burst_uses_cached_credit_without_republishing_until_low_water() {
        let control = EgressControlPlane::<NoopRawMutex, 2, 2>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());
        let requested = NonZeroU8::new(32).unwrap();

        network.begin_active_run(&mut lease, &context, key(1), requested);
        assert!(lease.commit_admission());
        let candidate = radio.try_receive_candidate().unwrap();
        radio
            .try_send_grant(EgressGrant::new(
                candidate.serial(),
                candidate.key(),
                requested,
            ))
            .unwrap();

        // Maintenance models the work before a failed global SRAM claim. It
        // must not consume any of the newly accepted burst credit.
        network.maintain_active_run(&mut lease, &context);
        assert_eq!(lease.remaining(), 32);

        for index in 0..24 {
            assert!(!lease.needs_maintenance());
            assert_eq!(lease.commit_admission(), index == 23);
            assert_eq!(radio.try_receive_candidate(), None);
        }
        assert_eq!(lease.remaining(), 8);

        // The next cold maintenance pass crosses the 25% refill watermark. It
        // keeps the remaining local credit usable while publishing one
        // successor, then suppresses further scans until exhaustion.
        assert!(lease.needs_maintenance());
        network.maintain_active_run(&mut lease, &context);
        assert!(!lease.needs_maintenance());
        assert!(!lease.commit_admission());
        let refill = radio.try_receive_candidate().unwrap();
        assert_eq!(refill.key(), key(1));
        assert!(!refill.requires_immediate_progress());
        assert_eq!(radio.try_receive_candidate(), None);
    }

    #[test]
    fn active_grant_uses_no_inactive_table_slot_until_the_key_changes() {
        let control = EgressControlPlane::<NoopRawMutex, 4, 4>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());
        let requested = NonZeroU8::new(32).unwrap();

        network.begin_active_run(&mut lease, &context, key(1), requested);
        let candidate = radio.try_receive_candidate().unwrap();
        radio
            .try_send_grant(EgressGrant::new(
                candidate.serial(),
                candidate.key(),
                requested,
            ))
            .unwrap();
        network.maintain_active_run(&mut lease, &context);

        assert_eq!(lease.remaining(), 32);
        assert!(network.state.granted.iter().all(Option::is_none));

        for _ in 0..3 {
            assert!(!lease.commit_admission());
        }
        network.begin_active_run(&mut lease, &context, key(2), requested);

        assert_eq!(network.matching_grant(key(1)).unwrap().1.remaining, 29);
    }

    #[test]
    fn changing_runs_preserves_each_keys_unused_local_credit() {
        let control = EgressControlPlane::<NoopRawMutex, 4, 4>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());
        let requested = NonZeroU8::new(32).unwrap();

        network.begin_active_run(&mut lease, &context, key(1), requested);
        let first = radio.try_receive_candidate().unwrap();
        radio
            .try_send_grant(EgressGrant::new(first.serial(), first.key(), requested))
            .unwrap();
        network.maintain_active_run(&mut lease, &context);
        for _ in 0..3 {
            assert!(!lease.commit_admission());
        }

        network.begin_active_run(&mut lease, &context, key(2), requested);
        let second = radio.try_receive_candidate().unwrap();
        radio
            .try_send_grant(EgressGrant::new(second.serial(), second.key(), requested))
            .unwrap();
        network.maintain_active_run(&mut lease, &context);
        assert!(!lease.commit_admission());

        network.begin_active_run(&mut lease, &context, key(1), requested);
        assert_eq!(radio.try_receive_candidate(), None);
        assert!(!lease.commit_admission());
        assert_eq!(lease.remaining(), 28);
        assert_eq!(network.matching_grant(key(2)).unwrap().1.remaining, 31);
    }

    #[test]
    fn epoch_reset_revokes_the_active_grant_before_another_admission() {
        let control = EgressControlPlane::<NoopRawMutex, 2, 2>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());
        let requested = NonZeroU8::new(2).unwrap();

        network.begin_active_run(&mut lease, &context, key(1), requested);
        let candidate = radio.try_receive_candidate().unwrap();
        radio
            .try_send_grant(EgressGrant::new(
                candidate.serial(),
                candidate.key(),
                requested,
            ))
            .unwrap();
        network.maintain_active_run(&mut lease, &context);
        assert_eq!(network.granted_keys(&lease), 1);

        network.reset_epoch(&mut lease);
        assert!(!lease.commit_admission());
        assert_eq!(network.granted_keys(&lease), 0);
    }

    #[test]
    fn unmatched_grants_and_epoch_residue_fail_closed() {
        let control = EgressControlPlane::<NoopRawMutex, 2, 2>::new();
        let (network, mut radio) = control.split();
        let mut state = EgressNetworkState::new();
        let mut network = EgressNetworkScheduler::new(network, &mut state);
        let mut lease = EgressBurstLease::new();
        let context = Context::from_waker(Waker::noop());

        network.begin_active_run(&mut lease, &context, key(1), NonZeroU8::new(1).unwrap());
        let candidate = radio.try_receive_candidate().unwrap();
        radio
            .try_send_grant(EgressGrant::new(
                candidate.serial(),
                key(2),
                NonZeroU8::new(1).unwrap(),
            ))
            .unwrap();
        network.begin_active_run(&mut lease, &context, key(2), NonZeroU8::new(1).unwrap());
        assert!(lease.commit_admission());
        assert_eq!(network.pending_candidates(), 2);

        network.reset_epoch(&mut lease);
        radio
            .try_send_grant(EgressGrant::new(
                candidate.serial(),
                candidate.key(),
                NonZeroU8::new(1).unwrap(),
            ))
            .unwrap();
        network.drain_grants(None);
        assert!(!lease.commit_admission());
        assert_eq!(network.pending_candidates(), 0);
        assert_eq!(network.granted_keys(&lease), 0);
    }
}
