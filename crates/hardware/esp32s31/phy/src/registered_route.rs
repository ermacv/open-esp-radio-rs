//! One registered-epoch lifecycle shared by every protocol route.
//!
//! A [`PhyDomain`] is one registered PHY epoch: the target-issued
//! [`RegisteredPhyState`] and the client set that shares it. Every protocol
//! route couples a domain to its own physical owner. A route selects that physical owner, its owners before
//! and after acquiring its client, and the client it may acquire. The client
//! acquisition, tracking evaluation, pending tracking, fail-stop and release
//! frontiers below are written once and instantiated per route; each keeps
//! the physical owner, the registration proof and the client model together,
//! exactly as the former per-route copies did.
//!
//! The routes are sealed. Safe code cannot add a route or rebuild an owner
//! from separate parts:
//!
//! ```compile_fail
//! use oer_esp32s31_phy::registered_route::PhyRoute;
//! struct Forged;
//! impl PhyRoute for Forged {}
//! ```

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{
        PhyClientAcquireError, PhyClientAcquireFailure, PhyClientAcquireOrdering,
        PhyClientAcquireOutcome, PhyClientReleaseError, PhyClientReleaseFailure,
        PhyClientReleaseOutcome, PhyClientSnapshot, PhyClientState, PhyModemClient,
        PhyPendingTrack, PhyPendingTracking, PhyTrackEvaluation, PhyTrackEvaluationFailure,
        PhyTrackPoisoned, PhyTrackTimeError,
    },
    tracking::parameters::PhyParamTrackRequest,
};

pub(crate) mod sealed {
    use super::PhyDomain;
    use crate::state::client::PhyModemClient;

    pub trait PhyRoute {
        /// Physical owner coupled to the registration, or `()` when the
        /// protocol keeps its hardware outside PHY.
        type Hardware;
        /// Owner before this route's client is acquired.
        type Unclaimed;
        /// Owner while this route's client is held.
        type Client;
        /// The only client this route may acquire or release.
        const CLIENT: PhyModemClient;

        fn unclaimed(hardware: Self::Hardware, domain: PhyDomain) -> Self::Unclaimed;

        fn client(hardware: Self::Hardware, domain: PhyDomain) -> Self::Client;
    }
}

/// One registered PHY epoch: the target-issued calibration state and the
/// client set that shares it.
///
/// Every protocol owner holds exactly one domain beside its physical owner,
/// and every registration path mints one. The domain is neither `Clone` nor
/// constructible outside the crate, so its calibration state and client set
/// cannot be replaced or paired with another epoch.
///
/// ```compile_fail
/// use oer_esp32s31_phy::registered_route::PhyDomain;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<PhyDomain>();
/// ```
#[must_use = "a registered PHY domain is the unique owner of its epoch"]
pub struct PhyDomain {
    pub(crate) registered: RegisteredPhyState,
    pub(crate) clients: PhyClientState,
}

impl PhyDomain {
    pub(crate) const fn new(registered: RegisteredPhyState, clients: PhyClientState) -> Self {
        Self {
            registered,
            clients,
        }
    }

