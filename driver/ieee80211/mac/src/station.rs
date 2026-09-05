//! Allocation-free STA authentication and association protocol.
//!
//! This module owns only IEEE 802.11 frame construction, response parsing,
//! and WPA2-Personal RSN selection. The caller retains the output buffer and
//! decides when to submit it to hardware, arm a deadline, or retry.

use crate::{
    ccmp::CCMP_HEADER_LEN,
    data::{
        DataHeControl, DataInterfaceRole, ETHERNET_HEADER_LEN, LLC_SNAP_HEADER_LEN,
        plan_data_encapsulation, plan_data_encapsulation_with_he_control,
    },
    he::{parse_he20_capabilities, parse_he20_operation},
    management::{MANAGEMENT_HEADER_LEN, MAX_SSID_LEN, MAX_SUPPORTED_RATES_LEN},
    scan::ScanRecord,
    security::WifiSecurityMode,
    wmm::{WmmParameterSet, parse_wmm_parameter_element},
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

/// Monotonic twelve-bit owner for one IEEE 802.11 transmit sequence space.
///
/// A sequence number is consumed for every newly encoded MPDU. Hardware
/// retries retain the already encoded header and therefore do not call
/// [`Self::take`].
///
/// This value deliberately represents only one space. Management/non-QoS
/// traffic and every QoS TID have independent counters, owned together by
/// [`StaTxSequenceCounters`].
///
/// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
/// ieee80211_ampdu_request` instructions 0x9a..0xa2 load the AddBA Starting
/// Sequence Number from the node's TID-indexed halfword at
/// `(tid + 0x50) * 2 + 0x0e`. The captured open-driver AddBA/action exchange
/// `HIL_OPEN_STA_SEQUENCE_SPACES_2026_07_30` demonstrated why an
/// interface-global counter is wrong: three action frames advanced the TID0
/// SSN before its first QoS MPDU. `libpp.a[pp.o]` keeps a hardware
/// retry attached to the already encoded frame, so retries do not consume a
/// new protocol sequence number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaSequenceCounter {
    next: u16,
}

impl StaSequenceCounter {
    pub const fn new(first: u16) -> Self {
        Self {
            next: first & 0x0fff,
        }
    }

    /// Consume the next sequence number, wrapping in the 802.11 twelve-bit
    /// sequence space.
    pub const fn take(&mut self) -> u16 {
        let sequence = self.next;
        self.next = self.next.wrapping_add(1) & 0x0fff;
        sequence
    }

    pub const fn peek(&self) -> u16 {
        self.next
    }
}

/// Allocation-free owner of all STA transmit sequence-number spaces.
///
/// IEEE 802.11 QoS traffic has one sequence space per TID. Management and
/// non-QoS data use the separate non-QoS space. Keeping the counters behind
/// this owner makes it impossible for an AddBA Action frame to silently
/// advance the SSN advertised for a QoS agreement.
///
/// The same initial value is safe for every entry because the spaces are
/// independent; callers may seed it from per-association entropy so a reset
/// does not reproduce the previous peer epoch's initial values.
#[derive(Debug, Eq, PartialEq)]
pub struct StaTxSequenceCounters {
    non_qos: StaSequenceCounter,
    qos: [StaSequenceCounter; 16],
}

impl StaTxSequenceCounters {
    pub const QOS_TID_COUNT: u8 = 16;

    pub const fn new(first: u16) -> Self {
        let counter = StaSequenceCounter::new(first);
        Self {
            non_qos: counter,
            qos: [counter; Self::QOS_TID_COUNT as usize],
        }
    }

    /// Borrow the management/non-QoS sequence-number owner.
    pub const fn non_qos_mut(&mut self) -> &mut StaSequenceCounter {
        &mut self.non_qos
    }

    pub const fn peek_non_qos(&self) -> u16 {
        self.non_qos.peek()
    }

    pub const fn take_non_qos(&mut self) -> u16 {
        self.non_qos.take()
    }

    /// Borrow one QoS/TID sequence-number owner.
    pub fn qos_mut(&mut self, tid: u8) -> Option<&mut StaSequenceCounter> {
        self.qos.get_mut(usize::from(tid))
    }

    pub fn peek_qos(&self, tid: u8) -> Option<u16> {
        self.qos.get(usize::from(tid)).map(StaSequenceCounter::peek)
    }

    pub fn take_qos(&mut self, tid: u8) -> Option<u16> {
        self.qos_mut(tid).map(StaSequenceCounter::take)
    }

