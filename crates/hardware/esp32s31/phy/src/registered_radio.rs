//! Coupled ownership of a powered radio and its target-registration proof.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::owner::PhyHal;

use oer_esp32s31_hal::owner::{Radio, state::Powered};

#[cfg(target_arch = "riscv32")]
use crate::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS;

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{
        PhyClientAcquireError, PhyClientAcquireFailure, PhyClientAcquireOrdering,
        PhyClientAcquireOutcome, PhyClientReleaseError, PhyClientReleaseFailure,
        PhyClientReleaseOutcome, PhyClientSnapshot, PhyClientState, PhyModemClient,
        PhyPendingTrack, PhyPendingTracking, PhyPllTrackClock, PhyTrackEvaluation,
        PhyTrackEvaluationFailure, PhyTrackPoisoned, PhyTrackTimeError,
    },
    tracking::parameters::PhyParamTrackRequest,
};

#[cfg(target_arch = "riscv32")]
mod wifi_integration;

/// Unique powered-radio owner carrying proof of target PHY registration.
///
/// The powered radio and raw registration proof cannot be extracted separately. This
/// prevents safe callers from pairing proof issued for one hardware epoch with
/// a different powered radio. Public APIs may inspect the calibrated state,
/// while crate-controlled role transitions move this owner without weakening
/// the association. The source-owned Wi-Fi, Bluetooth, and IEEE 802.15.4
/// client set is stored in the same owner; acquire, release, and periodic
/// evaluation either return the complete owner or an affine pending request
/// which still retains the complete hardware epoch.
///
/// This token records completion of the target registration path. It does not,
/// by itself, claim RF qualification or operational link readiness.
///
/// ```compile_fail
/// use oer_esp32s31_phy::RegisteredPhyRadio;
///
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<RegisteredPhyRadio<()>>();
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_hal::{Radio, state::Powered};
/// use oer_esp32s31_phy::{RegisteredPhyRadio, RegisteredPhyState};
///
/// fn forge<P>(radio: Radio<P, Powered>, phy: RegisteredPhyState) -> RegisteredPhyRadio<P> {
///     RegisteredPhyRadio { radio, phy }
/// }
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::{PhyConfig, PhyState, RegisteredPhyRadio};
///
/// fn replace_state<P>(registered: &mut RegisteredPhyRadio<P>) {
///     let ordinary = PhyState::new(PhyConfig::production());
///     let _old = core::mem::replace(registered.state_mut(), ordinary);
/// }
/// ```
///
/// ```compile_fail
/// use core::ops::DerefMut;
/// use oer_esp32s31_phy::{PhyState, RegisteredPhyRadio};
///
/// fn requires_mutable_state<T: DerefMut<Target = PhyState>>() {}
/// requires_mutable_state::<RegisteredPhyRadio<()>>();
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::RegisteredPhyRadio;
///
/// fn split<P>(registered: RegisteredPhyRadio<P>) {
///     let (_radio, _phy) = registered.into_raw_parts();
/// }
/// ```
#[must_use = "a registered PHY radio uniquely owns its powered hardware epoch"]
pub struct RegisteredPhyRadio<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    clients: PhyClientState,
}

impl<P> RegisteredPhyRadio<P> {
    /// Lend only cold MAC operations while retaining registration authority.
    #[cfg(target_arch = "riscv32")]
    pub fn cold_mac_parts(
        &mut self,
    ) -> (&mut P, oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal<'_>) {
        self.radio.cold_mac_parts()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn close_cold_interrupt_phase(
        &mut self,
    ) -> oer_esp32s31_hal::types::MacInterruptEnableState {
        self.radio.close_cold_interrupt_phase()
    }

    /// Inspect the calibrated state without weakening its hardware association.
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
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
        self.phy.state().calibration_cache(previous.identity())
    }