    /// Mint the domain of one completed target registration.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        let epoch = witness.epoch();
        Self::new(
            RegisteredPhyState::from_target_completion(state, witness),
            PhyClientState::for_registration(
                crate::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
                epoch,
            ),
        )
    }

    /// Borrow the registered calibration state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect the shared client set without exposing its raw mask.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Replace an older cache for this epoch with the currently committed
    /// semantic calibration state.
    ///
    /// Consuming the prior cache preserves the single-owner persistence
    /// contract. Its platform-derived identity is retained; the next cold
    /// registration still validates that identity against the physical chip.
    pub fn refresh_calibration_cache(
        &self,
        previous: crate::state::PhyCalibrationCache,
    ) -> crate::state::PhyCalibrationCache {
        self.registered
            .state()
            .calibration_cache(previous.identity())
    }

    /// Inspect registered-policy conditions without sampling temperature,
    /// advancing deadlines or acquiring RF. Values describe retained state,
    /// not a job plan.
    pub fn inspect_tracking(
        &self,
        now_micros: u64,
    ) -> Result<crate::tracking::inspection::Inspection, PhyTrackTimeError> {
        crate::tracking::inspection::Inspection::registered(
            &self.registered,
            self.client_snapshot(),
            now_micros,
        )
    }

    /// Wait for tracking demand without transferring the domain to a timer.
    ///
    /// Cancellation and timer errors leave the domain unchanged. The returned
    /// observation is not a hardware grant: obtain the required physical
    /// exclusion before invoking a consuming evaluation. No timestamp is
    /// refreshed here.
    pub async fn wait_for_tracking_demand(
        &self,
        timer: &mut impl crate::state::client::PhyTrackingTimer,
    ) -> Result<Option<crate::tracking::schedule::Demand>, PhyTrackTimeError> {
        use crate::tracking::schedule::Schedule;
        loop {
            match self
                .client_snapshot()
                .tracking_schedule_at(timer.now_micros())?
            {
                Schedule::Inactive => return Ok(None),
                Schedule::Due(demand) => return Ok(Some(demand)),
                Schedule::At(deadline) => timer.wait_until_micros(deadline).await,
            }
        }
    }
}

/// Closed set of protocol routes that own a registered PHY epoch.
pub trait PhyRoute: sealed::PhyRoute {}

/// Wi-Fi route: the registration stays coupled to the powered radio.
pub struct WifiRoute<P>(
    core::convert::Infallible,
    core::marker::PhantomData<fn() -> P>,
);

/// Bluetooth route: the Controller keeps its hardware outside PHY.
pub enum BluetoothRoute {}

impl<P> PhyRoute for WifiRoute<P> {}
impl PhyRoute for BluetoothRoute {}

type Hardware<R> = <R as sealed::PhyRoute>::Hardware;
type Unclaimed<R> = <R as sealed::PhyRoute>::Unclaimed;
type Client<R> = <R as sealed::PhyRoute>::Client;

fn client_owner<R: PhyRoute>(
    hardware: Hardware<R>,
    registered: RegisteredPhyState,
    clients: PhyClientState,
) -> Client<R> {
    <R as sealed::PhyRoute>::client(hardware, PhyDomain::new(registered, clients))
}

/// Acquire this route's client for one owner's parts.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure retains the physical owner, proof and client set"
)]
pub(crate) fn acquire<R: PhyRoute>(
    hardware: Hardware<R>,
    domain: PhyDomain,
    clock: &mut impl crate::state::client::PhyPllTrackClock,
) -> Result<PhyClientAcquire<R>, PhyClientAcquireFailureOwner<R>> {
    let PhyDomain {
        registered,
        clients,
    } = domain;
    match clients.acquire(<R as sealed::PhyRoute>::CLIENT, clock) {
        Ok(outcome) => Ok(PhyClientAcquire {
            hardware,
            registered,
            outcome,
        }),
        Err(failure) => Err(PhyClientAcquireFailureOwner {
            hardware,
            registered,
            failure,
        }),
    }
}

/// Release this route's client for one owner's parts.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure retains the physical owner, proof and client set"
)]
pub(crate) fn release<R: PhyRoute>(
    hardware: Hardware<R>,
    domain: PhyDomain,
) -> Result<PhyClientRelease<R>, PhyClientReleaseFailureOwner<R>> {
    let PhyDomain {
        registered,
        clients,
    } = domain;
    match clients.release(<R as sealed::PhyRoute>::CLIENT) {
        Ok(outcome) => Ok(PhyClientRelease {
            hardware,
            registered,
            outcome,
        }),
        Err(failure) => Err(PhyClientReleaseFailureOwner {
            hardware,
            registered,
            failure,
        }),
    }
}

