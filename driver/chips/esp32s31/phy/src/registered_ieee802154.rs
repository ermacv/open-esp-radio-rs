//! Proof-preserving ESP32-S31 IEEE 802.15.4 lifecycle.
//!
//! These wrappers keep the target-issued [`RegisteredPhyState`] beside the
//! exact whole-radio HAL role owner through clock, reset, foundation and static
//! MAC-policy transitions. None of the types in this module can be decomposed
//! into an independently reusable proof and hardware owner.
//!
//! This lifecycle is deliberately incomplete. Its timing-ready transition
//! establishes the recovered common BTBB body and protocol timing after
//! terminal common-PHY registration, but does not claim RF qualification,
//! ownership of a shared 160 MHz PLL refcount, IRQ routing, DMA ownership, or
//! an operational IEEE 802.15.4 MAC.
//!
//! The dedicated HAL route establishes IEEE clocks before common-PHY client
//! registration, matching the public enable order. The target-only PHY runner
//! executes the existing `TargetPhyRegisterPort` directly over
//! `Ieee802154Clocked::common_phy_parts`; it neither converts the owner through
//! Wi-Fi nor treats model completion as RF qualification.
//!
//! The issued owner cannot be duplicated:
//!
//! ```compile_fail
//! use open_esp_radio_esp32s31_phy::RegisteredIeee802154Clocked;
//!
//! fn requires_clone<T: Clone>() {}
//! requires_clone::<RegisteredIeee802154Clocked<()>>();
//! ```
//!
//! Safe callers cannot construct a registered IEEE owner from separate parts:
//!
//! ```compile_fail
//! use open_esp_radio_esp32s31_hal::Ieee802154Clocked;
//! use open_esp_radio_esp32s31_phy::{
//!     RegisteredIeee802154Clocked, RegisteredPhyState,
//! };
//!
//! fn forge<P>(
//!     role: Ieee802154Clocked<P>,
//!     registered: RegisteredPhyState,
//! ) -> RegisteredIeee802154Clocked<P> {
//!     RegisteredIeee802154Clocked { role, registered }
//! }
//! ```
//!
//! Nor can safe callers decompose an issued owner and splice its proof into a
//! different hardware epoch:
//!
//! ```compile_fail
//! use open_esp_radio_esp32s31_hal::Ieee802154Clocked;
//! use open_esp_radio_esp32s31_phy::{
//!     RegisteredIeee802154Clocked, RegisteredPhyState,
//! };
//!
//! fn split<P>(
//!     owner: RegisteredIeee802154Clocked<P>,
//! ) -> (Ieee802154Clocked<P>, RegisteredPhyState) {
//!     owner.into_parts()
//! }
//! ```
//!
//! A registered common-PHY owner cannot skip the protocol timing transition:
//!
//! ```compile_fail
//! use open_esp_radio_esp32s31_hal::Ieee802154PlatformControl;
//! use open_esp_radio_esp32s31_phy::RegisteredIeee802154Clocked;
//!
//! fn skip_timing<P: Ieee802154PlatformControl>(owner: RegisteredIeee802154Clocked<P>) {
//!     let _reset = owner.reset_mac();
//! }
//! ```
//!
//! Reset failure cannot erase timing evidence and recover the pre-timing type:
//!
//! ```compile_fail
//! use open_esp_radio_esp32s31_phy::RegisteredIeee802154ResetTransitionFailure;
//!
//! fn erase_timing<P>(failure: RegisteredIeee802154ResetTransitionFailure<P>) {
//!     let _clocked = failure.into_clocked();
//! }
//! ```

use core::fmt;

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::SharedPhyHal;
use open_esp_radio_esp32s31_hal::{
    Ieee802154Clocked, Ieee802154FoundationCheckpoint, Ieee802154FoundationConfigured,
    Ieee802154FoundationTransitionFailure, Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint,
    Ieee802154MacPolicyConfigured, Ieee802154MacPolicyRecovery,
    Ieee802154MacPolicyTransitionFailure, Ieee802154OperationCompleted, Ieee802154OperationFailed,
    Ieee802154OperationPollBudget, Ieee802154PlatformControl, Ieee802154PolledOperationEvidence,
    Ieee802154PolledOperationFailure, Ieee802154ReadbackError, Ieee802154Reset,
    Ieee802154ResetCheckpoint, Ieee802154ResetTransitionFailure,
    Ieee802154TimingReady as HalIeee802154TimingReady,
};

#[cfg(target_arch = "riscv32")]
use crate::phy_client::DEFAULT_PLL_TRACK_PERIOD_MICROS;
use crate::{
    PhyState, RegisteredPhyState,
    phy_client::{
        PhyClientAcquireError, PhyClientAcquireFailure, PhyClientAcquireOrdering,
        PhyClientAcquireOutcome, PhyClientSnapshot, PhyClientState, PhyModemClient,
        PhyPendingTrack, PhyPendingTracking, PhyPllTrackClock, PhyTrackPoisoned,
    },
    phy_param_tracking::{
        PhyParamTrackRequest, PhyParamTrackingAction, PhyParamTrackingParameters,
    },
};