    /// Consume a data-frame sequence number from the wire-format-selected
    /// space: `None` for a non-QoS header, or `Some(tid)` for a QoS header.
    pub fn take_data(&mut self, qos_tid: Option<u8>) -> Option<u16> {
        match qos_tid {
            Some(tid) => self.take_qos(tid),
            None => Some(self.take_non_qos()),
        }
    }
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSecurityError {
    SecurityModeMismatch,
    MissingRsn,
    MalformedRsn,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    ManagementFrameProtectionRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedRsn {
    length: u8,
    bytes: [u8; SELECTED_RSN_IE_LEN],
}

impl SelectedRsn {
    const EMPTY: Self = Self {
        length: 0,
        bytes: [0; SELECTED_RSN_IE_LEN],
    };

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenAuthenticationRequest {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
}

impl OpenAuthenticationRequest {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        let required = MANAGEMENT_HEADER_LEN + 6;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            OPEN_AUTHENTICATION_FRAME_CONTROL,
            self.bssid,
            self.source,
            self.bssid,
            self.sequence_number,
        );
        frame[24..26].copy_from_slice(&OPEN_SYSTEM_ALGORITHM.to_le_bytes());
        frame[26..28].copy_from_slice(&OPEN_SYSTEM_REQUEST_SEQUENCE.to_le_bytes());
        frame[28..30].copy_from_slice(&0_u16.to_le_bytes());
        Ok(required)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenAuthenticationResponse {
    pub status_code: u16,
}

/// Management transition by which the selected AP ends the STA relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectKind {
    Disassociation,
    Deauthentication,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDisconnect {
    pub kind: StaDisconnectKind,
    pub reason_code: u16,
}

/// One unprotected STA-originated Action management frame.
///
/// BlockAck negotiation uses this common 24-byte management header followed
/// by the nine-byte action body owned by the MAC BlockAck state machine.
///
/// SOURCE[PROMOTED_RX_AMPDU]: reviewed promoted ADDBA response builder,
/// where the same header was constructed around
/// `rx_ampdu::write_successful_addba_response`; the frame-control subtype is
/// the IEEE 802.11 Action management subtype also parsed by
/// `libnet80211.a`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaActionFrame<'a> {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
    pub body: &'a [u8],
}

impl StaActionFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        let required = MANAGEMENT_HEADER_LEN.checked_add(self.body.len()).ok_or(
            StationFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            ACTION_FRAME_CONTROL,
            self.bssid,
            self.source,
            self.bssid,
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..].copy_from_slice(self.body);
        Ok(required)
    }
}

/// Parse a response addressed to this station and BSSID.
///
/// `None` means that the frame is valid input but belongs to another
/// management exchange. This lets an RX loop ignore beacons and other peers
/// without treating them as protocol errors.
pub fn parse_open_authentication_response(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<OpenAuthenticationResponse> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 6
        || read_u16(frame, 0)? & 0x00fc != OPEN_AUTHENTICATION_FRAME_CONTROL
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
        || read_u16(frame, 24)? != OPEN_SYSTEM_ALGORITHM
        || read_u16(frame, 26)? != OPEN_SYSTEM_RESPONSE_SEQUENCE
    {
        return None;
    }
    Some(OpenAuthenticationResponse {
        status_code: read_u16(frame, 28)?,
    })
}

/// Parse a Disassociation or Deauthentication sent by the selected AP.
///
/// An authentication wait must treat this as a protocol transition rather
/// than an absent response. In particular, an AP may reject the first Open
/// Authentication after a rapid station reset by first clearing its previous
/// relationship with a Deauthentication frame.
///
/// SOURCE: complete
/// `libnet80211.a[ieee80211_sta.o]::sta_recv_mgmt`: branches `.L723`
/// (Disassociation) and `.L729` (Deauthentication) read the reason code at
/// management-body offset zero (`frame + 24`) and immediately call
/// `ieee80211_sta_new_state(g_ic, 0, (reason << 8) | subtype)`.
pub fn parse_sta_disconnect(frame: &[u8], local: [u8; 6], bssid: [u8; 6]) -> Option<StaDisconnect> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 2
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
    {
        return None;
    }

    let kind = match read_u16(frame, 0)? & 0x00fc {
        DISASSOCIATION_FRAME_CONTROL => StaDisconnectKind::Disassociation,
        DEAUTHENTICATION_FRAME_CONTROL => StaDisconnectKind::Deauthentication,
        _ => return None,
    };
    Some(StaDisconnect {
        kind,
        reason_code: read_u16(frame, MANAGEMENT_HEADER_LEN)?,
    })
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

/// One unprotected 802.11 data MPDU sent by a station through its AP.
///
/// This is the frame shape used for EAPOL before CCMP keys are installed.
/// The caller owns the Ethernet payload and the output DMA buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDataFrame<'a> {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub destination: [u8; 6],
    pub sequence_number: u16,
    pub ether_type: u16,
    pub payload: &'a [u8],
}

