//! Target-registered common-PHY ownership for the Bluetooth client.
//!
//! Bluetooth already owns its outer Controller and platform epoch. These
//! wrappers therefore retain only the target-issued PHY-registration proof and
//! the source-owned shared-PHY client set. The concrete target runners borrow
//! the outer platform and [`oer_esp32s31_hal::owner::SharedPhyHal`] for one
//! terminal operation; neither resource can escape through this module.
//!
//! Registration and Bluetooth-client acquisition are deliberately separate.
//! A completed `register_chipv7_phy` transition does not imply that the
//! Bluetooth client bit was acquired, and a pending initial tracking request
//! must finish before the client owner can advance into BTBB initialization.
//! None of these states claims RF qualification or operational Link Layer
//! readiness.

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{
        PhyClientAcquireError, PhyClientAcquireFailure, PhyClientAcquireOrdering,
        PhyClientAcquireOutcome, PhyClientReleaseError, PhyClientReleaseFailure,
        PhyClientReleaseOutcome, PhyClientSnapshot, PhyClientState, PhyModemClient,
        PhyPendingTrack, PhyPendingTracking, PhyPllTrackClock, PhyTrackEvaluation,
        PhyTrackEvaluationFailure, PhyTrackPoisoned, PhyTrackTimeError,
    },
    tracking::parameters::{PhyParamTrackRequest, PhyParamTrackingAction},
};

/// Target-registered common PHY before the Bluetooth client is acquired.
///
/// The private fields prevent safe code from replacing the registered state or
/// supplying an unrelated client-set image. This owner is neither `Copy` nor
/// `Clone` and exposes no decomposer.
#[must_use = "the target-registered Bluetooth PHY owner is unique"]
pub struct RegisteredBluetoothPhy {
    registered: RegisteredPhyState,
    clients: PhyClientState,
}

impl RegisteredBluetoothPhy {
    /// Mint the Bluetooth owner after one concrete target registration run.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        Self {
            registered: RegisteredPhyState::from_target_completion(state, witness),
            clients: PhyClientState::for_registered_epoch(
                crate::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
            ),
        }
    }

    /// Borrow the target-registered PHY state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect the source-owned client set without exposing its bit image.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Acquire exactly the Bluetooth shared-PHY client.
    ///
    /// A due initial tracking request remains affine with the registered state
    /// and must pass through the target tracking runner before the caller can
    /// obtain [`RegisteredBluetoothPhyClient`].
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the registered PHY and client owner"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyClientAcquire, RegisteredBluetoothPhyClientAcquireFailure>
    {
        let Self {
            registered,
            clients,
        } = self;
        match clients.acquire(PhyModemClient::Bluetooth, clock) {
            Ok(outcome) => Ok(RegisteredBluetoothPhyClientAcquire {
                registered,
                outcome,
            }),
            Err(failure) => Err(RegisteredBluetoothPhyClientAcquireFailure {
                registered,
                failure,
            }),
        }
    }
}

/// Successful Bluetooth-client acquisition retaining target registration.
#[must_use = "Bluetooth client acquisition retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyClientAcquire {
    registered: RegisteredPhyState,
    outcome: PhyClientAcquireOutcome,
}

impl RegisteredBluetoothPhyClientAcquire {
    /// Return the reviewed first/later-client ordering.
    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.outcome.ordering()
    }

    /// Borrow the immediate tracking request, when one is due.
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.outcome.request()
    }

    /// Finish the software acquisition edge or retain pending target work.
    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the allocation-free registered PHY owner"
    )]
    pub fn into_owner(
        self,
    ) -> Result<RegisteredBluetoothPhyClient, RegisteredBluetoothPhyPendingTrack> {
        let Self {
            registered,
            outcome,
        } = self;
        match outcome.into_owner() {
            Ok(clients) => Ok(RegisteredBluetoothPhyClient {
                registered,
                clients,
            }),
            Err(pending) => Err(RegisteredBluetoothPhyPendingTrack {
                registered,
                pending,
            }),
        }
    }
}

/// Rejected Bluetooth-client acquisition retaining the unchanged owner.
#[must_use = "failed Bluetooth client acquisition retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyClientAcquireFailure {
    registered: RegisteredPhyState,
    failure: PhyClientAcquireFailure,
}

impl RegisteredBluetoothPhyClientAcquireFailure {
    /// Inspect the exact source-owned client-set rejection.
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }

    /// Recover the unchanged pre-acquisition owner.
    pub fn into_owner(self) -> RegisteredBluetoothPhy {
        RegisteredBluetoothPhy {
            registered: self.registered,
            clients: self.failure.into_owner(),
        }
    }
}