    /// Inspect the source-owned client set without exposing its raw mask.
    /// Inspect registered-policy conditions without sampling temperature, advancing
    /// deadlines or acquiring RF. Values describe retained state, not a job plan.
    pub fn inspect_tracking(
        &self,
        now_micros: u64,
    ) -> Result<crate::tracking::inspection::Inspection, crate::state::client::PhyTrackTimeError>
    {
        crate::tracking::inspection::Inspection::registered(
            &self.phy,
            self.client_snapshot(),
            now_micros,
        )
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Acquire one protocol client while retaining the registered radio epoch.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure must retain radio, PHY proof, and client owner"
    )]
    pub fn acquire_client(
        self,
        client: PhyModemClient,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredPhyClientAcquire<P>, RegisteredPhyClientAcquireFailure<P>> {
        let Self {
            radio,
            phy,
            clients,
        } = self;
        match clients.acquire(client, clock) {
            Ok(outcome) => Ok(RegisteredPhyClientAcquire {
                radio,
                phy,
                outcome,
            }),
            Err(failure) => Err(RegisteredPhyClientAcquireFailure {
                radio,
                phy,
                failure,
            }),
        }
    }

    /// Release one protocol client while retaining the registered radio epoch.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure must retain radio, PHY proof, and client owner"
    )]
    pub fn release_client(
        self,
        client: PhyModemClient,
    ) -> Result<RegisteredPhyClientRelease<P>, RegisteredPhyClientReleaseFailure<P>> {
        let Self {
            radio,
            phy,
            clients,
        } = self;
        match clients.release(client) {
            Ok(outcome) => Ok(RegisteredPhyClientRelease {
                radio,
                phy,
                outcome,
            }),
            Err(failure) => Err(RegisteredPhyClientReleaseFailure {
                radio,
                phy,
                failure,
            }),
        }
    }

    /// Evaluate one periodic callback for this exact registered epoch.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure must retain radio, PHY proof, and client owner"
    )]
    pub fn evaluate_periodic_tracking(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredPhyTrackEvaluation<P>, RegisteredPhyTrackEvaluationFailure<P>> {
        let Self {
            radio,
            phy,
            clients,
        } = self;
        match clients.evaluate_periodic_tracking(clock) {
            Ok(evaluation) => Ok(RegisteredPhyTrackEvaluation {
                radio,
                phy,
                evaluation,
            }),
            Err(failure) => Err(RegisteredPhyTrackEvaluationFailure {
                radio,
                phy,
                failure,
            }),
        }
    }

    /// Recheck the deadline after a timer or other event wakes the radio owner.
    /// Unlike a dedicated periodic callback, an early wake must not issue
    /// hardware work. Callers can arm an absolute timer using the client
    /// snapshot's `next_tracking_deadline_micros` and must obtain exclusive
    /// hardware access before committing to this consuming evaluation. Use
    /// `tracking_schedule_at` for read-only admission decisions.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the complete hardware epoch"
    )]
    pub fn evaluate_due_tracking(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredPhyTrackEvaluation<P>, RegisteredPhyTrackEvaluationFailure<P>> {
        let Self {
            radio,
            phy,
            clients,
        } = self;
        match clients.evaluate_immediate_tracking(clock) {
            Ok(evaluation) => Ok(RegisteredPhyTrackEvaluation {
                radio,
                phy,
                evaluation,
            }),
            Err(failure) => Err(RegisteredPhyTrackEvaluationFailure {
                radio,
                phy,
                failure,
            }),
        }
    }

    /// Wait for demand without transferring the physical owner to a timer.
    ///
    /// Cancellation and timer errors leave the registered owner unchanged.
    /// The returned observation is not a hardware grant: obtain the required
    /// physical exclusion and recheck the current clients before invoking the
    /// consuming execution transition. No timestamp is refreshed here.
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

/// Compact terminal registration epoch retained inside the cold async runner.
///
/// The client manager is minted only when a caller elects to keep the
/// registered owner. This avoids copying scheduler state through every cold
/// registration future variant; callers which intentionally downgrade into
/// the existing Wi-Fi cold owner never construct or immediately discard it.
#[cfg(target_arch = "riscv32")]
pub(crate) struct TargetRegisteredPhyEpoch<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
}

