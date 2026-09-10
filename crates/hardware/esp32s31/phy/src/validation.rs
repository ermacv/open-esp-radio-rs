//! ABI-only bridges for compiled production comparison.
//!
//! This module is absent from ordinary builds. It performs argument/result
//! conversion and semantic fixture construction only. Hardware operations and
//! state publication use the same transitions and executors as production.

#![cfg(feature = "validation-probes")]

/// Execute the accredited-domain production behavior of
/// `phy_get_i2c_hostid_new` and project its typed host to the vendor ABI.
#[cfg(target_arch = "riscv32")]
pub fn configure_and_select_phy_i2c_host(
    platform: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    block: u8,
) -> u32 {
    let Some(block) = crate::analog::i2c::PhyI2cBlock::from_vendor_abi(block) else {
        return u32::MAX;
    };

    match oer_esp32s31_hal::phy::i2c::configure_and_select_host(platform, block) {
        oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host0 => 0,
        oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host1 => 1,
    }
}

/// Construct ordinary semantic inputs without a registration or admission proof.
pub fn calibration_tracking_state(
    parameters: crate::tracking::calibration::PhyCalibrationTrackingParameters,
) -> crate::PhyState {
    crate::PhyState::calibration_tracking_fixture(parameters)
}

/// Select the real combined child directly for an isolated compiled probe.
/// This does not manufacture a registered owner or a successful completion.
pub fn calibration_tracking(
    state: &mut crate::PhyState,
    clients: crate::tracking::parameters::PhyParamTrackRequest,
) -> crate::tracking::parameters::PhyParamTrackingCalibrationTransition<'_> {
    use crate::tracking::parameters::{
        PhyParamTrackingAction, PhyParamTrackingCalibrationTransition, PhyParamTrackingPolicy,
        PhyTrackingDiagnostics,
    };
    PhyParamTrackingCalibrationTransition::lower(
        PhyParamTrackingAction::CalibrationTrack {
            clients,
            diagnostics: PhyTrackingDiagnostics::Disabled,
        },
        PhyParamTrackingPolicy::for_registered_state(state),
        crate::tracking::calibration::Scope::Both,
        state,
    )
    .expect("calibration action selects the combined child")
}

/// Select the complete parent with the same policy projection as registration.
/// The returned model owns no physical radio capability.
pub fn parameter_tracking(
    state: &crate::PhyState,
    clients: crate::tracking::parameters::PhyParamTrackRequest,
) -> crate::state::client::PhyPendingTracking {
    crate::state::client::PhyPendingTracking::for_validation(
        clients,
        crate::tracking::parameters::PhyParamTrackingPolicy::for_registered_state(state),
    )
}

/// Exercise the existing RFPLL branch inside the real parent. This override
/// exists only in validation builds and does not enable registered operation.
pub fn parameter_tracking_with_rfpll(
    state: &crate::PhyState,
    clients: crate::tracking::parameters::PhyParamTrackRequest,
) -> crate::state::client::PhyPendingTracking {
    let mut policy =
        crate::tracking::parameters::PhyParamTrackingPolicy::for_registered_state(state);
    policy.rfpll_cap_tracking_enabled = true;
    crate::state::client::PhyPendingTracking::for_validation(clients, policy)
}

/// Seed ordinary state for comparison of the whole parameter parent.
/// Gain adjustment is an independent retained input, not a derived attenuation.
pub fn parameter_tracking_state(
    parameters: crate::tracking::calibration::PhyCalibrationTrackingParameters,
    gain_adjustment: i8,
    relaxed_threshold: bool,
) -> crate::PhyState {
    crate::PhyState::parameter_tracking_fixture(parameters, gain_adjustment, relaxed_threshold)
}

/// Read the semantic RFPLL reference without exposing a mutable ABI image.
pub fn rfpll_reference_temperature(state: &crate::PhyState) -> i16 {
    state.rfpll_tracking_request(None).reference_temperature
}
