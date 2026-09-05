//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms form the source-owned AP protocol boundary.
//! Association ownership, deferred
//! queues and wakeups stay with the radio owner.

use crate::{
    block_ack::{BlockAckAction, parse_block_ack_action},
    ccmp::CCMP_HEADER_LEN,
    channel::WifiChannel,
    data::{DataInterfaceRole, ETHERNET_HEADER_LEN, LLC_SNAP_HEADER_LEN, plan_data_encapsulation},
    ht::{
        HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN, HtPeerCapabilities, ht_capability_ie_for_peer,
        ht_operation_ie, ht_peer_capabilities,
    },
    security::WifiSecurityMode,
    station_power_save::STA_NULL_DATA_FRAME_LEN,
};

pub mod profile;
use profile::Advertisement;

const AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN: usize = 51;
pub const AP_ASSOCIATION_RESPONSE_BODY_LEN: usize =
    AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN + HT_CAPABILITY_IE_LEN + HT_OPERATION_IE_LEN;
pub const AP_AUTHENTICATION_RESPONSE_LEN: usize = 30;
pub const AP_PEER_DISCONNECT_LEN: usize = MANAGEMENT_HEADER_LEN + 2;
pub const AP_ASSOCIATION_RESPONSE_LEN: usize = 24 + AP_ASSOCIATION_RESPONSE_BODY_LEN;

const MANAGEMENT_HEADER_LEN: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAssociationResponseError {
    MissingAssociationId,
    InvalidSequenceNumber,
    OutputTooSmall { required: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerDisconnectKind {
    Disassociation,
    Deauthentication,
}

/// Invalid AP data-frame construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApDataFrameError {
    InvalidAccessPoint,
    InvalidPeer,
    InvalidSequenceNumber,
    InvalidUserPriority,
    EthernetFrameTooShort,
    NoAmsduFrames,
    AmsduTooLong { length: usize, maximum: usize },
    OutputTooSmall { required: usize },
}

const AMSDU_SUBFRAME_HEADER_LEN: usize = 14;
/// Baseline HT Max A-MSDU Length selected while HT Capabilities bit 11 is
/// clear. Every AP frame codec below is fenced to the advertised class.
pub const AP_AMSDU_BASELINE_MAX_LEN: usize = 3_839;

/// One unprotected 802.11 data MPDU sent by an AP to its client.
///
/// The initial AP uses this for EAPOL before the controlled port opens. The
/// frame shape is portable; descriptor metadata and queue selection remain in
/// the chip TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApDataFrame<'payload> {
    pub access_point: [u8; 6],
    pub destination: [u8; 6],
    pub sequence_number: u16,
    pub ether_type: u16,
    pub payload: &'payload [u8],
}

impl ApDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        let mut ethernet = [0; ETHERNET_HEADER_LEN];
        ethernet[..6].copy_from_slice(&self.destination);
        ethernet[6..12].copy_from_slice(&self.access_point);
        ethernet[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        let plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
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
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let llc_end = header_len + plan.llc_snap.len();
        frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(self.payload);
        Ok(required)
    }
}

/// One plaintext Ethernet-II MPDU for an explicitly Open AP epoch.
///
/// Unlike [`ApDataFrame`], which models an AP-originated EAPOL payload, this
/// codec preserves the complete caller-owned Ethernet header and validates
/// the destination against the selected unicast/group peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApUnprotectedDataFrame<'frame> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: u16,
    pub more_data: bool,
    pub ethernet: &'frame [u8],
}

impl ApUnprotectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        if self.ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        let mut ethernet_header = [0; ETHERNET_HEADER_LEN];
        ethernet_header.copy_from_slice(&self.ethernet[..ETHERNET_HEADER_LEN]);
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            0,
            false,
            false,
        )
        .expect("priority zero is valid for plaintext non-QoS data");
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(plan.llc_snap.len())
            .and_then(|length| length.checked_add(self.ethernet.len() - ETHERNET_HEADER_LEN))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let llc_end = header_len + plan.llc_snap.len();
        frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(&self.ethernet[ETHERNET_HEADER_LEN..]);
        Ok(required)
    }
}

