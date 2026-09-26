//! Target registration and tracking of the shared PHY domain under the radio
//! arbiter.

use super::*;
use crate::{
    concurrent::{ConcurrentPhy, ConcurrentPhyError, Slot, admit_maintenance},
    registered_route::PhyDomain,
};
use oer_esp32s31_hal::shared_radio::{
    ClientQuiescence, ModemClockError, PhyClockModule, PlatformClockProvider, SharedRadioLease,
};

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

/// Shared domain registration that did not complete cleanly.
#[must_use = "a started registration failure is fail-stop"]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free failure retains the exact registration transition"
)]
pub enum ConcurrentPhyRegisterFailure {
    /// Rejected before any register access; no modem clock is held.
    Rejected(ConcurrentPhyError),
    /// The registration transition failed and the domain stays unregistered.
    /// `clocks` reports whether the domain's modem clock modules were
    /// released.
    Failed {
        /// The failed registration transition.
        failure: PhyDomainRegisterFailure,
        /// Release of `PHY` and `PHY_CALIBRATION` after the failure.
        clocks: Result<(), ModemClockError>,
    },
    /// The domain registered, but `PHY_CALIBRATION` was not released; the
    /// modem clocks are poisoned until reset.
    CalibrationClock {
        /// The completed registration outputs.
        registration: ConcurrentPhyRegistration,
        /// Why `PHY_CALIBRATION` was not released.
        error: ModemClockError,
    },
}

/// RF close or wake of the shared domain that did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentRfError {
    /// Rejected before any register access; the domain is unchanged.
    Rejected(ConcurrentPhyError),
    /// RF close preparation failed after every issued transaction completed;
    /// RF stays open and the domain is unchanged.
    Recoverable(PhyTargetPortError),
    /// Hardware work failed ambiguously; the domain is poisoned.
    Failed(PhyTargetPortError),
    /// RF changed state, but the domain's modem clock module did not follow;
    /// the modem clocks are poisoned until reset.
    Clock(ModemClockError),
}

/// Take `PHY`, then `PHY_CALIBRATION`, as `esp_phy_enable` does before
/// calibration or RF wake.
fn enable_phy_clocks(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &mut impl PlatformClockProvider,
) -> Result<(), ModemClockError> {
    lease.enable_phy_modem_clocks(PhyClockModule::Phy, clocks)?;
    if let Err(error) = lease.enable_phy_modem_clocks(PhyClockModule::Calibration, clocks) {
        return Err(
            match lease.disable_phy_modem_clocks(PhyClockModule::Phy, clocks) {
                Ok(()) => error,
                Err(rollback) => rollback,
            },
        );
    }
    Ok(())
}

/// Release both modules after a registration that did not complete.
fn release_phy_clocks(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &mut impl PlatformClockProvider,
) -> Result<(), ModemClockError> {
    let calibration = lease.disable_phy_modem_clocks(PhyClockModule::Calibration, clocks);
    let phy = lease.disable_phy_modem_clocks(PhyClockModule::Phy, clocks);
    calibration.and(phy)
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
/// This is the calibrating first `esp_phy_enable`: `PHY` and
/// `PHY_CALIBRATION` are enabled through the arbiter before calibration, and
/// `PHY_CALIBRATION` is released after it. `PHY` stays held while RF is open.
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
    clocks: &mut impl PlatformClockProvider,
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
        Slot::Registered(_) | Slot::RfClosed(_) | Slot::Pending { .. } => {
            return Err(ConcurrentPhyRegisterFailure::Rejected(
                ConcurrentPhyError::AlreadyRegistered,
            ));
        }
    }
    enable_phy_clocks(lease, clocks).map_err(|error| {
        ConcurrentPhyRegisterFailure::Rejected(ConcurrentPhyError::Clock(error))
    })?;
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    match PhyDomain::register::<P, _, D, O>(platform, &mut registers, config, observer).await {
        Ok(registered) => {
            let (domain, calibration_cache, outcome, counters) = registered.into_parts();
            *phy.slot_mut() = Slot::Registered(domain);
            let registration = ConcurrentPhyRegistration {
                calibration_cache,
                outcome,
                counters,
            };
            match lease.disable_phy_modem_clocks(PhyClockModule::Calibration, clocks) {
                Ok(()) => Ok(registration),
                Err(error) => Err(ConcurrentPhyRegisterFailure::CalibrationClock {
                    registration,
                    error,
                }),
            }
        }
        Err(failure) => Err(ConcurrentPhyRegisterFailure::Failed {
            failure,
            clocks: release_phy_clocks(lease, clocks),
        }),
    }
}