impl StaDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        let ethernet = ethernet_header(self.destination, self.source, self.ether_type);
        let plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            self.source,
            ethernet,
            7,
            false,
            false,
        )
        .expect("priority seven is a valid recovered queue class");
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(plan.llc_snap.len())
            .and_then(|length| length.checked_add(self.payload.len()))
            .ok_or(StationFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame.fill(0);
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let llc_end = header_len + plan.llc_snap.len();
        frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(self.payload);
        Ok(required)
    }
}

/// One protected data MPDU prepared for S31 hardware CCMP encryption.
///
/// The CCMP header carries the packet number owned by the installed hardware
/// key token. The payload remains plaintext in DMA memory; the MAC encrypts it
/// and writes the eight-byte CCMP MIC into caller-reserved trailer space.
///
/// `peer_qos` is the association result. It selects the same legacy/QoS header
/// boundary as the recovered `net80211_encap::plan_data_encapsulation` path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedDataFrame<'a> {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub destination: [u8; 6],
    pub sequence_number: u16,
    pub user_priority: u8,
    pub peer_qos: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ether_type: u16,
    pub payload: &'a [u8],
}

impl StaProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        self.encode_with_he_control(DataHeControl::Disabled, output)
    }

    /// Encode a protected QoS MPDU with a hardware-generated HE-Control field.
    ///
    /// The returned chip-independent DMA image keeps CCMP immediately after
    /// the 26-byte QoS header. ESP32-S31 accounts for the four bytes inserted
    /// on air in private TX metadata, not as bytes in this frame.
    pub fn encode_with_he_control(
        self,
        he_control: DataHeControl,
        output: &mut [u8],
    ) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        if self.user_priority > 7 {
            return Err(StationFrameError::UserPriorityOutOfRange);
        }
        if !self.peer_qos
            && matches!(
                he_control,
                DataHeControl::HardwareGeneratedBufferStatusReport
            )
        {
            return Err(StationFrameError::HeControlRequiresQos);
        }
        let ethernet = ethernet_header(self.destination, self.source, self.ether_type);
        let mut plan = plan_data_encapsulation_with_he_control(
            DataInterfaceRole::Station,
            self.bssid,
            self.source,
            ethernet,
            self.user_priority,
            self.peer_qos,
            false,
            he_control,
        )
        .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        // Exact `net80211_tx::encapsulate_ordinary` mutation after successful
        // CCMP key selection.
        plan.header[1] |= 0x40;
        let header_len = usize::from(plan.header_len);
        let dma_header_len = plan.dma_header_len();
        let required = dma_header_len
            .checked_add(CCMP_HEADER_LEN)
            .and_then(|length| length.checked_add(plan.llc_snap.len()))
            .and_then(|length| length.checked_add(self.payload.len()))
            .ok_or(StationFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        // Every byte in the returned DMA image is initialized by one of the
        // exact writes below. Do not clear the complete frame first: for a
        // full 32-MPDU aggregate that redundant pass wrote roughly 48 KiB to
        // PSRAM before immediately overwriting it with the real payload.
        //
        // SOURCE: complete `libnet80211.a[ieee80211_output.o]::
        // ieee80211_encap_esfbuf` mutates the ESF header/headroom and retains
        // the existing payload; it does not clear the complete MPDU.
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_end = dma_header_len + CCMP_HEADER_LEN;
        frame[dma_header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        let llc_end = ccmp_end + plan.llc_snap.len();
        frame[ccmp_end..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(self.payload);
        Ok(required)
    }
}

/// Metadata for converting one owned Ethernet frame to a protected data MPDU
/// in its existing allocation.
///
/// The buffer owner reserves prefix space before exposing the Ethernet slice
/// to a network stack. Once the stack returns ownership, [`Self::encode_in_place`]
/// replaces only the Ethernet header and reserved prefix. The payload is
/// already at its final DMA offset and is never copied.
///
/// This is the chip-independent half of the vendor cache-TX ESF contract.
/// Retaining the allocation until TX/BlockAck completion remains the
/// responsibility of the chip-specific DMA owner.
///
/// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
/// ieee80211_alloc_tx_buf` type-nine branch stores the referenced netstack
/// data pointer in the ESF DMA descriptor and calls `s_netstack_ref`.
/// Complete `ieee80211_encap_esfbuf` mutates the ESF data boundary and writes
/// the 802.11/LLC prefix without copying the retained payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedEthernetFrame {
    pub bssid: [u8; 6],
    pub sequence_number: u16,
    pub user_priority: u8,
    pub peer_qos: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
}

/// Location of an MPDU produced inside a larger owned allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedStaFrame {
    pub offset: usize,
    pub length: usize,
}

