//! Allocation-free STA authentication and association protocol.
//!
//! Frame construction, response parsing and WPA2-Personal RSN selection use
//! caller-owned output buffers. The separate [`sequence`] module retains the
//! transmit sequence spaces. The caller decides when to submit a frame to
//! hardware, arm a deadline, or retry.

use crate::{
    ccmp::CCMP_HEADER_LEN,
    data::{
        DataHeControl, DataInterfaceRole, ETHERNET_HEADER_LEN, LLC_SNAP_HEADER_LEN,
        plan_data_encapsulation, plan_data_encapsulation_with_he_control,
    },
    extensions::wmm::{WmmParameterSet, parse_wmm_parameter_element},
    he::{parse_he20_capabilities, parse_he20_operation},
    management::{MANAGEMENT_HEADER_LEN, MAX_SSID_LEN, MAX_SUPPORTED_RATES_LEN},
    scan::ScanRecord,
    security::WifiSecurityMode,
};

const OPEN_AUTHENTICATION_FRAME_CONTROL: u16 = 0x00b0;
const DISASSOCIATION_FRAME_CONTROL: u16 = 0x00a0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const ASSOCIATION_REQUEST_FRAME_CONTROL: u16 = 0x0000;
const ASSOCIATION_RESPONSE_FRAME_CONTROL: u16 = 0x0010;
const OPEN_SYSTEM_ALGORITHM: u16 = 0;
const OPEN_SYSTEM_REQUEST_SEQUENCE: u16 = 1;
const OPEN_SYSTEM_RESPONSE_SEQUENCE: u16 = 2;
const ASSOCIATION_FIXED_BODY_LEN: usize = 4;
const ASSOCIATION_CAPABILITY_MASK: u16 = 0x0431;
const SUPPORTED_RATES_ELEMENT_CAPACITY: usize = 8;
const SELECTED_RSN_IE_LEN: usize = 22;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const RSN_CIPHER_CCMP: u8 = 4;
const RSN_AKM_PSK: u8 = 2;
const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
const RSN_CAPABILITY_MFPC: u16 = 1 << 7;
const RSN_CAPABILITY_SPP_AMSDU_CAPABLE: u16 = 1 << 10;
const HE_UL_MU_POWER_CAPABILITY_IE_LEN: usize = 14;
const HE_UL_MU_POWER_CAPABILITY_EXTENSION_ID: u8 = 60;
const POWER_CAPABILITY_IE_LEN: usize = 4;

/// Baseline HT A-MSDU limit selected when HT Capabilities Info bit 11 is
/// clear. The 7,935-byte extension remains deliberately unadvertised.
const HT_AMSDU_BASELINE_MAX_LEN: usize = 3_839;
const AMSDU_SUBFRAME_HEADER_LEN: usize = 14;
/// Bytes added when one Ethernet-II frame is encoded as a protected QoS data
/// MPDU, excluding the hardware-owned CCMP MIC and FCS.
///
/// The Ethernet DA/SA/EtherType header is replaced by the 26-byte QoS MAC
/// header, the eight-byte CCMP header and the eight-byte LLC/SNAP header.
pub const STA_PROTECTED_QOS_ETHERNET_OVERHEAD: usize =
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN
        - ETHERNET_HEADER_LEN;
/// Prefix space that lets an Ethernet frame become a protected QoS MPDU
/// without moving its payload.
///
/// If the Ethernet header starts at this offset, the Ethernet payload already
/// begins at the final `QoS header + CCMP header + LLC/SNAP` boundary. The
/// encoder only has to preserve DA/SA/EtherType and overwrite the prefix.
pub const STA_PROTECTED_QOS_ETHERNET_HEADROOM: usize = STA_PROTECTED_QOS_ETHERNET_OVERHEAD;

pub mod sequence;
pub use sequence::{StaSequenceCounter, StaTxSequenceCounters};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFrameError {
    InvalidBssid,
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
    NoAmsduFrames,
    EthernetFrameTooShort,
    AmsduTooLong { length: usize, maximum: usize },
    SequenceNumberOutOfRange,
    UserPriorityOutOfRange,
    HeControlRequiresQos,
    AmsduRequiresQos,
    EthernetHeadroomTooSmall { required: usize, available: usize },
    OutputTooSmall { required: usize },
}

mod association;
mod data;
mod management;
mod security;

pub use association::{
    AssociationCapabilities, AssociationRequest, AssociationRequestError, AssociationResponse,
    HeUlMuPowerCapability, HeUlMuPowerCapabilityError, StaAssociationPhy, StaAssociationPreference,
    StaPowerCapability, StaPowerCapabilityError, parse_association_response,
};
pub use data::{
    EncodedStaFrame, StaDataFrame, StaProtectedAmsduFrame, StaProtectedDataFrame,
    StaProtectedEthernetFrame, sta_protected_amsdu_frame_length,
    sta_protected_amsdu_pair_frame_length,
};
pub use management::{
    OpenAuthenticationRequest, OpenAuthenticationResponse, StaActionFrame, StaDisconnect,
    StaDisconnectKind, parse_open_authentication_response, parse_sta_disconnect,
};
pub use security::{SelectedRsn, StaSecurityError, select_association_rsn, select_wpa2_psk_rsn};

pub(crate) fn validate_peer(bssid: [u8; 6], sequence_number: u16) -> Result<(), StationFrameError> {
    if bssid == [0; 6] || bssid == [0xff; 6] || bssid[0] & 1 != 0 {
        return Err(StationFrameError::InvalidBssid);
    }
    if sequence_number > 0x0fff {
        return Err(StationFrameError::SequenceNumberOutOfRange);
    }
    Ok(())
}

fn write_management_header(
    frame: &mut [u8],
    frame_control: u16,
    destination: [u8; 6],
    source: [u8; 6],
    bssid: [u8; 6],
    sequence_number: u16,
) {
    frame[0..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&destination);
    frame[10..16].copy_from_slice(&source);
    frame[16..22].copy_from_slice(&bssid);
    frame[22..24].copy_from_slice(&(sequence_number << 4).to_le_bytes());
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

#[cfg(test)]
mod tests;