/// Evaluate one periodic callback (`immediate == false`) or recheck a due
/// deadline after a wake (`immediate == true`) for one owner's parts.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free failure retains the physical owner, proof and client set"
)]
pub(crate) fn evaluate<R: PhyRoute>(
    hardware: Hardware<R>,
    domain: PhyDomain,
    clock: &mut impl crate::state::client::PhyPllTrackClock,
    immediate: bool,
) -> Result<PhyTrackEvaluationOwner<R>, PhyTrackEvaluationFailureOwner<R>> {
    let PhyDomain {
        registered,
        clients,
    } = domain;
    let result = if immediate {
        clients.evaluate_immediate_tracking(clock)
    } else {
        clients.evaluate_periodic_tracking(clock)
    };
    match result {
        Ok(evaluation) => Ok(PhyTrackEvaluationOwner {
            hardware,
            registered,
            evaluation,
        }),
        Err(failure) => Err(PhyTrackEvaluationFailureOwner {
            hardware,
            registered,
            failure,
        }),
    }
}

/// Successful client acquisition coupled to its registered epoch.
#[must_use = "client acquisition retains the registered PHY owner"]
pub struct PhyClientAcquire<R: PhyRoute> {
    hardware: Hardware<R>,
    registered: RegisteredPhyState,
    outcome: PhyClientAcquireOutcome,
}

impl<R: PhyRoute> PhyClientAcquire<R> {
    /// Client added by this transition.
    pub const fn client(&self) -> PhyModemClient {
        self.outcome.client()
    }

    /// Whether the client set was empty before this acquisition.
    pub const fn was_empty(&self) -> bool {
        self.outcome.was_empty()
    }

    /// Return the reviewed first/later-client ordering.
    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.outcome.ordering()
    }

    /// Borrow the immediate tracking request, when one is due.
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.outcome.request()
    }

    /// Recover the client owner only when no hardware tracking is due.
    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the allocation-free registered owner"
    )]
    pub fn into_owner(self) -> Result<Client<R>, PhyPendingTrackOwner<R>> {
        let Self {
            hardware,
            registered,
            outcome,
        } = self;
        match outcome.into_owner() {
            Ok(clients) => Ok(client_owner::<R>(hardware, registered, clients)),
            Err(pending) => Err(PhyPendingTrackOwner {
                hardware,
                registered,
                pending,
            }),
        }
    }
}

/// Rejected client acquisition retaining the unchanged registered owner.
#[must_use = "failed acquisition retains the registered PHY owner"]
pub struct PhyClientAcquireFailureOwner<R: PhyRoute> {
    hardware: Hardware<R>,
    registered: RegisteredPhyState,
    failure: PhyClientAcquireFailure,
}

impl<R: PhyRoute> PhyClientAcquireFailureOwner<R> {
    /// Inspect the exact source-owned client-set rejection.
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }

    /// Recover the unchanged pre-acquisition owner.
    pub fn into_owner(self) -> Unclaimed<R> {
        <R as sealed::PhyRoute>::unclaimed(
            self.hardware,
            PhyDomain::new(self.registered, self.failure.into_owner()),
        )
    }
}

/// Successful client release coupled to its registered epoch.
///
/// The route decides what may follow: a last release can lead toward RF
/// close, and a non-final release keeps the other clients running.
#[must_use = "client release retains the registered PHY owner"]
pub struct PhyClientRelease<R: PhyRoute> {
    pub(crate) hardware: Hardware<R>,
    pub(crate) registered: RegisteredPhyState,
    pub(crate) outcome: PhyClientReleaseOutcome,
}

impl<R: PhyRoute> PhyClientRelease<R> {
    /// Client removed by this transition.
    pub const fn client(&self) -> PhyModemClient {
        self.outcome.client()
    }

    /// Whether the saved pre-release mask contained no other PHY client.
    pub const fn is_last(&self) -> bool {
        self.outcome.is_last()
    }

