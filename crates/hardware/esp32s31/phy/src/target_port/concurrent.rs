//! Target registration and tracking of the shared PHY domain under the radio
//! arbiter.

use super::*;
use crate::{
    concurrent::{
        ConcurrentPhy, ConcurrentPhyError, MaintenancePolicy, Slot, admit_maintenance,
        evaluate_periodic_tracking,
    },
    domain::PhyDomain,
    state::client::PhyModemClient,
    state::client::PhyPllTrackClock,
};
use core::fmt::Write as _;
use oer_esp32s31_hal::shared_radio::{
    ClientQuiescence, ModemClockError, PhyClockModule, PlatformClockProvider, SharedRadioLease,
};
use oer_phy_trace::{
    ChannelResult, PoisonedBy, Registration, RfLifecycle, RfOperation, RfResult, TrackingTick,
    WifiChannel,
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
// CAPABILITY: phy-protocol-consumer-cold-registration-calibration-entry-wifi, phy-protocol-consumer-cold-registration-calibration-entry-bluetooth, phy-protocol-consumer-cold-registration-calibration-entry-ieee802154, cold-registration
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
    let result =
        register_concurrent_phy_untraced::<P, D, O>(lease, platform, clocks, config, observer)
            .await;
    oer_trace::emit(&match &result {
        Ok(registration) => Registration::Calibrated(crate::trace::calibration_path(
            registration.outcome.calibration_path,
        )),
        Err(ConcurrentPhyRegisterFailure::Rejected(error)) => {
            Registration::Refused(crate::trace::refusal(*error))
        }
        Err(ConcurrentPhyRegisterFailure::Failed { failure, .. }) => {
            Registration::Failed(trace::register_fault(&failure.error()))
        }
        Err(ConcurrentPhyRegisterFailure::CalibrationClock { registration, .. }) => {
            Registration::Calibrated(crate::trace::calibration_path(
                registration.outcome.calibration_path,
            ))
        }
    });
    result
}

#[allow(
    clippy::result_large_err,
    reason = "fail-stop error retains the allocation-free PHY transition"
)]
async fn register_concurrent_phy_untraced<P, D, O>(
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
    oer_trace::emit(&Registration::Started);
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
            // DIAGNOSTIC #38 arm A: republish before the calibration clock release.
            if option_env!("OER_DIAG38_ARM") == Some("A") {
                diag_republish_bluetooth_tx_gain(lease);
            }
            let released = lease.disable_phy_modem_clocks(PhyClockModule::Calibration, clocks);
            // DIAGNOSTIC #38 arm B: republish after the calibration clock release.
            if option_env!("OER_DIAG38_ARM") == Some("B") {
                diag_republish_bluetooth_tx_gain(lease);
            }
            match released {
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
/// `phy_close_rf` run as on every route, the temperature sensor powers down
/// (`phy_xpd_tsens`), then the common PHY clock is released. The registered calibration epoch is retained for
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
// CAPABILITY: phy-lifecycle-boundaries-rf-and-analog-shutdown, phy-lifecycle-boundaries-rf-sleep-power-down, whole-radio-active-operation-power-saving-and-shutdown-rf-sleep-modem-power-down, whole-radio-active-operation-power-saving-and-shutdown-full-powered-shutdown, phy-protocol-consumer-complete-last-client-rf-analog-shutdown-wifi, whole-radio-active-operation-power-saving-and-shutdown-shared-rf-powered-idle-frontier
pub async fn close_concurrent_rf<P, D>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clocks: &mut impl PlatformClockProvider,
) -> Result<(), ConcurrentRfError>
where
    D: PhyAsyncDelay,
{
    let result = close_concurrent_rf_untraced::<P, D>(lease, platform, clocks).await;
    emit_rf(RfOperation::Close, &result);
    result
}

async fn close_concurrent_rf_untraced<P, D>(
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
            .map(|()| {
                // `esp_phy_disable` powers the temperature sensor down after
                // `phy_close_rf` (`phy_xpd_tsens`); the retained wake powers it
                // up again. Its SAR2 analog block is then reset like every
                // other powered-down analog block.
                oer_esp32s31_hal::phy::temperature::power_down(&mut registers);
            })
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
            trace::record_poison(
                PoisonedBy::RfClose,
                trace::port_fault(error),
                oer_phy_trace::Slot::Registered,
                domain.client_snapshot(),
                domain.phy_state(),
                &mut registers,
            );
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
// CAPABILITY: phy-lifecycle-boundaries-rf-wake-from-retained-sleep, whole-radio-active-operation-power-saving-and-shutdown-rf-wake-resume, phy-protocol-consumer-resume-after-rf-sleep-wifi
pub async fn wake_concurrent_rf<D>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &mut impl PlatformClockProvider,
) -> Result<(), ConcurrentRfError>
where
    D: PhyAsyncDelay,
{
    let result = wake_concurrent_rf_untraced::<D>(lease, clocks).await;
    emit_rf(RfOperation::Wake, &result);
    result
}

async fn wake_concurrent_rf_untraced<D>(
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
        trace::record_poison(
            PoisonedBy::RfWake,
            trace::port_fault(error),
            oer_phy_trace::Slot::RfClosed,
            domain.client_snapshot(),
            domain.phy_state(),
            &mut registers,
        );
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
    let clients_before = pending.snapshot();
    let mut tracking = pending.begin_tracking(registered.tracking_policy());

    let (mut registers, phy, mut grant) = lease.phy_hal_with_attachment_and_grant();
    if !tracking.describes(&registers) {
        let error = TargetPhyParamTrackingError::EpochMismatch;
        trace::record_poison(
            PoisonedBy::Tracking,
            trace::tracking_fault(&error),
            oer_phy_trace::Slot::Pending,
            clients_before,
            registered.target_state_mut(),
            &mut registers,
        );
        *phy.slot_mut() = Slot::Poisoned;
        return Err(ConcurrentPhyTrackingError::Failed(error));
    }
    let result = {
        let state = registered.target_state_mut();
        let mut port = TargetPhyParamTrackingPort::<_, _, _, D, _>::new(
            platform,
            &mut registers,
            &mut grant,
            observer,
        );
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
            trace::record_poison(
                PoisonedBy::Tracking,
                trace::tracking_fault(&error),
                oer_phy_trace::Slot::Pending,
                clients_before,
                registered.target_state_mut(),
                &mut registers,
            );
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
            let error = TargetPhyParamTrackingError::MissingCompletedOwner;
            trace::record_poison(
                PoisonedBy::Tracking,
                trace::tracking_fault(&error),
                oer_phy_trace::Slot::Pending,
                clients_before,
                registered.target_state_mut(),
                &mut registers,
            );
            *phy.slot_mut() = Slot::Poisoned;
            Err(ConcurrentPhyTrackingError::Failed(error))
        }
    }
}

