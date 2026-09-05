//! Station association request construction and response admission.

use super::*;

/// Relative HE UL-MU transmit-power capability for MAC rates 16 through 25.
///
/// The caller supplies calibrated Rust-owned PHY gain-table indices. No ROM
/// function, C ABI callback or vendor global is retained at this boundary.
///
/// SOURCE: complete `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_ulmu_pwrcap` queries `phy_get_max_pwr` for rates 16..=25,
/// subtracts every rate 17..=25 primary byte from rate 16, then writes the
/// nine differences after Extension ID 60 and two reserved zero bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeUlMuPowerCapability {
    relative_to_rate_16: [u8; 9],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeUlMuPowerCapabilityError {
    HigherPowerThanRate16 { rate: u8 },
}

impl HeUlMuPowerCapability {
    pub fn from_rate_power_indices(
        rate_16_through_25: [i8; 10],
    ) -> Result<Self, HeUlMuPowerCapabilityError> {
        let base = i16::from(rate_16_through_25[0]);
        let mut relative_to_rate_16 = [0_u8; 9];
        for (offset, (relative, power)) in relative_to_rate_16
            .iter_mut()
            .zip(rate_16_through_25[1..].iter().copied())
            .enumerate()
        {
            let difference = base - i16::from(power);
            if !(0..=i16::from(u8::MAX)).contains(&difference) {
                return Err(HeUlMuPowerCapabilityError::HigherPowerThanRate16 {
                    rate: 17 + offset as u8,
                });
            }
            *relative = difference as u8;
        }
        Ok(Self {
            relative_to_rate_16,
        })
    }

    pub const fn relative_to_rate_16(self) -> [u8; 9] {
        self.relative_to_rate_16
    }

    fn encode(self) -> [u8; HE_UL_MU_POWER_CAPABILITY_IE_LEN] {
        let mut element = [0_u8; HE_UL_MU_POWER_CAPABILITY_IE_LEN];
        element[0] = 255;
        element[1] = 12;
        element[2] = HE_UL_MU_POWER_CAPABILITY_EXTENSION_ID;
        element[3..12].copy_from_slice(&self.relative_to_rate_16);
        element
    }
}

/// Minimum and maximum transmit power advertised by an HE STA, in dBm.
///
/// SOURCE: complete `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_power_cap` writes Element ID 33, the result of
/// `hal_get_tx_min_pwr`, and `hal_get_tx_pwr(16, 1)`. Complete
/// `ieee80211_assoc_req_construct` emits this element immediately after RSN
/// and before Extended Supported Rates whenever the HE cipher path is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPowerCapability {
    minimum_dbm: i8,
    maximum_dbm: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerCapabilityError {
    MinimumAboveMaximum,
}

impl StaPowerCapability {
    pub const fn new(minimum_dbm: i8, maximum_dbm: i8) -> Result<Self, StaPowerCapabilityError> {
        if minimum_dbm > maximum_dbm {
            return Err(StaPowerCapabilityError::MinimumAboveMaximum);
        }
        Ok(Self {
            minimum_dbm,
            maximum_dbm,
        })
    }

    pub const fn minimum_dbm(self) -> i8 {
        self.minimum_dbm
    }

    pub const fn maximum_dbm(self) -> i8 {
        self.maximum_dbm
    }

