//! Protocol-neutral registration of one shared PHY domain.
//!
//! Registration needs only the shared-PHY registers, the platform services
//! used by the target port and the caller's calibration inputs. It does not
//! depend on which protocol route lends the registers: every route runs the
//! same transition here, and the route layer only wraps the resulting
//! [`PhyDomain`] in its own owner.

use super::*;
use crate::registered_route::PhyDomain;

/// Caller-owned inputs for one production common-PHY registration.
///
/// A valid retained cache seeds cold partial calibration; the transition
/// still republishes every hardware-resident product and returns a fresh
/// cache only after terminal success.
pub struct PhyRegisterConfig {
    calibration_identity: PhyCalibrationIdentity,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl PhyRegisterConfig {
    /// Request one fresh full registration and calibration cache.
    pub const fn new(calibration_identity: PhyCalibrationIdentity) -> Self {
        Self {
            calibration_identity,
            calibration_cache: None,
        }
    }

    /// Supply a retained cache for validated cold partial calibration.
    pub fn with_calibration_cache(mut self, calibration_cache: PhyCalibrationCache) -> Self {
        self.calibration_cache = Some(calibration_cache);
        self
    }

    fn into_transition(self) -> PhyRegisterTransition {
        PhyRegisterTransition::with_production_config_and_calibration(
            self.calibration_identity,
            self.calibration_cache,
        )
    }
}

/// Successful registration of one shared PHY domain.
///
/// The domain carries the target-issued registration proof and an empty
/// client set. It does not retain the borrowed platform or registers.
#[must_use = "a registered PHY domain is the unique owner of its epoch"]
pub struct PhyDomainRegistered {
    domain: PhyDomain,
    calibration_cache: Option<PhyCalibrationCache>,
    outcome: PhyRegisterOutcome,
    counters: PhyTargetPortCounters,
}

impl PhyDomainRegistered {
    /// Borrow the registered domain.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Borrow the fresh cache produced by this run, when requested.
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    /// Inspect the terminal registration result.
    pub const fn outcome(&self) -> PhyRegisterOutcome {
        self.outcome
    }

    /// Inspect concrete target operation counts for diagnostics.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Move all successful outputs.
    pub fn into_parts(
        self,
    ) -> (
        PhyDomain,
        Option<PhyCalibrationCache>,
        PhyRegisterOutcome,
        PhyTargetPortCounters,
    ) {
        (
            self.domain,
            self.calibration_cache,
            self.outcome,
            self.counters,
        )
    }
}

/// Fail-stop result of a domain registration.
///
/// The borrowed platform and registers return to their owner, but this type
/// exposes no retry or state extractor: any failure may follow an ambiguous
/// hardware edge. [`Self::failure_cleanup_completed`] distinguishes executed
/// terminal cleanup from an epoch requiring shared-PHY escalation.
#[must_use = "failed PHY registration retains the poisoned transition"]
pub struct PhyDomainRegisterFailure {
    transition: PhyRegisterTransition,
    counters: PhyTargetPortCounters,
    error: TargetPhyRegisterError,
}

impl PhyDomainRegisterFailure {
    /// Whether the real target transition completed its failure cleanup.
    /// This does not authorize retry or mint a registered domain.
    pub fn failure_cleanup_completed(&self) -> bool {
        self.transition.failure_cleanup_completed()
    }

    /// Inspect the exact terminal failure.
    pub const fn error(&self) -> TargetPhyRegisterError {
        self.error
    }

    /// Inspect concrete target operations completed before failure.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    /// Inspect the last semantic state for fail-stop diagnostics only.
    pub fn state(&self) -> Option<&PhyState> {
        self.transition.state()
    }
}

/// Run one registration transition through the concrete target port.
///
/// A fresh registration epoch begins on `registers` before the first edge;
/// the returned epoch is the one the completed state must describe.
pub(super) async fn execute_registration<P, R, D, O>(
    transition: &mut PhyRegisterTransition,
    platform: &mut P,
    registers: &mut R,
    observer: O,
) -> (
    Result<PhyRegisterOutcome, PhyRegisterRunError<PhyTargetPortError>>,
    PhyTargetPortCounters,
    TargetRegistrationWitness,
)
where
    R: PhyInitializationAccess,
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let epoch = registers.begin_registration_epoch();
    let mut port = TargetPhyRegisterPort::<_, _, D, _>::new(platform, registers, observer);
    let result = run_phy_register(transition, &mut port).await;
    (
        result,
        port.counters(),
        TargetRegistrationWitness::new(epoch),
    )
}

impl PhyDomain {
    /// Register the shared PHY through whichever route lends `registers`.
    ///
    /// This is the protocol-neutral production mint path for a registered
    /// domain. The route owner wraps the result; the domain alone grants no
    /// register access.
    ///
    /// # Cancellation
    ///
    /// Drive this future to a terminal result once polled. Cancellation may
    /// strand a partially applied hardware edge and never returns
    /// registration proof; the owner of `registers` must stay fail-stop until
    /// hardware reset.
    #[must_use = "PHY registration must be driven to a terminal result"]
    #[allow(
        clippy::result_large_err,
        reason = "fail-stop error retains the allocation-free PHY transition"
    )]
    pub async fn register<P, R, D, O>(
        platform: &mut P,
        registers: &mut R,
        config: PhyRegisterConfig,
        observer: O,
    ) -> Result<PhyDomainRegistered, PhyDomainRegisterFailure>
    where
        R: PhyInitializationAccess,
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        let mut transition = config.into_transition();
        let (result, counters, witness) =
            execute_registration::<P, R, D, O>(&mut transition, platform, registers, observer)
                .await;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(PhyDomainRegisterFailure {
                    transition,
                    counters,
                    error: TargetPhyRegisterError::Run(error),
                });
            }
        };
        match transition.into_model_parts() {
            Ok((state, calibration_cache)) => Ok(PhyDomainRegistered {
                domain: PhyDomain::from_target_completion(state, witness),
                calibration_cache,
                outcome,
                counters,
            }),
            Err(transition) => Err(PhyDomainRegisterFailure {
                transition,
                counters,
                error: TargetPhyRegisterError::MissingCompletedModelOwner,
            }),
        }
    }
}

