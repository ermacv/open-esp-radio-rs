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
const RSN_CAPABILITY_SPP_AMSDU_CAPABLE: u16 = 1 << 10;
// One-stream HT20 with short guard interval. Channel-width, STBC, LDPC,
// large A-MSDU and 40-MHz claims remain disabled until their matching
// Rust-owned paths are enabled.
//
// SOURCE[PROMOTED_HE20_PEER]: reviewed promoted HT20 capability image,
// originally qualified by the strict ESP32-S31 STA WPA2/ADDBA throughput HIL.
const HT20_CAPABILITY_IE: [u8; 28] = station_ht_capability_ie(0x0020, 0x00);
// One-stream HT40 with short guard intervals for both 20 and 40 MHz and
// spatial multiplexing power save disabled. Although the S31 has one receive
// stream, advertising static SMPS (`0x0062`) made the controlled Linux HT40 AP
// accept authentication but discard the association request. The vendor
// builder preserves the interface's SMPS bits and ordinary ESP STA requests
// use the disabled value (`bits 2..3 = 0b11`), giving `0x006e`.
//
// SOURCE: `libnet80211.a[ieee80211_ht.o]::
// ieee80211_add_htcap_body` reads the base capability at node offset `0x14c`,
// adds Supported Channel Width at `+0x4e`, SGI20 at `+0x8e`, and SGI40 at
// `+0xaa` without replacing the base SMPS bits. IEEE 802.11 HT Capabilities
// Info defines bits 2..3 value `0b11` as SMPS disabled. Selection remains
// gated by the complete AP HT Capabilities/Operation IEs through
// `ScanRecord::ht40_secondary_channel`; hardware CBW support is the complete
// rev0 ROM `phy_bb_bss_cbw40` implementation promoted into the S31 PAC/HAL.
const HT40_CAPABILITY_IE: [u8; 28] = station_ht_capability_ie(0x006e, 0x00);
// Exact HT20 capability carried beside the HE capability in the complete
// vendor association request. It differs from the deliberately narrow
// standalone HT20 profile above: SMPS is disabled, RX STBC is one stream,
// the receive A-MPDU limit is 65,535 bytes and minimum spacing is 4 us.
//
// SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: qualified frame 7624.
// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
// ieee80211_add_htcap_body` produces the same capability fields.
const HE20_HT_CAPABILITY_IE: [u8; 28] = station_ht_capability_ie(0x112c, 0x17);

/// Build the complete vendor-shaped one-stream HT capability image.
///
/// The Supported MCS Set begins at complete-IE byte five. Its byte 12 is the
/// TX MCS parameters field, therefore it is complete-IE byte 17. Keeping this
/// layout in one builder prevents AP/STA capability images from drifting.
const fn station_ht_capability_ie(capability_info: u16, ampdu_parameters: u8) -> [u8; 28] {
    let mut element = [0_u8; 28];
    element[0] = 45;
    element[1] = 26;
    element[2] = capability_info as u8;
    element[3] = (capability_info >> 8) as u8;
    element[4] = ampdu_parameters;
    element[5] = 0xff;
    element[17] = 0x01;
    element
}
// Exact one-stream HE20 MCS0-9 capability captured from the vendor
// association request and retained as a comparison oracle.
// This must not be relabelled HE40: complete
// `libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` writes zero to
// complete IE byte nine (the first HE PHY Capabilities byte / Channel Width
// Set) on both its STA branches. The chip's vendor path advertises 40 MHz
// separately through HT Capabilities, as represented by HT40_CAPABILITY_IE.
//
// SOURCE[PROMOTED_HE20_ASSOCIATION]: reviewed promoted HE20 MCS9 capability
// image, originally compared with the request constructed
// by pinned `libnet80211.a`.
//
// FIELD AUDIT: complete
// `libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` proves that
// byte 11 bit 3 is the S31 `g_phy_cap_rx_stbc` advertisement. Byte 15 bits
// 2..4 advertise triggered SU beamforming feedback, triggered MU partial-
// bandwidth feedback and triggered CQI; byte 18 bit 1 advertises
// non-triggered CQI.
const HE20_VENDOR_MCS9_CAPABILITY_IE: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];

const fn owned_he20_mcs9_capability_ie() -> [u8; 24] {
    let mut capability = HE20_VENDOR_MCS9_CAPABILITY_IE;
    // HE MAC Capabilities bit 1 is TWT Requester Support. The open driver has
    // no TWT negotiation or wake transaction owner, so it must not inherit
    // that vendor claim.
    capability[3] &= !(1 << 1);
    // The hardware beamforming-report sequence and its rate profile are
    // Rust-owned, so keep the two beamforming-feedback bits. There is still
    // no open triggered or non-triggered CQI report producer: advertising
    // either capability could make an AP schedule a response the STA cannot
    // generate. Clear only those two independently recovered claims.
    //
    // SOURCE[HIL_OPEN_HE20_CQI_CAPABILITY_MASK_2026_07_30]: ESP32-S31 rev0
    // associated with FRITZ!Box 7530 FN on channel 1 after these exact two
    // bits were cleared, completed WPA2/CCMP and DHCP, then passed a complete
    // 30-profile LDPC MCS0..9 x GI/LTF A-MPDU matrix with zero failed
    // profiles. Evidence:
    // `/tmp/open-radio-he20-owned-cqi-mask-hil-20260730.log`.
    capability[15] &= !(1 << 4);
    capability[18] &= !(1 << 1);
    capability
}

const HE20_OWNED_MCS9_CAPABILITY_IE: [u8; 24] = owned_he20_mcs9_capability_ie();
const HE_UL_MU_POWER_CAPABILITY_IE_LEN: usize = 14;
const HE_UL_MU_POWER_CAPABILITY_EXTENSION_ID: u8 = 60;
const POWER_CAPABILITY_IE_LEN: usize = 4;
// Exact vendor Extended Capabilities IE adjacent to the HE/UL-MU capability
// pair. Retain Event and Multiple BSSID, but clear Extended Capabilities bit
// 77 (TWT requester) until negotiation and wake ownership exist.
//
// SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: exact frame 7624 bytes.
// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
// ieee80211_add_extcap` emits this 12-byte body for the captured STA state.
const HE20_EXTENDED_CAPABILITY_IE: [u8; 14] =
    [127, 12, 0x80, 0x00, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0];
// WMM Information, version one, U-APSD disabled.
//
// SOURCE: the same promoted `sta_link.rs::WMM_INFORMATION_IE`, cross-checked
// against `libnet80211.a` association-request construction.
const WMM_INFORMATION_IE: [u8; 9] = [221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00];

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

/// Complete scan-to-channel decision consumed by the PHY channel transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationSelection {
    pub phy: StaAssociationPhy,
    pub primary_channel: u8,
    /// Primary channel number for 20 MHz, center frequency in MHz for HT40.
    pub channel_or_frequency: u16,
    /// Recovered ESP32-S31 CBW encoding: 0=20 MHz, 2=above, 3=below.
    pub cbw: u8,
}

/// Select the strongest open-driver PHY supported by one scanned peer.
///
/// Automatic mode prefers the 150-Mbit/s one-stream HT40 path when the peer's
/// HT Capabilities and HT Operation agree on a usable secondary channel. HE
/// remains 20-MHz-only because the complete ESP32-S31 vendor HE capability
/// builder advertises a zero HE Channel Width Set. Otherwise the selection is
/// HE20 MCS9 or the conservative HT20 fallback.
///
/// SOURCE: complete `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_hecap` and the complete HT Capabilities/Operation IEs retained
/// by [`ScanRecord::ht40_secondary_channel`]. The above/below CBW values are
/// independently recovered from rev0 ROM `phy_bb_bss_cbw40`.
pub fn select_sta_association(
    access_point: &ScanRecord,
    preference: StaAssociationPreference,
) -> StaAssociationSelection {
    let he20_supported = parse_he20_capabilities(access_point.he_capability_ie_bytes())
        .is_ok_and(|capability| capability.supports_bidirectional_mcs9())
        && parse_he20_operation(access_point.he_operation_ie_bytes()).is_ok();
    let phy = if preference == StaAssociationPreference::PreferHe20 && he20_supported {
        StaAssociationPhy::He20
    } else if preference == StaAssociationPreference::ForceHt20 {
        StaAssociationPhy::Ht20
    } else if access_point.ht40_secondary_channel().is_some() {
        StaAssociationPhy::Ht40
    } else if he20_supported {
        StaAssociationPhy::He20
    } else {
        StaAssociationPhy::Ht20
    };

    let primary_frequency = 2_407 + u16::from(access_point.channel) * 5;
    let (channel_or_frequency, cbw) = if phy == StaAssociationPhy::Ht40 {
        match access_point.ht40_secondary_channel() {
            Some(crate::scan::HtSecondaryChannel::Above) => (primary_frequency + 10, 2),
            Some(crate::scan::HtSecondaryChannel::Below) => (primary_frequency - 10, 3),
            None => (u16::from(access_point.channel), 0),
        }
    } else {
        (u16::from(access_point.channel), 0)
    };
    StaAssociationSelection {
        phy,
        primary_channel: access_point.channel,
        channel_or_frequency,
        cbw,
    }
}