impl StaProtectedEthernetFrame {
    /// Convert one Ethernet-II frame to a protected MPDU without moving its
    /// payload.
    ///
    /// `ethernet_offset` points to the DA byte written by the network stack;
    /// `ethernet_length` includes the fourteen-byte Ethernet header. On
    /// success the returned region starts in the reserved headroom and ends at
    /// exactly the same byte as the input Ethernet frame.
    pub fn encode_in_place(
        self,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
        he_control: DataHeControl,
    ) -> Result<EncodedStaFrame, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        if self.user_priority > 7 {
            return Err(StationFrameError::UserPriorityOutOfRange);
        }
        if !self.peer_qos
            && matches!(
                he_control,
                DataHeControl::HardwareGeneratedBufferStatusReport
            )
        {
            return Err(StationFrameError::HeControlRequiresQos);
        }
        if ethernet_length < ETHERNET_HEADER_LEN {
            return Err(StationFrameError::EthernetFrameTooShort);
        }
        let ethernet_end = ethernet_offset.checked_add(ethernet_length).ok_or(
            StationFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if storage.len() < ethernet_end {
            return Err(StationFrameError::OutputTooSmall {
                required: ethernet_end,
            });
        }

        // Preserve the bytes that the new CCMP/LLC prefix overlaps before
        // mutating the shared allocation.
        let destination = storage[ethernet_offset..ethernet_offset + 6]
            .try_into()
            .expect("six-byte Ethernet destination");
        let source = storage[ethernet_offset + 6..ethernet_offset + 12]
            .try_into()
            .expect("six-byte Ethernet source");
        let ether_type =
            u16::from_be_bytes([storage[ethernet_offset + 12], storage[ethernet_offset + 13]]);
        let ethernet = ethernet_header(destination, source, ether_type);
        let mut plan = plan_data_encapsulation_with_he_control(
            DataInterfaceRole::Station,
            self.bssid,
            source,
            ethernet,
            self.user_priority,
            self.peer_qos,
            false,
            he_control,
        )
        .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        plan.header[1] |= 0x40;

        let header_len = usize::from(plan.header_len);
        let dma_header_len = plan.dma_header_len();
        let prefix_len = dma_header_len + CCMP_HEADER_LEN + plan.llc_snap.len();
        let headroom = prefix_len - ETHERNET_HEADER_LEN;
        let frame_offset = ethernet_offset.checked_sub(headroom).ok_or(
            StationFrameError::EthernetHeadroomTooSmall {
                required: headroom,
                available: ethernet_offset,
            },
        )?;
        let frame_length = ethernet_length + headroom;
        let frame_end =
            frame_offset
                .checked_add(frame_length)
                .ok_or(StationFrameError::OutputTooSmall {
                    required: usize::MAX,
                })?;
        debug_assert_eq!(frame_end, ethernet_end);
        debug_assert_eq!(
            frame_offset + prefix_len,
            ethernet_offset + ETHERNET_HEADER_LEN
        );

        let frame = &mut storage[frame_offset..frame_end];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_end = dma_header_len + CCMP_HEADER_LEN;
        frame[dma_header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        frame[ccmp_end..prefix_len].copy_from_slice(&plan.llc_snap);

        Ok(EncodedStaFrame {
            offset: frame_offset,
            length: frame_length,
        })
    }

    /// Coalesce two Ethernet frames into one protected A-MSDU in the first
    /// frame's allocation.
    ///
    /// The first Ethernet payload is moved forward inside `storage`; the
    /// second frame is copied behind it. The returned MPDU starts at the same
    /// offset as [`Self::encode_in_place`], so an S31 metadata word can remain
    /// immediately before it. The caller may release the second frame as soon
    /// as this method returns successfully.
    ///
    /// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
    /// ieee80211_encap_amsdu`. Its `.L940` branch uses `memmove` to grow the
    /// first cache ESF in place; `.L950` copies the following ESF body into
    /// that allocation and calls `ieee80211_recycle_cache_eb` immediately.
    /// Thus vendor A-MSDU construction copies between netstack owners; only
    /// the resulting MPDU remains referenced through A-MPDU/DMA completion.
    pub fn encode_amsdu_pair_in_place(
        self,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
        second_ethernet: &[u8],
    ) -> Result<EncodedStaFrame, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        if self.user_priority > 7 {
            return Err(StationFrameError::UserPriorityOutOfRange);
        }
        if !self.peer_qos {
            return Err(StationFrameError::AmsduRequiresQos);
        }
        let ethernet_end = ethernet_offset.checked_add(ethernet_length).ok_or(
            StationFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if ethernet_length < ETHERNET_HEADER_LEN || second_ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(StationFrameError::EthernetFrameTooShort);
        }
        if storage.len() < ethernet_end {
            return Err(StationFrameError::OutputTooSmall {
                required: ethernet_end,
            });
        }

        let first_header: [u8; ETHERNET_HEADER_LEN] = storage
            [ethernet_offset..ethernet_offset + ETHERNET_HEADER_LEN]
            .try_into()
            .expect("Ethernet length checked above");
        let source: [u8; 6] = first_header[6..12]
            .try_into()
            .expect("six-byte Ethernet source");
        let required =
            sta_protected_amsdu_pair_frame_length(ethernet_length, second_ethernet.len())?;
        let frame_offset = ethernet_offset
            .checked_sub(STA_PROTECTED_QOS_ETHERNET_HEADROOM)
            .ok_or(StationFrameError::EthernetHeadroomTooSmall {
                required: STA_PROTECTED_QOS_ETHERNET_HEADROOM,
                available: ethernet_offset,
            })?;
        let frame_end =
            frame_offset
                .checked_add(required)
                .ok_or(StationFrameError::OutputTooSmall {
                    required: usize::MAX,
                })?;
        if storage.len() < frame_end {
            return Err(StationFrameError::OutputTooSmall {
                required: frame_end,
            });
        }

        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            source,
            first_header,
            self.user_priority,
            true,
            false,
        )
        .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        plan.header[1] |= 0x40;
        plan.header[24] |= 0x80;

