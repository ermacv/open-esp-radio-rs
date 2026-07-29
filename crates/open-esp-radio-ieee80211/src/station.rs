//! Allocation-free STA authentication and association protocol.
//!
//! This module owns only IEEE 802.11 frame construction, response parsing,
//! and WPA2-Personal RSN selection. The caller retains the output buffer and
//! decides when to submit it to hardware, arm a deadline, or retry.

use crate::{
    ccmp::CCMP_HEADER_LEN,
    data::{plan_data_encapsulation, DataInterfaceRole, ETHERNET_HEADER_LEN, LLC_SNAP_HEADER_LEN},
    he::{parse_he20_capabilities, parse_he20_operation},
    management::{MANAGEMENT_HEADER_LEN, MAX_SSID_LEN, MAX_SUPPORTED_RATES_LEN},
    scan::ScanRecord,
    wmm::{parse_wmm_parameter_element, WmmParameterSet},
};

const OPEN_AUTHENTICATION_FRAME_CONTROL: u16 = 0x00b0;
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
// One-stream HT20 with short guard interval. Channel-width, STBC, LDPC,
// large A-MSDU and 40-MHz claims remain disabled until their matching
// Rust-owned paths are enabled.
//
// SOURCE: promoted migration
// `migration/esp32s31-hybrid-runtime/src/sta_link.rs::HT20_CAPABILITY_IE`,
// originally qualified by the strict ESP32-S31 STA WPA2/ADDBA throughput HIL.
const HT20_CAPABILITY_IE: [u8; 28] = [
    45, 26, 0x20, 0x00, 0x00, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
// One-stream HT40 with short guard intervals for both 20 and 40 MHz and
// spatial multiplexing power save disabled. Although the S31 has one receive
// stream, advertising static SMPS (`0x0062`) made the controlled Linux HT40 AP
// accept authentication but discard the association request. The vendor
// builder preserves the interface's SMPS bits and ordinary ESP STA requests
// use the disabled value (`bits 2..3 = 0b11`), giving `0x006e`.
//
// SOURCE: `_oracles/libnet80211.a[ieee80211_ht.o]::
// ieee80211_add_htcap_body` reads the base capability at node offset `0x14c`,
// adds Supported Channel Width at `+0x4e`, SGI20 at `+0x8e`, and SGI40 at
// `+0xaa` without replacing the base SMPS bits. IEEE 802.11 HT Capabilities
// Info defines bits 2..3 value `0b11` as SMPS disabled. Selection remains
// gated by the complete AP HT Capabilities/Operation IEs through
// `ScanRecord::ht40_secondary_channel`; hardware CBW support is the complete
// rev0 ROM `phy_bb_bss_cbw40` implementation promoted into the S31 PAC/HAL.
const HT40_CAPABILITY_IE: [u8; 28] = [
    45, 26, 0x6e, 0x00, 0x00, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
// Exact one-stream HE20 MCS0-9 capability captured from the vendor
// association request and qualified by the old strict HE association HIL.
// This must not be relabelled HE40: complete
// `_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` writes zero to
// complete IE byte nine (the first HE PHY Capabilities byte / Channel Width
// Set) on both its STA branches. The chip's vendor path advertises 40 MHz
// separately through HT Capabilities, as represented by HT40_CAPABILITY_IE.
//
// SOURCE: promoted migration
// `migration/esp32s31-hybrid-runtime/src/sta_link.rs::
// HE20_MCS9_CAPABILITY_IE`, originally compared with the request constructed
// by pinned `_oracles/libnet80211.a`.
//
// FIELD AUDIT: complete
// `_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` proves that
// byte 11 bit 3 is the S31 `g_phy_cap_rx_stbc` advertisement. Bytes 15 bits
// 2..4 and 18 bit 1 advertise triggered SU/MU beamforming feedback, triggered
// CQI and non-triggered CQI. The open RX metadata path already decodes the
// HE-SIG-A2 STBC bit, but controlled STBC downlink HIL is still pending and
// the CQI report producer is not yet owned. Preserving the vendor request here
// is an oracle-compatibility choice, not a claim that those feedback paths are
// qualified.
const HE20_MCS9_CAPABILITY_IE: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];
// WMM Information, version one, U-APSD disabled.
//
// SOURCE: the same promoted `sta_link.rs::WMM_INFORMATION_IE`, cross-checked
// against `_oracles/libnet80211.a` association-request construction.
const WMM_INFORMATION_IE: [u8; 9] = [221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00];
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

/// Monotonic twelve-bit sequence-number owner for one STA transmit session.
///
/// A sequence number is consumed for every newly encoded MPDU. Hardware
/// retries retain the already encoded header and therefore do not call
/// [`Self::take`]. Keeping this state in one value prevents authentication,
/// association, EAPOL, and connected data paths from restarting overlapping
/// ad-hoc ranges.
///
/// SOURCE: the promoted migration data path
/// `migration/esp32s31-hybrid-runtime/src/net80211_encap.rs` advanced one
/// interface-owned counter and published its low twelve bits in Sequence
/// Control. `_oracles/libpp.a[pp.o]` keeps retry handling attached to the same
/// queued frame rather than allocating another protocol sequence number.
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

/// Per-traffic-class receive history for IEEE 802.11 duplicate suppression.
///
/// A retransmission is a duplicate only when its Retry bit is set and its
/// complete Sequence Control value (sequence plus fragment number) matches
/// the last accepted MPDU in the same non-QoS or QoS/TID sequence space.
///
/// SOURCE: the wire fields and comparison rule are the IEEE 802.11 Retry and
/// Sequence Control contract. The pinned vendor receive owner implementing
/// this boundary is `_oracles/libnet80211.a[ieee80211_input.o]`; keeping the
/// state here avoids leaking its node-layout offsets into the open driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaRxDuplicateFilter {
    last_sequence_control: [u16; 17],
    valid: u32,
}

