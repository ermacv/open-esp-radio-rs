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
    sync::atomic::{AtomicBool, Ordering},
    task::Context,
};

use embassy_futures::select::select;
use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal, waitqueue::GenericAtomicWaker};
use open_esp_radio_dma::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError,
};

use crate::EgressGrantKey;
#[cfg(feature = "tx-phase-telemetry")]
use crate::tx_performance::TxPerformanceSample;

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
pub type DefaultEgressControlledNetwork<'control, M, N> = EgressControlledNetwork<
    N,
    EgressRadioOwner<'control, M, DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_CONTROL_DEPTH>,
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

/// Static storage for the two one-way streams and their wake edges.
pub struct EgressControlPlane<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize> {
    candidates: AffineSpscQueue<EgressCandidate, CANDIDATE_DEPTH>,
    grants: AffineSpscQueue<EgressGrant, GRANT_DEPTH>,
    radio_progress: Signal<M, ()>,
    radio_progress_pending: AtomicBool,
    radio_waiting: AtomicBool,
    grant_send_blocked: AtomicBool,
    network_progress: GenericAtomicWaker<M>,
    #[cfg(feature = "tx-phase-telemetry")]
    telemetry: EgressControlTelemetry,
}

impl<M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressControlPlane<M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            candidates: AffineSpscQueue::new(),
            grants: AffineSpscQueue::new(),
            radio_progress: Signal::new(),
            radio_progress_pending: AtomicBool::new(false),
            radio_waiting: AtomicBool::new(false),
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
        let (candidate_tx, candidate_rx) = self.candidates.split();
        let (grant_tx, grant_rx) = self.grants.split();
        (
            EgressNetworkPort {
                candidate_tx,
                grant_rx,
                radio_progress: &self.radio_progress,
                radio_progress_pending: &self.radio_progress_pending,
                radio_waiting: &self.radio_waiting,
                grant_send_blocked: &self.grant_send_blocked,
                network_progress: &self.network_progress,
                #[cfg(feature = "tx-phase-telemetry")]
                telemetry: &self.telemetry,
            },
            EgressRadioPort {
                candidate_rx,
                grant_tx,
                radio_progress: &self.radio_progress,
                radio_progress_pending: &self.radio_progress_pending,
                radio_waiting: &self.radio_waiting,
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
    candidate_tx: AffineSpscSender<'control, EgressCandidate, CANDIDATE_DEPTH>,
    grant_rx: AffineSpscReceiver<'control, EgressGrant, GRANT_DEPTH>,
    radio_progress: &'control Signal<M, ()>,
    radio_progress_pending: &'control AtomicBool,
    radio_waiting: &'control AtomicBool,
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
        lease.remaining = 0;
        lease.maintenance_at = 0;
        lease.requested_frames = requested_frames.get();
        lease.refill_pending = self.matching_pending_slot(key).is_some();
        self.maintain_active_run(lease, context);
    }

    /// Stop spending the prior key while retaining its unused bounded grant.
    pub(crate) fn end_active_run(&mut self, lease: &mut EgressBurstLease) {
        self.release_active_grant(lease);
        lease.key = None;
        lease.grant_slot = None;
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
        self.drain_grants();
        let matching_grant = self.matching_grant(key);
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
        if self.matching_pending_slot(key).is_some() {
            lease.refill_pending = true;
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
        self.drain_grants();
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
        let active = active_slot.is_some_and(|slot| {
            lease.remaining != 0
                || self.state.granted[slot].is_some_and(|grant| grant.remaining != 0)
        });
        inactive + usize::from(active)
    }

    /// Return a partially spent active lease to its bounded key slot.
    #[inline(never)]
    fn release_active_grant(&mut self, lease: &mut EgressBurstLease) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.flush_active_telemetry(lease);
        let Some(slot) = lease.grant_slot.map(usize::from) else {
            return;
        };
        let Some(mut grant) = self.state.granted.get(slot).copied().flatten() else {
            return;
        };
        grant.remaining = grant.remaining.saturating_add(lease.remaining);
        if grant.remaining == 0 {
            self.state.granted[slot] = None;
            if self.state.cached_grant_slot == u8::try_from(slot).ok() {
                self.state.cached_grant_slot = None;
            }
        } else {
            self.state.granted[slot] = Some(grant);
        }
        lease.grant_slot = None;
        lease.remaining = 0;
        lease.maintenance_at = 0;
        #[cfg(feature = "tx-phase-telemetry")]
        {
            lease.accounted_remaining = 0;
        }
    }

    fn drain_grants(&mut self) {
        for _ in 0..DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET {
            let Some(grant) = self.port.try_receive_grant() else {
                return;
            };
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
    candidate_rx: AffineSpscReceiver<'control, EgressCandidate, CANDIDATE_DEPTH>,
    grant_tx: AffineSpscSender<'control, EgressGrant, GRANT_DEPTH>,
    radio_progress: &'control Signal<M, ()>,
    radio_progress_pending: &'control AtomicBool,
    radio_waiting: &'control AtomicBool,
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
    deferred: Option<EgressCandidate>,
}

impl<'control, M: RawMutex, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>
{
    pub const fn new(port: EgressRadioPort<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>) -> Self {
        Self {
            port,
            deferred: None,
        }
    }

    pub const fn wake_handle(&self) -> EgressRadioWake<'control, M> {
        self.port.wake_handle()
    }

    /// Echo every currently visible candidate without influencing TX.
    ///
    /// The deferred slot preserves the exact candidate if the reverse SPSC
    /// is full. No radio or DMA ownership survives this synchronous call.
    pub fn service_shadow(&mut self) -> bool {
        #[cfg(feature = "tx-phase-telemetry")]
        self.port
            .telemetry
            .radio_service_calls
            .fetch_add(1, Ordering::Relaxed);
        let wake = self.port.wake_handle();
        // Consume the coalesced edge before the flag. A publication racing
        // after this point sets both again and remains visible to the next
        // hardware-idle scheduler boundary.
        let _ = wake.progress_signal().try_take();
        if !wake.progress_flag().swap(false, Ordering::AcqRel) {
            return false;
        }
        let mut progressed = false;
        for _ in 0..DEFAULT_EGRESS_RADIO_SERVICE_BUDGET {
            let candidate = match self.deferred.take() {
                Some(candidate) => candidate,
                None => match self.port.try_receive_candidate() {
                    Some(candidate) => candidate,
                    None => return progressed,
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
                return progressed;
            }
        }
        // Work may remain, or Core1 may have replenished the queue while this
        // finite turn was running. Keep the coalesced edge visible so the
        // outer scheduler revisits us without retaining this call frame.
        wake.progress_flag().store(true, Ordering::Release);
        progressed
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
    pub async fn wait_or(&self, payload: impl Future<Output = ()>) {
        if !self.active {
            payload.await;
            return;
        }
        let wake = self.scheduler.wake_handle();
        let waiting = wake.waiting_flag();
        waiting.store(true, Ordering::Release);
        if wake.progress_flag().load(Ordering::Acquire) {
            waiting.store(false, Ordering::Release);
            return;
        }
        let guard = EgressProgressWaitGuard { waiting };
        let _ = select(payload, wake.progress_signal().wait()).await;
        drop(guard);
    }
}

struct EgressProgressWaitGuard<'control> {
    waiting: &'control AtomicBool,
}

impl Drop for EgressProgressWaitGuard<'_> {
    fn drop(&mut self) {
        self.waiting.store(false, Ordering::Release);
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

impl<'control, M: RawMutex, N, const CANDIDATE_DEPTH: usize, const GRANT_DEPTH: usize>
    EgressControlledNetwork<N, EgressRadioOwner<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>>
{
    pub fn with_egress_control(
        inner: N,
        scheduler: &'control mut EgressRadioScheduler<'control, M, CANDIDATE_DEPTH, GRANT_DEPTH>,
    ) -> Self {
        Self::new(inner, EgressRadioOwner::new(scheduler))
    }

    pub fn service_egress_control(&mut self) -> bool {
        self.radio.service()
    }

    pub async fn wait_egress_or(&self, payload: impl Future<Output = ()>) {
        self.radio.wait_or(payload).await;
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
        num::{NonZeroU8, NonZeroU32},
        sync::atomic::{AtomicUsize, Ordering},
        task::{Context, Waker},
    };
    use std::{sync::Arc, task::Wake};

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

        network.drain_grants();
        assert_eq!(
            network.granted_keys(&lease),
            DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET
        );
        network.drain_grants();
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
        network.drain_grants();
        assert!(!lease.commit_admission());
        assert_eq!(network.pending_candidates(), 0);
        assert_eq!(network.granted_keys(&lease), 0);
    }
}