/// Result of one periodic tracking tick of the shared domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentTrackingTick {
    /// No tracking was due.
    NotDue,
    /// Tracking was due and ran to completion.
    Tracked(PhyParamTrackingOutcome),
    /// The domain cannot track now (not registered, RF closed, or poisoned);
    /// nothing changed.
    Unavailable(ConcurrentPhyError),
    /// Tracking is due, but the domain's [`MaintenancePolicy::Quiesced`]
    /// needs client proofs; run [`maintain_concurrent_phy`] with them.
    AwaitingQuiescence,
}

/// One tick of ESP-IDF's periodic `phy_track_pll`: evaluate the clients'
/// tracking period and, when tracking is due, run it under the domain's
/// admission policy.
///
/// Under [`MaintenancePolicy::Vendor`] the tracking transaction runs at once
/// with protocols running, as the vendor timer callback does under its PHY
/// lock; the lease is that lock, and the tracking graph brackets its
/// RF-sensitive regions with the grant-protect request.
///
/// # Errors
///
/// The tracking clock was rejected, or tracking started and failed; after a
/// failure the domain is poisoned.
///
/// # Cancellation
///
/// Once tracking starts, drive this future to a terminal result;
/// cancellation then leaves hardware partially updated and requires reset.
// CAPABILITY: phy-calibration-state-and-tracking-periodic-tracking-service, whole-radio-active-operation-power-saving-and-shutdown-shared-phy-tracking, phy-protocol-consumer-periodic-parameter-calibration-tracking-wifi, phy-protocol-consumer-periodic-parameter-calibration-tracking-ieee802154, bluetooth-idle-phy-maintenance, bluetooth-periodic-phy-maintenance
pub async fn track_concurrent_phy<P, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clock: &mut impl PhyPllTrackClock,
    observer: O,
) -> Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let result = track_concurrent_phy_untraced::<P, D, O>(lease, platform, clock, observer).await;
    oer_trace::emit(&match &result {
        Ok(ConcurrentTrackingTick::NotDue) => TrackingTick::NotDue,
        Ok(ConcurrentTrackingTick::Tracked(outcome)) => {
            TrackingTick::Tracked(crate::trace::progress(outcome))
        }
        Ok(ConcurrentTrackingTick::Unavailable(error)) => {
            TrackingTick::Unavailable(crate::trace::refusal(*error))
        }
        Ok(ConcurrentTrackingTick::AwaitingQuiescence) => TrackingTick::AwaitingQuiescence,
        Err(ConcurrentPhyTrackingError::Rejected(error)) => {
            TrackingTick::Refused(crate::trace::refusal(*error))
        }
        Err(ConcurrentPhyTrackingError::Failed(error)) => {
            TrackingTick::Failed(trace::tracking_fault(error))
        }
    });
    if let Ok(ConcurrentTrackingTick::Tracked(outcome)) = &result
        && crate::trace::committed_reference(outcome)
        && let Ok(state) = lease.attachment().phy_state()
    {
        oer_trace::emit(&crate::trace::temperatures(state));
    }
    result
}