/// One protected Ethernet-II frame sent from an AP to its authorized peer.
///
/// The caller owns the pairwise packet number. This codec merely places the
/// supplied CCMP header in the DMA image; ESP32-S31 hardware encrypts the
/// plaintext LLC/SNAP payload and appends the MIC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedDataFrame<'frame> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: u16,
    pub user_priority: u8,
    pub peer_qos: bool,
    /// Set the IEEE 802.11 More Data bit for a frame released from an AP
    /// power-save queue while further traffic remains buffered.
    pub more_data: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ethernet: &'frame [u8],
}

pub const AP_PROTECTED_QOS_ETHERNET_OVERHEAD: usize =
    26 + CCMP_HEADER_LEN + 8 - ETHERNET_HEADER_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedApFrame {
    pub offset: usize,
    pub length: usize,
}

impl ApProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        if self.user_priority > 7 {
            return Err(ApDataFrameError::InvalidUserPriority);
        }
        if self.ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        // The initial AP owns one pairwise slot. Group-addressed Ethernet
        // needs the distinct GTK packet-number owner and is rejected by the
        // chip engine before this codec is called.
        let mut ethernet_header = [0; ETHERNET_HEADER_LEN];
        ethernet_header.copy_from_slice(&self.ethernet[..ETHERNET_HEADER_LEN]);
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            self.user_priority,
            self.peer_qos,
            false,
        )
        .ok_or(ApDataFrameError::InvalidUserPriority)?;
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        plan.header[1] |= 0x40;
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(CCMP_HEADER_LEN)
            .and_then(|length| length.checked_add(plan.llc_snap.len()))
            .and_then(|length| length.checked_add(self.ethernet.len() - ETHERNET_HEADER_LEN))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_end = header_len + CCMP_HEADER_LEN;
        frame[header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        let llc_end = ccmp_end + plan.llc_snap.len();
        frame[ccmp_end..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(&self.ethernet[ETHERNET_HEADER_LEN..]);
        Ok(required)
    }
}

/// One AP-originated QoS A-MSDU backed by a finite slice of Ethernet frames.
///
/// `ccmp_header = None` selects an Open MPDU. `Some` selects the plaintext
/// CCMP DMA image; hardware encrypts the A-MSDU body and appends its MIC. All
/// subframes must target the exact unicast receiver carried by the outer
/// From-DS header. This stricter production contract prevents a scheduler
/// from coalescing traffic from different AP peer queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAmsduFrame<'a> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: u16,
    pub user_priority: u8,
    pub more_data: bool,
    pub ccmp_header: Option<[u8; CCMP_HEADER_LEN]>,
    pub ethernet_frames: &'a [&'a [u8]],
}

/// Return the complete DMA-resident MPDU length for a bounded AP A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS. `protected`
/// controls only the retained eight-byte CCMP header; the A-MSDU body has the
/// same IEEE 802.11 representation for Open and CCMP epochs.
pub fn ap_amsdu_frame_length(
    ethernet_frames: &[&[u8]],
    protected: bool,
) -> Result<usize, ApDataFrameError> {
    if ethernet_frames.is_empty() {
        return Err(ApDataFrameError::NoAmsduFrames);
    }
    let mut amsdu_length = 0_usize;
    for (index, ethernet) in ethernet_frames.iter().copied().enumerate() {
        if ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        let msdu_length = LLC_SNAP_HEADER_LEN
            .checked_add(ethernet.len() - ETHERNET_HEADER_LEN)
            .ok_or(ApDataFrameError::AmsduTooLong {
                length: usize::MAX,
                maximum: AP_AMSDU_BASELINE_MAX_LEN,
            })?;
        if msdu_length > usize::from(u16::MAX) {
            return Err(ApDataFrameError::AmsduTooLong {
                length: msdu_length,
                maximum: AP_AMSDU_BASELINE_MAX_LEN,
            });
        }
        let subframe_length = AMSDU_SUBFRAME_HEADER_LEN.checked_add(msdu_length).ok_or(
            ApDataFrameError::AmsduTooLong {
                length: usize::MAX,
                maximum: AP_AMSDU_BASELINE_MAX_LEN,
            },
        )?;
        amsdu_length =
            amsdu_length
                .checked_add(subframe_length)
                .ok_or(ApDataFrameError::AmsduTooLong {
                    length: usize::MAX,
                    maximum: AP_AMSDU_BASELINE_MAX_LEN,
                })?;
        if index + 1 != ethernet_frames.len() {
            amsdu_length = amsdu_length
                .checked_add((4 - (subframe_length & 3)) & 3)
                .ok_or(ApDataFrameError::AmsduTooLong {
                    length: usize::MAX,
                    maximum: AP_AMSDU_BASELINE_MAX_LEN,
                })?;
        }
    }
    if amsdu_length > AP_AMSDU_BASELINE_MAX_LEN {
        return Err(ApDataFrameError::AmsduTooLong {
            length: amsdu_length,
            maximum: AP_AMSDU_BASELINE_MAX_LEN,
        });
    }
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(if protected { CCMP_HEADER_LEN } else { 0 })
        .and_then(|length| length.checked_add(amsdu_length))
        .ok_or(ApDataFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: AP_AMSDU_BASELINE_MAX_LEN,
        })
}