    fn encode(self) -> [u8; POWER_CAPABILITY_IE_LEN] {
        [33, 2, self.minimum_dbm as u8, self.maximum_dbm as u8]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationPhy {
    Legacy,
    Ht20,
    Ht40,
    He20,
}

impl StaAssociationPhy {
    pub const fn bandwidth_mhz(self) -> u8 {
        match self {
            Self::Ht40 => 40,
            Self::Legacy | Self::Ht20 | Self::He20 => 20,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Ht20 => "ht20",
            Self::Ht40 => "ht40",
            Self::He20 => "he20",
        }
    }
}

/// Caller policy applied before constructing one STA association request.
///
/// These are preferences rather than unchecked PHY claims. `PreferHe20`
/// still falls back when the peer does not advertise the complete HE20 MCS9
/// contract; `ForceHt20` is retained as the diagnostic negative-control mode
/// and may subsequently be rejected by [`AssociationRequest::encode`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationPreference {
    Automatic,
    PreferHe20,
    ForceHt20,
}

/// Complete local capability elements supplied by the hardware/role profile.
///
/// The portable encoder owns peer admission, security selection and IE order.
/// The caller owns which local capabilities can be advertised; this type has
/// no default chip profile and does not derive capabilities from peer claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationCapabilities {
    pub ht20: [u8; 28],
    pub ht40: [u8; 28],
    pub he20_ht: [u8; 28],
    pub he20: [u8; 24],
    pub he20_extended: [u8; 14],
    pub wmm: [u8; 9],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationRequest<'a> {
    pub source: [u8; 6],
    pub access_point: &'a ScanRecord,
    pub sequence_number: u16,
    pub listen_interval: u16,
    pub phy: StaAssociationPhy,
    /// Exact BSS security selected by the station request.
    pub security: WifiSecurityMode,
    /// HE Power Capability derived from the same calibrated rate-16 power
    /// source used by the MAC. Non-HE modes must leave it absent.
    pub power_capability: Option<StaPowerCapability>,
    /// Runtime-calibrated UL-MU power capability required by the complete HE
    /// association contract. Non-HE modes must leave it absent.
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
}