async fn track_concurrent_phy_untraced<P, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clock: &mut impl PhyPllTrackClock,
    observer: O,
) -> Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
{
    let due = if lease.attachment().tracking_pending() {
        true
    } else {
        match evaluate_periodic_tracking(lease, clock) {
            Ok(due) => due,
            Err(
                error @ (ConcurrentPhyError::NotRegistered
                | ConcurrentPhyError::RfClosed
                | ConcurrentPhyError::Poisoned),
            ) => return Ok(ConcurrentTrackingTick::Unavailable(error)),
            Err(error) => return Err(ConcurrentPhyTrackingError::Rejected(error)),
        }
    };
    if !due {
        return Ok(ConcurrentTrackingTick::NotDue);
    }
    if lease.attachment().maintenance_policy() == MaintenancePolicy::Quiesced {
        return Ok(ConcurrentTrackingTick::AwaitingQuiescence);
    }
    maintain_concurrent_phy::<P, D, O>(lease, platform, &[], observer)
        .await
        .map(ConcurrentTrackingTick::Tracked)
}

/// Wi-Fi channel operation on the shared domain that did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentWifiChannelError {
    /// Rejected before any register access: the domain is not settled, the
    /// borrow describes another registration, or Wi-Fi is not a client.
    Rejected(ConcurrentPhyError),
    /// The channel transaction failed after it started.
    Failed(PhyTargetPortError),
}

fn wifi_channel_state<'domain>(
    phy: &'domain mut ConcurrentPhy,
    channel: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> Result<&'domain mut PhyState, ConcurrentWifiChannelError> {
    let domain = phy
        .settled_mut()
        .map_err(ConcurrentWifiChannelError::Rejected)?;
    if !domain.clients.describes(channel) {
        return Err(ConcurrentWifiChannelError::Rejected(
            ConcurrentPhyError::EpochMismatch,
        ));
    }
    if !domain.client_snapshot().contains(PhyModemClient::Wifi) {
        return Err(ConcurrentWifiChannelError::Rejected(
            ConcurrentPhyError::ClientAbsent(PhyModemClient::Wifi),
        ));
    }
    Ok(domain.registered.target_state_mut())
}

/// Select a Wi-Fi channel on the shared domain before the MAC runs, as the
/// registered Wi-Fi route's channel selection does.
///
/// `phy` and `channel` come from one lease: the HAL Wi-Fi client lends the
/// channel HAL together with the arbiter attachment.
///
/// # Errors
///
/// Rejected before any register access, or failed after the channel
/// transaction started.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result.
pub async fn select_concurrent_wifi_channel<D: PhyAsyncDelay, P, O: PhyTargetObserver>(
    phy: &mut ConcurrentPhy,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let result = async {
        let state = wifi_channel_state(phy, channel)?;
        select_phy_channel_with_hal::<D, _, _>(state, channel_or_frequency, cbw, channel, observer)
            .await
            .map_err(ConcurrentWifiChannelError::Failed)
    }
    .await;
    emit_wifi_channel(channel_or_frequency, cbw, &result);
    result
}

/// Stop the Wi-Fi MAC, retune the shared domain and restart the MAC, as the
/// registered Wi-Fi route's channel switch does.
///
/// # Errors
///
/// As [`select_concurrent_wifi_channel`].
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result.
// CAPABILITY: phy-protocol-consumer-protocol-channel-switching-wifi
pub async fn switch_concurrent_wifi_channel<D: PhyAsyncDelay, P, O: PhyTargetObserver>(
    phy: &mut ConcurrentPhy,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let result = async {
        let state = wifi_channel_state(phy, channel)?;
        switch_phy_channel_with_hal_and_mac_restart::<D, _, _>(
            state,
            channel_or_frequency,
            cbw,
            channel,
            observer,
        )
        .await
        .map_err(ConcurrentWifiChannelError::Failed)
    }
    .await;
    emit_wifi_channel(channel_or_frequency, cbw, &result);
    result
}