#[cfg(target_arch = "riscv32")]
impl<P> TargetRegisteredPhyEpoch<P> {
    pub(crate) fn from_target_completion(
        radio: Radio<P, Powered>,
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        Self {
            radio,
            phy: RegisteredPhyState::from_target_completion(state, witness),
        }
    }

    pub(crate) const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    pub(crate) fn into_registered_radio(mut self) -> RegisteredPhyRadio<P> {
        // The coupled radio still carries the epoch its registration began.
        let clients = PhyClientState::for_registration_of(
            DEFAULT_PLL_TRACK_PERIOD_MICROS,
            &*self.radio.phy_hal_mut(),
        );
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients,
        }
    }
}

/// Successful client acquisition coupled to its registered hardware epoch.
#[must_use = "client acquisition retains the registered radio owner"]
pub struct RegisteredPhyClientAcquire<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    outcome: PhyClientAcquireOutcome,
}

impl<P> RegisteredPhyClientAcquire<P> {
    pub const fn client(&self) -> PhyModemClient {
        self.outcome.client()
    }

    pub const fn was_empty(&self) -> bool {
        self.outcome.was_empty()
    }

    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.outcome.ordering()
    }

    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.outcome.request()
    }

    /// Recover the registered owner only when no hardware tracking is due.
    #[allow(
        clippy::result_large_err,
        reason = "pending work must retain the allocation-free registered hardware owner"
    )]
    pub fn into_owner(self) -> Result<RegisteredPhyRadio<P>, RegisteredPhyPendingTrack<P>> {
        let Self {
            radio,
            phy,
            outcome,
        } = self;
        match outcome.into_owner() {
            Ok(clients) => Ok(RegisteredPhyRadio {
                radio,
                phy,
                clients,
            }),
            Err(pending) => Err(RegisteredPhyPendingTrack {
                radio,
                phy,
                pending,
            }),
        }
    }
}

/// Rejected client acquisition retaining the unchanged registered owner.
#[must_use = "failed acquisition retains the registered radio owner"]
pub struct RegisteredPhyClientAcquireFailure<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    failure: PhyClientAcquireFailure,
}

impl<P> RegisteredPhyClientAcquireFailure<P> {
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }

    pub fn into_owner(self) -> RegisteredPhyRadio<P> {
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients: self.failure.into_owner(),
        }
    }
}

/// Successful client release coupled to its registered hardware epoch.
#[must_use = "client release retains the registered radio owner"]
pub struct RegisteredPhyClientRelease<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    outcome: PhyClientReleaseOutcome,
}

impl<P> RegisteredPhyClientRelease<P> {
    pub(crate) fn from_detached_parts(
        radio: Radio<P, Powered>,
        phy: RegisteredPhyState,
        outcome: PhyClientReleaseOutcome,
    ) -> Self {
        Self {
            radio,
            phy,
            outcome,
        }
    }

    pub const fn client(&self) -> PhyModemClient {
        self.outcome.client()
    }

    pub const fn is_last(&self) -> bool {
        self.outcome.is_last()
    }

    /// Resolve the saved-mask last-client decision without erasing it.
    ///
    /// The last-client variant still owns a physically powered radio. It is a
    /// candidate for a later retained-sleep or shutdown transaction, not proof
    /// that either transaction has run.
    pub fn into_disposition(self) -> RegisteredPhyClientReleaseDisposition<P> {
        let Self {
            radio,
            phy,
            outcome,
        } = self;
        let is_last = outcome.is_last();
        let clients = outcome.into_owner();
        if is_last {
            debug_assert!(clients.snapshot().is_empty());
            RegisteredPhyClientReleaseDisposition::Last(RegisteredPhyPoweredIdle {
                radio,
                phy,
                clients,
            })
        } else {
            RegisteredPhyClientReleaseDisposition::Remaining(RegisteredPhyRadio {
                radio,
                phy,
                clients,
            })
        }
    }
}