impl AssociationRequest<'_> {
    pub fn encode(
        self,
        output: &mut [u8],
        capabilities: &AssociationCapabilities,
    ) -> Result<usize, AssociationRequestError> {
        validate_peer(self.access_point.bssid, self.sequence_number)
            .map_err(AssociationRequestError::Frame)?;
        let ssid = self.access_point.ssid_bytes();
        if ssid.len() > MAX_SSID_LEN {
            return Err(AssociationRequestError::Frame(
                StationFrameError::SsidTooLong,
            ));
        }

        let rates_len = usize::from(self.access_point.supported_rates_len)
            + usize::from(self.access_point.extended_supported_rates_len);
        if rates_len == 0 {
            return Err(AssociationRequestError::Frame(
                StationFrameError::NoSupportedRates,
            ));
        }
        if rates_len > MAX_SUPPORTED_RATES_LEN {
            return Err(AssociationRequestError::Frame(
                StationFrameError::TooManySupportedRates,
            ));
        }
        let first_rates_len = rates_len.min(SUPPORTED_RATES_ELEMENT_CAPACITY);
        let extended_rates_len = rates_len - first_rates_len;
        let selected_rsn = select_association_rsn(self.access_point, self.security)
            .map_err(AssociationRequestError::Security)?;
        let (ht_capability, he_capability, power_capability, he_ul_mu_power) = match self.phy {
            StaAssociationPhy::Legacy => (None, None, None, None),
            StaAssociationPhy::Ht20 if self.access_point.ht_capability_ie_present => {
                (Some(&capabilities.ht20), None, None, None)
            }
            StaAssociationPhy::Ht20 => {
                return Err(AssociationRequestError::HtUnsupportedByAccessPoint);
            }
            StaAssociationPhy::Ht40 if self.access_point.ht40_secondary_channel().is_some() => {
                (Some(&capabilities.ht40), None, None, None)
            }
            StaAssociationPhy::Ht40 => {
                return Err(AssociationRequestError::Ht40UnsupportedByAccessPoint);
            }
            StaAssociationPhy::He20
                if self.access_point.ht_capability_ie_present
                    && parse_he20_capabilities(self.access_point.he_capability_ie_bytes())
                        .is_ok_and(|capability| capability.supports_bidirectional_mcs9())
                    && parse_he20_operation(self.access_point.he_operation_ie_bytes()).is_ok() =>
            {
                (
                    Some(&capabilities.he20_ht),
                    Some(&capabilities.he20),
                    Some(
                        self.power_capability
                            .ok_or(AssociationRequestError::MissingPowerCapability)?,
                    ),
                    Some(
                        self.he_ul_mu_power
                            .ok_or(AssociationRequestError::MissingHeUlMuPowerCapability)?,
                    ),
                )
            }
            StaAssociationPhy::He20 => {
                return Err(AssociationRequestError::He20UnsupportedByAccessPoint);
            }
        };
        if self.phy != StaAssociationPhy::He20 {
            if self.power_capability.is_some() {
                return Err(AssociationRequestError::UnexpectedPowerCapability);
            }
            if self.he_ul_mu_power.is_some() {
                return Err(AssociationRequestError::UnexpectedHeUlMuPowerCapability);
            }
        }
        let phy_information_len = if let Some(capability) = ht_capability {
            capability.len()
                + he_capability.map_or(0, |capability| capability.len())
                + he_ul_mu_power.map_or(0, |_| HE_UL_MU_POWER_CAPABILITY_IE_LEN)
                + capabilities.wmm.len()
                + usize::from(he_capability.is_some()) * capabilities.he20_extended.len()
        } else {
            0
        };
        let required = MANAGEMENT_HEADER_LEN
            + ASSOCIATION_FIXED_BODY_LEN
            + 2
            + ssid.len()
            + 2
            + first_rates_len
            + usize::from(extended_rates_len != 0) * (2 + extended_rates_len)
            + selected_rsn.as_bytes().len()
            + power_capability.map_or(0, |_| POWER_CAPABILITY_IE_LEN)
            + phy_information_len;
        if output.len() < required {
            return Err(AssociationRequestError::Frame(
                StationFrameError::OutputTooSmall { required },
            ));
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            ASSOCIATION_REQUEST_FRAME_CONTROL,
            self.access_point.bssid,
            self.source,
            self.access_point.bssid,
            self.sequence_number,
        );
        // Derive Privacy from the exact requested mode, rather than merely
        // reflecting an untrusted scan record. Candidate admission already
        // requires the same value; this keeps the transmitted request
        // fail-closed if a record is ever assembled outside that path.
        let capability = ((self.access_point.capability_info & ASSOCIATION_CAPABILITY_MASK) | 1)
            & !0x0010
            | match self.security {
                WifiSecurityMode::Open => 0,
                WifiSecurityMode::Wpa2Personal => 0x0010,
            };
        frame[24..26].copy_from_slice(&capability.to_le_bytes());
        frame[26..28].copy_from_slice(&self.listen_interval.to_le_bytes());

        let mut offset = MANAGEMENT_HEADER_LEN + ASSOCIATION_FIXED_BODY_LEN;
        write_element(frame, &mut offset, 0, ssid);

        frame[offset] = 1;
        frame[offset + 1] = first_rates_len as u8;
        offset += 2;
        copy_rates(
            self.access_point,
            0,
            &mut frame[offset..offset + first_rates_len],
        );
        offset += first_rates_len;

        let rsn = selected_rsn.as_bytes();
        frame[offset..offset + rsn.len()].copy_from_slice(rsn);
        offset += rsn.len();
        if let Some(capability) = power_capability {
            let capability = capability.encode();
            frame[offset..offset + capability.len()].copy_from_slice(&capability);
            offset += capability.len();
        }

        // SOURCE: complete `libnet80211.a[ieee80211_output.o]::
        // ieee80211_assoc_req_construct` appends Extended Supported Rates
        // only after the selected RSN and the HE Power Capability.
        if extended_rates_len != 0 {
            frame[offset] = 50;
            frame[offset + 1] = extended_rates_len as u8;
            offset += 2;
            copy_rates(
                self.access_point,
                first_rates_len,
                &mut frame[offset..offset + extended_rates_len],
            );
            offset += extended_rates_len;
        }

        if let Some(capability) = ht_capability {
            frame[offset..offset + capability.len()].copy_from_slice(capability);
            offset += capability.len();
            if let Some(capability) = he_capability {
                frame[offset..offset + capability.len()].copy_from_slice(capability);
                offset += capability.len();
            }
            if let Some(capability) = he_ul_mu_power {
                let capability = capability.encode();
                frame[offset..offset + capability.len()].copy_from_slice(&capability);
                offset += capability.len();
            }
            frame[offset..offset + capabilities.wmm.len()].copy_from_slice(&capabilities.wmm);
            offset += capabilities.wmm.len();
            if he_capability.is_some() {
                frame[offset..offset + capabilities.he20_extended.len()]
                    .copy_from_slice(&capabilities.he20_extended);
                offset += capabilities.he20_extended.len();
            }
        }
        debug_assert_eq!(offset, required);
        Ok(required)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationRequestError {
    Frame(StationFrameError),
    Security(StaSecurityError),
    HtUnsupportedByAccessPoint,
    Ht40UnsupportedByAccessPoint,
    He20UnsupportedByAccessPoint,
    MissingPowerCapability,
    MissingHeUlMuPowerCapability,
    UnexpectedPowerCapability,
    UnexpectedHeUlMuPowerCapability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationResponse {
    pub capability_info: u16,
    pub status_code: u16,
    pub association_id: u16,
    pub ht_capability: bool,
    pub he_capability: bool,
    pub he_operation: bool,
    pub wmm: bool,
    pub wmm_parameters: Option<WmmParameterSet>,
}

impl AssociationResponse {
    /// Match the AP's successful response to the exact security mode selected
    /// from scan admission. There is no fallback between Open and WPA2.
    pub const fn matches_security(self, security: WifiSecurityMode) -> bool {
        let privacy = self.capability_info & 0x0010 != 0;
        match security {
            WifiSecurityMode::Open => !privacy,
            WifiSecurityMode::Wpa2Personal => privacy,
        }
    }
}

pub fn parse_association_response(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<AssociationResponse> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 6
        || read_u16(frame, 0)? & 0x00fc != ASSOCIATION_RESPONSE_FRAME_CONTROL
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
    {
        return None;
    }

    let mut ht_capability = false;
    let mut he_capability = false;
    let mut he_operation = false;
    let mut wmm = false;
    let mut wmm_parameters = None;
    let mut offset = MANAGEMENT_HEADER_LEN + 6;
    while offset + 2 <= frame.len() {
        let id = frame[offset];
        let length = usize::from(frame[offset + 1]);
        let end = offset.checked_add(2 + length)?;
        if end > frame.len() {
            return None;
        }
        let value = &frame[offset + 2..end];
        ht_capability |= id == 45 && length == 26;
        he_capability |= id == 255 && value.first() == Some(&35);
        he_operation |= id == 255 && value.first() == Some(&36);
        let is_wmm = id == 221 && length >= 6 && value.get(..4) == Some(&[0x00, 0x50, 0xf2, 0x02]);
        wmm |= is_wmm;
        if is_wmm {
            wmm_parameters = parse_wmm_parameter_element(&frame[offset..end]).or(wmm_parameters);
        }
        offset = end;
    }

    Some(AssociationResponse {
        capability_info: read_u16(frame, 24)?,
        status_code: read_u16(frame, 26)?,
        association_id: read_u16(frame, 28)? & 0x3fff,
        ht_capability,
        he_capability,
        he_operation,
        // An AP that returned HT Capability accepted the WMM/QoS data path,
        // even if a bounded RX prefix omitted a later vendor WMM element.
        wmm: wmm || ht_capability || he_capability,
        wmm_parameters,
    })
}

fn write_element(frame: &mut [u8], offset: &mut usize, id: u8, value: &[u8]) {
    frame[*offset] = id;
    frame[*offset + 1] = value.len() as u8;
    *offset += 2;
    frame[*offset..*offset + value.len()].copy_from_slice(value);
    *offset += value.len();
}

fn copy_rates(access_point: &ScanRecord, start: usize, output: &mut [u8]) {
    let ordinary = access_point.supported_rates_bytes();
    for (destination, index) in output.iter_mut().zip(start..) {
        *destination = if index < ordinary.len() {
            ordinary[index]
        } else {
            access_point.extended_supported_rates_bytes()[index - ordinary.len()]
        };
    }
}