        let header_len = usize::from(plan.header_len);
        debug_assert_eq!(header_len, crate::data::IEEE80211_QOS_DATA_HEADER_LEN);
        let first_subframe = frame_offset + header_len + CCMP_HEADER_LEN;
        let first_payload_length = ethernet_length - ETHERNET_HEADER_LEN;
        let first_payload_destination =
            first_subframe + AMSDU_SUBFRAME_HEADER_LEN + LLC_SNAP_HEADER_LEN;
        storage.copy_within(
            ethernet_offset + ETHERNET_HEADER_LEN..ethernet_end,
            first_payload_destination,
        );

        storage[frame_offset..frame_offset + header_len]
            .copy_from_slice(&plan.header[..header_len]);
        storage[frame_offset + 22..frame_offset + 24]
            .copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_offset = frame_offset + header_len;
        storage[ccmp_offset..ccmp_offset + CCMP_HEADER_LEN].copy_from_slice(&self.ccmp_header);

        let first_msdu_length = LLC_SNAP_HEADER_LEN + first_payload_length;
        storage[first_subframe..first_subframe + 12].copy_from_slice(&first_header[..12]);
        storage[first_subframe + 12..first_subframe + 14]
            .copy_from_slice(&(first_msdu_length as u16).to_be_bytes());
        let first_llc = first_subframe + AMSDU_SUBFRAME_HEADER_LEN;
        storage[first_llc..first_llc + 6].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
        storage[first_llc + 6..first_llc + 8].copy_from_slice(&first_header[12..14]);

        let first_subframe_length =
            AMSDU_SUBFRAME_HEADER_LEN + LLC_SNAP_HEADER_LEN + first_payload_length;
        let first_padding = (4 - (first_subframe_length & 3)) & 3;
        let second_subframe = first_subframe + first_subframe_length + first_padding;
        storage[second_subframe - first_padding..second_subframe].fill(0);

        let second_payload = &second_ethernet[ETHERNET_HEADER_LEN..];
        let second_msdu_length = LLC_SNAP_HEADER_LEN + second_payload.len();
        storage[second_subframe..second_subframe + 12].copy_from_slice(&second_ethernet[..12]);
        storage[second_subframe + 12..second_subframe + 14]
            .copy_from_slice(&(second_msdu_length as u16).to_be_bytes());
        let second_llc = second_subframe + AMSDU_SUBFRAME_HEADER_LEN;
        storage[second_llc..second_llc + 6].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
        storage[second_llc + 6..second_llc + 8].copy_from_slice(&second_ethernet[12..14]);
        let second_payload_offset = second_llc + LLC_SNAP_HEADER_LEN;
        storage[second_payload_offset..second_payload_offset + second_payload.len()]
            .copy_from_slice(second_payload);
        debug_assert_eq!(second_payload_offset + second_payload.len(), frame_end);

        Ok(EncodedStaFrame {
            offset: frame_offset,
            length: required,
        })
    }
}