/// Terminal successful tracking of a route whose hardware is lent.
#[must_use = "successful PHY tracking returns the settled client owner"]
pub struct PhyTrackingSuccess<R: crate::registered_route::PhyRoute> {
    owner: <R as crate::registered_route::sealed::PhyRoute>::Client,
    outcome: PhyParamTrackingOutcome,
}

impl<R: crate::registered_route::PhyRoute> PhyTrackingSuccess<R> {
    /// Borrow the settled client owner.
    pub const fn owner(&self) -> &<R as crate::registered_route::sealed::PhyRoute>::Client {
        &self.owner
    }

    /// Inspect the exact source-owned tracking outcome.
    pub const fn outcome(&self) -> PhyParamTrackingOutcome {
        self.outcome
    }

    /// Move the settled owner and semantic outcome together.
    pub fn into_parts(
        self,
    ) -> (
        <R as crate::registered_route::sealed::PhyRoute>::Client,
        PhyParamTrackingOutcome,
    ) {
        (self.owner, self.outcome)
    }
}

/// Fail-stop tracking result retaining the poisoned registered epoch.
#[must_use = "failed PHY tracking poisons its registered epoch"]
pub struct PhyTrackingFailure<R: crate::registered_route::PhyRoute> {
    poisoned: crate::registered_route::PhyTrackPoisonedOwner<R>,
    error: TargetPhyParamTrackingError,
}

impl<R: crate::registered_route::PhyRoute> PhyTrackingFailure<R> {
    /// Inspect the exact target or completion failure.
    pub const fn error(&self) -> TargetPhyParamTrackingError {
        self.error
    }

    /// Borrow the poisoned owner for terminal diagnostics.
    pub const fn poisoned(&self) -> &crate::registered_route::PhyTrackPoisonedOwner<R> {
        &self.poisoned
    }
}

impl<R> crate::registered_route::PhyPendingTrackingOwner<R>
where
    R: crate::registered_route::PhyRoute + crate::registered_route::sealed::PhyRoute<Hardware = ()>,
{
    /// Execute this pending tracking request through registers lent by the
    /// route owner.
    ///
    /// The registered state, client set and request stay inside this owner;
    /// `platform` and `registers` are borrowed for this one terminal
    /// operation. Registers of another registration poison the request before
    /// any hardware access. With `deadline`, the transaction runs inside the
    /// caller's absolute budget in `D`'s clock domain: a suspended child is
    /// woken at the deadline and completion at or after it poisons the epoch.
    /// Synchronous hardware transactions cannot be preempted by the guard.
    ///
    /// # Cancellation
    ///
    /// Once polled, drive this future to a terminal result. Cancellation drops
    /// the unique pending owner and requires an out-of-band hardware reset.
    #[must_use = "PHY tracking must be driven to a terminal result"]
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the poisoned registered epoch"
    )]
    pub async fn track<P, Regs, D, O>(
        mut self,
        platform: &mut P,
        registers: &mut Regs,
        observer: O,
        deadline: Option<crate::tracking::deadline::TrackingDeadline>,
    ) -> Result<PhyTrackingSuccess<R>, PhyTrackingFailure<R>>
    where
        Regs: PhyInitializationAccess,
        D: PhyAsyncDelay,
        O: PhyTargetObserver,
    {
        if !self.describes(&*registers) {
            return Err(PhyTrackingFailure {
                poisoned: self.fail(),
                error: TargetPhyParamTrackingError::EpochMismatch,
            });
        }
        let result = {
            let (state, pending) = self.target_tracking_parts();
            let mut port =
                TargetPhyParamTrackingPort::<_, _, D, _>::new(platform, registers, observer);
            match deadline {
                Some(deadline) => crate::tracking::deadline::run(
                    deadline,
                    D::now_micros,
                    |remaining| D::after_micros(crate::executor::wait::Kind::Completion, remaining),
                    run_phy_param_tracking(pending, state, &mut port),
                )
                .await
                .map_err(TargetPhyParamTrackingError::Deadline)
                .and_then(|result| result.map_err(TargetPhyParamTrackingError::Run)),
                None => run_phy_param_tracking(pending, state, &mut port)
                    .await
                    .map_err(TargetPhyParamTrackingError::Run),
            }
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(PhyTrackingFailure {
                    poisoned: self.fail(),
                    error,
                });
            }
        };
        match self.into_client_owner() {
            Ok(owner) => Ok(PhyTrackingSuccess { owner, outcome }),
            Err(tracking) => Err(PhyTrackingFailure {
                poisoned: tracking.fail(),
                error: TargetPhyParamTrackingError::MissingCompletedOwner,
            }),
        }
    }
}