/// Registered whole-radio owner after IEEE 802.15.4 clock readback.
///
/// This state preserves target PHY-registration authority, but does not prove
/// PHY/RF qualification, BTBB or PLL ownership, a shared-clock client bit, IRQ,
/// DMA, or operational MAC readiness.
#[must_use = "the registered IEEE 802.15.4 clock owner is unique"]
pub struct RegisteredIeee802154Clocked<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    clients: PhyClientState,
}

impl<P> RegisteredIeee802154Clocked<P> {
    /// Couple the exact target-completed state to its dedicated IEEE owner.
    ///
    /// The private target witness means a caller-driven model transition
    /// cannot reach this constructor. Keeping the constructor in this module
    /// also prevents the role and proof from becoming separately observable.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        role: Ieee802154Clocked<P>,
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        Self {
            role,
            registered: RegisteredPhyState::from_target_completion(state, witness),
            clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
        }
    }

    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Acquire the dedicated IEEE 802.15.4 bit before BTBB/timing work.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the IEEE role, PHY proof, and client owner"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredIeee802154ClientAcquire<P>, RegisteredIeee802154ClientAcquireFailure<P>>
    {
        let Self {
            role,
            registered,
            clients,
        } = self;
        match clients.acquire(PhyModemClient::Ieee802154, clock) {
            Ok(outcome) => Ok(RegisteredIeee802154ClientAcquire {
                role,
                registered,
                outcome,
            }),
            Err(failure) => Err(RegisteredIeee802154ClientAcquireFailure {
                role,
                registered,
                failure,
            }),
        }
    }
}

impl<P: Ieee802154PlatformControl> RegisteredIeee802154Clocked<P> {
    /// Borrow the integration token without separating it from the proof.
    pub fn peripheral(&self) -> &P {
        self.role.peripheral()
    }
}

/// Successful IEEE client acquisition retaining the registered role owner.
#[must_use = "IEEE client acquisition retains the registered role owner"]
pub struct RegisteredIeee802154ClientAcquire<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    outcome: PhyClientAcquireOutcome,
}

impl<P> RegisteredIeee802154ClientAcquire<P> {
    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.outcome.ordering()
    }

    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.outcome.request()
    }

    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the allocation-free IEEE role and registered PHY"
    )]
    pub fn into_owner(
        self,
    ) -> Result<RegisteredIeee802154Client<P>, RegisteredIeee802154PendingTrack<P>> {
        let Self {
            role,
            registered,
            outcome,
        } = self;
        match outcome.into_owner() {
            Ok(clients) => Ok(RegisteredIeee802154Client {
                role,
                registered,
                clients,
            }),
            Err(pending) => Err(RegisteredIeee802154PendingTrack {
                role,
                registered,
                pending,
            }),
        }
    }
}

/// Rejected IEEE client acquisition retaining the unchanged registered role.
#[must_use = "failed IEEE client acquisition retains the registered role owner"]
pub struct RegisteredIeee802154ClientAcquireFailure<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    failure: PhyClientAcquireFailure,
}

impl<P> RegisteredIeee802154ClientAcquireFailure<P> {
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }

    pub fn into_owner(self) -> RegisteredIeee802154Clocked<P> {
        RegisteredIeee802154Clocked {
            role: self.role,
            registered: self.registered,
            clients: self.failure.into_owner(),
        }
    }
}

/// Registered IEEE route after the shared-PHY client bit was acquired.
#[must_use = "the registered IEEE client owner is unique"]
pub struct RegisteredIeee802154Client<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    clients: PhyClientState,
}

impl<P> RegisteredIeee802154Client<P> {
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Initialize common BTBB and IEEE timing only after client acquisition.
    pub fn initialize_baseband_and_ieee802154_timing(self) -> RegisteredIeee802154TimingReady<P> {
        let parts = establish_registered_timing(self, |role, registered| {
            let prerequisite =
                crate::ieee802154_timing_boundary::from_terminal_registration(role, registered);
            role.initialize_baseband_and_ieee802154_timing(prerequisite)
        });

        RegisteredIeee802154TimingReady {
            role: parts.role,
            prerequisites: RegisteredIeee802154Prerequisites {
                registered: parts.registered,
                clients: parts.clients,
                timing: parts.timing,
            },
        }
    }
}

/// Pending immediate tracking request retaining the dedicated IEEE role.
#[must_use = "pending IEEE tracking retains the registered role owner"]
pub struct RegisteredIeee802154PendingTrack<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    pending: PhyPendingTrack,
}

impl<P> RegisteredIeee802154PendingTrack<P> {
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    pub fn begin_tracking(
        self,
        parameters: PhyParamTrackingParameters,
    ) -> RegisteredIeee802154PendingTracking<P> {
        RegisteredIeee802154PendingTracking {
            role: self.role,
            registered: self.registered,
            pending: self.pending.begin_tracking(parameters),
        }
    }