/// Target-registered PHY with the Bluetooth client acquired and settled.
///
/// This is the lower owner that a Bluetooth Controller may retain across BTBB
/// and BLE-engine initialization for an always-awake profile. It proves neither
/// a per-event RF-ready instant nor operational radio-engine readiness.
#[must_use = "the registered Bluetooth PHY client owner is unique"]
pub struct RegisteredBluetoothPhyClient {
    registered: RegisteredPhyState,
    clients: PhyClientState,
}

impl RegisteredBluetoothPhyClient {
    /// Change only the diagnostic thermal thresholds, returning their old policy.
    /// This does not mutate samples, acknowledge demand or grant RF access.
    /// The caller retains exclusive client authority and must arrange quiescence.
    #[cfg(target_arch = "riscv32")]
    pub fn set_tracking_debug(
        &mut self,
        debug: crate::state::PhyTemperatureTrackingDebug,
    ) -> crate::state::PhyTemperatureTrackingDebug {
        let state = self.registered.target_state_mut();
        let old = state.temperature_tracking_debug();
        state.set_temperature_tracking_debug(debug.first, debug.second);
        old
    }

    /// Borrow the target-registered PHY state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect the settled source-owned client set.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Release the Bluetooth client while retaining the registered common-PHY
    /// state and the source-owned last-client fact.
    ///
    /// This is a software ownership transition only. The outer Controller must
    /// still quiesce BTBB, timers and interrupts and reunite its physical radio
    /// owners before any RF close or cold release may begin.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered Bluetooth owner"
    )]
    pub fn release_phy_client(
        self,
    ) -> Result<RegisteredBluetoothPhyClientRelease, RegisteredBluetoothPhyClientReleaseFailure>
    {
        let Self {
            registered,
            clients,
        } = self;
        match clients.release(PhyModemClient::Bluetooth) {
            Ok(outcome) => Ok(RegisteredBluetoothPhyClientRelease {
                registered,
                outcome,
            }),
            Err(failure) => Err(RegisteredBluetoothPhyClientReleaseFailure {
                registered,
                failure,
            }),
        }
    }

    /// Inspect registered-policy conditions without sampling temperature,
    /// advancing deadlines or acquiring the shared RF hardware.
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

    /// Evaluate one source-compatible periodic callback for this Bluetooth
    /// client without acquiring physical RF access.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered Bluetooth owner"
    )]
    pub fn evaluate_periodic_tracking(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyTrackEvaluation, RegisteredBluetoothPhyTrackEvaluationFailure>
    {
        let Self {
            registered,
            clients,
        } = self;
        match clients.evaluate_periodic_tracking(clock) {
            Ok(evaluation) => Ok(RegisteredBluetoothPhyTrackEvaluation {
                registered,
                evaluation,
            }),
            Err(failure) => Err(RegisteredBluetoothPhyTrackEvaluationFailure {
                registered,
                failure,
            }),
        }
    }

    /// Recheck an absolute deadline after a timer or another Controller event
    /// wakes the owner. An early wake returns the unchanged settled owner.
    /// A due request still requires Controller quiescence and physical shared-
    /// PHY admission before target execution begins.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the allocation-free registered Bluetooth owner"
    )]
    pub fn evaluate_due_tracking(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyTrackEvaluation, RegisteredBluetoothPhyTrackEvaluationFailure>
    {
        let Self {
            registered,
            clients,
        } = self;
        match clients.evaluate_immediate_tracking(clock) {
            Ok(evaluation) => Ok(RegisteredBluetoothPhyTrackEvaluation {
                registered,
                evaluation,
            }),
            Err(failure) => Err(RegisteredBluetoothPhyTrackEvaluationFailure {
                registered,
                failure,
            }),
        }
    }

    /// Wait for scheduling demand while borrowing, rather than transferring,
    /// the registered Bluetooth owner to the timer.
    ///
    /// The result is an observation only. It neither refreshes timestamps nor
    /// grants access to Controller, BTBB or common-PHY hardware.
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

/// Successful Bluetooth-client release retaining its registered PHY epoch.
///
/// The result is detached from the Controller's physical owners. It proves
/// only the source-owned client transition and cannot close RF by itself.
#[must_use = "Bluetooth release must be retained through Controller teardown"]
pub struct RegisteredBluetoothPhyClientRelease {
    registered: RegisteredPhyState,
    outcome: PhyClientReleaseOutcome,
}

/// Last Bluetooth client after the complete target RF-close graph.
/// Controller, timer, temperature power and platform clocks still belong to
/// the outer lifecycle. Retiring registration cannot release those owners.
#[must_use = "retain the closed registration until physical owner reunion"]
pub struct RegisteredBluetoothPhyRfClosed {
    registered: RegisteredPhyState,
}

impl RegisteredBluetoothPhyRfClosed {
    /// Read the final state while retaining its physical-close provenance.
    pub const fn state(&self) -> &PhyState {
        self.registered.state()
    }
    /// Final calibrated state, including the pre-close temperature observation.
    #[cfg(target_arch = "riscv32")]
    pub fn into_retired_state(self) -> PhyState {
        self.registered.into_retired_state()
    }
}

/// RF-close failure retaining the released client and registered epoch.
#[must_use = "failed RF close retains the physical shutdown obligation"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPhyRfCloseFailure {
    _owner: RegisteredBluetoothPhyClientRelease,
    error: crate::PhyTargetPortError,
    retryable: bool,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPhyRfCloseFailure {
    /// Whether preparation or RF close left an ambiguous hardware epoch.
    /// A false result retains the unchanged release; it does not prove RF off.
    pub const fn hardware_ambiguous(&self) -> bool {
        !self.retryable
    }

    /// The first failing preparation or hardware operation.
    pub const fn error(&self) -> crate::PhyTargetPortError {
        self.error
    }

    /// Recover the exact release only when preparation completed without any
    /// ambiguous hardware operation. A started close remains owned by failure.
    #[allow(
        clippy::result_large_err,
        reason = "both branches retain the affine PHY registration"
    )]
    pub fn into_retry(self) -> Result<RegisteredBluetoothPhyClientRelease, Self> {
        if self.retryable {
            Ok(self._owner)
        } else {
            Err(self)
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl RegisteredBluetoothPhyClientRelease {
    /// Close the last client's physical RF through the outer stopped Controller.
    ///
    /// The caller retains the matching platform, stopped Controller/BTBB,
    /// inactive IRQ routes and drained timers for this exact registration.
    /// No second client may use RF during the operation. Non-final release is
    /// rejected before MMIO. The temperature preflight and close graph are the
    /// same target implementation used by the Wi-Fi radio lifecycle.
    ///
    /// # Cancellation
    /// Once polled, drive this future to completion and retain the outer owners.
    /// A completed preparation failure may return the original release for retry;
    /// cancellation or ambiguous hardware failure never authorizes reuse.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the complete registered PHY epoch without allocation"
    )]
    pub async fn close_rf<P, D: crate::PhyAsyncDelay>(
        mut self,
        platform: &mut P,
        registers: &mut oer_esp32s31_hal::owner::SharedPhyHal<
            '_,
            oer_esp32s31_hal::owner::route::Bluetooth,
        >,
    ) -> Result<RegisteredBluetoothPhyRfClosed, BluetoothPhyRfCloseFailure> {
        if !self.is_last() {
            return Err(BluetoothPhyRfCloseFailure {
                _owner: self,
                error: crate::PhyTargetPortError::HardwareInvariant,
                retryable: true,
            });
        }
        if let Err(error) = crate::target_port::close_bluetooth_rf::<P, D>(
            platform,
            registers,
            self.registered.target_state_mut(),
        )
        .await
        {
            let (error, retryable) = match error {
                crate::target_port::PhyRfCloseTemperatureFailure::Recoverable(error) => {
                    (error, true)
                }
                crate::target_port::PhyRfCloseTemperatureFailure::HardwareAmbiguous(error) => {
                    (error, false)
                }
            };
            return Err(BluetoothPhyRfCloseFailure {
                _owner: self,
                error,
                retryable,
            });
        }
        Ok(RegisteredBluetoothPhyRfClosed {
            registered: self.registered,
        })
    }
}