/// Physical disposition after one registered PHY client is released.
///
/// This enum prevents the last-client fact from being discarded while the
/// caller chooses the next whole-radio lifecycle transition.
#[must_use = "client release disposition retains the registered hardware epoch"]
pub enum RegisteredPhyClientReleaseDisposition<P> {
    /// At least one protocol client still owns the shared PHY lifetime.
    Remaining(RegisteredPhyRadio<P>),
    /// No protocol client remains, but RF and its clocks are still powered.
    Last(RegisteredPhyPoweredIdle<P>),
}

/// Registered, physically powered PHY epoch with no active protocol client.
///
/// This is deliberately not named `Sleeping` or `Shutdown`: creation changes
/// only the source-owned client/tracker state. A later physical lifecycle
/// transaction must consume this owner before claiming RF sleep or power-off.
#[must_use = "the powered idle owner must be retained or physically transitioned"]
pub struct RegisteredPhyPoweredIdle<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    clients: PhyClientState,
}

impl<P> RegisteredPhyPoweredIdle<P> {
    /// Inspect calibration state without weakening its hardware association.
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    /// Inspect the empty client set associated with this powered epoch.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Explicitly retain the registered radio in its powered state.
    ///
    /// This performs no hardware work. It is suitable for an always-powered
    /// policy or for reacquiring a client before physical shutdown begins.
    pub fn retain_powered(self) -> RegisteredPhyRadio<P> {
        debug_assert!(self.clients.snapshot().is_empty());
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients: self.clients,
        }
    }

    /// Observe temperature and close the physical RF domain using the current
    /// ESP32-S31 vendor ordering.
    ///
    /// Temperature acquisition is a recoverable preflight. Once the close
    /// graph begins, any hardware error poisons the epoch: callers cannot
    /// recover a powered owner from a partially closed radio.
    ///
    /// # Cancellation
    ///
    /// Once polled, this future must be driven to a terminal result. Dropping
    /// it can strand a temperature I2C transaction; after the synchronous
    /// close graph begins it can also strand a partially closed RF domain.
    #[cfg(target_arch = "riscv32")]
    #[must_use = "RF close must be driven to a terminal ownership result"]
    pub async fn close_rf<D: crate::PhyAsyncDelay>(
        mut self,
    ) -> Result<RegisteredPhyRfClosed<P>, RegisteredPhyRfCloseFailure<P>> {
        if let Err(failure) = crate::target_port::observe_temperature_before_rf_close::<P, D>(
            &mut self.radio,
            self.phy.target_state_mut(),
        )
        .await
        {
            return Err(match failure {
                crate::target_port::PhyRfCloseTemperatureFailure::Recoverable(error) => {
                    RegisteredPhyRfCloseFailure::Preparation(
                        RegisteredPhyRfClosePreparationFailure { owner: self, error },
                    )
                }
                crate::target_port::PhyRfCloseTemperatureFailure::HardwareAmbiguous(error) => {
                    RegisteredPhyRfCloseFailure::Started(RegisteredPhyRfClosePoisoned {
                        radio: self.radio,
                        phy: self.phy,
                        clients: self.clients,
                        error,
                    })
                }
            });
        }

        if let Err(error) = crate::target_port::execute_rf_close::<P, D>(&mut self.radio) {
            return Err(RegisteredPhyRfCloseFailure::Started(
                RegisteredPhyRfClosePoisoned {
                    radio: self.radio,
                    phy: self.phy,
                    clients: self.clients,
                    error,
                },
            ));
        }

        Ok(RegisteredPhyRfClosed {
            radio: self.radio,
            phy: self.phy,
            clients: self.clients,
        })
    }
}

/// Registered zero-client epoch after the physical RF close graph completes.
///
/// Platform clocks and the temperature sensor are deliberately outside this
/// state transition. A later retained-wake or full-platform-shutdown owner must make
/// those outer lifecycle decisions explicitly.
#[must_use = "the closed RF epoch must be retained, woken, or shut down"]
pub struct RegisteredPhyRfClosed<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    clients: PhyClientState,
}

impl<P> RegisteredPhyRfClosed<P> {
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    pub const fn peripheral(&self) -> &P {
        self.radio.peripheral()
    }

