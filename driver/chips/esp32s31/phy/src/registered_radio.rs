//! Coupled ownership of a powered radio and its target-registration proof.

use open_esp_radio_esp32s31_hal::{Radio, state::Powered};

use crate::{PhyState, RegisteredPhyState};

#[path = "registered_ieee802154.rs"]
mod ieee802154;

pub use ieee802154::{
    RegisteredIeee802154Clocked, RegisteredIeee802154FoundationConfigured,
    RegisteredIeee802154FoundationTransitionFailure, RegisteredIeee802154MacPolicyConfigured,
    RegisteredIeee802154MacPolicyRecovery, RegisteredIeee802154MacPolicyTransitionFailure,
    RegisteredIeee802154OperationCompleted, RegisteredIeee802154OperationFailed,
    RegisteredIeee802154Reset, RegisteredIeee802154ResetTransitionFailure,
};

/// Unique powered-radio owner carrying proof of target PHY registration.
///
/// The radio and proof have private fields and no public decomposer. This
/// prevents safe callers from pairing proof issued for one hardware epoch with
/// a different powered radio. Public APIs may inspect the calibrated state,
/// while crate-controlled role transitions move this owner without weakening
/// the association.
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
}

impl<P> RegisteredPhyRadio<P> {
    /// Couple the exact target-completed state and powered-radio epoch.
    #[cfg(target_arch = "riscv32")]
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

    /// Inspect the calibrated state without weakening its hardware association.
    pub const fn state(&self) -> &PhyState {
        self.phy.state()
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