impl ApAmsduFrame<'_> {
    /// Encode every Ethernet input into one From-DS QoS A-MSDU.
    ///
    /// Validation and capacity checks complete before the output is mutated.
    /// This lets a packet-number owner first encode with a placeholder CCMP
    /// header, then consume and patch the real PN only after full admission.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.peer[0] & 1 != 0 || self.peer == [0; 6] {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        if self.user_priority > 7 {
            return Err(ApDataFrameError::InvalidUserPriority);
        }
        let Some(first) = self.ethernet_frames.first().copied() else {
            return Err(ApDataFrameError::NoAmsduFrames);
        };
        for ethernet in self.ethernet_frames.iter().copied() {
            if ethernet.len() < ETHERNET_HEADER_LEN {
                return Err(ApDataFrameError::EthernetFrameTooShort);
            }
            if ethernet[..6] != self.peer {
                return Err(ApDataFrameError::InvalidPeer);
            }
        }
        let required = ap_amsdu_frame_length(self.ethernet_frames, self.ccmp_header.is_some())?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }

        let first_header: [u8; ETHERNET_HEADER_LEN] = first[..ETHERNET_HEADER_LEN]
            .try_into()
            .expect("A-MSDU Ethernet length validated above");
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            first_header,
            self.user_priority,
            true,
            false,
        )
        .ok_or(ApDataFrameError::InvalidUserPriority)?;
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.ccmp_header.is_some() {
            plan.header[1] |= 0x40;
        }
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        plan.header[24] |= 0x80;

        let frame = &mut output[..required];
        frame[..crate::data::IEEE80211_QOS_DATA_HEADER_LEN]
            .copy_from_slice(&plan.header[..crate::data::IEEE80211_QOS_DATA_HEADER_LEN]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let mut offset = crate::data::IEEE80211_QOS_DATA_HEADER_LEN;
        if let Some(ccmp_header) = self.ccmp_header {
            frame[offset..offset + CCMP_HEADER_LEN].copy_from_slice(&ccmp_header);
            offset += CCMP_HEADER_LEN;
        }
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
                frame[offset..offset + padding].fill(0);
                offset += padding;
            }
        }
        debug_assert_eq!(offset, required);
        Ok(required)
    }
}

