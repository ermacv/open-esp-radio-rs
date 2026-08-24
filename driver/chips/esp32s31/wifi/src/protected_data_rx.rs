//! Role-neutral validated receive view for hardware-deciphered data MPDUs.
//!
//! Peer admission, authorization, duplicate history and reorder policy remain
//! with AP/STA. Once a role admits the public header, both roles must decode
//! the S31 CCMP result and Ethernet payload through this single path.

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxError, RxIngressConfig, RxPhyInfo, RxSegment, decode_normalized_rx_metadata, view_ccmp_data,
    view_normalized_rx_frame,
};
use open_esp_radio_ieee80211::ccmp::CcmpHeader;
use open_esp_radio_ieee80211::data::{
    DataDecapError, DataDecapsulation, DataInterfaceRole, decapsulate_data_frames,
};
use open_esp_radio_ieee80211::fragmentation::{
    OpenDataFragment, OpenDataFragmentError, parse_open_data_fragment,
};
use open_esp_radio_wifi_softmac::{MacRxCryptoStatus, MacRxEvidence, MacRxMetadata};

const RETRY: u16 = 0x0800;
const QOS_SUBTYPE: u16 = 0x0080;
const DATA_TYPE: u16 = 0x0008;
const MORE_FRAGMENTS: u16 = 0x0400;
const PROTECTED: u16 = 0x4000;
const ORDERED: u16 = 0x8000;
const TO_FROM_DS: u16 = 0x0300;

#[derive(Clone, Copy, Debug)]
pub struct ProtectedDataRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub frame_control: u16,
    pub sequence_control: u16,
    pub tid: Option<u8>,
    pub retry: bool,
    pub ccmp_header: CcmpHeader,
    payload_offset: usize,
    payload_length: usize,
    metadata: MacRxMetadata<RxPhyInfo>,
}

/// Validate the chip RX/CCMP state and recover the role-independent public
/// ordering identity. No peer state is read or mutated here.
#[inline(always)]
pub fn view_protected_data(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
) -> Result<ProtectedDataRxView<'_>, RxError> {
    let data = view_ccmp_data(&segment, ingress)?;
    let mpdu = &data.mpdu[..data.frame.mpdu.length];
    if mpdu.len() < 24 {
        return Err(RxError::Bounds);
    }
    let mut metadata = decode_normalized_rx_metadata(segment.buffer).ok_or(RxError::Metadata)?;
    // `view_ccmp_data` admits only the successful S31 RX state and rejects
    // the dedicated MIC-failure state before exposing plaintext.
    metadata.crypto =
        MacRxEvidence::HardwareObserved(MacRxCryptoStatus::DecryptedAndIntegrityVerified);
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
    let tid = if frame_control & QOS_SUBTYPE != 0 && mpdu.len() >= 26 {
        Some(mpdu[24] & 0x0f)
    } else {
        None
    };
    Ok(ProtectedDataRxView {
        raw: segment.buffer,
        mpdu,
        frame_control,
        sequence_control,
        tid,
        retry: frame_control & RETRY != 0,
        ccmp_header: data.frame.ccmp_header,
        payload_offset: data.frame.payload_offset,
        payload_length: data.frame.payload_length,
        metadata,
    })
}

pub struct ProtectedDataDecapsulation<'frame> {
    pub frames: DataDecapsulation<'frame>,
    pub raw: &'frame [u8],
    pub amsdu: bool,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

impl<'frame> ProtectedDataRxView<'frame> {
    /// Apply only the portable STA/AP address mapping after role admission.
    #[inline(always)]
    pub fn decapsulate(
        self,
        role: DataInterfaceRole,
    ) -> Result<ProtectedDataDecapsulation<'frame>, DataDecapError> {
        let frames =
            decapsulate_data_frames(role, self.mpdu, self.payload_offset, self.payload_length)?;
        let amsdu = frames.is_amsdu();
        let mut metadata = self.metadata;
        metadata.amsdu = MacRxEvidence::ProtocolValidated(amsdu);
        Ok(ProtectedDataDecapsulation {
            frames,
            raw: self.raw,
            amsdu,
            metadata,
        })
    }
}