    pub fn fail(self) -> RegisteredIeee802154TrackPoisoned<P> {
        RegisteredIeee802154TrackPoisoned {
            role: self.role,
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// In-flight tracking transition retaining the dedicated IEEE role.
#[must_use = "in-flight IEEE tracking retains the registered role owner"]
pub struct RegisteredIeee802154PendingTracking<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    pending: PhyPendingTracking,
}

impl<P> RegisteredIeee802154PendingTracking<P> {
    pub const fn action(&self) -> PhyParamTrackingAction {
        self.pending.action()
    }

    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_tracking_parts(
        &mut self,
    ) -> (
        &mut P,
        SharedPhyHal<'_>,
        &mut PhyState,
        &mut PhyPendingTracking,
    )
    where
        P: open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl,
    {
        let Self {
            role,
            registered,
            pending,
        } = self;
        let (platform, registers) = role.common_phy_parts();
        (platform, registers, registered.target_state_mut(), pending)
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "incomplete target work retains the allocation-free IEEE hardware epoch"
    )]
    pub(crate) fn into_client_owner(self) -> Result<RegisteredIeee802154Client<P>, Self> {
        let Self {
            role,
            registered,
            pending,
        } = self;
        match pending.into_owner() {
            Ok(clients) => Ok(RegisteredIeee802154Client {
                role,
                registered,
                clients,
            }),
            Err(pending) => Err(Self {
                role,
                registered,
                pending,
            }),
        }
    }

    pub fn fail(self) -> RegisteredIeee802154TrackPoisoned<P> {
        RegisteredIeee802154TrackPoisoned {
            role: self.role,
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// Fail-stop dedicated IEEE epoch after ambiguous tracking hardware work.
#[must_use = "failed IEEE tracking poisons the registered role epoch"]
pub struct RegisteredIeee802154TrackPoisoned<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    poisoned: PhyTrackPoisoned,
}

impl<P> RegisteredIeee802154TrackPoisoned<P> {
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.poisoned.request()
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.poisoned.snapshot()
    }
}

impl<P: Ieee802154PlatformControl> RegisteredIeee802154TrackPoisoned<P> {
    pub fn peripheral(&self) -> &P {
        self.role.peripheral()
    }
}

struct RegisteredIeee802154TimingParts<P, Timing> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
    clients: PhyClientState,
    timing: Timing,
}

fn establish_registered_timing<P, Timing>(
    owner: RegisteredIeee802154Client<P>,
    transition: impl FnOnce(&mut Ieee802154Clocked<P>, &RegisteredPhyState) -> Timing,
) -> RegisteredIeee802154TimingParts<P, Timing> {
    let RegisteredIeee802154Client {
        mut role,
        registered,
        clients,
    } = owner;
    let timing = transition(&mut role, &registered);
    RegisteredIeee802154TimingParts {
        role,
        registered,
        clients,
        timing,
    }
}

struct RegisteredIeee802154Prerequisites {
    registered: RegisteredPhyState,
    clients: PhyClientState,
    timing: HalIeee802154TimingReady,
}

impl RegisteredIeee802154Prerequisites {
    const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    const fn timing_gain_parameter(&self) -> u8 {
        self.timing.gain_parameter()
    }

    const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }
}

/// Registered whole-radio owner after common BTBB and IEEE timing completed.
///
/// This is the first production registered state allowed to pulse the MAC
/// reset. It retains both the terminal common-PHY proof and the affine PAC
/// timing marker; neither can be separated from the exact HAL route.
#[must_use = "the registered IEEE 802.15.4 timing-ready owner is unique"]
pub struct RegisteredIeee802154TimingReady<P> {
    role: Ieee802154Clocked<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154TimingReady<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the gain byte projected from the terminal common-PHY state.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Inspect the retained shared-PHY client set.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.prerequisites.client_snapshot()
    }
}

impl<P: Ieee802154PlatformControl> RegisteredIeee802154TimingReady<P> {
    /// Borrow the integration token without separating it from either proof.
    pub fn peripheral(&self) -> &P {
        self.role.peripheral()
    }

    /// Pulse and read back the private IEEE 802.15.4 MAC and APB resets.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure must retain the exact HAL owner and PHY proof"
    )]
    pub fn reset_mac(
        self,
    ) -> Result<RegisteredIeee802154Reset<P>, RegisteredIeee802154ResetTransitionFailure<P>> {
        let Self {
            role,
            prerequisites,
        } = self;
        match preserve_prerequisites(role, prerequisites, Ieee802154Clocked::reset_mac) {
            Ok((role, prerequisites)) => Ok(RegisteredIeee802154Reset {
                role,
                prerequisites,
            }),
            Err((failure, prerequisites)) => Err(RegisteredIeee802154ResetTransitionFailure {
                failure,
                prerequisites,
            }),
        }
    }
}

/// Registered whole-radio owner after the private reset sequence read back.
///
/// Reset completion retains common-PHY and protocol timing evidence but is not
/// RF qualification and establishes no shared PLL refcount, interrupt route,
/// DMA frontier, or operational MAC state.
#[must_use = "the registered IEEE 802.15.4 reset owner is unique"]
pub struct RegisteredIeee802154Reset<P> {
    role: Ieee802154Reset<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154Reset<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }
}

impl<P: Ieee802154PlatformControl> RegisteredIeee802154Reset<P> {
    /// Borrow the integration token without separating it from the proof.
    pub fn peripheral(&self) -> &P {
        self.role.peripheral()
    }