/// One protected QoS A-MSDU prepared for S31 hardware CCMP encryption.
///
/// Every borrowed element is a complete Ethernet-II frame. The encoder emits
/// one outer To-DS QoS MPDU and converts each Ethernet frame to the IEEE
/// 802.11 A-MSDU subframe form (DA, SA, big-endian MSDU length, RFC1042
/// LLC/SNAP body and non-final four-byte padding). The returned length stops
/// before the hardware-owned CCMP MIC and FCS, exactly like
/// [`StaProtectedDataFrame`].
///
/// SOURCE: IEEE 802.11 A-MSDU wire format, cross-checked against the inverse
/// iterator in `data::AmsduSubframes`. The 3,839-byte ceiling is the baseline
/// The smaller standard A-MSDU length used by the current bounded encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedAmsduFrame<'a> {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
    pub user_priority: u8,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ethernet_frames: &'a [&'a [u8]],
}

/// Returns the encoded protected QoS MPDU length for an A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS, matching
/// [`StaProtectedAmsduFrame::encode`]. Keeping this calculation beside the
/// encoder lets a bounded A-MPDU owner check its negotiated byte ceiling
/// before consuming a sequence number or CCMP packet number.
pub fn sta_protected_amsdu_frame_length(
    ethernet_frames: &[&[u8]],
) -> Result<usize, StationFrameError> {
    if ethernet_frames.is_empty() {
        return Err(StationFrameError::NoAmsduFrames);
    }

    let mut amsdu_length = 0_usize;
    for (index, ethernet) in ethernet_frames.iter().copied().enumerate() {
        if ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(StationFrameError::EthernetFrameTooShort);
        }
        let msdu_length = LLC_SNAP_HEADER_LEN
            .checked_add(ethernet.len() - ETHERNET_HEADER_LEN)
            .ok_or(StationFrameError::AmsduTooLong {
                length: usize::MAX,
                maximum: HT_AMSDU_BASELINE_MAX_LEN,
            })?;
        if msdu_length > usize::from(u16::MAX) {
            return Err(StationFrameError::AmsduTooLong {
                length: msdu_length,
                maximum: HT_AMSDU_BASELINE_MAX_LEN,
            });
        }
        let subframe_length = AMSDU_SUBFRAME_HEADER_LEN.checked_add(msdu_length).ok_or(
            StationFrameError::AmsduTooLong {
                length: usize::MAX,
                maximum: HT_AMSDU_BASELINE_MAX_LEN,
            },
        )?;
        amsdu_length =
            amsdu_length
                .checked_add(subframe_length)
                .ok_or(StationFrameError::AmsduTooLong {
                    length: usize::MAX,
                    maximum: HT_AMSDU_BASELINE_MAX_LEN,
                })?;
        if index + 1 != ethernet_frames.len() {
            amsdu_length = amsdu_length
                .checked_add((4 - (subframe_length & 3)) & 3)
                .ok_or(StationFrameError::AmsduTooLong {
                    length: usize::MAX,
                    maximum: HT_AMSDU_BASELINE_MAX_LEN,
                })?;
        }
    }
    if amsdu_length > HT_AMSDU_BASELINE_MAX_LEN {
        return Err(StationFrameError::AmsduTooLong {
            length: amsdu_length,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        });
    }

    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(CCMP_HEADER_LEN)
        .and_then(|length| length.checked_add(amsdu_length))
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })
}

/// Encoded protected-MPDU length for exactly two Ethernet MSDUs.
///
/// This is the non-mutating admission half of
/// [`StaProtectedEthernetFrame::encode_amsdu_pair_in_place`]. It allows a
/// bounded DMA owner to check both the 3,839-byte negotiated A-MSDU class and
/// its allocation capacity before consuming sequence and CCMP numbers.
pub fn sta_protected_amsdu_pair_frame_length(
    first_ethernet_length: usize,
    second_ethernet_length: usize,
) -> Result<usize, StationFrameError> {
    if first_ethernet_length < ETHERNET_HEADER_LEN || second_ethernet_length < ETHERNET_HEADER_LEN {
        return Err(StationFrameError::EthernetFrameTooShort);
    }
    let first_subframe = first_ethernet_length
        .checked_add(LLC_SNAP_HEADER_LEN)
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })?;
    let second_subframe = second_ethernet_length
        .checked_add(LLC_SNAP_HEADER_LEN)
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })?;
    let first_padding = (4 - (first_subframe & 3)) & 3;
    let amsdu_length = first_subframe
        .checked_add(first_padding)
        .and_then(|length| length.checked_add(second_subframe))
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })?;
    if amsdu_length > HT_AMSDU_BASELINE_MAX_LEN {
        return Err(StationFrameError::AmsduTooLong {
            length: amsdu_length,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        });
    }
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(CCMP_HEADER_LEN)
        .and_then(|length| length.checked_add(amsdu_length))
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })
}

