//! Proof-preserving ESP32-S31 IEEE 802.15.4 lifecycle.
//!
//! These wrappers keep the target-issued [`RegisteredPhyState`] beside the
//! exact whole-radio HAL role owner through clock, reset, foundation and static
//! MAC-policy transitions. None of the types in this module can be decomposed
//! into an independently reusable proof and hardware owner.
//!
//! This lifecycle is deliberately incomplete. It does not establish PHY or RF
//! qualification, BTBB readiness, ownership of the shared 160 MHz PLL gate, a
//! multi-client clock bit/refcount, IRQ routing, DMA ownership, or an
//! operational IEEE 802.15.4 MAC.
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

use core::fmt;

use open_esp_radio_esp32s31_hal::{
    Ieee802154Clocked, Ieee802154FoundationCheckpoint, Ieee802154FoundationConfigured,
    Ieee802154FoundationTransitionFailure, Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint,
    Ieee802154MacPolicyConfigured, Ieee802154MacPolicyRecovery,
    Ieee802154MacPolicyTransitionFailure, Ieee802154OperationCompleted, Ieee802154OperationFailed,
    Ieee802154OperationPollBudget, Ieee802154PlatformControl, Ieee802154PolledOperationEvidence,
    Ieee802154PolledOperationFailure, Ieee802154ReadbackError, Ieee802154Reset,
    Ieee802154ResetCheckpoint, Ieee802154ResetTransitionFailure,
};

use crate::{PhyState, RegisteredPhyState};

/// Registered whole-radio owner after IEEE 802.15.4 clock readback.
///
/// This state preserves target PHY-registration authority, but does not prove
/// PHY/RF qualification, BTBB or PLL ownership, a shared-clock client bit, IRQ,
/// DMA, or operational MAC readiness.
#[must_use = "the registered IEEE 802.15.4 clock owner is unique"]
pub struct RegisteredIeee802154Clocked<P> {
    role: Ieee802154Clocked<P>,
    registered: RegisteredPhyState,
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
        }
    }

    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }
}

impl<P: Ieee802154PlatformControl> RegisteredIeee802154Clocked<P> {
    /// Borrow the integration token without separating it from the proof.
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
        let Self { role, registered } = self;
        match preserve_registration(role, registered, Ieee802154Clocked::reset_mac) {
            Ok((role, registered)) => Ok(RegisteredIeee802154Reset { role, registered }),
            Err((failure, registered)) => Err(RegisteredIeee802154ResetTransitionFailure {
                failure,
                registered,
            }),
        }
    }
}

/// Registered whole-radio owner after the private reset sequence read back.
///
/// Reset completion is not PHY/RF qualification and establishes no BTBB/PLL
/// client ownership, interrupt route, DMA frontier, or operational MAC state.
#[must_use = "the registered IEEE 802.15.4 reset owner is unique"]
pub struct RegisteredIeee802154Reset<P> {
    role: Ieee802154Reset<P>,
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154Reset<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
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
        let Self { role, registered } = self;
        match preserve_registration(role, registered, Ieee802154Reset::configure_foundation) {
            Ok((role, registered)) => {
                Ok(RegisteredIeee802154FoundationConfigured { role, registered })
            }
            Err((failure, registered)) => Err(RegisteredIeee802154FoundationTransitionFailure {
                failure,
                registered,
            }),
        }
    }
}

/// Registered whole-radio owner with an interrupt-masked MAC foundation.
///
/// The foundation is non-operational. It proves neither PHY/RF qualification
/// nor BTBB/PLL client ownership and provides no IRQ route or DMA owner.
#[must_use = "the registered IEEE 802.15.4 foundation owner is unique"]
pub struct RegisteredIeee802154FoundationConfigured<P> {
    role: Ieee802154FoundationConfigured<P>,
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154FoundationConfigured<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
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
        let Self { role, registered } = self;
        match preserve_registration(role, registered, |role| role.configure_mac_policy(policy)) {
            Ok((role, registered)) => {
                Ok(RegisteredIeee802154MacPolicyConfigured { role, registered })
            }
            Err((failure, registered)) => Err(RegisteredIeee802154MacPolicyTransitionFailure {
                failure,
                registered,
            }),
        }
    }
}

