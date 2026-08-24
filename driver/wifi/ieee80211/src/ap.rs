//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms form the source-owned AP protocol boundary.
//! Association ownership, deferred
//! queues and wakeups stay with the radio owner.

use crate::{
    block_ack::{BlockAckAction, parse_block_ack_action},
    ccmp::CCMP_HEADER_LEN,
    channel::WifiChannel,
    data::{DataInterfaceRole, ETHERNET_HEADER_LEN, plan_data_encapsulation},
    ht::{
        HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN, HtPeerCapabilities, ht_capability_ie_for_peer,
        ht_operation_ie, ht_peer_capabilities,
    },
    security::WifiSecurityMode,
    station_power_save::STA_NULL_DATA_FRAME_LEN,
};

const AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN: usize = 51;
pub const AP_ASSOCIATION_RESPONSE_BODY_LEN: usize =
    AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN + HT_CAPABILITY_IE_LEN + HT_OPERATION_IE_LEN;
pub const AP_AUTHENTICATION_RESPONSE_LEN: usize = 30;
pub const AP_PEER_DISCONNECT_LEN: usize = MANAGEMENT_HEADER_LEN + 2;
pub const AP_ASSOCIATION_RESPONSE_LEN: usize = 24 + AP_ASSOCIATION_RESPONSE_BODY_LEN;

const MANAGEMENT_HEADER_LEN: usize = 24;

const AP_LEGACY_ASSOCIATION_RESPONSE: [u8; AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN] = [
    0x31, 0x04, 0x00, 0x00, 0x01, 0xc0, 0x01, 0x08, 0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60,
    0x32, 0x04, 0x6c, 0x12, 0x24, 0x48, 0x2a, 0x01, 0x00, 0xdd, 0x18, 0x00, 0x50, 0xf2, 0x02, 0x01,
    0x01, 0x04, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4, 0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62,
    0x32, 0x2f, 0x00,
];

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
    OutputTooSmall { required: usize },
}

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
                maximum_legacy_rate_500kbps: maximum_ap_legacy_rate(information_elements),
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
        if remaining[0] == 221 && length >= 4 && record[2..6] == [0x00, 0x50, 0xf2, 0x01] {
            observation.legacy_wpa_present = true;
        }
        remaining = &remaining[record.len()..];
    }
    observation
}

const AP_BG_LEGACY_RATES_500KBPS: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];