impl RegisteredBluetoothPhyClientRelease {
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

    /// Recover the registered pre-acquisition owner only when this release
    /// removed the last client.
    ///
    /// A future shared-radio composition may retain another protocol client;
    /// that case returns this exact release unchanged instead of erasing the
    /// saved-mask disposition.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc error is the exact affine release owner and cannot be boxed or reduced"
    )]
    pub fn into_registered_phy(self) -> Result<RegisteredBluetoothPhy, Self> {
        if !self.outcome.is_last() {
            return Err(self);
        }
        let Self {
            registered,
            outcome,
        } = self;
        let clients = outcome.into_owner();
        debug_assert!(clients.snapshot().is_empty());
        Ok(RegisteredBluetoothPhy {
            registered,
            clients,
        })
    }
}

/// Rejected Bluetooth-client release retaining the unchanged registered owner.
#[must_use = "failed release retains the registered Bluetooth PHY owner"]
pub struct RegisteredBluetoothPhyClientReleaseFailure {
    registered: RegisteredPhyState,
    failure: PhyClientReleaseFailure,
}

impl RegisteredBluetoothPhyClientReleaseFailure {
    /// Inspect the exact source-owned release rejection.
    pub const fn error(&self) -> PhyClientReleaseError {
        self.failure.error()
    }

    /// Recover the owner left unchanged by the rejected release.
    pub fn into_owner(self) -> RegisteredBluetoothPhyClient {
        RegisteredBluetoothPhyClient {
            registered: self.registered,
            clients: self.failure.into_owner(),
        }
    }
}