    /// Configure and read back the interrupt-masked MAC foundation.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure must retain the exact HAL owner and PHY proof"
    )]
    pub fn configure_foundation(
        self,
    ) -> Result<
        RegisteredIeee802154FoundationConfigured<P>,
        RegisteredIeee802154FoundationTransitionFailure<P>,
    > {
        let Self {
            role,
            prerequisites,
        } = self;
        match preserve_prerequisites(role, prerequisites, Ieee802154Reset::configure_foundation) {
            Ok((role, prerequisites)) => Ok(RegisteredIeee802154FoundationConfigured {
                role,
                prerequisites,
            }),
            Err((failure, prerequisites)) => Err(RegisteredIeee802154FoundationTransitionFailure {
                failure,
                prerequisites,
            }),
        }
    }
}

/// Registered whole-radio owner with an interrupt-masked MAC foundation.
///
/// The foundation retains common-PHY and protocol timing evidence but remains
/// non-operational. It proves no RF qualification or shared PLL refcount and
/// provides no IRQ route or DMA owner.
#[must_use = "the registered IEEE 802.15.4 foundation owner is unique"]
pub struct RegisteredIeee802154FoundationConfigured<P> {
    role: Ieee802154FoundationConfigured<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154FoundationConfigured<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Borrow the integration token without separating it from the proof.
    pub fn peripheral(&self) -> &P {
        self.role.peripheral()
    }

    /// Configure and read back the known static, interrupt-masked MAC policy.
    ///
    /// This does not configure the opaque RF-dependent TX-power mapping and
    /// cannot establish operational readiness.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure must retain the exact HAL owner and PHY proof"
    )]
    pub fn configure_mac_policy(
        self,
        policy: Ieee802154MacPolicy,
    ) -> Result<
        RegisteredIeee802154MacPolicyConfigured<P>,
        RegisteredIeee802154MacPolicyTransitionFailure<P>,
    > {
        let Self {
            role,
            prerequisites,
        } = self;
        match preserve_prerequisites(role, prerequisites, |role| {
            role.configure_mac_policy(policy)
        }) {
            Ok((role, prerequisites)) => Ok(RegisteredIeee802154MacPolicyConfigured {
                role,
                prerequisites,
            }),
            Err((failure, prerequisites)) => Err(RegisteredIeee802154MacPolicyTransitionFailure {
                failure,
                prerequisites,
            }),
        }
    }
}

/// Registered whole-radio owner after static MAC-policy readback.
///
/// Events and aborts remain masked between finite polled ED/CCA operations.
/// This retains common-PHY and protocol timing evidence but is not RF
/// qualification, shared PLL client accounting, IRQ routing, DMA readiness, or
/// an operational RX/TX IEEE 802.15.4 MAC. In particular, the policy channel
/// proves only the ZBMAC frequency-code field; it is not evidence of an
/// RFPLL/PHY retune.
#[must_use = "the registered IEEE 802.15.4 policy owner is unique"]
pub struct RegisteredIeee802154MacPolicyConfigured<P> {
    role: Ieee802154MacPolicyConfigured<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

/// Successful finite ED/CCA result retaining both the reusable radio owner and
/// its target-issued PHY-registration proof.
#[must_use = "a completed registered IEEE 802.15.4 operation retains its reusable owner"]
pub struct RegisteredIeee802154OperationCompleted<P> {
    operation: Ieee802154OperationCompleted<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154OperationCompleted<P> {
    /// Return the exact MAC-level result and polling evidence.
    pub const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        self.operation.evidence()
    }

    /// Borrow the target-registered PHY state without separating its proof.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Recover the same registered static-policy owner for another serialized
    /// operation.
    pub fn into_owner(self) -> RegisteredIeee802154MacPolicyConfigured<P> {
        RegisteredIeee802154MacPolicyConfigured {
            role: self.operation.into_owner(),
            prerequisites: self.prerequisites,
        }
    }
}

/// Terminal registered ED/CCA failure retaining the radio and PHY proof
/// without a safe recovery transition.
#[must_use = "a failed registered IEEE 802.15.4 operation retains a terminal owner"]
pub struct RegisteredIeee802154OperationFailed<P> {
    operation: Ieee802154OperationFailed<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154OperationFailed<P> {
    /// Return complete typed abort, timeout, or invariant evidence.
    pub const fn failure(&self) -> Ieee802154PolledOperationFailure {
        self.operation.failure()
    }

    /// Borrow the target-registered PHY state without separating its proof.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Borrow the integration token for terminal diagnostics only.
    pub fn peripheral(&self) -> &P {
        self.operation.peripheral()
    }
}

impl<P> RegisteredIeee802154MacPolicyConfigured<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Borrow the integration token without separating it from the proof.
    pub fn peripheral(&self) -> &P {
        self.role.peripheral()
    }