    /// Inspect the post-release client set without discarding the saved
    /// last-client fact.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.outcome.owner().snapshot()
    }
}

/// Registered domain of a lent-hardware route after the complete RF-close
/// graph, with an empty client set.
///
/// The route's outer owner still holds its protocol hardware, temperature
/// power and platform clocks. The domain can retire with the cold release,
/// move with the retained radio root to another route through
/// [`crate::RetainedPhy`], or wake RF again on its route without a new
/// registration.
#[must_use = "retain the closed registration until physical owner reunion"]
pub struct PhyRfClosed<R: PhyRoute> {
    pub(crate) domain: PhyDomain,
    route: core::marker::PhantomData<fn() -> R>,
}

impl<R: PhyRoute> PhyRfClosed<R> {
    pub(crate) fn new(domain: PhyDomain) -> Self {
        debug_assert!(domain.client_snapshot().is_empty());
        Self {
            domain,
            route: core::marker::PhantomData,
        }
    }

    /// Read the final state while retaining its physical-close provenance.
    pub const fn state(&self) -> &PhyState {
        self.domain.phy_state()
    }

    /// Borrow the closed registered PHY domain.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Final calibrated state, including the pre-close temperature
    /// observation. The registration proof is retired.
    #[cfg(target_arch = "riscv32")]
    pub fn into_retired_state(self) -> PhyState {
        self.domain.registered.into_retired_state()
    }
}

/// Rejected client release retaining the unchanged registered owner.
#[must_use = "failed release retains the registered PHY owner"]
pub struct PhyClientReleaseFailureOwner<R: PhyRoute> {
    hardware: Hardware<R>,
    registered: RegisteredPhyState,
    failure: PhyClientReleaseFailure,
}

impl<R: PhyRoute> PhyClientReleaseFailureOwner<R> {
    /// Inspect the exact source-owned release rejection.
    pub const fn error(&self) -> PhyClientReleaseError {
        self.failure.error()
    }

    /// Recover the owner left unchanged by the rejected release.
    pub fn into_owner(self) -> Client<R> {
        client_owner::<R>(self.hardware, self.registered, self.failure.into_owner())
    }
}

/// Scheduler evaluation retaining the exact registered epoch.
#[must_use = "tracking evaluation retains the registered PHY owner"]
pub struct PhyTrackEvaluationOwner<R: PhyRoute> {
    hardware: Hardware<R>,
    registered: RegisteredPhyState,
    evaluation: PhyTrackEvaluation,
}

impl<R: PhyRoute> PhyTrackEvaluationOwner<R> {
    /// Borrow the request emitted by this evaluation, when one is due.
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.evaluation.request()
    }

    /// Recover the settled owner or retain the due request in its affine
    /// pending state.
    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the allocation-free registered owner"
    )]
    pub fn into_owner(self) -> Result<Client<R>, PhyPendingTrackOwner<R>> {
        let Self {
            hardware,
            registered,
            evaluation,
        } = self;
        match evaluation.into_owner() {
            Ok(clients) => Ok(client_owner::<R>(hardware, registered, clients)),
            Err(pending) => Err(PhyPendingTrackOwner {
                hardware,
                registered,
                pending,
            }),
        }
    }
}

/// Invalid clock evaluation retaining the unchanged registered owner.
#[must_use = "failed tracking evaluation retains the registered PHY owner"]
pub struct PhyTrackEvaluationFailureOwner<R: PhyRoute> {
    hardware: Hardware<R>,
    registered: RegisteredPhyState,
    failure: PhyTrackEvaluationFailure,
}

impl<R: PhyRoute> PhyTrackEvaluationFailureOwner<R> {
    /// Inspect the monotonic-time failure without recovering mutable state.
    pub const fn error(&self) -> PhyTrackTimeError {
        self.failure.error()
    }