impl StaRxDuplicateFilter {
    pub const fn new() -> Self {
        Self {
            last_sequence_control: [0; 17],
            valid: 0,
        }
    }

    /// Observes one valid MPDU and returns whether it must be discarded.
    ///
    /// `tid` is `None` for the legacy/non-QoS sequence space and `0..=15` for
    /// a QoS data TID. An invalid TID is treated as a new non-QoS frame so a
    /// malformed caller value cannot poison a valid QoS history slot.
    #[inline(never)]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_rx_hot")
    )]
    pub fn is_duplicate(&mut self, retry: bool, sequence_control: u16, tid: Option<u8>) -> bool {
        let index = match tid {
            Some(tid @ 0..=15) => usize::from(tid) + 1,
            _ => 0,
        };
        let mask = 1_u32 << index;
        if retry && self.valid & mask != 0 && self.last_sequence_control[index] == sequence_control
        {
            return true;
        }
        self.last_sequence_control[index] = sequence_control;
        self.valid |= mask;
        false
    }
}

impl Default for StaRxDuplicateFilter {
    fn default() -> Self {
        Self::new()
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

/// One unprotected STA-originated Action management frame.
///
/// BlockAck negotiation uses this common 24-byte management header followed
/// by the nine-byte action body owned by the MAC BlockAck state machine.
///
/// SOURCE: promoted migration
/// `migration/esp32s31-hybrid-runtime/src/sta_link.rs::send_addba_response`,
/// where the same header was constructed around
/// `rx_ampdu::write_successful_addba_response`; the frame-control subtype is
/// the IEEE 802.11 Action management subtype also parsed by
/// `_oracles/libnet80211.a`.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationPhy {
    Legacy,
    Ht20,
    Ht40,
    He20,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationRequest<'a> {
    pub source: [u8; 6],
    pub access_point: &'a ScanRecord,
    pub sequence_number: u16,
    pub listen_interval: u16,
    pub phy: StaAssociationPhy,
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
        let (ht_capability, he_capability) = match self.phy {
            StaAssociationPhy::Legacy => (None, None),
            StaAssociationPhy::Ht20 if self.access_point.ht_capability_ie_present => {
                (Some(&HT20_CAPABILITY_IE), None)
            }
            StaAssociationPhy::Ht20 => {
                return Err(AssociationRequestError::HtUnsupportedByAccessPoint);
            }
            StaAssociationPhy::Ht40 if self.access_point.ht40_secondary_channel().is_some() => {
                (Some(&HT40_CAPABILITY_IE), None)
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
                (Some(&HT20_CAPABILITY_IE), Some(&HE20_MCS9_CAPABILITY_IE))
            }
            StaAssociationPhy::He20 => {
                return Err(AssociationRequestError::He20UnsupportedByAccessPoint);
            }
        };
        let phy_information_len = if let Some(capability) = ht_capability {
            capability.len()
                + he_capability.map_or(0, |capability| capability.len())
                + WMM_INFORMATION_IE.len()
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

        let rsn = selected_rsn.as_bytes();
        frame[offset..offset + rsn.len()].copy_from_slice(rsn);
        offset += rsn.len();
        if let Some(capability) = ht_capability {
            frame[offset..offset + capability.len()].copy_from_slice(capability);
            offset += capability.len();
            if let Some(capability) = he_capability {
                frame[offset..offset + capability.len()].copy_from_slice(capability);
                offset += capability.len();
            }
            frame[offset..offset + WMM_INFORMATION_IE.len()].copy_from_slice(&WMM_INFORMATION_IE);
            offset += WMM_INFORMATION_IE.len();
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
        validate_peer(self.bssid, self.sequence_number)?;
        if self.user_priority > 7 {
            return Err(StationFrameError::UserPriorityOutOfRange);
        }
        let ethernet = ethernet_header(self.destination, self.source, self.ether_type);
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            self.source,
            ethernet,
            self.user_priority,
            self.peer_qos,
            false,
        )
        .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        // Exact `net80211_tx::encapsulate_ordinary` mutation after successful
        // CCMP key selection.
        plan.header[1] |= 0x40;
        let header_len = usize::from(plan.header_len);
        let required = header_len
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
        frame.fill(0);
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_end = header_len + CCMP_HEADER_LEN;
        frame[header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        let llc_end = ccmp_end + plan.llc_snap.len();
        frame[ccmp_end..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(self.payload);
        Ok(required)
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
    /// SOURCE: `_oracles/libpp.a[pp.o]::ppResortTxAMPDU` retains the complete
    /// CCMP-ready MPDU across a missing BlockAck bit and changes only retry
    /// metadata.
    ///
    /// SOURCE[HIL_OPEN_HT40_AMSDU_BODY_REUSE_2026_07_29]:
    /// `esp32s31_rust` commit
    /// `8d69a294a5ab0f40f55313a292ca1d0fc1c4a853`,
    /// `firmware/esp32s31/app/src/open_radio_phy_prelude_hil.rs`. The
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
        0,
        0,
    ]);
    Ok(selected)
}

fn validate_peer(bssid: [u8; 6], sequence_number: u16) -> Result<(), StationFrameError> {
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
        let mut frame = [0_u8; 30];
        frame[0] = 0xb0;
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
        frame[28..30].copy_from_slice(&17_u16.to_le_bytes());
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
    fn mixed_wpa2_wpa3_ap_is_narrowed_to_wpa2_psk_ccmp() {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8], [0, 0x0f, 0xac, 2]], 0x80);
        let selected = select_wpa2_psk_rsn(&record).unwrap();
        assert_eq!(selected.as_bytes().len(), SELECTED_RSN_IE_LEN);
        assert_eq!(&selected.as_bytes()[8..14], &[1, 0, 0, 0x0f, 0xac, 4]);
        assert_eq!(&selected.as_bytes()[14..20], &[1, 0, 0, 0x0f, 0xac, 2]);
        assert_eq!(&selected.as_bytes()[20..22], &[0, 0]);
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
                0,
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
        }
        .encode(&mut output)
        .unwrap();
        let phy_start = length - HT40_CAPABILITY_IE.len() - WMM_INFORMATION_IE.len();
        assert_eq!(&output[phy_start..phy_start + 4], &[45, 26, 0x6e, 0]);
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
    fn sta_rx_duplicate_filter_requires_retry_and_matching_sequence_space() {
        let mut filter = StaRxDuplicateFilter::new();
        assert!(!filter.is_duplicate(false, 0x1230, None));
        assert!(filter.is_duplicate(true, 0x1230, None));
        assert!(!filter.is_duplicate(false, 0x1230, None));
        assert!(!filter.is_duplicate(true, 0x1240, None));

        assert!(!filter.is_duplicate(false, 0x2000, Some(3)));
        assert!(!filter.is_duplicate(true, 0x2000, Some(4)));
        assert!(filter.is_duplicate(true, 0x2000, Some(3)));
    }
}