/// Registered whole-radio owner after static MAC-policy readback.
///
/// Events and aborts remain masked between finite polled ED/CCA operations.
/// This is not PHY/RF qualification, BTBB or PLL client ownership, IRQ routing,
/// DMA readiness, or an operational RX/TX IEEE 802.15.4 MAC. In particular,
/// the policy channel proves only the ZBMAC frequency-code field; it is not
/// evidence of an RFPLL/PHY retune.
#[must_use = "the registered IEEE 802.15.4 policy owner is unique"]
pub struct RegisteredIeee802154MacPolicyConfigured<P> {
    role: Ieee802154MacPolicyConfigured<P>,
    registered: RegisteredPhyState,
}

/// Successful finite ED/CCA result retaining both the reusable radio owner and
/// its target-issued PHY-registration proof.
#[must_use = "a completed registered IEEE 802.15.4 operation retains its reusable owner"]
pub struct RegisteredIeee802154OperationCompleted<P> {
    operation: Ieee802154OperationCompleted<P>,
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154OperationCompleted<P> {
    /// Return the exact MAC-level result and polling evidence.
    pub const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        self.operation.evidence()
    }

    /// Borrow the target-registered PHY state without separating its proof.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Recover the same registered static-policy owner for another serialized
    /// operation.
    pub fn into_owner(self) -> RegisteredIeee802154MacPolicyConfigured<P> {
        RegisteredIeee802154MacPolicyConfigured {
            role: self.operation.into_owner(),
            registered: self.registered,
        }
    }
}