/// Close RF after the last client left the shared domain, and release its
/// `PHY` modem clock module.
///
/// This is the last `esp_phy_disable`: the temperature preflight and
/// `phy_close_rf` run as on every route, then the common PHY clock is
/// released. The registered calibration epoch is retained for
/// [`wake_concurrent_rf`].
///
/// # Errors
///
/// Rejected before any register access when the domain is not RF-open,
/// clients are active or the borrow describes another registration. A
/// preparation failure after completed transactions keeps RF open; an
/// ambiguous hardware failure poisons the domain.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. Cancellation leaves
/// the domain unregistered in software while RF may be partially closed; the
/// radio then requires reset.
pub async fn close_concurrent_rf<P, D>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clocks: &mut impl PlatformClockProvider,
) -> Result<(), ConcurrentRfError>
where
    D: PhyAsyncDelay,
{
    let mut domain = lease
        .attachment_mut()
        .idle_domain()
        .map_err(ConcurrentRfError::Rejected)?;
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    if !domain.clients.describes(&registers) {
        *phy.slot_mut() = Slot::Registered(domain);
        return Err(ConcurrentRfError::Rejected(
            ConcurrentPhyError::EpochMismatch,
        ));
    }
    let closed = match radio_lifecycle::observe_temperature_with_hal::<P, D>(
        platform,
        &mut registers,
        domain.registered.target_state_mut(),
    )
    .await
    {
        Ok(()) => radio_lifecycle::execute_rf_close_with_hal::<D>(&mut registers)
            .map_err(PhyRfCloseTemperatureFailure::HardwareAmbiguous),
        Err(failure) => Err(failure),
    };
    match closed {
        Ok(()) => {
            *phy.slot_mut() = Slot::RfClosed(domain);
            lease
                .disable_phy_modem_clocks(PhyClockModule::Phy, clocks)
                .map_err(ConcurrentRfError::Clock)
        }
        Err(PhyRfCloseTemperatureFailure::Recoverable(error)) => {
            *phy.slot_mut() = Slot::Registered(domain);
            Err(ConcurrentRfError::Recoverable(error))
        }
        Err(PhyRfCloseTemperatureFailure::HardwareAmbiguous(error)) => {
            *phy.slot_mut() = Slot::Poisoned;
            Err(ConcurrentRfError::Failed(error))
        }
    }
}

/// Wake RF of the closed shared domain for its next client.
///
/// This is a later first `esp_phy_enable`: `PHY` and `PHY_CALIBRATION` are
/// enabled, the retained registration is restored by `phy_wakeup_init`
/// without calibration, and `PHY_CALIBRATION` is released. No client is
/// acquired; the next [`acquire_client`](crate::concurrent::acquire_client)
/// evaluates tracking.
///
/// # Errors
///
/// Rejected before any register access when the domain is not RF-closed, the
/// borrow describes another registration or the clocks could not be
/// enabled. A started wake that fails poisons the domain.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result. After the first wake
/// edge every failure is fail-stop and requires reset.
pub async fn wake_concurrent_rf<D>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &mut impl PlatformClockProvider,
) -> Result<(), ConcurrentRfError>
where
    D: PhyAsyncDelay,
{
    let domain = lease
        .attachment_mut()
        .closed_domain()
        .map_err(ConcurrentRfError::Rejected)?;
    let describes = {
        let (registers, _) = lease.phy_hal_with_attachment();
        domain.clients.describes(&registers)
    };
    if !describes {
        *lease.attachment_mut().slot_mut() = Slot::RfClosed(domain);
        return Err(ConcurrentRfError::Rejected(
            ConcurrentPhyError::EpochMismatch,
        ));
    }
    if let Err(error) = enable_phy_clocks(lease, clocks) {
        *lease.attachment_mut().slot_mut() = Slot::RfClosed(domain);
        return Err(ConcurrentRfError::Rejected(ConcurrentPhyError::Clock(
            error,
        )));
    }
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    if let Err(error) =
        radio_lifecycle::execute_rf_wake_with_hal::<D>(&mut registers, domain.phy_state()).await
    {
        *phy.slot_mut() = Slot::Poisoned;
        return Err(ConcurrentRfError::Failed(error));
    }
    *phy.slot_mut() = Slot::Registered(domain);
    lease
        .disable_phy_modem_clocks(PhyClockModule::Calibration, clocks)
        .map_err(ConcurrentRfError::Clock)
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