impl ApProtectedDataFrame<'_> {
    /// Convert an Ethernet-II frame in its owned allocation to an AP-originated
    /// protected QoS MPDU without moving the payload.
    pub fn encode_in_place(
        self,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
    ) -> Result<EncodedApFrame, ApDataFrameError> {
        if !self.peer_qos {
            return Err(ApDataFrameError::InvalidUserPriority);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        if self.user_priority > 7 || ethernet_length < ETHERNET_HEADER_LEN {
            return Err(if self.user_priority > 7 {
                ApDataFrameError::InvalidUserPriority
            } else {
                ApDataFrameError::EthernetFrameTooShort
            });
        }
        let ethernet_end = ethernet_offset.checked_add(ethernet_length).ok_or(
            ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if storage.len() < ethernet_end {
            return Err(ApDataFrameError::OutputTooSmall {
                required: ethernet_end,
            });
        }
        let destination: [u8; 6] = storage[ethernet_offset..ethernet_offset + 6]
            .try_into()
            .expect("six-byte Ethernet destination");
        if destination != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        let source: [u8; 6] = storage[ethernet_offset + 6..ethernet_offset + 12]
            .try_into()
            .expect("six-byte Ethernet source");
        let ether_type =
            u16::from_be_bytes([storage[ethernet_offset + 12], storage[ethernet_offset + 13]]);
        let mut ethernet_header = [0_u8; ETHERNET_HEADER_LEN];
        ethernet_header[..6].copy_from_slice(&destination);
        ethernet_header[6..12].copy_from_slice(&source);
        ethernet_header[12..14].copy_from_slice(&ether_type.to_be_bytes());
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            self.user_priority,
            true,
            false,
        )
        .ok_or(ApDataFrameError::InvalidUserPriority)?;
        plan.header[1] |= 0x40;
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        let header_len = usize::from(plan.header_len);
        let prefix_len = header_len + CCMP_HEADER_LEN + plan.llc_snap.len();
        let headroom = prefix_len - ETHERNET_HEADER_LEN;
        let frame_offset = ethernet_offset
            .checked_sub(headroom)
            .ok_or(ApDataFrameError::OutputTooSmall { required: headroom })?;
        let frame_length = ethernet_length + headroom;
        let frame = &mut storage[frame_offset..ethernet_end];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_end = header_len + CCMP_HEADER_LEN;
        frame[header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        frame[ccmp_end..prefix_len].copy_from_slice(&plan.llc_snap);
        Ok(EncodedApFrame {
            offset: frame_offset,
            length: frame_length,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementRequest<'a> {
    OpenAuthentication {
        peer: [u8; 6],
    },
    Association {
        peer: [u8; 6],
        security: ApAssociationSecurityObservation<'a>,
        /// Highest legacy rate shared with the B/G ERP rate set advertised
        /// by this AP, in 500-kbit/s units. Zero means no common rate.
        maximum_legacy_rate_500kbps: u8,
        /// Complete one-stream HT receive facts supplied by the peer.
        ht_capabilities: Option<HtPeerCapabilities>,
        /// The peer supplied WMM information or HT, both of which imply QoS
        /// data framing for this profile.
        qos_supported: bool,
    },
    Disassociation {
        peer: [u8; 6],
        reason: u16,
    },
    Deauthentication {
        peer: [u8; 6],
        reason: u16,
    },
    BlockAck {
        peer: [u8; 6],
        action: BlockAckAction,
    },
}

/// Exact on-air security facts from one Association Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationSecurityObservation<'a> {
    pub privacy: bool,
    pub rsn_ie: Option<&'a [u8]>,
    pub rsn_ie_count: u8,
    pub rsnxe: Option<&'a [u8]>,
    pub rsnxe_count: u8,
    /// A legacy WPA vendor IE (00:50:f2:01) was present. It is never an
    /// acceptable substitute for RSN and makes a mixed request invalid.
    pub legacy_wpa_present: bool,
    /// At least one IE header or payload was truncated. Absence inferred from
    /// a malformed tail is never an Open-security proof.
    pub malformed_elements: bool,
}

/// Parse only management requests addressed to this AP.
///
/// Beacons, responses, foreign BSS traffic and unsupported authentication
/// algorithms are intentionally ignored rather than being promoted into AP
/// state transitions.
pub fn parse_ap_management_request<'a>(
    profile: &Advertisement,
    frame: &'a [u8],
    access_point: [u8; 6],
) -> Option<ApManagementRequest<'a>> {
    if frame.len() < MANAGEMENT_HEADER_LEN
        || frame[4..10] != access_point
        || frame[16..22] != access_point
    {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & 0x000c != 0 {
        return None;
    }
    let subtype = (frame_control >> 4) & 0x0f;
    let peer = frame[10..16].try_into().ok()?;
    match subtype {
        11 => {
            let body = frame.get(24..30)?;
            let algorithm = u16::from_le_bytes([body[0], body[1]]);
            let transaction = u16::from_le_bytes([body[2], body[3]]);
            let status = u16::from_le_bytes([body[4], body[5]]);
            (algorithm == 0 && transaction == 1 && status == 0)
                .then_some(ApManagementRequest::OpenAuthentication { peer })
        }
        0 => {
            let fixed = frame.get(24..28)?;
            let capabilities = u16::from_le_bytes([fixed[0], fixed[1]]);
            let information_elements = frame.get(28..)?;
            let ht_capabilities = ht_peer_capabilities(information_elements);
            Some(ApManagementRequest::Association {
                peer,
                security: association_security_observation(
                    information_elements,
                    capabilities & 0x0010 != 0,
                ),
                maximum_legacy_rate_500kbps: maximum_ap_legacy_rate(profile, information_elements),
                ht_capabilities,
                qos_supported: ht_capabilities.is_some() || supports_wmm(information_elements),
            })
        }
        10 | 12 => {
            let body = frame.get(24..26)?;
            let reason = u16::from_le_bytes([body[0], body[1]]);
            if subtype == 10 {
                Some(ApManagementRequest::Disassociation { peer, reason })
            } else {
                Some(ApManagementRequest::Deauthentication { peer, reason })
            }
        }
        13 => Some(ApManagementRequest::BlockAck {
            peer,
            action: parse_block_ack_action(frame.get(MANAGEMENT_HEADER_LEN..)?)?,
        }),
        _ => None,
    }
}

/// One unprotected AP-originated Action management frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApActionFrame<'a> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: u16,
    pub body: &'a [u8],
}

impl ApActionFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApAssociationResponseError> {
        if self.sequence_number > 0x0fff {
            return Err(ApAssociationResponseError::InvalidSequenceNumber);
        }
        let required = MANAGEMENT_HEADER_LEN.checked_add(self.body.len()).ok_or(
            ApAssociationResponseError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if output.len() < required {
            return Err(ApAssociationResponseError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            0x00d0,
            self.access_point,
            self.peer,
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..].copy_from_slice(self.body);
        Ok(required)
    }
}

fn supports_wmm(bytes: &[u8]) -> bool {
    let mut remaining = bytes;
    while remaining.len() >= 2 {
        let length = usize::from(remaining[1]);
        let Some(record) = remaining.get(..length.saturating_add(2)) else {
            return false;
        };
        if remaining[0] == 221 && length >= 6 && record[2..6] == [0x00, 0x50, 0xf2, 0x02] {
            return true;
        }
        remaining = &remaining[record.len()..];
    }
    false
}

fn association_security_observation(
    bytes: &[u8],
    privacy: bool,
) -> ApAssociationSecurityObservation<'_> {
    let mut remaining = bytes;
    let mut observation = ApAssociationSecurityObservation {
        privacy,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };
    while !remaining.is_empty() {
        if remaining.len() < 2 {
            observation.malformed_elements = true;
            break;
        }
        let length = usize::from(remaining[1]);
        let Some(record) = remaining.get(..length.saturating_add(2)) else {
            observation.malformed_elements = true;
            break;
        };
        if remaining[0] == 48 {
            observation.rsn_ie_count = observation.rsn_ie_count.saturating_add(1);
            observation.rsn_ie.get_or_insert(record);
        }
        if remaining[0] == 244 {
            observation.rsnxe_count = observation.rsnxe_count.saturating_add(1);
            observation.rsnxe.get_or_insert(record);
        }
        if remaining[0] == 221 && length >= 4 && record[2..6] == [0x00, 0x50, 0xf2, 0x01] {
            observation.legacy_wpa_present = true;
        }
        remaining = &remaining[record.len()..];
    }
    observation
}

fn maximum_ap_legacy_rate(profile: &Advertisement, bytes: &[u8]) -> u8 {
    let mut maximum = 0;
    let mut remaining = bytes;
    while let Some((&id, tail)) = remaining.split_first() {
        let Some((&length, payload)) = tail.split_first() else {
            break;
        };
        let length = usize::from(length);
        let Some(value) = payload.get(..length) else {
            break;
        };
        if id == 1 || id == 50 {
            for encoded in value {
                let rate = encoded & 0x7f;
                if profile.legacy_rates.supports(rate) {
                    maximum = maximum.max(rate);
                }
            }
        }
        remaining = &payload[length..];
    }
    maximum
}

/// Encode the response to one successful or rejected Open System request.
pub fn write_open_authentication_response(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    management_sequence: u16,
) -> Result<usize, ApAssociationResponseError> {
    if management_sequence > 0x0fff {
        return Err(ApAssociationResponseError::InvalidSequenceNumber);
    }
    if output.len() < AP_AUTHENTICATION_RESPONSE_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_AUTHENTICATION_RESPONSE_LEN,
        });
    }
    let frame = &mut output[..AP_AUTHENTICATION_RESPONSE_LEN];
    frame.fill(0);
    write_management_header(frame, 0x00b0, access_point, peer, management_sequence);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame[28..30].copy_from_slice(&status.to_le_bytes());
    Ok(AP_AUTHENTICATION_RESPONSE_LEN)
}

