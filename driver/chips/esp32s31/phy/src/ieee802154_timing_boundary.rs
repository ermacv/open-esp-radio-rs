//! Narrow unsafe boundary coupling terminal common-PHY proof to PAC timing.

use open_esp_radio_esp32s31_hal::{Ieee802154Clocked, Ieee802154TimingPrerequisite};

use crate::RegisteredPhyState;

/// Mint the single-use timing prerequisite for the exact retained IEEE owner.
///
/// This is the only unsafe operation permitted by the PHY crate. Its inputs
/// remain private production types: `RegisteredPhyState` can be issued only by
/// terminal target execution, and the role stays mutably borrowed until the
/// caller immediately consumes the prerequisite on that same owner. The gain
/// byte is projected here from the terminal model and is never a caller input.
#[allow(
    unsafe_code,
    reason = "this function is the affine terminal common-PHY proof boundary"
)]
pub(crate) fn from_terminal_registration<P>(
    _role: &mut Ieee802154Clocked<P>,
    registered: &RegisteredPhyState,
) -> Ieee802154TimingPrerequisite {
    let gain_parameter = registered.state().register_init_parameters().parameter_120;

    // SAFETY: the only production constructor of `RegisteredPhyState` accepts
    // the private target-registration witness after the same retained
    // `Ieee802154Clocked` owner reached terminal common-PHY completion. This
    // function receives both indivisible parts from that wrapper, projects the
    // gain byte from the terminal state, and returns the affine prerequisite
    // only to the immediate timing transition on the still-borrowed owner.
    unsafe { Ieee802154TimingPrerequisite::from_terminal_common_phy(gain_parameter) }
}