    /// Run finite raw energy detection while preserving target registration.
    ///
    /// The signed result remains uncalibrated; registration does not by itself
    /// turn this MAC transaction into an RF-performance qualification.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free terminal failure must retain the radio and PHY proof"
    )]
    pub fn energy_detection_raw(
        self,
        duration: u16,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<RegisteredIeee802154OperationCompleted<P>, RegisteredIeee802154OperationFailed<P>>
    {
        let Self {
            role,
            prerequisites,
        } = self;
        match role.energy_detection_raw(duration, budget) {
            Ok(operation) => Ok(RegisteredIeee802154OperationCompleted {
                operation,
                prerequisites,
            }),
            Err(operation) => Err(RegisteredIeee802154OperationFailed {
                operation,
                prerequisites,
            }),
        }
    }

    /// Run finite CCA with the proved static policy while preserving target
    /// registration.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free terminal failure must retain the radio and PHY proof"
    )]
    pub fn clear_channel_assessment(
        self,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<RegisteredIeee802154OperationCompleted<P>, RegisteredIeee802154OperationFailed<P>>
    {
        let Self {
            role,
            prerequisites,
        } = self;
        match role.clear_channel_assessment(budget) {
            Ok(operation) => Ok(RegisteredIeee802154OperationCompleted {
                operation,
                prerequisites,
            }),
            Err(operation) => Err(RegisteredIeee802154OperationFailed {
                operation,
                prerequisites,
            }),
        }
    }

    /// Return the static MAC policy which passed semantic readback.
    ///
    /// Its channel is only the ZBMAC frequency-code policy value, not proof of
    /// an RFPLL/PHY channel transition.
    pub const fn policy(&self) -> Ieee802154MacPolicy {
        self.role.policy()
    }
}

/// Failed reset readback retaining the exact registered timing-ready owner.
#[must_use = "a failed registered reset transition retains its previous owner"]
pub struct RegisteredIeee802154ResetTransitionFailure<P> {
    failure: Ieee802154ResetTransitionFailure<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154ResetTransitionFailure<P> {
    /// Return the first private-reset checkpoint whose readback mismatched.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.failure.error()
    }

    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Recover the exact preceding timing-ready owner for a reset retry.
    pub fn into_timing_ready(self) -> RegisteredIeee802154TimingReady<P> {
        let (role, prerequisites) = map_prerequisites(
            self.failure,
            self.prerequisites,
            Ieee802154ResetTransitionFailure::into_clocked,
        );
        RegisteredIeee802154TimingReady {
            role,
            prerequisites,
        }
    }
}

impl<P> fmt::Debug for RegisteredIeee802154ResetTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154ResetTransitionFailure")
            .field("error", &self.error())
            .finish_non_exhaustive()
    }
}

/// Failed foundation readback retaining the exact registered reset owner.
#[must_use = "a failed registered foundation transition retains its previous owner"]
pub struct RegisteredIeee802154FoundationTransitionFailure<P> {
    failure: Ieee802154FoundationTransitionFailure<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154FoundationTransitionFailure<P> {
    /// Return the first foundation checkpoint whose readback mismatched.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.failure.error()
    }

    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Recover the exact preceding registered reset owner for a retry.
    pub fn into_reset(self) -> RegisteredIeee802154Reset<P> {
        let (role, prerequisites) = map_prerequisites(
            self.failure,
            self.prerequisites,
            Ieee802154FoundationTransitionFailure::into_reset,
        );
        RegisteredIeee802154Reset {
            role,
            prerequisites,
        }
    }
}

impl<P> fmt::Debug for RegisteredIeee802154FoundationTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154FoundationTransitionFailure")
            .field("error", &self.error())
            .finish_non_exhaustive()
    }
}

/// Failed static-policy readback retaining the proof until recovery is typed.
#[must_use = "a failed registered policy transition retains its radio and proof"]
pub struct RegisteredIeee802154MacPolicyTransitionFailure<P> {
    failure: Ieee802154MacPolicyTransitionFailure<P>,
    prerequisites: RegisteredIeee802154Prerequisites,
}

impl<P> RegisteredIeee802154MacPolicyTransitionFailure<P> {
    /// Return the first foundation or static-policy checkpoint which failed.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.failure.error()
    }

    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        self.prerequisites.phy_state()
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        self.prerequisites.timing_gain_parameter()
    }

    /// Recover the strongest still-proved registered owner.
    pub fn into_recovery(self) -> RegisteredIeee802154MacPolicyRecovery<P> {
        let Self {
            failure,
            prerequisites,
        } = self;
        match preserve_prerequisites(failure, prerequisites, |failure| {
            match failure.into_recovery() {
                Ieee802154MacPolicyRecovery::Foundation(role) => Ok(role),
                Ieee802154MacPolicyRecovery::Reset(role) => Err(role),
            }
        }) {
            Ok((role, prerequisites)) => RegisteredIeee802154MacPolicyRecovery::Foundation(
                RegisteredIeee802154FoundationConfigured {
                    role,
                    prerequisites,
                },
            ),
            Err((role, prerequisites)) => {
                RegisteredIeee802154MacPolicyRecovery::Reset(RegisteredIeee802154Reset {
                    role,
                    prerequisites,
                })
            }
        }
    }
}

impl<P> fmt::Debug for RegisteredIeee802154MacPolicyTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154MacPolicyTransitionFailure")
            .field("error", &self.error())
            .finish_non_exhaustive()
    }
}