/// Vendor state timer used by ordinary Authentication and Association.
///
/// SOURCE: complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state`, ordinary non-mesh auth branch `.L347` and
/// association branch `.L353`, both arm their software timer with immediate
/// `0x3e8`.
pub const STA_RESPONSE_TIMEOUT_MS: u32 = 1_000;

/// Bounded open Authentication attempts retained by the qualified STA path.
pub const STA_AUTHENTICATION_ATTEMPT_LIMIT: u16 = 3;

/// One uniquely numbered Open Authentication transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAuthenticationAttempt {
    pub ordinal: u16,
    pub sequence_number: u16,
    pub response_timeout_ms: u32,
}

/// Protocol-level reason why one Authentication attempt ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationFailure {
    Timeout,
    PeerDisconnect(StaDisconnect),
    Rejected { status_code: u16 },
}

/// Result of observing a management frame or expiring an attempt deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationEvent {
    Irrelevant,
    Authenticated {
        attempt: u16,
        total_received_frames: u32,
    },
    Retry {
        attempt: u16,
        failure: StaAuthenticationFailure,
        received_frames: u32,
        total_received_frames: u32,
    },
    Failed {
        attempts: u16,
        failure: StaAuthenticationFailure,
        total_received_frames: u32,
    },
}

/// Invalid executor interaction with [`StaAuthenticationRuntime`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationRuntimeError {
    AttemptAlreadyActive,
    NoActiveAttempt,
    Terminal,
}

/// Allocation-free owner of the ordinary Open Authentication retry epoch.
///
/// This type owns protocol policy and state only. A target executor arms RX,
/// submits the returned sequence number, reports every received descriptor,
/// and supplies extracted management frames. It therefore remains independent
/// of Embassy, DMA layout and the ESP32-S31 MAC.
///
/// SOURCE: complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` ordinary Authentication branch arms the 1,000-ms
/// state timer. The three-attempt bound is the hardware-qualified open STA
/// policy previously owned by the ESP32-S31 HIL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAuthenticationRuntime {
    local: [u8; 6],
    bssid: [u8; 6],
    attempts_started: u16,
    active: bool,
    terminal: bool,
    received_frames: u32,
    total_received_frames: u32,
}

impl StaAuthenticationRuntime {
    pub const fn new(local: [u8; 6], bssid: [u8; 6]) -> Self {
        Self {
            local,
            bssid,
            attempts_started: 0,
            active: false,
            terminal: false,
            received_frames: 0,
            total_received_frames: 0,
        }
    }

    /// Start the next bounded attempt and consume exactly one management
    /// sequence number. Hardware retransmission of the encoded request does
    /// not call this method again.
    pub fn begin_attempt(
        &mut self,
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAuthenticationAttempt, StaAuthenticationRuntimeError> {
        if self.terminal || self.attempts_started >= STA_AUTHENTICATION_ATTEMPT_LIMIT {
            return Err(StaAuthenticationRuntimeError::Terminal);
        }
        if self.active {
            return Err(StaAuthenticationRuntimeError::AttemptAlreadyActive);
        }
        self.attempts_started += 1;
        self.active = true;
        self.received_frames = 0;
        Ok(StaAuthenticationAttempt {
            ordinal: self.attempts_started,
            sequence_number: sequence.take(),
            response_timeout_ms: STA_RESPONSE_TIMEOUT_MS,
        })
    }

    /// Account for one completed RX descriptor, including a frame which is
    /// not a valid management input. This preserves the diagnostic count while
    /// keeping frame parsing separately typed.
    pub fn observe_received_frame(&mut self) -> Result<(), StaAuthenticationRuntimeError> {
        if !self.active {
            return Err(StaAuthenticationRuntimeError::NoActiveAttempt);
        }
        self.received_frames = self.received_frames.saturating_add(1);
        Ok(())
    }

    /// Classify one extracted management frame for the active peer exchange.
    pub fn observe_management_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<StaAuthenticationEvent, StaAuthenticationRuntimeError> {
        if !self.active {
            return Err(StaAuthenticationRuntimeError::NoActiveAttempt);
        }
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return Ok(self.finish_retryable(StaAuthenticationFailure::PeerDisconnect(disconnect)));
        }
        let Some(response) = parse_open_authentication_response(frame, self.local, self.bssid)
        else {
            return Ok(StaAuthenticationEvent::Irrelevant);
        };
        self.finish_attempt();
        self.terminal = true;
        if response.status_code == 0 {
            Ok(StaAuthenticationEvent::Authenticated {
                attempt: self.attempts_started,
                total_received_frames: self.total_received_frames,
            })
        } else {
            Ok(StaAuthenticationEvent::Failed {
                attempts: self.attempts_started,
                failure: StaAuthenticationFailure::Rejected {
                    status_code: response.status_code,
                },
                total_received_frames: self.total_received_frames,
            })
        }
    }

    /// Expire the complete vendor state deadline for the active attempt.
    pub fn response_timed_out(
        &mut self,
    ) -> Result<StaAuthenticationEvent, StaAuthenticationRuntimeError> {
        if !self.active {
            return Err(StaAuthenticationRuntimeError::NoActiveAttempt);
        }
        Ok(self.finish_retryable(StaAuthenticationFailure::Timeout))
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.total_received_frames
            .saturating_add(self.received_frames)
    }

    pub const fn active_received_frames(&self) -> u32 {
        self.received_frames
    }

    fn finish_retryable(&mut self, failure: StaAuthenticationFailure) -> StaAuthenticationEvent {
        let received_frames = self.received_frames;
        self.finish_attempt();
        if self.attempts_started < STA_AUTHENTICATION_ATTEMPT_LIMIT {
            StaAuthenticationEvent::Retry {
                attempt: self.attempts_started,
                failure,
                received_frames,
                total_received_frames: self.total_received_frames,
            }
        } else {
            self.terminal = true;
            StaAuthenticationEvent::Failed {
                attempts: self.attempts_started,
                failure,
                total_received_frames: self.total_received_frames,
            }
        }
    }

    fn finish_attempt(&mut self) {
        self.total_received_frames = self
            .total_received_frames
            .saturating_add(self.received_frames);
        self.received_frames = 0;
        self.active = false;
    }
}

/// Compatibility schedule for Association retransmission inside the vendor
/// one-second state deadline.
///
/// This 160-ms cadence comes from the hardware-qualified pre-transfer open
/// STA runtime, not from a recovered vendor timer body. It remains explicit so
/// later blob comparison can replace one policy value without changing the
/// executor loop.
pub struct StaAssociationRetrySchedule;

impl StaAssociationRetrySchedule {
    pub const INTERVAL_MS: u32 = 160;

    pub const fn attempt_at(elapsed_ms: u32) -> Option<u16> {
        if elapsed_ms < STA_RESPONSE_TIMEOUT_MS && elapsed_ms.is_multiple_of(Self::INTERVAL_MS) {
            Some((elapsed_ms / Self::INTERVAL_MS + 1) as u16)
        } else {
            None
        }
    }
}

/// One uniquely numbered Association transmission inside a state epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationAttempt {
    pub ordinal: u16,
    pub sequence_number: u16,
    pub elapsed_ms: u32,
}

