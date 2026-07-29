//! ESP32-S31 HE20 peer installation at the protocol/PAC boundary.

use open_esp_radio_ieee80211::he::{parse_he20_peer_state, He20PeerState, HeElementError};
use open_esp_radio_pac_esp32s31::{MacHe20PeerConfig, MacHe20PeerError, RadioRegisters};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20InstallError {
    Element(HeElementError),
    Hardware(MacHe20PeerError),
}

/// Parse and install one associated HE20 peer without retaining vendor node
/// layout or exposing raw MMIO above the PAC.
///
/// SOURCE: the lifecycle and exact register transforms were promoted from
/// `migration/esp32s31-hybrid-runtime/src/he.rs`; parsing was checked against
/// pinned `_oracles/libnet80211.a` and the PAC leaves against
/// `_oracles/libpp.a`.
pub fn install_he20_peer(
    registers: &mut RadioRegisters,
    capability: &[u8],
    operation: &[u8],
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_index: u8,
) -> Result<He20PeerState, He20InstallError> {
    let state = parse_he20_peer_state(capability, operation).map_err(He20InstallError::Element)?;
    registers
        .program_he20_peer(
            MacHe20PeerConfig {
                packet_padding_eight_us: state.packet_padding_eight_us,
                operation_parameters: state.operation_parameters,
                bss_color_information: state.bss_color_information,
                extended_range_single_user: state.extended_range_single_user,
            },
            state.rts_threshold,
        )
        .map_err(He20InstallError::Hardware)?;
    registers
        .program_he20_association(association_id, minimum_mpdu_start_spacing, bssid_index)
        .map_err(He20InstallError::Hardware)?;
    registers.initialize_he_buffer_status_report();
    Ok(state)
}