/// Scheduler evaluation retaining the exact registered Bluetooth epoch.
#[must_use = "tracking evaluation retains the registered Bluetooth owner"]
pub struct RegisteredBluetoothPhyTrackEvaluation {
    registered: RegisteredPhyState,
    evaluation: PhyTrackEvaluation,
}

impl RegisteredBluetoothPhyTrackEvaluation {
    /// Borrow the request emitted by this evaluation, when one is due.
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.evaluation.request()
    }

    /// Recover the settled owner or retain the due request in its affine
    /// pending state.
    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the allocation-free registered Bluetooth owner"
    )]
    pub fn into_owner(
        self,
    ) -> Result<RegisteredBluetoothPhyClient, RegisteredBluetoothPhyPendingTrack> {
        let Self {
            registered,
            evaluation,
        } = self;
        match evaluation.into_owner() {
            Ok(clients) => Ok(RegisteredBluetoothPhyClient {
                registered,
                clients,
            }),
            Err(pending) => Err(RegisteredBluetoothPhyPendingTrack {
                registered,
                pending,
            }),
        }
    }
}

/// Invalid clock evaluation retaining the unchanged Bluetooth PHY owner.
#[must_use = "failed tracking evaluation retains the registered Bluetooth owner"]
pub struct RegisteredBluetoothPhyTrackEvaluationFailure {
    registered: RegisteredPhyState,
    failure: PhyTrackEvaluationFailure,
}

impl RegisteredBluetoothPhyTrackEvaluationFailure {
    /// Inspect the monotonic-time failure without recovering mutable state.
    pub const fn error(&self) -> PhyTrackTimeError {
        self.failure.error()
    }

    /// Recover the owner left unchanged by the rejected clock evaluation.
    pub fn into_owner(self) -> RegisteredBluetoothPhyClient {
        RegisteredBluetoothPhyClient {
            registered: self.registered,
            clients: self.failure.into_owner(),
        }
    }
}

/// Pending immediate tracking after Bluetooth-client acquisition.
#[must_use = "pending Bluetooth tracking retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyPendingTrack {
    registered: RegisteredPhyState,
    pending: PhyPendingTrack,
}

impl RegisteredBluetoothPhyPendingTrack {
    /// Borrow the exact source-owned tracking request.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    /// Begin tracking with policy projected from this registered PHY epoch.
    pub fn begin_tracking(self) -> RegisteredBluetoothPhyPendingTracking {
        let policy = self.registered.tracking_policy();
        RegisteredBluetoothPhyPendingTracking {
            registered: self.registered,
            pending: self.pending.begin_tracking(policy),
        }
    }

    /// Enter fail-stop state without attempting target tracking work.
    pub fn fail(self) -> RegisteredBluetoothPhyTrackPoisoned {
        RegisteredBluetoothPhyTrackPoisoned {
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// In-flight Bluetooth tracking retaining the target registration proof.
#[must_use = "in-flight Bluetooth tracking retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyPendingTracking {
    registered: RegisteredPhyState,
    pending: PhyPendingTracking,
}

impl RegisteredBluetoothPhyPendingTracking {
    /// Inspect the next semantic target operation.
    pub const fn action(&self) -> PhyParamTrackingAction {
        self.pending.action()
    }

    /// Borrow the last committed target-registered PHY state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_tracking_parts(&mut self) -> (&mut PhyState, &mut PhyPendingTracking) {
        (self.registered.target_state_mut(), &mut self.pending)
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "incomplete target work retains the allocation-free Bluetooth PHY epoch"
    )]
    pub(crate) fn into_client_owner(self) -> Result<RegisteredBluetoothPhyClient, Self> {
        let Self {
            registered,
            pending,
        } = self;
        match pending.into_owner() {
            Ok(clients) => Ok(RegisteredBluetoothPhyClient {
                registered,
                clients,
            }),
            Err(pending) => Err(Self {
                registered,
                pending,
            }),
        }
    }

    /// Consume ambiguous work into a non-recoverable owner.
    pub fn fail(self) -> RegisteredBluetoothPhyTrackPoisoned {
        RegisteredBluetoothPhyTrackPoisoned {
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// Fail-stop Bluetooth PHY epoch after ambiguous tracking hardware work.
#[must_use = "failed Bluetooth tracking poisons the registered PHY epoch"]
pub struct RegisteredBluetoothPhyTrackPoisoned {
    registered: RegisteredPhyState,
    poisoned: PhyTrackPoisoned,
}

impl RegisteredBluetoothPhyTrackPoisoned {
    /// Borrow the last committed semantic state for diagnostics.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Borrow the exact tracking request which poisoned the epoch.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.poisoned.request()
    }

    /// Inspect the retained client set without any recovery authority.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.poisoned.snapshot()
    }
}

#[cfg(test)]
mod tests;