/// Record an RF close or wake.
fn emit_rf(operation: RfOperation, result: &Result<(), ConcurrentRfError>) {
    let result = match result {
        Ok(()) => RfResult::Done,
        Err(ConcurrentRfError::Rejected(error)) => RfResult::Refused(crate::trace::refusal(*error)),
        Err(ConcurrentRfError::Recoverable(error)) => {
            RfResult::Recoverable(trace::port_fault(*error))
        }
        Err(ConcurrentRfError::Failed(error)) => RfResult::Failed(trace::port_fault(*error)),
        Err(ConcurrentRfError::Clock(_)) => RfResult::Failed(oer_phy_trace::Fault {
            stage: oer_phy_trace::FaultStage::Clock,
            detail: 0,
        }),
    };
    oer_trace::emit(&RfLifecycle { operation, result });
}

/// Record a Wi-Fi channel selection.
fn emit_wifi_channel(
    channel_or_frequency: u16,
    bandwidth: u8,
    result: &Result<(), ConcurrentWifiChannelError>,
) {
    let result = match result {
        Ok(()) => ChannelResult::Done,
        Err(ConcurrentWifiChannelError::Rejected(error)) => {
            ChannelResult::Refused(crate::trace::refusal(*error))
        }
        Err(ConcurrentWifiChannelError::Failed(error)) => {
            ChannelResult::Failed(trace::port_fault(*error))
        }
    };
    oer_trace::emit(&WifiChannel {
        channel_or_frequency,
        bandwidth,
        result,
    });
}

/// DIAGNOSTIC #38 (not for merge): recompute the BT/15.4 TX gain table from
/// the registered state and publish it into gain memory again. Returns
/// whether a registered domain existed.
pub fn diag_republish_bluetooth_tx_gain(lease: &mut SharedRadioLease<'_, ConcurrentPhy>) -> bool {
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    let Ok(state) = phy.phy_state() else {
        return false;
    };
    let image = crate::calibration::bluetooth::calculate_bluetooth_tx_gain(
        state.bluetooth_tx_gain_parameters(),
    );
    crate::hardware::publish_bluetooth_tx_gain_memory(&mut registers, image);
    true
}

/// DIAGNOSTIC #38 (not for merge): write RF frequency-memory records 25, 27
/// and 62 (2425/2427/2462 MHz) and the frequency-channel partition registers
/// (image indices 0..=4: FREQUENCY_CONTROL, FREQUENCY_MEMORY_READ_CONTROL,
/// FREQUENCY_PARAMETER_1_STATUS, I2C_NUMBER_CONTROL,
/// FREQUENCY_MEMORY_READ_RESULT). Word 0 of a record is
/// cap[7:0] | cap_high_i2c << 8 | sdm_low_i2c << 16, word 1 the three upper
/// SDM bytes (lower-middle, upper-middle, most significant).
pub fn diag_frequency_report(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    out: &mut impl core::fmt::Write,
) {
    let _ = out.write_str("diag38 freq regs");
    for index in 0..=4 {
        let _ = write!(out, " {:08x}", lease.phy_register_image(index).unwrap_or(0));
    }
    let _ = out.write_str("\n");
    let (mut registers, phy) = lease.phy_hal_with_attachment();
    if let Ok(state) = phy.phy_state() {
        let [initial, middle, outer] = state.diag_xtal_duty();
        let _ = write!(
            out,
            "diag38 xtal duty initial {initial:02x} middle(2440) {middle:02x} outer(2480) {outer:02x}\n"
        );
    }
    for entry in [25_u8, 27, 62] {
        let mut words = [0_u32; 3];
        for (word_index, word) in words.iter_mut().enumerate() {
            let (address, mode) =
                crate::analog::frequency::diag_rf_record_word_address(entry, word_index as u8);
            *word = oer_esp32s31_hal::phy::frequency::read_memory(&mut registers, address, mode);
        }
        let cap = (words[0] & 0xff) | (((words[0] >> 14) & 1) << 8);
        let _ = write!(
            out,
            "diag38 freq mem {} ({} MHz) {:06x} {:06x} {:06x} cap {:03x}\n",
            entry,
            2400 + u32::from(entry),
            words[0],
            words[1],
            words[2],
            cap,
        );
    }
}

/// DIAGNOSTIC #38 (not for merge): write the tracking and BT/15.4 gain
/// inputs of the registered state.
pub fn diag_tracking_report(
    lease: &SharedRadioLease<'_, ConcurrentPhy>,
    out: &mut impl core::fmt::Write,
) {
    let Ok(state) = lease.attachment().phy_state() else {
        let _ = out.write_str("diag38 no registered PHY state\n");
        return;
    };
    let _ = write!(
        out,
        "diag38 temp {:?}\ndiag38 txpwr {:?}\ndiag38 cal {:?}\ndiag38 btgain {:?}\n",
        state.temperature_observation(),
        state.tx_power_tracking_parameters(false),
        state.calibration_tracking_parameters(None),
        state.bluetooth_tx_gain_parameters(),
    );
}
