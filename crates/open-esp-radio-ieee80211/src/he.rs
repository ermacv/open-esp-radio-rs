//! Allocation-free parsing for the bounded 2.4-GHz HE20 capability prefix.
//!
//! This module does not enable HE transmission. It owns the stateless peer
//! representation recovered from the former migration runtime. Register
//! programming belongs to the chip MAC/PAC boundary.

pub const HE_CAPABILITIES_EXTENSION_ID: u8 = 35;
pub const HE_OPERATION_EXTENSION_ID: u8 = 36;
pub const HE_CAPABILITIES_IE_MIN_LEN: usize = 24;
pub const HE_OPERATION_IE_MIN_LEN: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMcsNssSupport {
    Mcs0To7,
    Mcs0To9,
    Mcs0To11,
    NotSupported,
}

impl HeMcsNssSupport {
    const fn from_map(map: u16, spatial_stream: u8) -> Self {
        let shift = spatial_stream.saturating_sub(1) as u32 * 2;
        match (map >> shift) & 0x03 {
            0 => Self::Mcs0To7,
            1 => Self::Mcs0To9,
            2 => Self::Mcs0To11,
            _ => Self::NotSupported,
        }
    }

    pub const fn supports_mcs9(self) -> bool {
        matches!(self, Self::Mcs0To9 | Self::Mcs0To11)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Capabilities {
    pub receive_mcs_map: u16,
    pub transmit_mcs_map: u16,
    pub receive_nss1: HeMcsNssSupport,
    pub transmit_nss1: HeMcsNssSupport,
}

impl He20Capabilities {
    pub const fn supports_bidirectional_mcs9(self) -> bool {
        self.receive_nss1.supports_mcs9() && self.transmit_nss1.supports_mcs9()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Operation {
    pub bss_color: u8,
    pub basic_mcs_nss_map: u16,
}

/// Bounded HE20 peer state consumed by a chip-specific register backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20PeerState {
    pub capability_prefix: [u8; HE_CAPABILITIES_IE_MIN_LEN],
    pub max_rate_code: u8,
    pub packet_padding_eight_us: u8,
    pub operation_parameters: u32,
    pub bss_color_information: u8,
    pub basic_mcs_nss_map: u16,
    pub rts_threshold: Option<u16>,
    pub extended_range_single_user: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeElementError {
    WrongElement,
    LengthMismatch,
    TooShort,
    WrongExtension,
}

fn validate_extension(
    element: &[u8],
    extension: u8,
    minimum_len: usize,
) -> Result<&[u8], HeElementError> {
    if element.first().copied() != Some(255) {
        return Err(HeElementError::WrongElement);
    }
    let Some(declared_len) = element.get(1).copied() else {
        return Err(HeElementError::TooShort);
    };
    if usize::from(declared_len).checked_add(2) != Some(element.len()) {
        return Err(HeElementError::LengthMismatch);
    }
    if element.len() < minimum_len {
        return Err(HeElementError::TooShort);
    }
    if element.get(2).copied() != Some(extension) {
        return Err(HeElementError::WrongExtension);
    }
    Ok(element)
}

pub fn parse_he20_capabilities(element: &[u8]) -> Result<He20Capabilities, HeElementError> {
    let element = validate_extension(
        element,
        HE_CAPABILITIES_EXTENSION_ID,
        HE_CAPABILITIES_IE_MIN_LEN,
    )?;
    let receive_mcs_map = u16::from_le_bytes([element[20], element[21]]);
    let transmit_mcs_map = u16::from_le_bytes([element[22], element[23]]);
    Ok(He20Capabilities {
        receive_mcs_map,
        transmit_mcs_map,
        receive_nss1: HeMcsNssSupport::from_map(receive_mcs_map, 1),
        transmit_nss1: HeMcsNssSupport::from_map(transmit_mcs_map, 1),
    })
}

pub fn parse_he20_operation(element: &[u8]) -> Result<He20Operation, HeElementError> {
    let element = validate_extension(element, HE_OPERATION_EXTENSION_ID, HE_OPERATION_IE_MIN_LEN)?;
    Ok(He20Operation {
        bss_color: element[6] & 0x3f,
        basic_mcs_nss_map: u16::from_le_bytes([element[7], element[8]]),
    })
}

/// Recover the peer fields installed by the pinned HE capability and
/// operation parsers.
///
/// Evidence: `migration/esp32s31-hybrid-runtime/src/he.rs`, recovered by
/// comparison with the pinned `ieee80211_parse_hecap` and
/// `ieee80211_parse_heopr` blob leaves. This function is deliberately pure;
/// the corresponding MMIO transforms are tracked separately in the S31 MAC.
pub fn parse_he20_peer_state(
    capability: &[u8],
    operation: &[u8],
) -> Result<He20PeerState, HeElementError> {
    let capability = validate_extension(
        capability,
        HE_CAPABILITIES_EXTENSION_ID,
        HE_CAPABILITIES_IE_MIN_LEN,
    )?;
    let operation = validate_extension(
        operation,
        HE_OPERATION_EXTENSION_ID,
        HE_OPERATION_IE_MIN_LEN,
    )?;

    let mut capability_prefix = [0_u8; HE_CAPABILITIES_IE_MIN_LEN];
    capability_prefix.copy_from_slice(&capability[..HE_CAPABILITIES_IE_MIN_LEN]);
    let max_rate_code = if capability[20] & 0x03 == 0 { 172 } else { 229 };
    let packet_padding_eight_us = if capability[15] & 0x80 == 0 {
        capability[18] >> 6
    } else {
        let ppe0 = *capability.get(24).ok_or(HeElementError::TooShort)?;
        let ppe1 = *capability.get(25).ok_or(HeElementError::TooShort)?;
        let ppet8 = ((ppe1 & 0x03) << 1) | (ppe0 >> 7);
        if ppe0 & 0x08 != 0 && ppe1 & 0x1c == 0x1c && ppet8 == 0 {
            2
        } else {
            0
        }
    };

    let operation_parameters =
        u32::from(operation[3]) | (u32::from(operation[4]) << 8) | (u32::from(operation[5]) << 16);
    let encoded_rts_threshold =
        (u16::from(operation[4] & 0x3f) << 4) | u16::from(operation[3] >> 4);
    let rts_threshold =
        (!matches!(encoded_rts_threshold, 0 | 0x03ff)).then_some(encoded_rts_threshold);

    Ok(He20PeerState {
        capability_prefix,
        max_rate_code,
        packet_padding_eight_us,
        operation_parameters,
        bss_color_information: operation[6],
        basic_mcs_nss_map: u16::from_le_bytes([operation[7], operation[8]]),
        rts_threshold,
        extended_range_single_user: operation[5] & 0x01 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_stream_mcs9_capability_without_optional_tails() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[20..22].copy_from_slice(&0xfffd_u16.to_le_bytes());
        element[22..24].copy_from_slice(&0xfffd_u16.to_le_bytes());
        let capability = parse_he20_capabilities(&element).unwrap();
        assert_eq!(capability.receive_nss1, HeMcsNssSupport::Mcs0To9);
        assert_eq!(capability.transmit_nss1, HeMcsNssSupport::Mcs0To9);
        assert!(capability.supports_bidirectional_mcs9());
    }

    #[test]
    fn parses_mandatory_operation_prefix_and_masks_bss_color() {
        let element = [255, 7, 36, 0, 0, 0, 0xc5, 0xfd, 0xff];
        let operation = parse_he20_operation(&element).unwrap();
        assert_eq!(operation.bss_color, 5);
        assert_eq!(operation.basic_mcs_nss_map, 0xfffd);
    }

    #[test]
    fn recovers_vendor_ap_he20_peer_state() {
        let capability = [
            0xff, 0x1a, 0x23, 0x05, 0x00, 0x18, 0x12, 0x00, 0x10, 0x22, 0x20, 0x02, 0xc0, 0x0f,
            0x41, 0x95, 0x08, 0x00, 0xcc, 0x00, 0xfa, 0xff, 0xfa, 0xff, 0x19, 0x1c, 0xc7, 0x71,
        ];
        let operation = [0xff, 0x07, 0x24, 0x04, 0x00, 0x01, 0x1b, 0xfc, 0xff];
        let state = parse_he20_peer_state(&capability, &operation).unwrap();
        assert_eq!(state.max_rate_code, 229);
        assert_eq!(state.packet_padding_eight_us, 2);
        assert_eq!(state.operation_parameters, 0x01_0004);
        assert_eq!(state.bss_color_information, 27);
        assert_eq!(state.basic_mcs_nss_map, 0xfffc);
        assert_eq!(state.rts_threshold, None);
        assert!(state.extended_range_single_user);
    }
}