/// Strongest registered owner recovered after static-policy readback failure.
///
/// Both variants retain the same target-issued proof, timing marker and exact
/// HAL owner. No recovery variant implies RF qualification, shared PLL/client
/// accounting, IRQ, DMA, or operational readiness.
#[must_use = "registered policy recovery retains the unique radio owner"]
pub enum RegisteredIeee802154MacPolicyRecovery<P> {
    /// Only requested policy fields mismatched; the foundation remains proved.
    Foundation(RegisteredIeee802154FoundationConfigured<P>),
    /// A foundation invariant mismatched and must be established again.
    Reset(RegisteredIeee802154Reset<P>),
}

impl<P> RegisteredIeee802154MacPolicyRecovery<P> {
    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        match self {
            Self::Foundation(owner) => owner.phy_state(),
            Self::Reset(owner) => owner.phy_state(),
        }
    }

    /// Inspect the retained terminal common-PHY gain projection.
    pub const fn timing_gain_parameter(&self) -> u8 {
        match self {
            Self::Foundation(owner) => owner.timing_gain_parameter(),
            Self::Reset(owner) => owner.timing_gain_parameter(),
        }
    }
}

impl<P: Ieee802154PlatformControl> RegisteredIeee802154MacPolicyRecovery<P> {
    /// Borrow the integration token without separating it from the proof.
    pub fn peripheral(&self) -> &P {
        match self {
            Self::Foundation(owner) => owner.peripheral(),
            Self::Reset(owner) => owner.peripheral(),
        }
    }
}

/// Move all registered timing prerequisites through one role transition.
///
/// Keeping this combinator in production code makes success and failure use
/// the same single-owner move. Host tests can exercise both branches without
/// inventing a second implementation of MMIO behavior already covered by the
/// HAL lifecycle engine.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free branch must move the exact failure owner and prerequisites"
)]
fn preserve_prerequisites<Before, After, Failure>(
    before: Before,
    prerequisites: RegisteredIeee802154Prerequisites,
    transition: impl FnOnce(Before) -> Result<After, Failure>,
) -> Result<(After, RegisteredIeee802154Prerequisites), (Failure, RegisteredIeee802154Prerequisites)>
{
    match transition(before) {
        Ok(after) => Ok((after, prerequisites)),
        Err(failure) => Err((failure, prerequisites)),
    }
}