/// Validated plaintext view of one complete unprotected data MPDU.
#[derive(Clone, Copy, Debug)]
pub struct UnprotectedDataRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub frame_control: u16,
    pub sequence_control: u16,
    pub tid: Option<u8>,
    pub retry: bool,
    payload_offset: usize,
    payload_length: usize,
    metadata: MacRxMetadata<RxPhyInfo>,
}

/// Validate one contiguous Open-network data MPDU without accepting a
/// protected, fragmented, truncated or non-data unit.
#[inline(always)]
pub fn view_unprotected_data(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
) -> Result<UnprotectedDataRxView<'_>, RxError> {
    let normalized = view_normalized_rx_frame(&segment, ingress)?;
    let mpdu = normalized.mpdu;
    if normalized.logical_length != mpdu.len() || mpdu.len() < 24 {
        return Err(RxError::Bounds);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    let subtype = frame_control & 0x00fc;
    if subtype != DATA_TYPE && subtype != DATA_TYPE | QOS_SUBTYPE {
        return Err(RxError::Ignored);
    }
    let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
    if frame_control & (MORE_FRAGMENTS | PROTECTED | ORDERED) != 0 || sequence_control & 0x000f != 0
    {
        return Err(RxError::Unsupported);
    }
    let qos = frame_control & QOS_SUBTYPE != 0;
    let mut payload_offset = 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
    if qos {
        payload_offset += 2;
    }
    if mpdu.len() < payload_offset {
        return Err(RxError::Bounds);
    }
    let tid = qos.then(|| mpdu[payload_offset - 2] & 0x0f);
    let mut metadata = normalized.metadata;
    metadata.crypto = MacRxEvidence::ProtocolValidated(MacRxCryptoStatus::Unprotected);
    Ok(UnprotectedDataRxView {
        raw: segment.buffer,
        mpdu,
        frame_control,
        sequence_control,
        tid,
        retry: frame_control & RETRY != 0,
        payload_offset,
        payload_length: mpdu.len() - payload_offset,
        metadata,
    })
}

impl<'frame> UnprotectedDataRxView<'frame> {
    #[inline(always)]
    pub fn decapsulate(
        self,
        role: DataInterfaceRole,
    ) -> Result<ProtectedDataDecapsulation<'frame>, DataDecapError> {
        let frames =
            decapsulate_data_frames(role, self.mpdu, self.payload_offset, self.payload_length)?;
        let amsdu = frames.is_amsdu();
        let mut metadata = self.metadata;
        metadata.amsdu = MacRxEvidence::ProtocolValidated(amsdu);
        Ok(ProtectedDataDecapsulation {
            frames,
            raw: self.raw,
            amsdu,
            metadata,
        })
    }
}

/// Validated Open-network view of one fragmented data MPDU.
///
/// Unlike [`UnprotectedDataRxView`], this value cannot be directly
/// decapsulated. Its fragment token must cross the fixed-capacity reassembly
/// owner before any Ethernet payload is exposed.
#[derive(Clone, Copy, Debug)]
pub struct UnprotectedDataFragmentRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub fragment: OpenDataFragment<'frame>,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnprotectedDataFragmentRxError {
    Radio(RxError),
    Fragment(OpenDataFragmentError),
}

/// Validate normalized S31 receive state and strictly parse one Open data
/// fragment for the selected STA/AP address mapping.
#[inline(always)]
pub fn view_unprotected_data_fragment(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
    role: DataInterfaceRole,
) -> Result<UnprotectedDataFragmentRxView<'_>, UnprotectedDataFragmentRxError> {
    let normalized = view_normalized_rx_frame(&segment, ingress)
        .map_err(UnprotectedDataFragmentRxError::Radio)?;
    let mpdu = normalized.mpdu;
    if normalized.logical_length != mpdu.len() {
        return Err(UnprotectedDataFragmentRxError::Radio(RxError::Bounds));
    }
    let fragment =
        parse_open_data_fragment(role, mpdu).map_err(UnprotectedDataFragmentRxError::Fragment)?;
    let mut metadata = normalized.metadata;
    metadata.crypto = MacRxEvidence::ProtocolValidated(MacRxCryptoStatus::Unprotected);
    metadata.amsdu = MacRxEvidence::ProtocolValidated(false);
    Ok(UnprotectedDataFragmentRxView {
        raw: segment.buffer,
        mpdu,
        fragment,
        metadata,
    })
}