/// Encode the AP HT association response as one complete MPDU.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame writer keeps each independently reviewed 802.11 field explicit at its boundary"
)]
pub fn write_ht_association_response_frame(
    profile: &Advertisement,
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    association_id: u16,
    management_sequence: u16,
    channel: WifiChannel,
    peer_ht: Option<HtPeerCapabilities>,
) -> Result<usize, ApAssociationResponseError> {
    write_ht_association_response_frame_for_security(
        profile,
        output,
        access_point,
        peer,
        status,
        association_id,
        management_sequence,
        channel,
        peer_ht,
        WifiSecurityMode::Wpa2Personal,
    )
}

/// Encode an association response with capability privacy matching the exact
/// AP mode. The WPA2 wrapper above retains its original bytes.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame writer keeps each independently reviewed 802.11 field explicit at its boundary"
)]
pub fn write_ht_association_response_frame_for_security(
    profile: &Advertisement,
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    association_id: u16,
    management_sequence: u16,
    channel: WifiChannel,
    peer_ht: Option<HtPeerCapabilities>,
    security: WifiSecurityMode,
) -> Result<usize, ApAssociationResponseError> {
    if management_sequence > 0x0fff {
        return Err(ApAssociationResponseError::InvalidSequenceNumber);
    }
    if output.len() < AP_ASSOCIATION_RESPONSE_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_ASSOCIATION_RESPONSE_LEN,
        });
    }
    let frame = &mut output[..AP_ASSOCIATION_RESPONSE_LEN];
    frame.fill(0);
    write_management_header(frame, 0x0010, access_point, peer, management_sequence);
    let body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN] = (&mut frame[MANAGEMENT_HEADER_LEN..])
        .try_into()
        .expect("checked association response body length");
    write_ht_association_response(profile, body, status, association_id, channel, peer_ht)?;
    if security == WifiSecurityMode::Open {
        body[..2].copy_from_slice(&profile.capabilities(security).to_le_bytes());
    }
    Ok(AP_ASSOCIATION_RESPONSE_LEN)
}

