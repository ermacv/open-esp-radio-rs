//! Coupled ownership of a powered radio and its target-registration proof.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::PhyHal;
use open_esp_radio_esp32s31_hal::{Radio, state::Powered};

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
    tracking::parameters::{PhyParamTrackRequest, PhyParamTrackingAction},
};

#[path = "registered_ieee802154.rs"]
mod ieee802154;

pub use ieee802154::{
    RegisteredIeee802154Client, RegisteredIeee802154ClientAcquire,
    RegisteredIeee802154ClientAcquireFailure, RegisteredIeee802154Clocked,
    RegisteredIeee802154FoundationConfigured, RegisteredIeee802154FoundationTransitionFailure,
    RegisteredIeee802154MacPolicyConfigured, RegisteredIeee802154MacPolicyRecovery,
    RegisteredIeee802154MacPolicyTransitionFailure, RegisteredIeee802154OperationCompleted,
    RegisteredIeee802154OperationFailed, RegisteredIeee802154PendingTrack,
    RegisteredIeee802154PendingTracking, RegisteredIeee802154Reset,
    RegisteredIeee802154ResetTransitionFailure, RegisteredIeee802154TimingReady,
    RegisteredIeee802154TrackPoisoned,
};

/// Unique powered-radio owner carrying proof of target PHY registration.
///
/// The radio and proof have private fields and no public decomposer. This
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
/// use open_esp_radio_esp32s31_phy::RegisteredPhyRadio;
///
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<RegisteredPhyRadio<()>>();
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_hal::{Radio, state::Powered};
/// use open_esp_radio_esp32s31_phy::{RegisteredPhyRadio, RegisteredPhyState};
///
/// fn forge<P>(radio: Radio<P, Powered>, phy: RegisteredPhyState) -> RegisteredPhyRadio<P> {
///     RegisteredPhyRadio { radio, phy }
/// }
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::{PhyConfig, PhyState, RegisteredPhyRadio};
///
/// fn replace_state<P>(registered: &mut RegisteredPhyRadio<P>) {
///     let ordinary = PhyState::new(PhyConfig::production());
///     let _old = core::mem::replace(registered.state_mut(), ordinary);
/// }
/// ```
///
/// ```compile_fail
/// use core::ops::DerefMut;
/// use open_esp_radio_esp32s31_phy::{PhyState, RegisteredPhyRadio};
///
/// fn requires_mutable_state<T: DerefMut<Target = PhyState>>() {}
/// requires_mutable_state::<RegisteredPhyRadio<()>>();
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_phy::RegisteredPhyRadio;
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
    /// Inspect the calibrated state without weakening its hardware association.
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
    }

    /// Inspect the source-owned client set without exposing its raw mask.
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

    /// Atomically discard proof at a crate-controlled legacy boundary.
    #[allow(
        dead_code,
        reason = "the legacy caller is target-only while this owner is also host-checked"
    )]
    pub(crate) fn into_ordinary_parts(self) -> (Radio<P, Powered>, PhyState) {
        (self.radio, self.phy.into_ordinary_state())
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

    pub(crate) fn into_registered_radio(self) -> RegisteredPhyRadio<P> {
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
        }
    }

    pub(crate) fn into_ordinary_parts(self) -> (Radio<P, Powered>, PhyState) {
        (self.radio, self.phy.into_ordinary_state())
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
    pub const fn client(&self) -> PhyModemClient {
        self.outcome.client()
    }

    pub const fn is_last(&self) -> bool {
        self.outcome.is_last()
    }

    pub fn into_owner(self) -> RegisteredPhyRadio<P> {
        RegisteredPhyRadio {
            radio: self.radio,
            phy: self.phy,
            clients: self.outcome.into_owner(),
        }
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
    pub const fn action(&self) -> PhyParamTrackingAction {
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