    /// Recover the owner left unchanged by the rejected clock evaluation.
    pub fn into_owner(self) -> Client<R> {
        client_owner::<R>(self.hardware, self.registered, self.failure.into_owner())
    }
}

/// Due tracking request retaining the complete registered epoch.
#[must_use = "pending tracking retains the registered PHY owner"]
pub struct PhyPendingTrackOwner<R: PhyRoute> {
    pub(crate) hardware: Hardware<R>,
    registered: RegisteredPhyState,
    pending: PhyPendingTrack,
}

impl<R: PhyRoute> PhyPendingTrackOwner<R> {
    /// Borrow the exact source-owned tracking request.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    /// Inspect the client set that owns this request.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.pending.snapshot()
    }

    /// Borrow the last committed registered PHY state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Begin tracking with policy projected from this registered PHY epoch.
    pub fn begin_tracking(self) -> PhyPendingTrackingOwner<R> {
        let policy = self.registered.tracking_policy();
        PhyPendingTrackingOwner {
            hardware: self.hardware,
            registered: self.registered,
            pending: self.pending.begin_tracking(policy),
        }
    }

    /// Enter fail-stop state without attempting target tracking work.
    pub fn fail(self) -> PhyTrackPoisonedOwner<R> {
        PhyTrackPoisonedOwner {
            hardware: self.hardware,
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// In-flight tracking retaining the target registration proof.
#[must_use = "in-flight tracking retains the registered PHY owner"]
pub struct PhyPendingTrackingOwner<R: PhyRoute> {
    pub(crate) hardware: Hardware<R>,
    pub(crate) registered: RegisteredPhyState,
    pub(crate) pending: PhyPendingTracking,
}

impl<R: PhyRoute> PhyPendingTrackingOwner<R> {
    /// Inspect the next semantic target operation.
    #[cfg(test)]
    pub(crate) const fn action(&self) -> crate::tracking::parameters::PhyParamTrackingAction {
        self.pending.action()
    }

    /// Borrow the last committed registered PHY state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect the client set that owns this request.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.pending.snapshot()
    }

    /// Whether this tracking belongs to the registration of `hardware`.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn describes(
        &self,
        hardware: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> bool {
        self.pending.describes(hardware)
    }

    /// Recover the client owner after the complete tracking transition.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "incomplete target work retains the allocation-free registered owner"
    )]
    pub(crate) fn into_client_owner(self) -> Result<Client<R>, Self> {
        let Self {
            hardware,
            registered,
            pending,
        } = self;
        match pending.into_owner() {
            Ok(clients) => Ok(client_owner::<R>(hardware, registered, clients)),
            Err(pending) => Err(Self {
                hardware,
                registered,
                pending,
            }),
        }
    }

    /// Consume ambiguous work into a non-recoverable owner.
    pub fn fail(self) -> PhyTrackPoisonedOwner<R> {
        PhyTrackPoisonedOwner {
            hardware: self.hardware,
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

impl<R> PhyPendingTrackingOwner<R>
where
    R: PhyRoute + sealed::PhyRoute<Hardware = ()>,
{
    /// Borrow the state and request of a route whose hardware is lent.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_tracking_parts(&mut self) -> (&mut PhyState, &mut PhyPendingTracking) {
        (self.registered.target_state_mut(), &mut self.pending)
    }
}

/// Fail-stop registered epoch after ambiguous tracking hardware work.
#[must_use = "failed tracking poisons the registered PHY epoch"]
pub struct PhyTrackPoisonedOwner<R: PhyRoute> {
    pub(crate) hardware: Hardware<R>,
    registered: RegisteredPhyState,
    poisoned: PhyTrackPoisoned,
}

impl<R: PhyRoute> PhyTrackPoisonedOwner<R> {
    /// Borrow the last committed registered PHY state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Borrow the request that was being serviced.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.poisoned.request()
    }

    /// Inspect the retained client set.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.poisoned.snapshot()
    }
}