/// Terminal registered ED/CCA failure retaining the radio and PHY proof
/// without a safe recovery transition.
#[must_use = "a failed registered IEEE 802.15.4 operation retains a terminal owner"]
pub struct RegisteredIeee802154OperationFailed<P> {
    operation: Ieee802154OperationFailed<P>,
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154OperationFailed<P> {
    /// Return complete typed abort, timeout, or invariant evidence.
    pub const fn failure(&self) -> Ieee802154PolledOperationFailure {
        self.operation.failure()
    }

    /// Borrow the target-registered PHY state without separating its proof.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Borrow the integration token for terminal diagnostics only.
    pub fn peripheral(&self) -> &P {
        self.operation.peripheral()
    }
}

impl<P> RegisteredIeee802154MacPolicyConfigured<P> {
    /// Borrow the target-registered PHY state without exposing mutable state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
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
        let Self { role, registered } = self;
        match role.energy_detection_raw(duration, budget) {
            Ok(operation) => Ok(RegisteredIeee802154OperationCompleted {
                operation,
                registered,
            }),
            Err(operation) => Err(RegisteredIeee802154OperationFailed {
                operation,
                registered,
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
        let Self { role, registered } = self;
        match role.clear_channel_assessment(budget) {
            Ok(operation) => Ok(RegisteredIeee802154OperationCompleted {
                operation,
                registered,
            }),
            Err(operation) => Err(RegisteredIeee802154OperationFailed {
                operation,
                registered,
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

/// Failed reset readback retaining the exact registered clocked owner.
#[must_use = "a failed registered reset transition retains its previous owner"]
pub struct RegisteredIeee802154ResetTransitionFailure<P> {
    failure: Ieee802154ResetTransitionFailure<P>,
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154ResetTransitionFailure<P> {
    /// Return the first private-reset checkpoint whose readback mismatched.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.failure.error()
    }

    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Recover the exact preceding registered clocked owner for a reset retry.
    pub fn into_clocked(self) -> RegisteredIeee802154Clocked<P> {
        let (role, registered) = map_registration(
            self.failure,
            self.registered,
            Ieee802154ResetTransitionFailure::into_clocked,
        );
        RegisteredIeee802154Clocked { role, registered }
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
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154FoundationTransitionFailure<P> {
    /// Return the first foundation checkpoint whose readback mismatched.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.failure.error()
    }

    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Recover the exact preceding registered reset owner for a retry.
    pub fn into_reset(self) -> RegisteredIeee802154Reset<P> {
        let (role, registered) = map_registration(
            self.failure,
            self.registered,
            Ieee802154FoundationTransitionFailure::into_reset,
        );
        RegisteredIeee802154Reset { role, registered }
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
    registered: RegisteredPhyState,
}

impl<P> RegisteredIeee802154MacPolicyTransitionFailure<P> {
    /// Return the first foundation or static-policy checkpoint which failed.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.failure.error()
    }

    /// Borrow the retained target-registered PHY state immutably.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Recover the strongest still-proved registered owner.
    pub fn into_recovery(self) -> RegisteredIeee802154MacPolicyRecovery<P> {
        let Self {
            failure,
            registered,
        } = self;
        match preserve_registration(failure, registered, |failure| {
            match failure.into_recovery() {
                Ieee802154MacPolicyRecovery::Foundation(role) => Ok(role),
                Ieee802154MacPolicyRecovery::Reset(role) => Err(role),
            }
        }) {
            Ok((role, registered)) => RegisteredIeee802154MacPolicyRecovery::Foundation(
                RegisteredIeee802154FoundationConfigured { role, registered },
            ),
            Err((role, registered)) => {
                RegisteredIeee802154MacPolicyRecovery::Reset(RegisteredIeee802154Reset {
                    role,
                    registered,
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
/// Both variants retain the same target-issued proof and exact HAL owner. No
/// recovery variant implies PHY/RF qualification, shared PLL/client ownership,
/// IRQ, DMA, or operational readiness.
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

/// Move one registration proof through the exact result of a role transition.
///
/// Keeping this combinator in production code makes success and failure use
/// the same single-owner move. Host tests can exercise both branches without
/// inventing a second implementation of MMIO behavior already covered by the
/// HAL lifecycle engine.
#[allow(
    clippy::result_large_err,
    reason = "the allocation-free branch must move the exact failure owner and registration proof"
)]
fn preserve_registration<Before, After, Failure>(
    before: Before,
    registered: RegisteredPhyState,
    transition: impl FnOnce(Before) -> Result<After, Failure>,
) -> Result<(After, RegisteredPhyState), (Failure, RegisteredPhyState)> {
    match transition(before) {
        Ok(after) => Ok((after, registered)),
        Err(failure) => Err((failure, registered)),
    }
}

/// Move one registration proof through an infallible typed recovery.
fn map_registration<Before, After>(
    before: Before,
    registered: RegisteredPhyState,
    transition: impl FnOnce(Before) -> After,
) -> (After, RegisteredPhyState) {
    (transition(before), registered)
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::{
        Ieee802154ClockImages, Ieee802154MacPolicy, Ieee802154Owned, Ieee802154PlatformControl,
        Ieee802154ReadbackError, Ieee802154ResetCheckpoint, Ieee802154ResetImages,
        PowerClockControl, PowerClockImages,
    };

    use crate::{PhyConfig, PhyState, RegisteredPhyState};

    use super::{
        RegisteredIeee802154Clocked, RegisteredIeee802154FoundationTransitionFailure,
        RegisteredIeee802154MacPolicyConfigured, RegisteredIeee802154MacPolicyRecovery,
        RegisteredIeee802154MacPolicyTransitionFailure, RegisteredIeee802154Reset,
        RegisteredIeee802154ResetTransitionFailure, map_registration, preserve_registration,
    };

    #[derive(Debug)]
    struct FakePlatform {
        clocks: Ieee802154ClockImages,
        resets: Ieee802154ResetImages,
        power: PowerClockImages,
        clock_sequences: u8,
        reset_edges: u8,
    }

    impl FakePlatform {
        const fn ready() -> Self {
            Self {
                clocks: Ieee802154ClockImages {
                    modem_clock_maps_configured: true,
                    pll_160m_clock_enabled: true,
                    modem_source_clock_configured: true,
                    coexistence_clock_enabled: true,
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
                power: PowerClockImages {
                    reset_released: true,
                    hp_active_icg_selected: true,
                    modem_bus_clock_enabled: true,
                    hp_active_clock_map_configured: true,
                    shared_clock_map_configured: true,
                    modem_source_clocks_configured: true,
                    phy_calibration_clocks_enabled: true,
                    phy_i2c_160mhz_selected: true,
                    phy_i2c_master_clock_enabled: true,
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

        fn configure_shared_modem_clock_map(&mut self) {}

        fn configure_modem_source_clocks(&mut self) {}

        fn set_wifi_baseband_reset(&mut self, _asserted: bool) {}

        fn enable_phy_calibration_clocks(&mut self) {}

        fn select_phy_i2c_160mhz_source(&mut self) {}

        fn enable_phy_i2c_master_clock(&mut self) {}

        fn power_clock_images(&self) -> PowerClockImages {
            self.power
        }
    }

    impl Ieee802154PlatformControl for FakePlatform {
        fn configure_modem_clock_maps(&mut self) {}

        fn configure_modem_source_clock(&mut self) {}

        fn enable_coexistence_clock(&mut self) {}

        fn enable_wifi_bb_80x1_clock(&mut self) {}

        fn enable_etm_clock(&mut self) {}

        fn enable_bt_apb_clocks(&mut self) {}

        fn enable_bt_ieee802154_common_baseband_clock(&mut self) {}

        fn enable_ieee802154_mac_clocks(&mut self) {
            self.clock_sequences += 1;
        }

        fn ieee802154_clock_images(&self) -> Ieee802154ClockImages {
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

    fn owner(platform: FakePlatform) -> RegisteredIeee802154Clocked<FakePlatform> {
        let role = Ieee802154Owned::claim_for_validation(platform)
            .power_up()
            .unwrap_or_else(|_| panic!("shared power readback must pass"))
            .into_ieee802154_clocked()
            .unwrap_or_else(|_| panic!("clock readback must pass"));
        RegisteredIeee802154Clocked {
            role,
            registered: registered_state(),
        }
    }

    fn registered_state() -> RegisteredPhyState {
        RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production()))
    }

    #[test]
    fn production_registration_combinators_cover_success_failure_and_recovery_moves() {
        let (after, registered) = match preserve_registration(41_u8, registered_state(), |before| {
            Ok::<_, &'static str>(before + 1)
        }) {
            Ok(success) => success,
            Err(_) => panic!("success branch must retain the proof"),
        };
        assert_eq!(after, 42);
        assert!(registered.state().phy_registered());

        let (failure, registered) = match preserve_registration(7_u8, registered_state(), |_| {
            Err::<u8, _>("typed failure")
        }) {
            Ok(_) => panic!("failure branch must retain the proof"),
            Err(failure) => failure,
        };
        assert_eq!(failure, "typed failure");
        assert!(registered.state().phy_registered());

        let (after, registered) = map_registration(3_u8, registered_state(), |before| before + 2);
        assert_eq!(after, 5);
        assert!(registered.state().phy_registered());
    }

    #[test]
    fn registered_full_chain_and_typed_recovery_surface_connects() {
        fn full_success_chain<P: Ieee802154PlatformControl>(
            owner: RegisteredIeee802154Clocked<P>,
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
        let clocked = owner(FakePlatform::ready());
        assert!(clocked.phy_state().phy_registered());
        assert_eq!(clocked.peripheral().clock_sequences, 1);

        let reset = clocked
            .reset_mac()
            .unwrap_or_else(|_| panic!("reset readback must pass"));
        assert!(reset.phy_state().phy_registered());
        assert_eq!(reset.peripheral().clock_sequences, 1);
        assert_eq!(reset.peripheral().reset_edges, 4);
    }

    #[test]
    fn registered_failures_retain_proof_and_only_typed_recovery() {
        let mut reset_platform = FakePlatform::ready();
        reset_platform.resets.mac_reset_released = false;
        let clocked = owner(reset_platform);
        let reset_failure: RegisteredIeee802154ResetTransitionFailure<_> = match clocked.reset_mac()
        {
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
        let clocked = reset_failure.into_clocked();
        assert!(clocked.phy_state().phy_registered());
        assert_eq!(clocked.peripheral().reset_edges, 4);
    }
}