/// Encode one AP-originated peer teardown frame.
pub fn write_ap_peer_disconnect(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    kind: ApPeerDisconnectKind,
    reason: u16,
    management_sequence: u16,
) -> Result<usize, ApAssociationResponseError> {
    if management_sequence > 0x0fff {
        return Err(ApAssociationResponseError::InvalidSequenceNumber);
    }
    if output.len() < AP_PEER_DISCONNECT_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_PEER_DISCONNECT_LEN,
        });
    }
    let frame = &mut output[..AP_PEER_DISCONNECT_LEN];
    frame.fill(0);
    let frame_control = match kind {
        ApPeerDisconnectKind::Disassociation => 0x00a0,
        ApPeerDisconnectKind::Deauthentication => 0x00c0,
    };
    write_management_header(
        frame,
        frame_control,
        access_point,
        peer,
        management_sequence,
    );
    frame[MANAGEMENT_HEADER_LEN..AP_PEER_DISCONNECT_LEN].copy_from_slice(&reason.to_le_bytes());
    Ok(AP_PEER_DISCONNECT_LEN)
}

fn write_management_header(
    frame: &mut [u8],
    frame_control: u16,
    access_point: [u8; 6],
    peer: [u8; 6],
    management_sequence: u16,
) {
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&peer);
    frame[10..16].copy_from_slice(&access_point);
    frame[16..22].copy_from_slice(&access_point);
    frame[22..24].copy_from_slice(&(management_sequence << 4).to_le_bytes());
}