/// Move all registered timing prerequisites through an infallible recovery.
fn map_prerequisites<Before, After>(
    before: Before,
    prerequisites: RegisteredIeee802154Prerequisites,
    transition: impl FnOnce(Before) -> After,
) -> (After, RegisteredIeee802154Prerequisites) {
    (transition(before), prerequisites)
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::{
        Ieee802154MacPolicy, Ieee802154Owned, Ieee802154PlatformClockImages,
        Ieee802154PlatformControl, Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
        Ieee802154ResetImages, Ieee802154TimingReady as HalIeee802154TimingReady,
        PlatformPowerClockImages, PowerClockControl,
    };

    use crate::{
        PhyConfig, PhyState, RegisteredPhyState,
        phy_client::{
            DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientAcquireOrdering, PhyClientState,
            PhyModemClient, PhyPllTrackClock,
        },
        phy_param_tracking::{PhyParamTrackingAction, PhyParamTrackingParameters},
    };

    use super::{
        RegisteredIeee802154Client, RegisteredIeee802154Clocked,
        RegisteredIeee802154FoundationTransitionFailure, RegisteredIeee802154MacPolicyConfigured,
        RegisteredIeee802154MacPolicyRecovery, RegisteredIeee802154MacPolicyTransitionFailure,
        RegisteredIeee802154Prerequisites, RegisteredIeee802154Reset,
        RegisteredIeee802154ResetTransitionFailure, RegisteredIeee802154TimingReady,
        establish_registered_timing, map_prerequisites, preserve_prerequisites,
    };

    #[derive(Debug)]
    struct FakePlatform {
        clocks: Ieee802154PlatformClockImages,
        resets: Ieee802154ResetImages,
        power: PlatformPowerClockImages,
        clock_sequences: u8,
        reset_edges: u8,
    }

    impl FakePlatform {
        const fn ready() -> Self {
            Self {
                clocks: Ieee802154PlatformClockImages {
                    hp_active_clock_maps_configured: true,
                    pll_160m_clock_enabled: true,
                    modem_source_clock_configured: true,
                    wifi_bb_80x1_clock_enabled: true,
                    etm_clock_enabled: true,
                    bt_apb_clock_enabled: true,
                    modem_security_apb_clock_enabled: true,
                    bt_ieee802154_common_baseband_clock_enabled: true,
                    ieee802154_apb_clock_enabled: true,
                    ieee802154_mac_clock_enabled: true,
                },
                resets: Ieee802154ResetImages {
                    mac_reset_released: true,
                    apb_reset_released: true,
                },
                power: PlatformPowerClockImages {
                    reset_released: true,
                    hp_active_icg_selected: true,
                    modem_bus_clock_enabled: true,
                    hp_active_clock_map_configured: true,
                    modem_source_clocks_configured: true,
                    phy_calibration_clocks_enabled: true,
                    phy_i2c_160mhz_selected: true,
                },
                clock_sequences: 0,
                reset_edges: 0,
            }
        }
    }

    impl PowerClockControl for FakePlatform {
        fn set_wifi_baseband_and_mac_reset(&mut self, _asserted: bool) {}

        fn select_hp_active_modem_icg(&mut self) {}

        fn apply_modem_icg_selection(&mut self) {}

        fn apply_sleep_icg_selection(&mut self) {}

        fn enable_modem_register_bus_clock(&mut self) {}

        fn configure_hp_active_modem_clock_map(&mut self) {}

        fn configure_modem_source_clocks(&mut self) {}

        fn set_wifi_baseband_reset(&mut self, _asserted: bool) {}

        fn enable_phy_calibration_clocks(&mut self) {}

        fn select_phy_i2c_160mhz_source(&mut self) {}

        fn platform_power_clock_images(&self) -> PlatformPowerClockImages {
            self.power
        }
    }

    impl Ieee802154PlatformControl for FakePlatform {
        fn configure_modem_clock_maps(&mut self) {}

        fn configure_modem_source_clock(&mut self) {}

        fn enable_wifi_bb_80x1_clock(&mut self) {}

        fn enable_etm_clock(&mut self) {}

        fn enable_bt_apb_clocks(&mut self) {}

        fn enable_bt_ieee802154_common_baseband_clock(&mut self) {}

        fn enable_ieee802154_mac_clocks(&mut self) {
            self.clock_sequences += 1;
        }

        fn ieee802154_platform_clock_images(&self) -> Ieee802154PlatformClockImages {
            self.clocks
        }

        fn set_ieee802154_mac_reset(&mut self, _asserted: bool) {
            self.reset_edges += 1;
        }

        fn set_ieee802154_apb_reset(&mut self, _asserted: bool) {
            self.reset_edges += 1;
        }

        fn ieee802154_reset_images(&self) -> Ieee802154ResetImages {
            self.resets
        }
    }

    struct FixedClock(u64);

    impl PhyPllTrackClock for FixedClock {
        fn now_micros(&mut self) -> u64 {
            self.0
        }
    }

    fn owner(platform: FakePlatform) -> RegisteredIeee802154Clocked<FakePlatform> {
        let role = Ieee802154Owned::claim_for_validation(platform)
            .power_up()
            .unwrap_or_else(|_| panic!("shared power readback must pass"))
            .into_ieee802154_clocked()
            .unwrap_or_else(|_| panic!("clock readback must pass"));
        RegisteredIeee802154Clocked {
            role,
            registered: registered_state(),
            clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
        }
    }

    fn client_owner(platform: FakePlatform) -> RegisteredIeee802154Client<FakePlatform> {
        let acquired = owner(platform)
            .acquire_phy_client(&mut FixedClock(0))
            .unwrap_or_else(|_| panic!("fresh IEEE client acquisition must succeed"));
        match acquired.into_owner() {
            Ok(owner) => owner,
            Err(_) => panic!("fresh IEEE timestamp must not request tracking"),
        }
    }

    fn registered_state() -> RegisteredPhyState {
        RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production()))
    }

    fn model_prerequisites() -> RegisteredIeee802154Prerequisites {
        let registered = registered_state();
        let gain_parameter = registered.state().register_init_parameters().parameter_120;
        let clients = PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire(PhyModemClient::Ieee802154, &mut FixedClock(0))
            .unwrap_or_else(|_| panic!("model client acquisition must succeed"))
            .into_owner()
            .unwrap_or_else(|_| panic!("fresh model timestamp must settle"));
        RegisteredIeee802154Prerequisites {
            registered,
            clients,
            timing: HalIeee802154TimingReady::for_host_ownership_model(gain_parameter),
        }
    }

    fn timing_ready_owner(platform: FakePlatform) -> RegisteredIeee802154TimingReady<FakePlatform> {
        let parts = establish_registered_timing(client_owner(platform), |_, registered| {
            let gain_parameter = registered.state().register_init_parameters().parameter_120;
            HalIeee802154TimingReady::for_host_ownership_model(gain_parameter)
        });
        RegisteredIeee802154TimingReady {
            role: parts.role,
            prerequisites: RegisteredIeee802154Prerequisites {
                registered: parts.registered,
                clients: parts.clients,
                timing: parts.timing,
            },
        }
    }

    #[test]
    fn production_prerequisite_combinators_cover_success_failure_and_recovery_moves() {
        let (after, prerequisites) =
            match preserve_prerequisites(41_u8, model_prerequisites(), |before| {
                Ok::<_, &'static str>(before + 1)
            }) {
                Ok(success) => success,
                Err(_) => panic!("success branch must retain all prerequisites"),
            };
        assert_eq!(after, 42);
        assert!(prerequisites.phy_state().phy_registered());
        assert_eq!(
            prerequisites.timing_gain_parameter(),
            prerequisites
                .phy_state()
                .register_init_parameters()
                .parameter_120
        );

        let (failure, prerequisites) =
            match preserve_prerequisites(7_u8, model_prerequisites(), |_| {
                Err::<u8, _>("typed failure")
            }) {
                Ok(_) => panic!("failure branch must retain all prerequisites"),
                Err(failure) => failure,
            };
        assert_eq!(failure, "typed failure");
        assert!(prerequisites.phy_state().phy_registered());

        let (after, prerequisites) =
            map_prerequisites(3_u8, model_prerequisites(), |before| before + 2);
        assert_eq!(after, 5);
        assert!(prerequisites.phy_state().phy_registered());
    }

    #[test]
    fn registered_timing_projection_has_no_caller_gain_argument() {
        let client = client_owner(FakePlatform::ready());
        let expected_gain = client.phy_state().register_init_parameters().parameter_120;
        let parts = establish_registered_timing(client, |_, registered| {
            registered.state().register_init_parameters().parameter_120
        });

        assert_eq!(parts.timing, expected_gain);
        assert!(parts.registered.state().phy_registered());
        assert_eq!(parts.role.peripheral().clock_sequences, 1);

        let _: fn(
            RegisteredIeee802154Client<FakePlatform>,
        ) -> RegisteredIeee802154TimingReady<FakePlatform> =
            RegisteredIeee802154Client::initialize_baseband_and_ieee802154_timing;
    }

    #[test]
    fn ieee_client_bit_blocks_timing_until_immediate_tracking_is_resolved() {
        let acquired = owner(FakePlatform::ready())
            .acquire_phy_client(&mut FixedClock(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1))
            .unwrap_or_else(|_| panic!("fresh IEEE client acquisition must succeed"));
        assert_eq!(
            acquired.ordering(),
            PhyClientAcquireOrdering::ArmThenSetThenEvaluate
        );
        let pending = match acquired.into_owner() {
            Ok(_) => panic!("elapsed first acquisition must request immediate tracking"),
            Err(pending) => pending,
        };
        assert!(!pending.request().wifi());
        assert!(pending.request().bluetooth_ieee802154());

        let tracking = pending.begin_tracking(PhyParamTrackingParameters {
            tracking_inhibited: true,
            rfpll_cap_tracking_enabled: false,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            shared_tracking_control: 0,
            bluetooth_ieee802154_power_control: 0,
            calibration_tracking_enabled: false,
            relaxed_power_tracking_threshold: false,
        });
        assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
        let poisoned = tracking.fail();
        assert!(
            poisoned
                .client_snapshot()
                .contains(PhyModemClient::Ieee802154)
        );

        let timing_ready = timing_ready_owner(FakePlatform::ready());
        assert!(
            timing_ready
                .client_snapshot()
                .contains(PhyModemClient::Ieee802154)
        );
    }

    #[test]
    fn registered_full_chain_and_typed_recovery_surface_connects() {
        fn full_success_chain<P: Ieee802154PlatformControl>(
            owner: RegisteredIeee802154TimingReady<P>,
            policy: Ieee802154MacPolicy,
        ) -> Option<RegisteredIeee802154MacPolicyConfigured<P>> {
            let reset = owner.reset_mac().ok()?;
            let foundation = reset.configure_foundation().ok()?;
            foundation.configure_mac_policy(policy).ok()
        }

        fn recover_foundation<P>(
            failure: RegisteredIeee802154FoundationTransitionFailure<P>,
        ) -> RegisteredIeee802154Reset<P> {
            failure.into_reset()
        }

        fn recover_policy<P>(
            failure: RegisteredIeee802154MacPolicyTransitionFailure<P>,
        ) -> RegisteredIeee802154MacPolicyRecovery<P> {
            failure.into_recovery()
        }

        let _ = full_success_chain::<FakePlatform>;
        let _ = recover_foundation::<FakePlatform>;
        let _ = recover_policy::<FakePlatform>;
    }

    #[test]
    fn registered_clock_and_reset_transitions_retain_one_proof_owner() {
        let timing_ready = timing_ready_owner(FakePlatform::ready());
        assert!(timing_ready.phy_state().phy_registered());
        assert_eq!(timing_ready.peripheral().clock_sequences, 1);
        let gain_parameter = timing_ready.timing_gain_parameter();

        let reset = timing_ready
            .reset_mac()
            .unwrap_or_else(|_| panic!("reset readback must pass"));
        assert!(reset.phy_state().phy_registered());
        assert_eq!(reset.timing_gain_parameter(), gain_parameter);
        assert_eq!(reset.peripheral().clock_sequences, 1);
        assert_eq!(reset.peripheral().reset_edges, 4);
    }

    #[test]
    fn registered_failures_retain_proof_and_only_typed_recovery() {
        let mut reset_platform = FakePlatform::ready();
        reset_platform.resets.mac_reset_released = false;
        let timing_ready = timing_ready_owner(reset_platform);
        let gain_parameter = timing_ready.timing_gain_parameter();
        let reset_failure: RegisteredIeee802154ResetTransitionFailure<_> =
            match timing_ready.reset_mac() {
                Ok(_) => panic!("invalid reset readback must fail"),
                Err(failure) => failure,
            };
        assert_eq!(
            reset_failure.error(),
            Ieee802154ReadbackError {
                checkpoint: Ieee802154ResetCheckpoint::MacResetReleased,
                expected: true,
                observed: false,
            }
        );
        assert_eq!(reset_failure.timing_gain_parameter(), gain_parameter);
        let timing_ready = reset_failure.into_timing_ready();
        assert!(timing_ready.phy_state().phy_registered());
        assert_eq!(timing_ready.timing_gain_parameter(), gain_parameter);
        assert_eq!(timing_ready.peripheral().reset_edges, 4);
    }
}