/// Protocol-level reason why an Association epoch ended unsuccessfully.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationFailure {
    Timeout,
    PeerDisconnect(StaDisconnect),
    Rejected { status_code: u16 },
}

/// Result of observing a management frame or completing one millisecond tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationEvent {
    Irrelevant,
    Associated {
        response: AssociationResponse,
        total_received_frames: u32,
    },
    Failed {
        failure: StaAssociationFailure,
        total_received_frames: u32,
    },
}

/// Invalid executor interaction with [`StaAssociationRuntime`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationRuntimeError {
    TickAlreadyActive,
    NoActiveTick,
    Terminal,
}

/// Allocation-free owner of one ordinary STA Association epoch.
///
/// A target executor begins one tick, optionally transmits the returned
/// attempt, reports every completed RX descriptor, supplies extracted
/// management frames, then finishes the tick. This type owns the one-second
/// deadline, retransmission cadence, management sequence consumption and
/// terminal response policy; it does not own timers, DMA or MAC registers.
///
/// SOURCE: complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` Association branch arms the 1,000-ms state timer.
/// The 160-ms retransmission cadence is the hardware-qualified open STA policy
/// previously owned by the ESP32-S31 HIL and remains isolated in
/// [`StaAssociationRetrySchedule`] pending recovery of the vendor timer body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationRuntime {
    local: [u8; 6],
    bssid: [u8; 6],
    elapsed_ms: u32,
    tick_active: bool,
    terminal: bool,
    received_frames: u32,
}

impl StaAssociationRuntime {
    pub const fn new(local: [u8; 6], bssid: [u8; 6]) -> Self {
        Self {
            local,
            bssid,
            elapsed_ms: 0,
            tick_active: false,
            terminal: false,
            received_frames: 0,
        }
    }

    /// Begin the current millisecond tick and consume a management sequence
    /// number exactly when the retry schedule calls for a new MPDU.
    pub fn begin_tick(
        &mut self,
        sequence: &mut StaSequenceCounter,
    ) -> Result<Option<StaAssociationAttempt>, StaAssociationRuntimeError> {
        if self.terminal || self.elapsed_ms >= STA_RESPONSE_TIMEOUT_MS {
            return Err(StaAssociationRuntimeError::Terminal);
        }
        if self.tick_active {
            return Err(StaAssociationRuntimeError::TickAlreadyActive);
        }
        self.tick_active = true;
        Ok(
            StaAssociationRetrySchedule::attempt_at(self.elapsed_ms).map(|ordinal| {
                StaAssociationAttempt {
                    ordinal,
                    sequence_number: sequence.take(),
                    elapsed_ms: self.elapsed_ms,
                }
            }),
        )
    }

    /// Account for one completed RX descriptor, including a frame which is
    /// not a valid management input.
    pub fn observe_received_frame(&mut self) -> Result<(), StaAssociationRuntimeError> {
        self.require_active_tick()?;
        self.received_frames = self.received_frames.saturating_add(1);
        Ok(())
    }