    /// Restore the physically closed RF domain while retaining the exact
    /// registered calibration epoch.
    ///
    /// No protocol client is acquired by this transition. Success returns a
    /// powered-idle owner from which Wi-Fi, Bluetooth or IEEE 802.15.4 may be
    /// acquired normally. Once polled, every failure is fail-stop because the
    /// first wake edge mutates the closed hardware domain.
    ///
    /// # Cancellation
    ///
    /// This future must be driven to a terminal result after its first poll.
    /// Dropping it may strand an in-flight I2C command or a partially restored
    /// RF domain.
    #[cfg(target_arch = "riscv32")]
    #[must_use = "retained RF wake must be driven to a terminal ownership result"]
    pub async fn wake_rf<D: crate::PhyAsyncDelay>(
        mut self,
    ) -> Result<RegisteredPhyPoweredIdle<P>, RegisteredPhyRfWakePoisoned<P>> {
        debug_assert!(self.clients.snapshot().is_empty());
        if let Err(error) =
            crate::target_port::execute_rf_wake::<P, D>(&mut self.radio, self.phy.state()).await
        {
            return Err(RegisteredPhyRfWakePoisoned {
                radio: self.radio,
                phy: self.phy,
                clients: self.clients,
                error,
            });
        }
        Ok(RegisteredPhyPoweredIdle {
            radio: self.radio,
            phy: self.phy,
            clients: self.clients,
        })
    }

    /// Power down the temperature sensor, release retained route clocks and
    /// return the physical radio to its cold ownership state.
    ///
    /// The previous registration proof is retired. A later power-up must run
    /// target registration again; the caller may supply the calibration cache
    /// retained from the previous registration result.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "no-alloc failure retains the exact closed radio, registration and client frontier"
    )]
    pub fn release_to_cold(
        mut self,
    ) -> Result<RegisteredPhyColdReleased<P>, RegisteredPhyColdReleaseFailure<P>> {
        debug_assert!(self.clients.snapshot().is_empty());
        oer_esp32s31_hal::phy::temperature::power_down(self.radio.phy_hal_mut());
        match self.radio.reunite_cold_after_phy_close() {
            Ok(radio) => Ok(RegisteredPhyColdReleased {
                radio,
                final_state: self.phy.into_retired_state(),
            }),
            Err(failure) => Err(RegisteredPhyColdReleaseFailure {
                _failure: failure,
                phy: self.phy,
                clients: self.clients,
            }),
        }
    }
}

/// Fail-stop epoch after retained RF wake started but did not complete.
#[cfg(target_arch = "riscv32")]
#[must_use = "partially restored RF hardware requires reset"]
pub struct RegisteredPhyRfWakePoisoned<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    clients: PhyClientState,
    error: crate::PhyTargetPortError,
}

#[cfg(target_arch = "riscv32")]
impl<P> RegisteredPhyRfWakePoisoned<P> {
    pub const fn error(&self) -> crate::PhyTargetPortError {
        self.error
    }

    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    pub const fn peripheral(&self) -> &P {
        self.radio.peripheral()
    }
}

/// Completed RF/analog shutdown and cold-route reunion.
#[cfg(target_arch = "riscv32")]
#[must_use = "cold release returns the radio owner needed for re-registration"]
pub struct RegisteredPhyColdReleased<P> {
    radio: Radio<P, oer_esp32s31_hal::owner::state::Owned>,
    final_state: PhyState,
}

#[cfg(target_arch = "riscv32")]
impl<P> RegisteredPhyColdReleased<P> {
    /// Inspect the retired state for diagnostics or cache comparison.
    pub const fn final_state(&self) -> &PhyState {
        &self.final_state
    }

