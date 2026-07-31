//! ESP32-S31 HE20 peer installation at the protocol/PAC boundary.

use open_esp_radio_esp32s31_pac::{MacHe20PeerConfig, MacHe20PeerError, RadioRegisters};
use open_esp_radio_ieee80211::he::{He20PeerState, HeElementError, parse_he20_peer_state};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20InstallError {
    Element(HeElementError),
    Hardware(MacHe20PeerError),
}

/// Parse and install one associated HE20 peer without retaining vendor node
/// layout or exposing raw MMIO above the PAC.
///
/// SOURCE: complete pinned `_oracles/libnet80211.a[ieee80211_he.o]`
/// capability/operation parsers and `_oracles/libpp.a[hal_mac_ctl.o]`
/// hardware leaves. The former migration copy is not an oracle.
pub fn install_he20_peer(
    registers: &mut RadioRegisters,
    capability: &[u8],
    operation: &[u8],
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_index: u8,
) -> Result<He20PeerState, He20InstallError> {
    let state = parse_he20_peer_state(capability, operation).map_err(He20InstallError::Element)?;
    program_he20_peer_state(
        registers,
        state,
        association_id,
        minimum_mpdu_start_spacing,
        bssid_index,
    )?;
    Ok(state)
}

/// Install an already parsed HE20 peer plan.
///
/// Association orchestration parses one immutable peer view before touching
/// hardware. Reusing that value here guarantees that rate control, HE-SIG
/// color/ER-SU policy and the programmed S31 registers cannot be derived from
/// different parses of mutable application storage.
pub fn program_he20_peer_state(
    registers: &mut RadioRegisters,
    state: He20PeerState,
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_index: u8,
) -> Result<(), He20InstallError> {
    registers
        .program_he20_peer(
            MacHe20PeerConfig {
                packet_padding_eight_us: state.packet_padding_eight_us,
                operation_parameters: state.operation_parameters,
                bss_color_information: state.bss_color_information,
                extended_range_single_user_disabled: state.extended_range_single_user_disabled,
            },
            state.rts_threshold,
        )
        .map_err(He20InstallError::Hardware)?;
    registers
        .program_he20_association(association_id, minimum_mpdu_start_spacing, bssid_index)
        .map_err(He20InstallError::Hardware)?;
    registers.initialize_he_buffer_status_report();
    Ok(())
}