    /// Classify one extracted management frame for the selected peer.
    pub fn observe_management_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<StaAssociationEvent, StaAssociationRuntimeError> {
        self.require_active_tick()?;
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return Ok(self.fail(StaAssociationFailure::PeerDisconnect(disconnect)));
        }
        let Some(response) = parse_association_response(frame, self.local, self.bssid) else {
            return Ok(StaAssociationEvent::Irrelevant);
        };
        if response.status_code != 0 {
            return Ok(self.fail(StaAssociationFailure::Rejected {
                status_code: response.status_code,
            }));
        }
        self.tick_active = false;
        self.terminal = true;
        Ok(StaAssociationEvent::Associated {
            response,
            total_received_frames: self.received_frames,
        })
    }

    /// Complete the current millisecond tick and expire the complete vendor
    /// state deadline after exactly 1,000 ticks.
    pub fn finish_tick(&mut self) -> Result<StaAssociationEvent, StaAssociationRuntimeError> {
        self.require_active_tick()?;
        self.tick_active = false;
        self.elapsed_ms = self.elapsed_ms.saturating_add(1);
        if self.elapsed_ms >= STA_RESPONSE_TIMEOUT_MS {
            Ok(self.fail(StaAssociationFailure::Timeout))
        } else {
            Ok(StaAssociationEvent::Irrelevant)
        }
    }

    pub const fn elapsed_ms(&self) -> u32 {
        self.elapsed_ms
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.received_frames
    }

    fn require_active_tick(&self) -> Result<(), StaAssociationRuntimeError> {
        if self.terminal {
            Err(StaAssociationRuntimeError::Terminal)
        } else if !self.tick_active {
            Err(StaAssociationRuntimeError::NoActiveTick)
        } else {
            Ok(())
        }
    }

    fn fail(&mut self, failure: StaAssociationFailure) -> StaAssociationEvent {
        self.tick_active = false;
        self.terminal = true;
        StaAssociationEvent::Failed {
            failure,
            total_received_frames: self.received_frames,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationRequest<'a> {
    pub source: [u8; 6],
    pub access_point: &'a ScanRecord,
    pub sequence_number: u16,
    pub listen_interval: u16,
    pub phy: StaAssociationPhy,
    /// HE Power Capability derived from the same calibrated rate-16 power
    /// source used by the MAC. Non-HE modes must leave it absent.
    pub power_capability: Option<StaPowerCapability>,
    /// Runtime-calibrated UL-MU power capability required by the complete HE
    /// association contract. Non-HE modes must leave it absent.
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
}

impl AssociationRequest<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, AssociationRequestError> {
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
        let selected_rsn =
            select_wpa2_psk_rsn(self.access_point).map_err(AssociationRequestError::Security)?;
        let (ht_capability, he_capability, power_capability, he_ul_mu_power) = match self.phy {
            StaAssociationPhy::Legacy => (None, None, None, None),
            StaAssociationPhy::Ht20 if self.access_point.ht_capability_ie_present => {
                (Some(&HT20_CAPABILITY_IE), None, None, None)
            }
            StaAssociationPhy::Ht20 => {
                return Err(AssociationRequestError::HtUnsupportedByAccessPoint);
            }
            StaAssociationPhy::Ht40 if self.access_point.ht40_secondary_channel().is_some() => {
                (Some(&HT40_CAPABILITY_IE), None, None, None)
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
                    Some(&HE20_HT_CAPABILITY_IE),
                    Some(&HE20_OWNED_MCS9_CAPABILITY_IE),
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
                + WMM_INFORMATION_IE.len()
                + usize::from(he_capability.is_some()) * HE20_EXTENDED_CAPABILITY_IE.len()
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
        let capability = (self.access_point.capability_info & ASSOCIATION_CAPABILITY_MASK) | 1;
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
            frame[offset..offset + WMM_INFORMATION_IE.len()].copy_from_slice(&WMM_INFORMATION_IE);
            offset += WMM_INFORMATION_IE.len();
            if he_capability.is_some() {
                frame[offset..offset + HE20_EXTENDED_CAPABILITY_IE.len()]
                    .copy_from_slice(&HE20_EXTENDED_CAPABILITY_IE);
                offset += HE20_EXTENDED_CAPABILITY_IE.len();
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
/// Max A-MSDU Length selected by the clear bit 11 in `HT40_CAPABILITY_IE`.
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
    if !access_point.privacy && access_point.rsn_ie_len == 0 {
        return Ok(SelectedRsn::EMPTY);
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
    if offset < body.len() {
        let capabilities = read_rsn_u16(body, &mut offset)?;
        if capabilities & RSN_CAPABILITY_MFPR != 0 {
            return Err(StaSecurityError::ManagementFrameProtectionRequired);
        }
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

fn is_rsn_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
    const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

    #[test]
    fn station_ht_profiles_put_tx_parameters_in_supported_mcs_byte_twelve() {
        for capability in [
            HT20_CAPABILITY_IE,
            HT40_CAPABILITY_IE,
            HE20_HT_CAPABILITY_IE,
        ] {
            assert_eq!(capability[5], 0xff);
            assert_eq!(capability[17], 0x01);
            assert_eq!(capability[18], 0);
        }
    }

    fn authentication_response(status_code: u16) -> [u8; 30] {
        let mut frame = [0_u8; 30];
        frame[0..2].copy_from_slice(&OPEN_AUTHENTICATION_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[26..28].copy_from_slice(&OPEN_SYSTEM_RESPONSE_SEQUENCE.to_le_bytes());
        frame[28..30].copy_from_slice(&status_code.to_le_bytes());
        frame
    }

    fn association_response(status_code: u16) -> [u8; 30] {
        let mut frame = [0_u8; 30];
        frame[0..2].copy_from_slice(&ASSOCIATION_RESPONSE_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame[26..28].copy_from_slice(&status_code.to_le_bytes());
        frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
        frame
    }

    fn deauthentication(reason_code: u16) -> [u8; MANAGEMENT_HEADER_LEN + 2] {
        let mut frame = [0_u8; MANAGEMENT_HEADER_LEN + 2];
        frame[0..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&reason_code.to_le_bytes());
        frame
    }

    fn access_point_with_rsn(akms: &[[u8; 4]], capabilities: u16) -> ScanRecord {
        let mut record = ScanRecord::EMPTY;
        record.ssid[..4].copy_from_slice(b"test");
        record.ssid_len = 4;
        record.bssid = BSSID;
        record.channel = 6;
        record.privacy = true;
        record.supported_rates[..4].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
        record.supported_rates_len = 4;
        let mut offset = 2;
        record.rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
        offset += 2;
        record.rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
        offset += 4;
        record.rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
        offset += 2;
        record.rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
        offset += 4;
        record.rsn_ie[offset..offset + 2].copy_from_slice(&(akms.len() as u16).to_le_bytes());
        offset += 2;
        for akm in akms {
            record.rsn_ie[offset..offset + 4].copy_from_slice(akm);
            offset += 4;
        }
        record.rsn_ie[offset..offset + 2].copy_from_slice(&capabilities.to_le_bytes());
        offset += 2;
        record.rsn_ie[0] = 48;
        record.rsn_ie[1] = (offset - 2) as u8;
        record.rsn_ie_len = offset as u8;
        record
    }

    #[test]
    fn encodes_open_authentication_request() {
        let mut output = [0xa5; 32];
        let length = OpenAuthenticationRequest {
            source: LOCAL,
            bssid: BSSID,
            sequence_number: 0x123,
        }
        .encode(&mut output)
        .unwrap();
        assert_eq!(length, 30);
        assert_eq!(&output[0..2], &[0xb0, 0]);
        assert_eq!(&output[4..10], &BSSID);
        assert_eq!(&output[10..16], &LOCAL);
        assert_eq!(&output[16..22], &BSSID);
        assert_eq!(&output[22..24], &[0x30, 0x12]);
        assert_eq!(&output[24..30], &[0, 0, 1, 0, 0, 0]);
        assert_eq!(output[30], 0xa5);
    }

    #[test]
    fn encodes_sta_action_frame_around_owned_body() {
        let body = [3, 1, 7, 0, 0, 0x02, 0x04, 0, 0];
        let mut output = [0xa5; 40];
        let length = StaActionFrame {
            source: LOCAL,
            bssid: BSSID,
            sequence_number: 0x123,
            body: &body,
        }
        .encode(&mut output)
        .unwrap();
        assert_eq!(length, 33);
        assert_eq!(&output[0..2], &[0xd0, 0]);
        assert_eq!(&output[4..10], &BSSID);
        assert_eq!(&output[10..16], &LOCAL);
        assert_eq!(&output[16..22], &BSSID);
        assert_eq!(&output[22..24], &[0x30, 0x12]);
        assert_eq!(&output[24..33], &body);
        assert_eq!(output[33], 0xa5);
    }

    #[test]
    fn parses_only_matching_open_authentication_response() {
        let frame = authentication_response(17);
        assert_eq!(
            parse_open_authentication_response(&frame, LOCAL, BSSID),
            Some(OpenAuthenticationResponse { status_code: 17 })
        );
        assert_eq!(
            parse_open_authentication_response(&frame, [0; 6], BSSID),
            None
        );
    }

    #[test]
    fn authentication_runtime_owns_attempt_sequence_deadline_and_timeout_limit() {
        let mut runtime = StaAuthenticationRuntime::new(LOCAL, BSSID);
        let mut sequence = StaSequenceCounter::new(0x0ffe);

        for ordinal in 1..=STA_AUTHENTICATION_ATTEMPT_LIMIT {
            let attempt = runtime.begin_attempt(&mut sequence).unwrap();
            assert_eq!(attempt.ordinal, ordinal);
            assert_eq!(attempt.sequence_number, (0x0ffd + ordinal) & 0x0fff);
            assert_eq!(attempt.response_timeout_ms, STA_RESPONSE_TIMEOUT_MS);
            runtime.observe_received_frame().unwrap();
            let event = runtime.response_timed_out().unwrap();
            if ordinal < STA_AUTHENTICATION_ATTEMPT_LIMIT {
                assert_eq!(
                    event,
                    StaAuthenticationEvent::Retry {
                        attempt: ordinal,
                        failure: StaAuthenticationFailure::Timeout,
                        received_frames: 1,
                        total_received_frames: u32::from(ordinal),
                    }
                );
            } else {
                assert_eq!(
                    event,
                    StaAuthenticationEvent::Failed {
                        attempts: ordinal,
                        failure: StaAuthenticationFailure::Timeout,
                        total_received_frames: u32::from(ordinal),
                    }
                );
            }
        }
        assert_eq!(
            runtime.begin_attempt(&mut sequence),
            Err(StaAuthenticationRuntimeError::Terminal)
        );
    }

    #[test]
    fn authentication_runtime_ignores_other_management_and_accepts_selected_peer() {
        let mut runtime = StaAuthenticationRuntime::new(LOCAL, BSSID);
        let mut sequence = StaSequenceCounter::new(7);
        let attempt = runtime.begin_attempt(&mut sequence).unwrap();
        runtime.observe_received_frame().unwrap();
        assert_eq!(
            runtime.observe_management_frame(&[0; 30]).unwrap(),
            StaAuthenticationEvent::Irrelevant
        );
        runtime.observe_received_frame().unwrap();
        assert_eq!(
            runtime
                .observe_management_frame(&authentication_response(0))
                .unwrap(),
            StaAuthenticationEvent::Authenticated {
                attempt: attempt.ordinal,
                total_received_frames: 2,
            }
        );
        assert_eq!(runtime.total_received_frames(), 2);
    }

    #[test]
    fn authentication_runtime_retries_disconnect_but_not_status_rejection() {
        let mut runtime = StaAuthenticationRuntime::new(LOCAL, BSSID);
        let mut sequence = StaSequenceCounter::new(0);
        runtime.begin_attempt(&mut sequence).unwrap();
        runtime.observe_received_frame().unwrap();
        assert_eq!(
            runtime
                .observe_management_frame(&deauthentication(1))
                .unwrap(),
            StaAuthenticationEvent::Retry {
                attempt: 1,
                failure: StaAuthenticationFailure::PeerDisconnect(StaDisconnect {
                    kind: StaDisconnectKind::Deauthentication,
                    reason_code: 1,
                }),
                received_frames: 1,
                total_received_frames: 1,
            }
        );

        runtime.begin_attempt(&mut sequence).unwrap();
        runtime.observe_received_frame().unwrap();
        assert_eq!(
            runtime
                .observe_management_frame(&authentication_response(17))
                .unwrap(),
            StaAuthenticationEvent::Failed {
                attempts: 2,
                failure: StaAuthenticationFailure::Rejected { status_code: 17 },
                total_received_frames: 2,
            }
        );
    }

    #[test]
    fn parses_only_disconnects_from_selected_access_point() {
        let mut frame = [0_u8; MANAGEMENT_HEADER_LEN + 2];
        frame[0..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            parse_sta_disconnect(&frame, LOCAL, BSSID),
            Some(StaDisconnect {
                kind: StaDisconnectKind::Deauthentication,
                reason_code: 1,
            })
        );

        frame[0..2].copy_from_slice(&DISASSOCIATION_FRAME_CONTROL.to_le_bytes());
        frame[24..26].copy_from_slice(&8_u16.to_le_bytes());
        assert_eq!(
            parse_sta_disconnect(&frame, LOCAL, BSSID),
            Some(StaDisconnect {
                kind: StaDisconnectKind::Disassociation,
                reason_code: 8,
            })
        );

        assert_eq!(parse_sta_disconnect(&frame, [0; 6], BSSID), None);
        assert_eq!(parse_sta_disconnect(&frame, LOCAL, [0; 6]), None);
        assert_eq!(
            parse_sta_disconnect(&frame[..MANAGEMENT_HEADER_LEN + 1], LOCAL, BSSID),
            None
        );

        frame[0..2].copy_from_slice(&OPEN_AUTHENTICATION_FRAME_CONTROL.to_le_bytes());
        assert_eq!(parse_sta_disconnect(&frame, LOCAL, BSSID), None);
    }

    #[test]
    fn mixed_wpa2_wpa3_ap_is_narrowed_to_wpa2_psk_ccmp() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8], [0, 0x0f, 0xac, 2]], 0x80);
        let selected = select_wpa2_psk_rsn(&record).unwrap();
        assert_eq!(selected.as_bytes().len(), SELECTED_RSN_IE_LEN);
        assert_eq!(&selected.as_bytes()[8..14], &[1, 0, 0, 0x0f, 0xac, 4]);
        assert_eq!(&selected.as_bytes()[14..20], &[1, 0, 0, 0x0f, 0xac, 2]);
        assert_eq!(&selected.as_bytes()[20..22], &[0, 4]);
    }

    #[test]
    fn required_management_frame_protection_is_rejected() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], RSN_CAPABILITY_MFPR);
        assert_eq!(
            select_wpa2_psk_rsn(&record),
            Err(StaSecurityError::ManagementFrameProtectionRequired)
        );
    }

    #[test]
    fn association_request_contains_selected_rsn() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        let mut output = [0; 96];
        let length = AssociationRequest {
            source: LOCAL,
            access_point: &record,
            sequence_number: 2,
            listen_interval: 1,
            phy: StaAssociationPhy::Legacy,
            power_capability: None,
            he_ul_mu_power: None,
        }
        .encode(&mut output)
        .unwrap();
        assert_eq!(&output[0..2], &[0, 0]);
        assert_eq!(&output[4..10], &BSSID);
        assert_eq!(&output[28..34], &[0, 4, b't', b'e', b's', b't']);
        assert_eq!(
            &output[length - SELECTED_RSN_IE_LEN..length],
            &[
                48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0,
                4,
            ]
        );
    }

    #[test]
    fn ht20_association_request_reproduces_the_migration_capabilities() {
        let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        record.ht_capability_ie_present = true;
        let mut output = [0; 160];
        let length = AssociationRequest {
            source: LOCAL,
            access_point: &record,
            sequence_number: 2,
            listen_interval: 1,
            phy: StaAssociationPhy::Ht20,
            power_capability: None,
            he_ul_mu_power: None,
        }
        .encode(&mut output)
        .unwrap();
        let phy_start = length - HT20_CAPABILITY_IE.len() - WMM_INFORMATION_IE.len();
        assert_eq!(
            &output[phy_start..phy_start + HT20_CAPABILITY_IE.len()],
            &HT20_CAPABILITY_IE
        );
        assert_eq!(
            &output[phy_start + HT20_CAPABILITY_IE.len()..length],
            &WMM_INFORMATION_IE
        );
    }

    #[test]
    fn ht20_request_fails_closed_when_the_ap_did_not_advertise_ht() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        assert_eq!(
            AssociationRequest {
                source: LOCAL,
                access_point: &record,
                sequence_number: 2,
                listen_interval: 1,
                phy: StaAssociationPhy::Ht20,
                power_capability: None,
                he_ul_mu_power: None,
            }
            .encode(&mut [0; 160]),
            Err(AssociationRequestError::HtUnsupportedByAccessPoint)
        );
    }

    #[test]
    fn ht40_request_claims_width_short_gi_and_disabled_smps() {
        let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        record.channel = 6;
        record.ht_capability_ie_present = true;
        record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
        record.ht_operation_ie_present = true;
        record.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
        let mut output = [0; 160];
        let length = AssociationRequest {
            source: LOCAL,
            access_point: &record,
            sequence_number: 2,
            listen_interval: 1,
            phy: StaAssociationPhy::Ht40,
            power_capability: None,
            he_ul_mu_power: None,
        }
        .encode(&mut output)
        .unwrap();
        let phy_start = length - HT40_CAPABILITY_IE.len() - WMM_INFORMATION_IE.len();
        assert_eq!(&output[phy_start..phy_start + 4], &[45, 26, 0x6e, 0]);
    }

    #[test]
    fn association_selection_owns_phy_and_center_channel_policy() {
        let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        record.channel = 6;
        record.ht_capability_ie_present = true;
        record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
        record.ht_operation_ie_present = true;
        record.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
        record.he_capability_ie[..HE20_VENDOR_MCS9_CAPABILITY_IE.len()]
            .copy_from_slice(&HE20_VENDOR_MCS9_CAPABILITY_IE);
        record.he_capability_ie_len = HE20_VENDOR_MCS9_CAPABILITY_IE.len() as u8;
        let he_operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
        record.he_operation_ie[..he_operation.len()].copy_from_slice(&he_operation);
        record.he_operation_ie_len = he_operation.len() as u8;

        let automatic = select_sta_association(&record, StaAssociationPreference::Automatic);
        assert_eq!(automatic.phy, StaAssociationPhy::Ht40);
        assert_eq!(automatic.primary_channel, 6);
        assert_eq!(automatic.channel_or_frequency, 2_447);
        assert_eq!(automatic.cbw, 2);

        let he20 = select_sta_association(&record, StaAssociationPreference::PreferHe20);
        assert_eq!(he20.phy, StaAssociationPhy::He20);
        assert_eq!(he20.channel_or_frequency, 6);
        assert_eq!(he20.cbw, 0);

        let ht20 = select_sta_association(&record, StaAssociationPreference::ForceHt20);
        assert_eq!(ht20.phy, StaAssociationPhy::Ht20);
        assert_eq!(ht20.channel_or_frequency, 6);
        assert_eq!(ht20.cbw, 0);
    }

    #[test]
    fn association_retry_schedule_is_finite_inside_vendor_deadline() {
        let mut attempts = [0_u16; 7];
        let mut count = 0;
        for elapsed_ms in 0..=STA_RESPONSE_TIMEOUT_MS {
            if let Some(attempt) = StaAssociationRetrySchedule::attempt_at(elapsed_ms) {
                attempts[count] = attempt;
                count += 1;
            }
        }
        assert_eq!(count, attempts.len());
        assert_eq!(attempts, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(StaAssociationRetrySchedule::attempt_at(159), None);
        assert_eq!(StaAssociationRetrySchedule::attempt_at(1_000), None);
        assert_eq!(STA_AUTHENTICATION_ATTEMPT_LIMIT, 3);
    }

    #[test]
    fn association_runtime_owns_epoch_schedule_sequence_and_timeout() {
        let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID);
        let mut sequence = StaSequenceCounter::new(0x0ffc);
        let mut attempts = [StaAssociationAttempt {
            ordinal: 0,
            sequence_number: 0,
            elapsed_ms: 0,
        }; 7];
        let mut attempt_count = 0;

        loop {
            if let Some(attempt) = runtime.begin_tick(&mut sequence).unwrap() {
                attempts[attempt_count] = attempt;
                attempt_count += 1;
            }
            runtime.observe_received_frame().unwrap();
            match runtime.finish_tick().unwrap() {
                StaAssociationEvent::Irrelevant => {}
                StaAssociationEvent::Failed {
                    failure,
                    total_received_frames,
                } => {
                    assert_eq!(failure, StaAssociationFailure::Timeout);
                    assert_eq!(total_received_frames, STA_RESPONSE_TIMEOUT_MS);
                    break;
                }
                event => panic!("unexpected association event: {event:?}"),
            }
        }

        assert_eq!(attempt_count, attempts.len());
        for (index, attempt) in attempts.into_iter().enumerate() {
            assert_eq!(attempt.ordinal, index as u16 + 1);
            assert_eq!(attempt.elapsed_ms, index as u32 * 160);
            assert_eq!(attempt.sequence_number, (0x0ffc + index as u16) & 0x0fff);
        }
        assert_eq!(runtime.elapsed_ms(), STA_RESPONSE_TIMEOUT_MS);
        assert_eq!(runtime.total_received_frames(), STA_RESPONSE_TIMEOUT_MS);
        assert_eq!(
            runtime.begin_tick(&mut sequence),
            Err(StaAssociationRuntimeError::Terminal)
        );
    }

    #[test]
    fn association_runtime_accepts_only_selected_peer_response() {
        let mut runtime = StaAssociationRuntime::new(LOCAL, BSSID);
        let mut sequence = StaSequenceCounter::new(7);
        assert_eq!(
            runtime.begin_tick(&mut sequence).unwrap().unwrap().ordinal,
            1
        );
        assert_eq!(
            runtime.begin_tick(&mut sequence),
            Err(StaAssociationRuntimeError::TickAlreadyActive)
        );
        runtime.observe_received_frame().unwrap();

        let mut other_peer = association_response(0);
        other_peer[10] ^= 1;
        assert_eq!(
            runtime.observe_management_frame(&other_peer),
            Ok(StaAssociationEvent::Irrelevant)
        );

        let response = AssociationResponse {
            capability_info: 0x0431,
            status_code: 0,
            association_id: 42,
            ht_capability: false,
            he_capability: false,
            he_operation: false,
            wmm: false,
            wmm_parameters: None,
        };
        assert_eq!(
            runtime.observe_management_frame(&association_response(0)),
            Ok(StaAssociationEvent::Associated {
                response,
                total_received_frames: 1,
            })
        );
        assert_eq!(
            runtime.finish_tick(),
            Err(StaAssociationRuntimeError::Terminal)
        );
    }

    #[test]
    fn association_runtime_reports_peer_disconnect_and_rejection() {
        let mut sequence = StaSequenceCounter::new(0);
        let mut disconnected = StaAssociationRuntime::new(LOCAL, BSSID);
        disconnected.begin_tick(&mut sequence).unwrap();
        disconnected.observe_received_frame().unwrap();
        assert_eq!(
            disconnected.observe_management_frame(&deauthentication(7)),
            Ok(StaAssociationEvent::Failed {
                failure: StaAssociationFailure::PeerDisconnect(StaDisconnect {
                    kind: StaDisconnectKind::Deauthentication,
                    reason_code: 7,
                }),
                total_received_frames: 1,
            })
        );

        let mut rejected = StaAssociationRuntime::new(LOCAL, BSSID);
        rejected.begin_tick(&mut sequence).unwrap();
        assert_eq!(
            rejected.observe_management_frame(&association_response(17)),
            Ok(StaAssociationEvent::Failed {
                failure: StaAssociationFailure::Rejected { status_code: 17 },
                total_received_frames: 0,
            })
        );
    }

    #[test]
    fn he20_request_preserves_vendor_body_except_unowned_cqi_claims() {
        let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        let ssid = b"FRITZ!Box 7530 FN";
        record.ssid[..ssid.len()].copy_from_slice(ssid);
        record.ssid_len = ssid.len() as u8;
        record.capability_info = 0x0431;
        record.supported_rates[..8]
            .copy_from_slice(&[0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60]);
        record.supported_rates_len = 8;
        record.extended_supported_rates[..4].copy_from_slice(&[0x6c, 0x12, 0x24, 0x48]);
        record.extended_supported_rates_len = 4;
        record.ht_capability_ie_present = true;
        record.he_capability_ie[..HE20_VENDOR_MCS9_CAPABILITY_IE.len()]
            .copy_from_slice(&HE20_VENDOR_MCS9_CAPABILITY_IE);
        record.he_capability_ie_len = HE20_VENDOR_MCS9_CAPABILITY_IE.len() as u8;
        let he_operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
        record.he_operation_ie[..he_operation.len()].copy_from_slice(&he_operation);
        record.he_operation_ie_len = he_operation.len() as u8;

        let power = HeUlMuPowerCapability::from_rate_power_indices([
            20, 20, 20, 19, 19, 18, 18, 16, 15, 20,
        ])
        .unwrap();
        assert_eq!(power.relative_to_rate_16(), [0, 0, 1, 1, 2, 2, 4, 5, 0]);

        let mut output = [0; 192];
        let length = AssociationRequest {
            source: LOCAL,
            access_point: &record,
            sequence_number: 2,
            listen_interval: 3,
            phy: StaAssociationPhy::He20,
            power_capability: Some(StaPowerCapability::new(-11, 20).unwrap()),
            he_ul_mu_power: Some(power),
        }
        .encode(&mut output)
        .unwrap();
        assert_eq!(
            &output[24..89],
            &[
                0x31, 0x04, 0x03, 0x00, 0x00, 17, b'F', b'R', b'I', b'T', b'Z', b'!', b'B', b'o',
                b'x', b' ', b'7', b'5', b'3', b'0', b' ', b'F', b'N', 1, 8, 0x8b, 0x96, 0x82, 0x84,
                0x0c, 0x18, 0x30, 0x60, 48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1,
                0, 0, 0x0f, 0xac, 2, 0, 4, 33, 2, 0xf5, 20, 50, 4, 0x6c, 0x12, 0x24, 0x48,
            ]
        );
        let expected_tail_len = HE20_HT_CAPABILITY_IE.len()
            + HE20_OWNED_MCS9_CAPABILITY_IE.len()
            + HE_UL_MU_POWER_CAPABILITY_IE_LEN
            + WMM_INFORMATION_IE.len()
            + HE20_EXTENDED_CAPABILITY_IE.len();
        let tail = &output[length - expected_tail_len..length];
        let mut offset = 0;
        assert_eq!(
            &tail[offset..offset + HE20_HT_CAPABILITY_IE.len()],
            &HE20_HT_CAPABILITY_IE
        );
        offset += HE20_HT_CAPABILITY_IE.len();
        assert_eq!(
            &tail[offset..offset + HE20_OWNED_MCS9_CAPABILITY_IE.len()],
            &HE20_OWNED_MCS9_CAPABILITY_IE
        );
        assert_eq!(
            HE20_OWNED_MCS9_CAPABILITY_IE[3],
            HE20_VENDOR_MCS9_CAPABILITY_IE[3] & !(1 << 1)
        );
        assert_eq!(
            HE20_OWNED_MCS9_CAPABILITY_IE[15],
            HE20_VENDOR_MCS9_CAPABILITY_IE[15] & !(1 << 4)
        );
        assert_eq!(
            HE20_OWNED_MCS9_CAPABILITY_IE[18],
            HE20_VENDOR_MCS9_CAPABILITY_IE[18] & !(1 << 1)
        );
        for index in 0..HE20_VENDOR_MCS9_CAPABILITY_IE.len() {
            if index != 3 && index != 15 && index != 18 {
                assert_eq!(
                    HE20_OWNED_MCS9_CAPABILITY_IE[index],
                    HE20_VENDOR_MCS9_CAPABILITY_IE[index],
                );
            }
        }
        offset += HE20_OWNED_MCS9_CAPABILITY_IE.len();
        assert_eq!(
            &tail[offset..offset + HE_UL_MU_POWER_CAPABILITY_IE_LEN],
            &[255, 12, 60, 0, 0, 1, 1, 2, 2, 4, 5, 0, 0, 0]
        );
        offset += HE_UL_MU_POWER_CAPABILITY_IE_LEN;
        assert_eq!(
            &tail[offset..offset + WMM_INFORMATION_IE.len()],
            &WMM_INFORMATION_IE
        );
        offset += WMM_INFORMATION_IE.len();
        assert_eq!(&tail[offset..], &HE20_EXTENDED_CAPABILITY_IE);
    }

    #[test]
    fn he_ul_mu_power_rejects_a_rate_above_the_reference() {
        assert_eq!(
            HeUlMuPowerCapability::from_rate_power_indices([
                20, 20, 20, 19, 19, 21, 18, 16, 15, 20,
            ]),
            Err(HeUlMuPowerCapabilityError::HigherPowerThanRate16 { rate: 21 })
        );
    }

    #[test]
    fn sta_data_frame_encodes_to_ds_llc_snap() {
        let mut output = [0; 64];
        let len = StaDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 0x123,
            ether_type: 0x888e,
            payload: &[1, 2, 3],
        }
        .encode(&mut output)
        .unwrap();
        assert_eq!(len, 35);
        assert_eq!(&output[..2], &[0x08, 0x01]);
        assert_eq!(&output[4..10], &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15]);
        assert_eq!(&output[10..16], &[2, 3, 4, 5, 6, 7]);
        assert_eq!(&output[16..22], &[0x20, 0x21, 0x22, 0x23, 0x24, 0x25]);
        assert_eq!(&output[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        assert_eq!(&output[32..len], &[1, 2, 3]);
    }

    #[test]
    fn protected_data_frame_selects_the_recovered_legacy_or_qos_layout() {
        let mut output = [0; 64];
        let frame = StaProtectedDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 0x123,
            user_priority: 7,
            peer_qos: true,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            ether_type: 0x888e,
            payload: &[1, 2, 3],
        };
        let len = frame.encode(&mut output).unwrap();
        assert_eq!(len, 45);
        assert_eq!(&output[..2], &[0x88, 0x41]);
        assert_eq!(&output[24..26], &[7, 0]);
        assert_eq!(&output[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        assert_eq!(&output[42..len], &[1, 2, 3]);

        let len = StaProtectedDataFrame {
            peer_qos: false,
            ..frame
        }
        .encode(&mut output)
        .unwrap();
        assert_eq!(len, 43);
        assert_eq!(&output[..2], &[0x08, 0x41]);
        assert_eq!(&output[24..32], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(&output[32..40], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        assert_eq!(&output[40..len], &[1, 2, 3]);
    }

    #[test]
    fn protected_data_frame_fully_overwrites_a_reused_dma_slot() {
        let frame = StaProtectedDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 0x123,
            user_priority: 7,
            peer_qos: true,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            ether_type: 0x888e,
            payload: &[1, 2, 3],
        };
        let mut zeroed = [0_u8; 64];
        let mut reused = [0xa5_u8; 64];

        let zeroed_len = frame.encode(&mut zeroed).unwrap();
        let reused_len = frame.encode(&mut reused).unwrap();

        assert_eq!(reused_len, zeroed_len);
        assert_eq!(&reused[..reused_len], &zeroed[..zeroed_len]);
    }

    #[test]
    fn protected_ethernet_frame_reuses_payload_at_its_final_dma_offset() {
        let ethernet = [
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x88, 0x8e, 1, 2, 3,
        ];
        let metadata = StaProtectedEthernetFrame {
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            sequence_number: 0x123,
            user_priority: 7,
            peer_qos: true,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        };
        let mut expected = [0_u8; 64];
        let expected_len = StaProtectedDataFrame {
            source: ethernet[6..12].try_into().unwrap(),
            bssid: metadata.bssid,
            destination: ethernet[..6].try_into().unwrap(),
            sequence_number: metadata.sequence_number,
            user_priority: metadata.user_priority,
            peer_qos: metadata.peer_qos,
            ccmp_header: metadata.ccmp_header,
            ether_type: u16::from_be_bytes([ethernet[12], ethernet[13]]),
            payload: &ethernet[14..],
        }
        .encode(&mut expected)
        .unwrap();

        let mut storage = [0xa5_u8; 64];
        storage[STA_PROTECTED_QOS_ETHERNET_HEADROOM
            ..STA_PROTECTED_QOS_ETHERNET_HEADROOM + ethernet.len()]
            .copy_from_slice(&ethernet);
        let encoded = metadata
            .encode_in_place(
                &mut storage,
                STA_PROTECTED_QOS_ETHERNET_HEADROOM,
                ethernet.len(),
                DataHeControl::Disabled,
            )
            .unwrap();

        assert_eq!(
            encoded,
            EncodedStaFrame {
                offset: 0,
                length: 45
            }
        );
        assert_eq!(encoded.length, expected_len);
        assert_eq!(&storage[..encoded.length], &expected[..expected_len]);
        // The three payload bytes began at offset 28 + 14 and remain at that
        // exact address after the prefix conversion.
        assert_eq!(&storage[42..45], &[1, 2, 3]);
    }

    #[test]
    fn protected_ethernet_frame_reports_missing_headroom_before_mutation() {
        let ethernet = [
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00,
        ];
        let mut storage = [0xa5_u8; 64];
        storage[27..27 + ethernet.len()].copy_from_slice(&ethernet);

        assert_eq!(
            StaProtectedEthernetFrame {
                bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
                sequence_number: 0x123,
                user_priority: 0,
                peer_qos: true,
                ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            }
            .encode_in_place(&mut storage, 27, ethernet.len(), DataHeControl::Disabled),
            Err(StationFrameError::EthernetHeadroomTooSmall {
                required: STA_PROTECTED_QOS_ETHERNET_HEADROOM,
                available: 27,
            })
        );
        assert_eq!(&storage[27..27 + ethernet.len()], &ethernet);
    }

    #[test]
    fn protected_he_control_keeps_ccmp_immediately_after_qos() {
        let mut output = [0xa5; 64];
        let len = StaProtectedDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 0x123,
            user_priority: 0,
            peer_qos: true,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            ether_type: 0x0806,
            payload: &[1, 2, 3],
        }
        .encode_with_he_control(
            DataHeControl::HardwareGeneratedBufferStatusReport,
            &mut output,
        )
        .unwrap();

        assert_eq!(len, 45);
        assert_eq!(&output[..2], &[0x88, 0xc1]);
        assert_eq!(&output[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x06]);
        assert_eq!(&output[42..len], &[1, 2, 3]);
    }

    #[test]
    fn protected_amsdu_encodes_two_ethernet_frames_and_round_trips() {
        let first = [
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00, 1, 2, 3,
        ];
        let second = [
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 8, 9, 10, 11, 12, 13, 0x88, 0xb5, 4, 5, 6,
        ];
        let ethernet_frames: [&[u8]; 2] = [&first, &second];
        let mut output = [0_u8; 96];
        assert_eq!(sta_protected_amsdu_frame_length(&ethernet_frames), Ok(87));
        let length = StaProtectedAmsduFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            sequence_number: 0x123,
            user_priority: 7,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            ethernet_frames: &ethernet_frames,
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(length, 87);
        assert_eq!(&output[..2], &[0x88, 0x41]);
        assert_eq!(&output[24..26], &[0x87, 0]);
        assert_eq!(&output[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(&output[34..40], &first[..6]);
        assert_eq!(&output[40..46], &first[6..12]);
        assert_eq!(&output[46..48], &[0, 11]);
        assert_eq!(&output[48..56], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
        assert_eq!(&output[56..59], &[1, 2, 3]);
        assert_eq!(&output[59..62], &[0, 0, 0]);

        let mut subframes = crate::data::amsdu_subframes(
            DataInterfaceRole::AccessPoint,
            &output[..length],
            34,
            length - 34,
        )
        .unwrap();
        let first_decoded = subframes.next().unwrap().unwrap();
        assert_eq!(first_decoded.destination, first[..6]);
        assert_eq!(first_decoded.source, first[6..12]);
        assert_eq!(first_decoded.ether_type, 0x0800);
        assert_eq!(first_decoded.payload, &[1, 2, 3]);
        let second_decoded = subframes.next().unwrap().unwrap();
        assert_eq!(second_decoded.destination, second[..6]);
        assert_eq!(second_decoded.source, second[6..12]);
        assert_eq!(second_decoded.ether_type, 0x88b5);
        assert_eq!(second_decoded.payload, &[4, 5, 6]);
        assert_eq!(subframes.next(), None);
    }

    #[test]
    fn protected_amsdu_pair_encodes_in_first_ethernet_allocation() {
        let first = [
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00, 1, 2, 3,
        ];
        let second = [
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 8, 9, 10, 11, 12, 13, 0x88, 0xb5, 4, 5, 6,
        ];
        let ethernet_frames: [&[u8]; 2] = [&first, &second];
        let metadata = StaProtectedEthernetFrame {
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            sequence_number: 0x123,
            user_priority: 7,
            peer_qos: true,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        };
        let mut expected = [0_u8; 96];
        let expected_length = StaProtectedAmsduFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: metadata.bssid,
            sequence_number: metadata.sequence_number,
            user_priority: metadata.user_priority,
            ccmp_header: metadata.ccmp_header,
            ethernet_frames: &ethernet_frames,
        }
        .encode(&mut expected)
        .unwrap();

        const ETHERNET_OFFSET: usize = STA_PROTECTED_QOS_ETHERNET_HEADROOM;
        let mut storage = [0xa5_u8; 128];
        storage[ETHERNET_OFFSET..ETHERNET_OFFSET + first.len()].copy_from_slice(&first);
        let encoded = metadata
            .encode_amsdu_pair_in_place(&mut storage, ETHERNET_OFFSET, first.len(), &second)
            .unwrap();

        assert_eq!(encoded.offset, 0);
        assert_eq!(encoded.length, expected_length);
        assert_eq!(
            &storage[..encoded.length],
            &expected[..expected_length],
            "in-place and owned encoders must emit identical MPDUs"
        );
        assert!(
            storage[encoded.length..].iter().all(|byte| *byte == 0xa5),
            "capacity beyond the encoded MPDU remains untouched"
        );
    }

    #[test]
    fn protected_amsdu_fully_overwrites_a_reused_output_slot() {
        let first = [
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00, 1, 2, 3,
        ];
        let second = [
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 8, 9, 10, 11, 12, 13, 0x88, 0xb5, 4, 5, 6,
        ];
        let ethernet_frames: [&[u8]; 2] = [&first, &second];
        let encoded = StaProtectedAmsduFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            sequence_number: 0x123,
            user_priority: 7,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            ethernet_frames: &ethernet_frames,
        };
        let mut zeroed = [0_u8; 96];
        let mut reused = [0xa5_u8; 96];
        let zeroed_length = encoded.encode(&mut zeroed).unwrap();
        let reused_length = encoded.encode(&mut reused).unwrap();

        assert_eq!(zeroed_length, reused_length);
        assert_eq!(
            &zeroed[..zeroed_length],
            &reused[..reused_length],
            "every returned byte, including A-MSDU padding, must be initialized"
        );
        assert_eq!(&reused[59..62], &[0, 0, 0]);
        assert!(
            reused[reused_length..].iter().all(|byte| *byte == 0xa5),
            "the encoder must not touch capacity beyond the returned frame"
        );

        let previous = reused;
        let refreshed_length = StaProtectedAmsduFrame {
            sequence_number: 0x456,
            ccmp_header: [9, 0, 0, 0x20, 0, 0, 0, 0],
            ..encoded
        }
        .refresh_header(&mut reused)
        .unwrap();
        assert_eq!(refreshed_length, reused_length);
        assert_eq!(&reused[22..24], &(0x456_u16 << 4).to_le_bytes());
        assert_eq!(&reused[26..34], &[9, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(
            &reused[34..reused_length],
            &previous[34..reused_length],
            "refresh must retain the already encoded A-MSDU body"
        );
    }

    #[test]
    fn protected_amsdu_rejects_the_unadvertised_large_class() {
        let ethernet = [0_u8; 2_000];
        let frames: [&[u8]; 2] = [&ethernet, &ethernet];
        assert_eq!(
            sta_protected_amsdu_frame_length(&frames),
            Err(StationFrameError::AmsduTooLong {
                length: 4_016,
                maximum: HT_AMSDU_BASELINE_MAX_LEN,
            })
        );
        assert_eq!(
            StaProtectedAmsduFrame {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
                sequence_number: 1,
                user_priority: 0,
                ccmp_header: [0; CCMP_HEADER_LEN],
                ethernet_frames: &frames,
            }
            .encode(&mut [0_u8; 4_096]),
            Err(StationFrameError::AmsduTooLong {
                length: 4_016,
                maximum: HT_AMSDU_BASELINE_MAX_LEN,
            })
        );
    }

    #[test]
    fn parses_association_response_and_masks_aid() {
        let mut frame = [0_u8; 60];
        frame[0] = 0x10;
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
        frame[30..58].copy_from_slice(&[
            45, 26, 0x20, 0, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
            0, 0,
        ]);
        let response = parse_association_response(&frame[..58], LOCAL, BSSID).unwrap();
        assert_eq!(response.status_code, 0);
        assert_eq!(response.association_id, 42);
        assert!(response.ht_capability);
        assert!(response.wmm);
    }

    #[test]
    fn association_response_retains_complete_wmm_parameters() {
        let mut frame = [0_u8; 56];
        frame[0] = 0x10;
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[28..30].copy_from_slice(&7_u16.to_le_bytes());
        frame[30..].copy_from_slice(&[
            221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 3, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
            0x43, 94, 0, 0x62, 0x32, 47, 0,
        ]);

        let response = parse_association_response(&frame, LOCAL, BSSID).unwrap();
        let parameters = response.wmm_parameters.unwrap();
        assert_eq!(parameters.parameter_set_count, 3);
        assert_eq!(
            parameters
                .access_category(crate::wmm::WmmAccessCategory::Video)
                .txop_limit_units_32_us,
            94
        );
        assert_eq!(
            parameters
                .access_category(crate::wmm::WmmAccessCategory::Voice)
                .txop_limit_units_32_us,
            47
        );
    }

    #[test]
    fn open_ap_needs_no_security_ie() {
        let record = ScanRecord {
            bssid: BSSID,
            supported_rates: [0x82; 8],
            supported_rates_len: 1,
            ..ScanRecord::EMPTY
        };
        assert!(select_wpa2_psk_rsn(&record).unwrap().as_bytes().is_empty());
    }

    #[test]
    fn sta_sequence_counter_is_monotonic_across_twelve_bit_wrap() {
        let mut sequence = StaSequenceCounter::new(0x1ffe);
        assert_eq!(sequence.take(), 0x0ffe);
        assert_eq!(sequence.take(), 0x0fff);
        assert_eq!(sequence.take(), 0x0000);
        assert_eq!(sequence.peek(), 0x0001);
    }

    #[test]
    fn sta_tx_sequence_spaces_do_not_advance_each_other() {
        let mut sequences = StaTxSequenceCounters::new(25);

        assert_eq!(sequences.take_non_qos(), 25);
        assert_eq!(sequences.take_non_qos(), 26);
        assert_eq!(sequences.peek_qos(0), Some(25));
        assert_eq!(sequences.peek_qos(5), Some(25));
        assert_eq!(sequences.peek_qos(7), Some(25));

        assert_eq!(sequences.take_qos(0), Some(25));
        assert_eq!(sequences.peek_qos(0), Some(26));
        assert_eq!(sequences.peek_qos(5), Some(25));
        assert_eq!(sequences.peek_qos(7), Some(25));
        assert_eq!(sequences.peek_non_qos(), 27);
    }

    #[test]
    fn sta_tx_sequence_space_rejects_invalid_tid_and_wraps_independently() {
        let mut sequences = StaTxSequenceCounters::new(0x0fff);

        assert_eq!(sequences.take_data(Some(15)), Some(0x0fff));
        assert_eq!(sequences.peek_qos(15), Some(0));
        assert_eq!(sequences.take_data(None), Some(0x0fff));
        assert_eq!(sequences.peek_non_qos(), 0);
        assert_eq!(sequences.take_data(Some(16)), None);
    }

    #[test]
    fn sta_rx_duplicate_filter_requires_retry_and_matching_sequence_space() {
        let mut filter = crate::data::RxDuplicateFilter::new();
        assert!(!filter.is_duplicate(false, 0x1230, None));
        assert!(filter.is_duplicate(true, 0x1230, None));
        assert!(!filter.is_duplicate(false, 0x1230, None));
        assert!(!filter.is_duplicate(true, 0x1240, None));

        assert!(!filter.is_duplicate(false, 0x2000, Some(3)));
        assert!(!filter.is_duplicate(true, 0x2000, Some(4)));
        assert!(filter.is_duplicate(true, 0x2000, Some(3)));
    }
}
