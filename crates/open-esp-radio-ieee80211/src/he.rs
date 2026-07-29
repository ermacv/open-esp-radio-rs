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

/// Maximum modulation constellation advertised for HE DCM.
///
/// The values are the two-bit HE PHY capability encoding, not an S31-private
/// enum. Keeping `NotSupported` distinct is important: a HE peer is not
/// necessarily able to receive DCM merely because it supports ordinary HE SU.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum HeDcmConstellation {
    #[default]
    NotSupported = 0,
    Bpsk = 1,
    Qpsk = 2,
    Qam16 = 3,
}

impl HeDcmConstellation {
    const fn from_encoding(encoding: u8) -> Self {
        match encoding & 0x03 {
            0 => Self::NotSupported,
            1 => Self::Bpsk,
            2 => Self::Qpsk,
            _ => Self::Qam16,
        }
    }

    pub const fn supports_bpsk(self) -> bool {
        !matches!(self, Self::NotSupported)
    }

    pub const fn supports_qpsk(self) -> bool {
        matches!(self, Self::Qpsk | Self::Qam16)
    }

    pub const fn supports_16qam(self) -> bool {
        matches!(self, Self::Qam16)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Capabilities {
    pub receive_mcs_map: u16,
    pub transmit_mcs_map: u16,
    pub receive_nss1: HeMcsNssSupport,
    pub transmit_nss1: HeMcsNssSupport,
    /// The peer can receive the optional HE SU 1x HE-LTF / 0.8-us GI form.
    ///
    /// SOURCE[LINUX_IEEE80211_HE_PHY_CAP1_GI_2026_07_29]: Linux v6.12
    /// `include/linux/ieee80211.h` names HE PHY capability byte 1 bit `0x40`
    /// `HE_LTF_AND_GI_FOR_HE_PPDUS_0_8US`. The S31 oracle's ordinary
    /// `ppSelectTxFormat` never emits GI/LTF selector zero, while HIL against a
    /// peer with this bit clear rejected selector zero for MCS0 through MCS9
    /// and accepted selectors one through three.
    pub one_ltf_800ns_gi: bool,
    /// The peer can transmit HE STBC below 80 MHz.
    ///
    /// HE PHY capability byte 2 bit 2. For the S31 non-AP role this is the
    /// peer capability required before attempting a controlled downlink RX
    /// STBC qualification.
    pub stbc_transmit_under_80_mhz: bool,
    /// The peer can receive HE STBC below 80 MHz.
    ///
    /// SOURCE[BLOB_LIBNET80211_HE_CAP_STBC]: complete
    /// `_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` copies
    /// `g_phy_cap_rx_stbc` into HE PHY capability byte 2 bit 3. Complete
    /// `esp_wifi_enable_rx_stbc` owns that one-byte capability flag and the
    /// corresponding interface-state bits.
    pub stbc_receive_under_80_mhz: bool,
    /// Maximum DCM constellation the peer can transmit.
    ///
    /// HE PHY capability byte 3 bits 1:0. This is useful for the open RX
    /// policy, but is deliberately separate from [`Self::dcm_receive`].
    pub dcm_transmit: HeDcmConstellation,
    /// Maximum DCM constellation the peer can receive.
    ///
    /// SOURCE[LINUX_IEEE80211_HE_PHY_CAP3_DCM_2026_07_29]: Linux
    /// `include/linux/ieee80211-he.h` names HE PHY capability byte 3 bits
    /// 4:3 `DCM_MAX_CONST_RX`. `_oracles/libpp.a[trc.o]::rcGetDCMMaxRate`
    /// independently maps the same four capability levels to disabled,
    /// BPSK/MCS0, QPSK/MCS1 and 16-QAM/MCS3 for the vendor BCC path.
    pub dcm_receive: HeDcmConstellation,
    /// The peer can send SU beamforming feedback in a Trigger frame response.
    pub triggered_su_beamforming_feedback: bool,
    /// The peer can send partial-bandwidth MU feedback in a Trigger response.
    pub triggered_mu_beamforming_partial_bandwidth_feedback: bool,
    /// The peer can send CQI feedback in a Trigger frame response.
    ///
    /// HE PHY capability byte 6 bit 4. This is distinct from
    /// [`Self::non_triggered_cqi_feedback`].
    pub triggered_cqi_feedback: bool,
    /// The peer can send CQI feedback without a Trigger frame.
    ///
    /// HE PHY capability byte 9 bit 1.
    pub non_triggered_cqi_feedback: bool,
}

impl He20Capabilities {
    pub const fn supports_bidirectional_mcs9(self) -> bool {
        self.receive_nss1.supports_mcs9() && self.transmit_nss1.supports_mcs9()
    }

    pub const fn supports_one_ltf_800ns_gi(self) -> bool {
        self.one_ltf_800ns_gi
    }

    pub const fn dcm_receive_constellation(self) -> HeDcmConstellation {
        self.dcm_receive
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
        one_ltf_800ns_gi: element[10] & 0x40 != 0,
        stbc_transmit_under_80_mhz: element[11] & 0x04 != 0,
        stbc_receive_under_80_mhz: element[11] & 0x08 != 0,
        dcm_transmit: HeDcmConstellation::from_encoding(element[12]),
        dcm_receive: HeDcmConstellation::from_encoding(element[12] >> 3),
        triggered_su_beamforming_feedback: element[15] & 0x04 != 0,
        triggered_mu_beamforming_partial_bandwidth_feedback: element[15] & 0x08 != 0,
        triggered_cqi_feedback: element[15] & 0x10 != 0,
        non_triggered_cqi_feedback: element[18] & 0x02 != 0,
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
        assert!(!capability.supports_one_ltf_800ns_gi());
        assert_eq!(
            capability.dcm_receive_constellation(),
            HeDcmConstellation::NotSupported
        );
    }

    #[test]
    fn parses_optional_one_ltf_800ns_gi_capability() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[10] = 0x40;
        assert!(parse_he20_capabilities(&element)
            .unwrap()
            .supports_one_ltf_800ns_gi());
    }

    #[test]
    fn parses_independent_dcm_transmit_and_receive_constellations() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        // Peer TX: QPSK (bits 1:0 = 2); peer RX: 16-QAM
        // (bits 4:3 = 3). NSS stays one when bits 2 and 5 are clear.
        element[12] = 0x1a;
        let capability = parse_he20_capabilities(&element).unwrap();
        assert_eq!(capability.dcm_transmit, HeDcmConstellation::Qpsk);
        assert_eq!(capability.dcm_receive, HeDcmConstellation::Qam16);
        assert!(capability.dcm_receive.supports_bpsk());
        assert!(capability.dcm_receive.supports_qpsk());
        assert!(capability.dcm_receive.supports_16qam());
    }

