//! Role-neutral validated receive view for hardware-deciphered data MPDUs.
//!
//! Peer admission, authorization, duplicate history and reorder policy remain
//! with AP/STA. Once a role admits the public header, both roles must decode
//! the S31 CCMP result and Ethernet payload through this single path.

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxError, RxIngressConfig, RxPhyInfo, RxSegment, decode_normalized_rx_metadata, view_ccmp_data,
};
use open_esp_radio_ieee80211::data::{
    DataDecapError, DataDecapsulation, DataInterfaceRole, decapsulate_data_frames,
};
use open_esp_radio_wifi_softmac::{MacRxCryptoStatus, MacRxEvidence, MacRxMetadata};

const RETRY: u16 = 0x0800;
const QOS_SUBTYPE: u16 = 0x0080;

#[derive(Clone, Copy, Debug)]
pub struct ProtectedDataRxView<'frame> {
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

/// Validate the chip RX/CCMP state and recover the role-independent public
/// ordering identity. No peer state is read or mutated here.
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
