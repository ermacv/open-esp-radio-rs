//! Allocation-free STA authentication and association protocol.
//!
//! This module owns only IEEE 802.11 frame construction, response parsing,
//! and WPA2-Personal RSN selection. The caller retains the output buffer and
//! decides when to submit it to hardware, arm a deadline, or retry.

use crate::{
    ccmp::CCMP_HEADER_LEN,
    data::{DataInterfaceRole, ETHERNET_HEADER_LEN, plan_data_encapsulation},
    management::{MANAGEMENT_HEADER_LEN, MAX_SSID_LEN, MAX_SUPPORTED_RATES_LEN},
    scan::ScanRecord,
};

const OPEN_AUTHENTICATION_FRAME_CONTROL: u16 = 0x00b0;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFrameError {
    InvalidBssid,
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
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
pub struct AssociationRequest<'a> {
    pub source: [u8; 6],
    pub access_point: &'a ScanRecord,
    pub sequence_number: u16,
    pub listen_interval: u16,
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
        let required = MANAGEMENT_HEADER_LEN
            + ASSOCIATION_FIXED_BODY_LEN
            + 2
            + ssid.len()
            + 2
            + first_rates_len
            + usize::from(extended_rates_len != 0) * (2 + extended_rates_len)
            + selected_rsn.as_bytes().len();
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
        debug_assert_eq!(offset, required);
        Ok(required)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationRequestError {
    Frame(StationFrameError),
    Security(StaSecurityError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationResponse {
    pub capability_info: u16,
    pub status_code: u16,
    pub association_id: u16,
    pub ht_capability: bool,
    pub wmm: bool,
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
    let mut wmm = false;
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
        wmm |= id == 221 && length >= 6 && value.get(..4) == Some(&[0x00, 0x50, 0xf2, 0x02]);
        offset = end;
    }

    Some(AssociationResponse {
        capability_info: read_u16(frame, 24)?,
        status_code: read_u16(frame, 26)?,
        association_id: read_u16(frame, 28)? & 0x3fff,
        ht_capability,
        // An AP that returned HT Capability accepted the WMM/QoS data path,
        // even if a bounded RX prefix omitted a later vendor WMM element.
        wmm: wmm || ht_capability,
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
    fn open_ap_needs_no_security_ie() {
        let record = ScanRecord {
            bssid: BSSID,
            supported_rates: [0x82; 8],
            supported_rates_len: 1,
            ..ScanRecord::EMPTY
        };
        assert!(select_wpa2_psk_rsn(&record).unwrap().as_bytes().is_empty());
    }
}