    #[test]
    fn parses_stbc_and_independent_cqi_feedback_capabilities() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[11] = 0x0c;
        element[15] = 0x1c;
        element[18] = 0x02;
        let capability = parse_he20_capabilities(&element).unwrap();
        assert!(capability.stbc_transmit_under_80_mhz);
        assert!(capability.stbc_receive_under_80_mhz);
        assert!(capability.triggered_su_beamforming_feedback);
        assert!(capability.triggered_mu_beamforming_partial_bandwidth_feedback);
        assert!(capability.triggered_cqi_feedback);
        assert!(capability.non_triggered_cqi_feedback);
    }

    #[test]
    fn decodes_the_vendor_s31_sta_stbc_and_cqi_advertisement() {
        let capability = [
            0xff, 0x16, 0x23, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d,
            0xc0, 0x1f, 0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
        ];
        let capability = parse_he20_capabilities(&capability).unwrap();
        assert!(!capability.stbc_transmit_under_80_mhz);
        assert!(capability.stbc_receive_under_80_mhz);
        assert_eq!(capability.dcm_transmit, HeDcmConstellation::Qam16);
        assert_eq!(capability.dcm_receive, HeDcmConstellation::Qam16);
        assert!(capability.triggered_su_beamforming_feedback);
        assert!(capability.triggered_mu_beamforming_partial_bandwidth_feedback);
        assert!(capability.triggered_cqi_feedback);
        assert!(capability.non_triggered_cqi_feedback);
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
        assert!(!parse_he20_capabilities(&capability)
            .unwrap()
            .supports_one_ltf_800ns_gi());
        assert_eq!(
            parse_he20_capabilities(&capability)
                .unwrap()
                .dcm_receive_constellation(),
            HeDcmConstellation::NotSupported
        );
    }
}