/// Build the finite AP HT association response body.
pub fn write_ht_association_response(
    profile: &Advertisement,
    body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: u16,
    association_id: u16,
    channel: WifiChannel,
    peer_ht: Option<HtPeerCapabilities>,
) -> Result<(), ApAssociationResponseError> {
    if status == 0 && association_id & 0x3fff == 0 {
        return Err(ApAssociationResponseError::MissingAssociationId);
    }
    body[..AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN].fill(0);
    body[..2].copy_from_slice(
        &profile
            .capabilities(WifiSecurityMode::Wpa2Personal)
            .to_le_bytes(),
    );
    body[6..8].copy_from_slice(&[1, 8]);
    body[8..16].copy_from_slice(profile.legacy_rates.supported());
    body[16..18].copy_from_slice(&[50, 4]);
    body[18..22].copy_from_slice(profile.legacy_rates.extended());
    body[22..25].copy_from_slice(&[42, 1, profile.erp_information()]);
    body[25..AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN].copy_from_slice(&profile.wmm.element());
    let ht_capability = ht_capability_ie_for_peer(profile.ht, channel, peer_ht);
    let ht_operation = ht_operation_ie(channel);
    let ht_capability_end = AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN + ht_capability.len();
    body[AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN..ht_capability_end]
        .copy_from_slice(&ht_capability);
    body[ht_capability_end..].copy_from_slice(&ht_operation);
    body[2..4].copy_from_slice(&status.to_le_bytes());
    let encoded_association_id = if status == 0 {
        0xc000 | (association_id & 0x3fff)
    } else {
        0
    };
    body[4..6].copy_from_slice(&encoded_association_id.to_le_bytes());
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPowerSaveObservation {
    Sleeping { peer: [u8; 6] },
    Active { peer: [u8; 6] },
    PsPoll { peer: [u8; 6], association_id: u16 },
}

/// Parse the RX-derived power-save edge used by the migrated AP owner.
///
/// Association validation is intentionally separate: the AP peer table must
/// confirm that `peer` currently owns the reported association ID.
pub fn observe_ap_power_save(frame: &[u8]) -> Option<ApPowerSaveObservation> {
    if frame.len() < 2 {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    let frame_type = (frame_control >> 2) & 3;
    let subtype = (frame_control >> 4) & 0x0f;

    if frame_type == 1 && subtype == 10 && frame.len() >= 16 {
        let raw_association_id = u16::from_le_bytes([frame[2], frame[3]]);
        let association_id = raw_association_id & 0x3fff;
        if raw_association_id & 0xc000 != 0xc000 || association_id == 0 {
            return None;
        }
        return Some(ApPowerSaveObservation::PsPoll {
            peer: frame[10..16].try_into().ok()?,
            association_id,
        });
    }

    if frame.len() < 24 {
        return None;
    }
    let to_ds = frame_control & 0x0100 != 0;
    let from_ds = frame_control & 0x0200 != 0;
    if frame_type != 2 || !to_ds || from_ds {
        return None;
    }
    let peer = frame[10..16].try_into().ok()?;
    if frame_control & 0x1000 != 0 {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

/// Parse an AP power-save edge while binding PS-Poll Receiver Address to the
/// active BSSID. Protected data admission validates its address mapping in the
/// data owner; the unprotected control frame needs this explicit check before
/// its TA/AID tuple can reserve buffered traffic.
pub fn observe_ap_power_save_for_access_point(
    frame: &[u8],
    access_point: [u8; 6],
) -> Option<ApPowerSaveObservation> {
    let observation = observe_ap_power_save(frame)?;
    if matches!(observation, ApPowerSaveObservation::PsPoll { .. })
        && frame.get(4..10) != Some(access_point.as_slice())
    {
        return None;
    }
    Some(observation)
}

/// Parse the exact legacy Null Data frame used by an associated station to
/// publish a power-management transition.
///
/// Unlike ordinary payload data, Null Data has no Ethernet body whose
/// successful decapsulation can establish the peer identity. Admission is
/// therefore deliberately complete at this boundary: the MPDU must be the
/// exact 24-byte To-DS Null Data geometry emitted by `StaNullDataFrame`, both
/// BSSID address fields must name the active AP, the transmitter must be a
/// valid unicast address, and fragmentation is rejected. Only the Retry and
/// Power Management flag bits may vary.
pub fn observe_ap_null_data_power_save_for_access_point(
    frame: &[u8],
    access_point: [u8; 6],
) -> Option<ApPowerSaveObservation> {
    const NULL_DATA_TO_DS_FRAME_CONTROL: u16 = 0x0148;
    const RETRY: u16 = 0x0800;
    const POWER_MANAGEMENT: u16 = 0x1000;

    if frame.len() != STA_NULL_DATA_FRAME_LEN {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & !(RETRY | POWER_MANAGEMENT) != NULL_DATA_TO_DS_FRAME_CONTROL
        || frame[4..10] != access_point
        || frame[16..22] != access_point
        || frame[22] & 0x0f != 0
    {
        return None;
    }
    let peer: [u8; 6] = frame[10..16].try_into().ok()?;
    if peer == [0; 6] || peer == [0xff; 6] || peer[0] & 1 != 0 {
        return None;
    }
    if frame_control & POWER_MANAGEMENT != 0 {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

#[cfg(test)]
mod tests;
