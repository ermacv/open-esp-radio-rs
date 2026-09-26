//! Target registration and tracking of the shared PHY domain under the radio
//! arbiter.

use super::*;
use crate::{
    concurrent::{ConcurrentPhy, ConcurrentPhyError, Slot, admit_maintenance},
    registered_route::PhyDomain,
};
use oer_esp32s31_hal::shared_radio::{ClientQuiescence, SharedRadioLease};

/// Outputs of the shared domain's registration.
#[must_use = "the registration cache and outcome describe the new epoch"]
pub struct ConcurrentPhyRegistration {
    calibration_cache: Option<PhyCalibrationCache>,
    outcome: PhyRegisterOutcome,
    counters: PhyTargetPortCounters,
}

impl ConcurrentPhyRegistration {
    pub const fn calibration_cache(&self) -> Option<&PhyCalibrationCache> {
        self.calibration_cache.as_ref()
    }

    pub const fn outcome(&self) -> PhyRegisterOutcome {
        self.outcome
    }

    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }

    pub fn into_calibration_cache(self) -> Option<PhyCalibrationCache> {
        self.calibration_cache
    }
}

/// Shared domain registration that did not register the domain.
#[must_use = "a started registration failure is fail-stop"]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free failure retains the exact registration transition"
)]
pub enum ConcurrentPhyRegisterFailure {
    /// Rejected before any register access.
    Rejected(ConcurrentPhyError),
    /// The registration transition failed.
    Failed(PhyDomainRegisterFailure),
}

/// Shared domain tracking that did not settle the domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentPhyTrackingError {
    /// Rejected before any register access; the domain stays pending.
    Rejected(ConcurrentPhyError),
    /// Tracking started and failed; the domain is poisoned.
    Failed(TargetPhyParamTrackingError),
}

/// Register the shared PHY domain through the arbiter's shared-PHY borrow.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. Cancellation may
/// strand a partially applied edge and leaves the domain unregistered; the
/// radio then requires reset.
#[must_use = "PHY registration must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "fail-stop error retains the allocation-free PHY transition"
)]
pub async fn register_concurrent_phy<P, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    config: PhyRegisterConfig,
    observer: O,
) -> Result<ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    match lease.attachment_mut().slot_mut() {
        Slot::Empty => {}
        Slot::Poisoned => {
            return Err(ConcurrentPhyRegisterFailure::Rejected(
                ConcurrentPhyError::Poisoned,
            ));
        }
        Slot::Registered(_) | Slot::Pending { .. } => {
            return Err(ConcurrentPhyRegisterFailure::Rejected(
                ConcurrentPhyError::AlreadyRegistered,
            ));
        }
    }
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    match PhyDomain::register::<P, _, D, O>(platform, &mut registers, config, observer).await {
        Ok(registered) => {
            let (domain, calibration_cache, outcome, counters) = registered.into_parts();
            *phy.slot_mut() = Slot::Registered(domain);
            Ok(ConcurrentPhyRegistration {
                calibration_cache,
                outcome,
                counters,
            })
        }
        Err(failure) => Err(ConcurrentPhyRegisterFailure::Failed(failure)),
    }
}

/// Run the pending tracking request once every active client proves
/// quiescence.
///
/// Admission is checked at the current PHY clock before any register access;
/// a rejection keeps the domain pending. With a `Until` proof, tracking runs
/// inside the earliest window: completion at or after it poisons the domain.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. Cancellation after
/// admission leaves the domain pending in software while hardware may be
/// partially updated; the radio then requires reset.
pub async fn maintain_concurrent_phy<P, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    proofs: &[ClientQuiescence<'_>],
    observer: O,
) -> Result<PhyParamTrackingOutcome, ConcurrentPhyTrackingError>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let Some(now) = D::now_micros() else {
        return Err(ConcurrentPhyTrackingError::Rejected(
            ConcurrentPhyError::ClockBehindProof,
        ));
    };
    let admission =
        admit_maintenance(lease, proofs, now).map_err(ConcurrentPhyTrackingError::Rejected)?;
    let deadline = match admission.release_by_micros() {
        Some(release_by) => {
            match core::num::NonZeroU64::new(release_by - now)
                .and_then(|budget| crate::tracking::deadline::TrackingDeadline::new(now, budget))
            {
                Some(deadline) => Some(deadline),
                None => {
                    return Err(ConcurrentPhyTrackingError::Rejected(
                        ConcurrentPhyError::WindowClosed,
                    ));
                }
            }
        }
        None => None,
    };

    let (mut registered, pending) = match core::mem::take(lease.attachment_mut().slot_mut()) {
        Slot::Pending {
            registered,
            pending,
        } => (registered, pending),
        other => {
            // Admission accepted only a pending slot.
            *lease.attachment_mut().slot_mut() = other;
            return Err(ConcurrentPhyTrackingError::Rejected(
                ConcurrentPhyError::NoTrackingPending,
            ));
        }
    };
    let mut tracking = pending.begin_tracking(registered.tracking_policy());

    let (mut registers, phy) = lease.phy_hal_with_attachment();
    if !tracking.describes(&registers) {
        *phy.slot_mut() = Slot::Poisoned;
        return Err(ConcurrentPhyTrackingError::Failed(
            TargetPhyParamTrackingError::EpochMismatch,
        ));
    }
    let result = {
        let state = registered.target_state_mut();
        let mut port =
            TargetPhyParamTrackingPort::<_, _, D, _>::new(platform, &mut registers, observer);
        match deadline {
            Some(deadline) => crate::tracking::deadline::run(
                deadline,
                D::now_micros,
                |remaining| D::after_micros(crate::executor::wait::Kind::Completion, remaining),
                run_phy_param_tracking(&mut tracking, state, &mut port),
            )
            .await
            .map_err(TargetPhyParamTrackingError::Deadline)
            .and_then(|result| result.map_err(TargetPhyParamTrackingError::Run)),
            None => run_phy_param_tracking(&mut tracking, state, &mut port)
                .await
                .map_err(TargetPhyParamTrackingError::Run),
        }
    };
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            *phy.slot_mut() = Slot::Poisoned;
            return Err(ConcurrentPhyTrackingError::Failed(error));
        }
    };
    match tracking.into_owner() {
        Ok(clients) => {
            *phy.slot_mut() = Slot::Registered(PhyDomain::new(registered, clients));
            Ok(outcome)
        }
        Err(_) => {
            *phy.slot_mut() = Slot::Poisoned;
            Err(ConcurrentPhyTrackingError::Failed(
                TargetPhyParamTrackingError::MissingCompletedOwner,
            ))
        }
    }
}