impl StaProtectedAmsduFrame<'_> {
    fn plan(&self) -> Result<(crate::data::DataEncapPlan, usize), StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        if self.user_priority > 7 {
            return Err(StationFrameError::UserPriorityOutOfRange);
        }
        let Some(first) = self.ethernet_frames.first().copied() else {
            return Err(StationFrameError::NoAmsduFrames);
        };
        if first.len() < ETHERNET_HEADER_LEN {
            return Err(StationFrameError::EthernetFrameTooShort);
        }
        let first_header: [u8; ETHERNET_HEADER_LEN] = first[..ETHERNET_HEADER_LEN]
            .try_into()
            .expect("length checked above");
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            self.source,
            first_header,
            self.user_priority,
            true,
            false,
        )
        .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        plan.header[1] |= 0x40;
        plan.header[24] |= 0x80;
        let required = sta_protected_amsdu_frame_length(self.ethernet_frames)?;
        Ok((plan, required))
    }

    fn write_header(
        &self,
        plan: crate::data::DataEncapPlan,
        required: usize,
        output: &mut [u8],
    ) -> Result<usize, StationFrameError> {
        let header_len = usize::from(plan.header_len);
        debug_assert_eq!(header_len, crate::data::IEEE80211_QOS_DATA_HEADER_LEN);
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let offset = header_len;
        frame[offset..offset + CCMP_HEADER_LEN].copy_from_slice(&self.ccmp_header);
        Ok(required)
    }

    /// Refresh the MAC and CCMP header of an already encoded A-MSDU.
    ///
    /// The caller must retain the body produced by [`Self::encode`] for the
    /// same `ethernet_frames`, source and BSSID. This bounded operation is
    /// intended for a statically owned TX slot whose plaintext body was not
    /// changed by on-the-fly hardware CCMP. It owns no DMA pointer and cannot
    /// change the encoded length.
    ///
    /// SOURCE: `libpp.a[pp.o]::ppResortTxAMPDU` retains the complete
    /// CCMP-ready MPDU across a missing BlockAck bit and changes only retry
    /// metadata.
    ///
    /// SOURCE\[HIL_OPEN_HT40_AMSDU_BODY_REUSE_2026_07_29]: the qualified
    /// production PSRAM/PSRAM image reused the body for more than 8,300
    /// accepted WPA2 HT40 MCS7 SGI aggregates; preparation fell from 768 us
    /// to 167 us and five-second samples sustained 102.8..109.7 Mbit/s.
    pub fn refresh_header(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        let (plan, required) = self.plan()?;
        self.write_header(plan, required, output)
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        let (plan, required) = self.plan()?;
        self.write_header(plan, required, output)?;

        let frame = &mut output[..required];
        let mut offset = crate::data::IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN;
        for (index, ethernet) in self.ethernet_frames.iter().copied().enumerate() {
            let payload = &ethernet[ETHERNET_HEADER_LEN..];
            let msdu_length = LLC_SNAP_HEADER_LEN + payload.len();
            frame[offset..offset + 12].copy_from_slice(&ethernet[..12]);
            frame[offset + 12..offset + 14].copy_from_slice(&(msdu_length as u16).to_be_bytes());
            offset += AMSDU_SUBFRAME_HEADER_LEN;
            frame[offset..offset + 6].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
            frame[offset + 6..offset + 8].copy_from_slice(&ethernet[12..14]);
            offset += LLC_SNAP_HEADER_LEN;
            frame[offset..offset + payload.len()].copy_from_slice(payload);
            offset += payload.len();
            if index + 1 != self.ethernet_frames.len() {
                let subframe_length =
                    AMSDU_SUBFRAME_HEADER_LEN + LLC_SNAP_HEADER_LEN + payload.len();
                let padding = (4 - (subframe_length & 3)) & 3;
                // Only A-MSDU alignment padding is not overwritten by the
                // header/payload copies above. Clear exactly these bytes so a
                // reused SRAM slot cannot expose an older MPDU, without
                // performing a redundant full-frame memset before every
                // aggregate build.
                frame[offset..offset + padding].fill(0);
                offset += padding;
            }
        }
        debug_assert_eq!(offset, required);
        Ok(required)
    }
}