    /// Recover the cold radio and capture a cache after all runtime calibration
    /// updates. `calibration_identity` must be derived by the platform from the
    /// same physical chip identity used for the next cold registration. The
    /// retired registration state is consumed and cannot authorize hardware
    /// access in the next epoch.
    pub fn into_parts(
        self,
        calibration_identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> (
        Radio<P, oer_esp32s31_hal::owner::state::Owned>,
        crate::state::PhyCalibrationCache,
    ) {
        let calibration_cache = self.final_state.calibration_cache(calibration_identity);
        (self.radio, calibration_cache)
    }
}

/// Fail-stop owner when neutral-root reconstruction rejects a pending restore.
#[cfg(target_arch = "riscv32")]
#[must_use = "failed shutdown retains an unrecoverable closed hardware epoch"]
pub struct RegisteredPhyColdReleaseFailure<P> {
    _failure: oer_esp32s31_hal::owner::ColdReunionFailure<P>,
    phy: RegisteredPhyState,
    clients: PhyClientState,
}

#[cfg(target_arch = "riscv32")]
impl<P> RegisteredPhyColdReleaseFailure<P> {
    pub const fn error(&self) -> oer_esp32s31_hal::owner::ColdReunionError {
        self._failure.error()
    }

    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }
}

/// RF-close failure classified by whether physical shutdown had begun.
#[cfg(target_arch = "riscv32")]
#[must_use = "RF-close failure retains the exact hardware epoch"]
pub enum RegisteredPhyRfCloseFailure<P> {
    /// Temperature preflight failed before any close mutation.
    Preparation(RegisteredPhyRfClosePreparationFailure<P>),
    /// A pre-close hardware transaction or physical close began and the epoch
    /// is no longer resumable.
    Started(RegisteredPhyRfClosePoisoned<P>),
}

/// Recoverable pre-close failure retaining the unchanged powered-idle owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "preparation failure retains the powered-idle owner"]
pub struct RegisteredPhyRfClosePreparationFailure<P> {
    owner: RegisteredPhyPoweredIdle<P>,
    error: crate::PhyTargetPortError,
}

#[cfg(target_arch = "riscv32")]
impl<P> RegisteredPhyRfClosePreparationFailure<P> {
    pub const fn error(&self) -> crate::PhyTargetPortError {
        self.error
    }

    pub fn into_owner(self) -> RegisteredPhyPoweredIdle<P> {
        self.owner
    }
}

/// Fail-stop epoch after physical RF close started but did not complete.
#[cfg(target_arch = "riscv32")]
#[must_use = "partially closed RF hardware requires reset"]
pub struct RegisteredPhyRfClosePoisoned<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    clients: PhyClientState,
    error: crate::PhyTargetPortError,
}

#[cfg(target_arch = "riscv32")]
impl<P> RegisteredPhyRfClosePoisoned<P> {
    pub const fn error(&self) -> crate::PhyTargetPortError {
        self.error
    }

    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    pub const fn peripheral(&self) -> &P {
        self.radio.peripheral()
    }
}

/// Rejected client release retaining the unchanged registered owner.
#[must_use = "failed release retains the registered radio owner"]
pub struct RegisteredPhyClientReleaseFailure<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    failure: PhyClientReleaseFailure,
}

impl<P> RegisteredPhyClientReleaseFailure<P> {
    pub const fn error(&self) -> PhyClientReleaseError {
        self.failure.error()
    }

    pub fn into_owner(self) -> RegisteredPhyRadio<P> {
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients: self.failure.into_owner(),
        }
    }
}

/// Periodic scheduler evaluation coupled to its registered hardware epoch.
#[must_use = "tracking evaluation retains the registered radio owner"]
pub struct RegisteredPhyTrackEvaluation<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    evaluation: PhyTrackEvaluation,
}

impl<P> RegisteredPhyTrackEvaluation<P> {
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.evaluation.request()
    }

    /// Recover the registered owner only when no hardware tracking is due.
    #[allow(
        clippy::result_large_err,
        reason = "pending work must retain the allocation-free registered hardware owner"
    )]
    pub fn into_owner(self) -> Result<RegisteredPhyRadio<P>, RegisteredPhyPendingTrack<P>> {
        let Self {
            radio,
            phy,
            evaluation,
        } = self;
        match evaluation.into_owner() {
            Ok(clients) => Ok(RegisteredPhyRadio {
                radio,
                phy,
                clients,
            }),
            Err(pending) => Err(RegisteredPhyPendingTrack {
                radio,
                phy,
                pending,
            }),
        }
    }
}