fn maximum_ap_legacy_rate(bytes: &[u8]) -> u8 {
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
                if AP_BG_LEGACY_RATES_500KBPS.contains(&rate) {
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
    write_ht_association_response(body, status, association_id, channel, peer_ht)?;
    if security == WifiSecurityMode::Open {
        body[..2].copy_from_slice(&0x0421_u16.to_le_bytes());
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
    body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: u16,
    association_id: u16,
    channel: WifiChannel,
    peer_ht: Option<HtPeerCapabilities>,
) -> Result<(), ApAssociationResponseError> {
    if status == 0 && association_id & 0x3fff == 0 {
        return Err(ApAssociationResponseError::MissingAssociationId);
    }
    body[..AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN]
        .copy_from_slice(&AP_LEGACY_ASSOCIATION_RESPONSE);
    let ht_capability = ht_capability_ie_for_peer(channel, peer_ht);
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
mod tests {
    use super::*;

    #[test]
    fn ap_eapol_data_frame_uses_from_ds_address_mapping() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut output = [0; 64];
        let len = ApDataFrame {
            access_point,
            destination: peer,
            sequence_number: 9,
            ether_type: 0x888e,
            payload: &[1, 2, 3],
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(len, 24 + 8 + 3);
        assert_eq!(&output[..2], &0x0208_u16.to_le_bytes());
        assert_eq!(&output[4..10], &peer);
        assert_eq!(&output[10..16], &access_point);
        assert_eq!(&output[16..22], &access_point);
        assert_eq!(&output[22..24], &0x0090_u16.to_le_bytes());
        assert_eq!(&output[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        assert_eq!(&output[32..35], &[1, 2, 3]);
    }

    #[test]
    fn protected_ap_frame_owns_from_ds_ccmp_and_plaintext_boundary() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let source = [2, 0, 0, 0, 0, 3];
        let mut ethernet = [0_u8; 18];
        ethernet[..6].copy_from_slice(&peer);
        ethernet[6..12].copy_from_slice(&source);
        ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        ethernet[14..].copy_from_slice(&[1, 2, 3, 4]);
        let ccmp = [3, 0, 0, 0x20, 0, 0, 0, 0];
        let mut output = [0; 96];
        let len = ApProtectedDataFrame {
            access_point,
            peer,
            sequence_number: 7,
            user_priority: 0,
            peer_qos: true,
            more_data: false,
            ccmp_header: ccmp,
            ethernet: &ethernet,
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(len, 26 + 8 + 8 + 4);
        assert_eq!(&output[..2], &0x4288_u16.to_le_bytes());
        assert_eq!(&output[4..10], &peer);
        assert_eq!(&output[10..16], &access_point);
        assert_eq!(&output[16..22], &source);
        assert_eq!(&output[22..24], &0x0070_u16.to_le_bytes());
        assert_eq!(&output[26..34], &ccmp);
        assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
        assert_eq!(&output[42..46], &[1, 2, 3, 4]);
    }

    #[test]
    fn protected_ap_frame_rejects_a_destination_outside_pairwise_owner() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut ethernet = [0_u8; 14];
        ethernet[..6].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
        let mut output = [0; 64];
        assert_eq!(
            ApProtectedDataFrame {
                access_point,
                peer,
                sequence_number: 0,
                user_priority: 0,
                peer_qos: false,
                more_data: false,
                ccmp_header: [0; 8],
                ethernet: &ethernet,
            }
            .encode(&mut output),
            Err(ApDataFrameError::InvalidPeer)
        );
    }

    #[test]
    fn protected_ap_qos_frame_encodes_in_network_headroom_without_payload_copy() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let source = [2, 0, 0, 0, 0, 3];
        let ethernet_offset = 40;
        let mut storage = [0_u8; 96];
        storage[ethernet_offset..ethernet_offset + 6].copy_from_slice(&peer);
        storage[ethernet_offset + 6..ethernet_offset + 12].copy_from_slice(&source);
        storage[ethernet_offset + 12..ethernet_offset + 14]
            .copy_from_slice(&0x0800_u16.to_be_bytes());
        storage[ethernet_offset + 14..ethernet_offset + 18].copy_from_slice(&[1, 2, 3, 4]);
        let encoded = ApProtectedDataFrame {
            access_point,
            peer,
            sequence_number: 7,
            user_priority: 0,
            peer_qos: true,
            more_data: false,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            ethernet: &[],
        }
        .encode_in_place(&mut storage, ethernet_offset, 18)
        .unwrap();
        assert_eq!(
            encoded.offset,
            ethernet_offset - AP_PROTECTED_QOS_ETHERNET_OVERHEAD
        );
        assert_eq!(encoded.length, 18 + AP_PROTECTED_QOS_ETHERNET_OVERHEAD);
        assert_eq!(
            &storage[encoded.offset..encoded.offset + 2],
            &0x4288_u16.to_le_bytes()
        );
        assert_eq!(
            &storage[ethernet_offset + 14..ethernet_offset + 18],
            &[1, 2, 3, 4]
        );
    }

    #[test]
    fn ap_action_frame_and_parser_preserve_per_peer_addba_identity() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let body = [3, 1, 7, 0, 0, 0x02, 0x08, 0, 0];
        let mut frame = [0_u8; 40];
        let length = ApActionFrame {
            access_point,
            peer,
            sequence_number: 9,
            body: &body,
        }
        .encode(&mut frame)
        .unwrap();
        // Reverse direction for the peer-originated response parsed by AP.
        frame[4..10].copy_from_slice(&access_point);
        frame[10..16].copy_from_slice(&peer);
        assert!(matches!(
            parse_ap_management_request(&frame[..length], access_point),
            Some(ApManagementRequest::BlockAck {
                peer: parsed_peer,
                action: BlockAckAction::AddbaResponse {
                    dialog_token: 7,
                    tid: 0,
                    window: 32,
                    ..
                },
            }) if parsed_peer == peer
        ));
    }

    #[test]
    fn association_response_owns_status_aid_and_ht_channel_capability() {
        let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
        let ht20 = WifiChannel::mhz20(6).unwrap();
        write_ht_association_response(&mut body, 17, 0x0123, ht20, None).unwrap();
        assert_eq!(&body[2..4], &17_u16.to_le_bytes());
        assert_eq!(&body[4..6], &[0, 0]);
        write_ht_association_response(&mut body, 0, 1, ht20, None).unwrap();
        assert_eq!(&body[4..6], &0xc001_u16.to_le_bytes());
        assert!(body.windows(2).any(|window| window == [45, 26]));
        assert!(body.windows(3).any(|window| window == [61, 22, 6]));

        let mut peer_ht_record = crate::ht::ht_capability_ie(ht20);
        peer_ht_record[4] = 0x17;
        let peer_ht = ht_peer_capabilities(&peer_ht_record).unwrap();
        write_ht_association_response(&mut body, 0, 1, ht20, Some(peer_ht)).unwrap();
        assert_eq!(
            body[AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN + 4],
            0x17,
            "association response must carry the vendor-negotiated peer spacing"
        );

        assert_eq!(
            write_ht_association_response(&mut body, 0, 0, ht20, None),
            Err(ApAssociationResponseError::MissingAssociationId)
        );

        let ht40 =
            WifiChannel::new_2_4_ghz(6, crate::channel::WifiChannelWidth::Mhz40Below).unwrap();
        write_ht_association_response(&mut body, 0, 1, ht40, None).unwrap();
        assert!(body.windows(4).any(|window| window == [45, 26, 0x6e, 0x10]));
        assert!(body.windows(4).any(|window| window == [61, 22, 6, 0x07]));
    }

    #[test]
    fn peer_disconnect_frames_own_subtype_reason_and_sequence() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut output = [0; AP_PEER_DISCONNECT_LEN];
        assert_eq!(
            write_ap_peer_disconnect(
                &mut output,
                access_point,
                peer,
                ApPeerDisconnectKind::Disassociation,
                4,
                7,
            ),
            Ok(AP_PEER_DISCONNECT_LEN),
        );
        assert_eq!(&output[..2], &0x00a0_u16.to_le_bytes());
        assert_eq!(&output[4..10], &peer);
        assert_eq!(&output[10..16], &access_point);
        assert_eq!(&output[22..24], &0x0070_u16.to_le_bytes());
        assert_eq!(&output[24..26], &4_u16.to_le_bytes());

        write_ap_peer_disconnect(
            &mut output,
            access_point,
            peer,
            ApPeerDisconnectKind::Deauthentication,
            2,
            8,
        )
        .unwrap();
        assert_eq!(&output[..2], &0x00c0_u16.to_le_bytes());
        assert_eq!(&output[24..26], &2_u16.to_le_bytes());
    }

    #[test]
    fn tim_bitmap_update_matches_aid_bit_selection() {
        use crate::beacon::{TimAssociationId, TimVirtualBitmap};

        let mut bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
        bitmap.set(TimAssociationId::new(7).unwrap(), true).unwrap();
        bitmap.set(TimAssociationId::new(8).unwrap(), true).unwrap();
        bitmap
            .set(TimAssociationId::new(15).unwrap(), true)
            .unwrap();
        assert_eq!(bitmap.partial().bitmap_offset(), 0);
        assert_eq!(bitmap.partial().octets(), &[0x80, 0x81]);
    }

    #[test]
    fn observes_only_to_ds_data_power_state() {
        let peer = [1, 2, 3, 4, 5, 6];
        let mut frame = [0_u8; 24];
        frame[10..16].copy_from_slice(&peer);
        frame[..2].copy_from_slice(&0x1108_u16.to_le_bytes());
        assert_eq!(
            observe_ap_power_save(&frame),
            Some(ApPowerSaveObservation::Sleeping { peer })
        );
        frame[..2].copy_from_slice(&0x0108_u16.to_le_bytes());
        assert_eq!(
            observe_ap_power_save(&frame),
            Some(ApPowerSaveObservation::Active { peer })
        );
        frame[..2].copy_from_slice(&0x0008_u16.to_le_bytes());
        assert_eq!(observe_ap_power_save(&frame), None);
    }

    #[test]
    fn ps_poll_owns_peer_and_association_id() {
        let peer = [1, 2, 3, 4, 5, 6];
        let mut frame = [0_u8; 16];
        frame[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
        frame[2..4].copy_from_slice(&0xc123_u16.to_le_bytes());
        frame[10..16].copy_from_slice(&peer);
        assert_eq!(
            observe_ap_power_save(&frame),
            Some(ApPowerSaveObservation::PsPoll {
                peer,
                association_id: 0x123
            })
        );
    }

    #[test]
    fn parses_only_requests_for_the_owned_bssid() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut authentication = [0_u8; 30];
        authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        authentication[4..10].copy_from_slice(&access_point);
        authentication[10..16].copy_from_slice(&peer);
        authentication[16..22].copy_from_slice(&access_point);
        authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            parse_ap_management_request(&authentication, access_point),
            Some(ApManagementRequest::OpenAuthentication { peer })
        );
        authentication[4] ^= 1;
        assert_eq!(
            parse_ap_management_request(&authentication, access_point),
            None
        );
    }

    #[test]
    fn association_retains_the_highest_common_advertised_legacy_rate() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut association = [0_u8; 42];
        association[4..10].copy_from_slice(&access_point);
        association[10..16].copy_from_slice(&peer);
        association[16..22].copy_from_slice(&access_point);
        association[28..34].copy_from_slice(&[1, 4, 0x82, 0x84, 0x0c, 0x30]);
        association[34..39].copy_from_slice(&[50, 3, 0x48, 0x6c, 0x7f]);
        association[39..42].copy_from_slice(&[48, 1, 0]);
        assert_eq!(
            parse_ap_management_request(&association, access_point),
            Some(ApManagementRequest::Association {
                peer,
                rsn_ie: Some(&association[39..42]),
                maximum_legacy_rate_500kbps: 108,
                ht_capabilities: None,
                qos_supported: false,
            })
        );
    }

    #[test]
    fn association_retains_the_peers_complete_ht40_receive_facts() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut association = [0_u8; 62];
        association[4..10].copy_from_slice(&access_point);
        association[10..16].copy_from_slice(&peer);
        association[16..22].copy_from_slice(&access_point);
        association[28..34].copy_from_slice(&[1, 4, 0x82, 0x84, 0x0c, 0x6c]);
        let channel =
            WifiChannel::new_2_4_ghz(6, crate::channel::WifiChannelWidth::Mhz40Above).unwrap();
        association[34..].copy_from_slice(&crate::ht::ht_capability_ie(channel));

        let Some(ApManagementRequest::Association {
            maximum_legacy_rate_500kbps,
            ht_capabilities: Some(ht),
            qos_supported,
            ..
        }) = parse_ap_management_request(&association, access_point)
        else {
            panic!("complete HT40 association request must parse");
        };
        assert_eq!(maximum_legacy_rate_500kbps, 108);
        assert!(ht.supports_40_mhz());
        assert!(ht.supports_short_guard_interval(crate::channel::WifiChannelWidth::Mhz40Above));
        assert_eq!(ht.highest_rx_mcs(), 7);
        assert!(qos_supported);
    }

    #[test]
    fn complete_response_encoders_own_addresses_sequence_and_status() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut authentication = [0xaa; AP_AUTHENTICATION_RESPONSE_LEN];
        write_open_authentication_response(&mut authentication, access_point, peer, 17, 7).unwrap();
        assert_eq!(&authentication[4..10], &peer);
        assert_eq!(&authentication[10..16], &access_point);
        assert_eq!(&authentication[26..28], &2_u16.to_le_bytes());
        assert_eq!(&authentication[28..30], &17_u16.to_le_bytes());

        let mut association = [0; AP_ASSOCIATION_RESPONSE_LEN];
        write_ht_association_response_frame(
            &mut association,
            access_point,
            peer,
            0,
            0xc001,
            8,
            WifiChannel::mhz20(6).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(&association[..2], &0x0010_u16.to_le_bytes());
        assert_eq!(&association[22..24], &0x0080_u16.to_le_bytes());
        assert_eq!(&association[28..30], &0xc001_u16.to_le_bytes());
    }
}