const fn ethernet_header(
    destination: [u8; 6],
    source: [u8; 6],
    ether_type: u16,
) -> [u8; ETHERNET_HEADER_LEN] {
    [
        destination[0],
        destination[1],
        destination[2],
        destination[3],
        destination[4],
        destination[5],
        source[0],
        source[1],
        source[2],
        source[3],
        source[4],
        source[5],
        (ether_type >> 8) as u8,
        ether_type as u8,
    ]
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

pub fn select_wpa2_psk_rsn(access_point: &ScanRecord) -> Result<SelectedRsn, StaSecurityError> {
    if !access_point.matches_security(WifiSecurityMode::Wpa2Personal) {
        return Err(StaSecurityError::SecurityModeMismatch);
    }
    let rsn = access_point.rsn_ie_bytes();
    if rsn.len() < 2 || rsn[0] != 48 || usize::from(rsn[1]) + 2 != rsn.len() {
        return Err(if rsn.is_empty() {
            StaSecurityError::MissingRsn
        } else {
            StaSecurityError::MalformedRsn
        });
    }

    let body = &rsn[2..];
    let mut offset = 0;
    if read_rsn_u16(body, &mut offset)? != 1 {
        return Err(StaSecurityError::UnsupportedVersion);
    }
    if !is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_CIPHER_CCMP) {
        return Err(StaSecurityError::UnsupportedGroupCipher);
    }

    let pairwise_count = usize::from(read_rsn_u16(body, &mut offset)?);
    let mut has_ccmp = false;
    for _ in 0..pairwise_count {
        has_ccmp |= is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_CIPHER_CCMP);
    }
    if !has_ccmp {
        return Err(StaSecurityError::UnsupportedPairwiseCipher);
    }

    let akm_count = usize::from(read_rsn_u16(body, &mut offset)?);
    let mut has_psk = false;
    for _ in 0..akm_count {
        has_psk |= is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_AKM_PSK);
    }
    if !has_psk {
        return Err(StaSecurityError::UnsupportedAkm);
    }
    let capabilities = if offset < body.len() {
        read_rsn_u16(body, &mut offset)?
    } else {
        0
    };
    if offset < body.len() {
        let pmkid_count = usize::from(read_rsn_u16(body, &mut offset)?);
        let pmkid_bytes = pmkid_count
            .checked_mul(16)
            .ok_or(StaSecurityError::MalformedRsn)?;
        skip_rsn_bytes(body, &mut offset, pmkid_bytes)?;
    }
    if offset < body.len() {
        // The optional Group Management Cipher Suite is retained only as a
        // syntactic boundary. This WPA2 profile does not negotiate PMF, and
        // MFPR is rejected below.
        if capabilities & RSN_CAPABILITY_MFPC == 0 {
            return Err(StaSecurityError::MalformedRsn);
        }
        let _group_management_cipher = read_rsn_suite(body, &mut offset)?;
    }
    if offset != body.len() {
        return Err(StaSecurityError::MalformedRsn);
    }
    if capabilities & RSN_CAPABILITY_MFPR != 0 {
        return Err(StaSecurityError::ManagementFrameProtectionRequired);
    }

    let mut selected = SelectedRsn::EMPTY;
    selected.length = SELECTED_RSN_IE_LEN as u8;
    // The open STA owns protected A-MSDU construction and receive
    // decapsulation, so it retains the vendor SPP A-MSDU-capable contract.
    // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: frame 7624 carries RSN
    // Capabilities 0x0400 in the successful HE association request.
    selected.bytes.copy_from_slice(&[
        48,
        20,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_AKM_PSK,
        RSN_CAPABILITY_SPP_AMSDU_CAPABLE as u8,
        (RSN_CAPABILITY_SPP_AMSDU_CAPABLE >> 8) as u8,
    ]);
    Ok(selected)
}

/// Select the association security IE for one exact requested mode.
///
/// Open never accepts a Privacy/RSN/WPA advertisement. WPA2 never accepts an
/// open or mixed WPA/WPA2 advertisement, and then validates the complete
/// retained RSN suites before returning a source-owned RSN element.
pub fn select_association_rsn(
    access_point: &ScanRecord,
    security: WifiSecurityMode,
) -> Result<SelectedRsn, StaSecurityError> {
    match security {
        WifiSecurityMode::Open if access_point.matches_security(security) => Ok(SelectedRsn::EMPTY),
        WifiSecurityMode::Open => Err(StaSecurityError::SecurityModeMismatch),
        WifiSecurityMode::Wpa2Personal => select_wpa2_psk_rsn(access_point),
    }
}

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

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn read_rsn_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, StaSecurityError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(StaSecurityError::MalformedRsn)?;
    *offset += 2;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_rsn_suite(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], StaSecurityError> {
    let value = bytes
        .get(*offset..*offset + 4)
        .ok_or(StaSecurityError::MalformedRsn)?;
    *offset += 4;
    Ok([value[0], value[1], value[2], value[3]])
}

fn skip_rsn_bytes(bytes: &[u8], offset: &mut usize, length: usize) -> Result<(), StaSecurityError> {
    let end = offset
        .checked_add(length)
        .ok_or(StaSecurityError::MalformedRsn)?;
    bytes
        .get(*offset..end)
        .ok_or(StaSecurityError::MalformedRsn)?;
    *offset = end;
    Ok(())
}

fn is_rsn_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}

#[cfg(test)]
mod tests;