/// Invalid periodic clock sample retaining the unchanged registered owner.
#[must_use = "failed tracking evaluation retains the registered radio owner"]
pub struct RegisteredPhyTrackEvaluationFailure<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    failure: PhyTrackEvaluationFailure,
}

impl<P> RegisteredPhyTrackEvaluationFailure<P> {
    pub const fn error(&self) -> PhyTrackTimeError {
        self.failure.error()
    }

    pub fn into_owner(self) -> RegisteredPhyRadio<P> {
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients: self.failure.into_owner(),
        }
    }
}

/// Scheduler request which still owns the exact registered hardware epoch.
#[must_use = "pending tracking retains the registered radio owner"]
pub struct RegisteredPhyPendingTrack<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    pending: PhyPendingTrack,
}

impl<P> RegisteredPhyPendingTrack<P> {
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.pending.snapshot()
    }

    /// Inspect the last committed PHY state without recovering mutable access.
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    /// Inspect the integration token without separating the hardware epoch.
    pub const fn peripheral(&self) -> &P {
        self.radio.peripheral()
    }

    pub fn begin_tracking(self) -> RegisteredPhyPendingTracking<P> {
        let policy = self.phy.tracking_policy();
        RegisteredPhyPendingTracking {
            radio: self.radio,
            phy: self.phy,
            pending: self.pending.begin_tracking(policy),
        }
    }

    /// Poison a request which cannot be executed on its target epoch.
    pub fn fail(self) -> RegisteredPhyTrackPoisoned<P> {
        RegisteredPhyTrackPoisoned {
            radio: self.radio,
            phy: self.phy,
            poisoned: self.pending.fail(),
        }
    }
}

/// In-flight outer tracking transition coupled to registered hardware.
#[must_use = "in-flight tracking retains the registered radio owner"]
pub struct RegisteredPhyPendingTracking<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    pending: PhyPendingTracking,
}

impl<P> RegisteredPhyPendingTracking<P> {
    #[cfg(test)]
    pub(crate) const fn action(&self) -> crate::tracking::parameters::PhyParamTrackingAction {
        self.pending.action()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.pending.snapshot()
    }

    /// Inspect the last committed PHY state without recovering mutable access.
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    /// Inspect the integration token without separating the hardware epoch.
    pub const fn peripheral(&self) -> &P {
        self.radio.peripheral()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_tracking_parts(
        &mut self,
    ) -> (&mut P, &mut PhyHal, &mut PhyState, &mut PhyPendingTracking) {
        let Self {
            radio,
            phy,
            pending,
        } = self;
        let (platform, registers) = radio.phy_hal_parts();
        (platform, registers, phy.target_state_mut(), pending)
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "incomplete target work must retain the allocation-free hardware epoch"
    )]
    pub(crate) fn into_registered_radio(self) -> Result<RegisteredPhyRadio<P>, Self> {
        let Self {
            radio,
            phy,
            pending,
        } = self;
        match pending.into_owner() {
            Ok(clients) => Ok(RegisteredPhyRadio {
                radio,
                phy,
                clients,
            }),
            Err(pending) => Err(Self {
                radio,
                phy,
                pending,
            }),
        }
    }

    /// Explicitly poison an interrupted or externally rejected target run.
    pub fn fail(self) -> RegisteredPhyTrackPoisoned<P> {
        RegisteredPhyTrackPoisoned {
            radio: self.radio,
            phy: self.phy,
            poisoned: self.pending.fail(),
        }
    }
}

/// Terminal fail-stop registered epoch after ambiguous tracking hardware work.
#[must_use = "failed tracking poisons the registered radio epoch"]
pub struct RegisteredPhyTrackPoisoned<P> {
    radio: Radio<P, Powered>,
    phy: RegisteredPhyState,
    poisoned: PhyTrackPoisoned,
}

impl<P> RegisteredPhyTrackPoisoned<P> {
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.poisoned.request()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.poisoned.snapshot()
    }

    pub const fn peripheral(&self) -> &P {
        self.radio.peripheral()
    }
}

#[cfg(test)]
mod tests;
