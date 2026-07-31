//! Allocation-free HE NDP Announcement parsing and encoding.

const NDPA_FRAME_CONTROL: u16 = 0x0054;
const NDPA_HEADER_SIZE: usize = 17;
const HE_STA_INFO_SIZE: usize = 4;
const HE_MARKER: u8 = 1 << 1;
const AID11_MASK: u32 = 0x07ff;
const ACTION_NO_ACK_FRAME_CONTROL: u16 = 0x00e0;
const MANAGEMENT_HEADER_SIZE: usize = 24;
const HE_ACTION_CATEGORY: u8 = 30;
const HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION: u8 = 0;
const HE_MIMO_CONTROL_SIZE: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeNdpaError {
    TooShort,
    NotNdpa,
    NotHe,
    MisalignedStationInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeNdpaEncodingError {
    OutputTooShort,
    NoStations,
    DialogTokenOutOfRange,
    AssociationIdOutOfRange,
    ResourceUnitIndexOutOfRange,
    ReversedResourceUnitRange,
    FeedbackTypeAndNgOutOfRange,
    NcOutOfRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeCompressedBeamformingReportError {
    TooShort,
    NotActionNoAck,
    NotHeCompressedBeamformingAndCqi,
    MissingAverageSnr,
}

/// One FCS-stripped HE NDP Announcement MPDU.
///
/// SOURCE: complete pinned `_oracles/libpp.a[wdev.o]::is_ndpa_to_dut`,
/// size `0x7e`. The blob receives an on-air length including the four-byte
/// FCS, subtracts 21 bytes, divides by four and walks station words from
/// offset 17. Removing the FCS produces the equivalent checked geometry
/// `(mpdu.len() - 17) / 4`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpa<'a> {
    mpdu: &'a [u8],
}

impl<'a> HeNdpa<'a> {
    pub fn parse(mpdu: &'a [u8]) -> Result<Self, HeNdpaError> {
        if mpdu.len() < NDPA_HEADER_SIZE + HE_STA_INFO_SIZE {
            return Err(HeNdpaError::TooShort);
        }
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        if frame_control & 0x00fc != NDPA_FRAME_CONTROL {
            return Err(HeNdpaError::NotNdpa);
        }
        if mpdu[16] & HE_MARKER == 0 {
            return Err(HeNdpaError::NotHe);
        }
        if (mpdu.len() - NDPA_HEADER_SIZE) % HE_STA_INFO_SIZE != 0 {
            return Err(HeNdpaError::MisalignedStationInfo);
        }
        Ok(Self { mpdu })
    }

    pub const fn frame_control(self) -> u16 {
        u16::from_le_bytes([self.mpdu[0], self.mpdu[1]])
    }

    pub const fn duration(self) -> u16 {
        u16::from_le_bytes([self.mpdu[2], self.mpdu[3]])
    }

    pub fn receiver_address(self) -> &'a [u8] {
        &self.mpdu[4..10]
    }

    pub fn transmitter_address(self) -> &'a [u8] {
        &self.mpdu[10..16]
    }

    /// Six-bit sounding-dialog token decoded by the complete blob leaf.
    pub const fn dialog_token(self) -> u8 {
        (self.mpdu[16] >> 2) & 0x3f
    }

    pub fn stations(self) -> HeNdpaStationIterator<'a> {
        HeNdpaStationIterator {
            remaining: &self.mpdu[NDPA_HEADER_SIZE..],
        }
    }

    /// Reproduce the complete blob's local-AID membership test.
    pub fn contains_association_id(self, association_id: u16) -> bool {
        association_id <= AID11_MASK as u16
            && self
                .stations()
                .any(|station| station.association_id() == association_id)
    }
}

/// One raw four-byte HE NDPA STA Info field.
///
/// Only AID11 is named because it is the only subfield consumed by complete
/// `is_ndpa_to_dut`. The remaining 21 bits stay available as a raw word until
/// an independent blob decoder or the 802.11 specification closes them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaStationInfo {
    raw: u32,
}

impl HeNdpaStationInfo {
    pub const fn association_id(self) -> u16 {
        (self.raw & AID11_MASK) as u16
    }

    pub const fn raw(self) -> u32 {
        self.raw
    }

    pub const fn resource_unit_start_index(self) -> u8 {
        ((self.raw >> 11) & 0x7f) as u8
    }

    pub const fn resource_unit_end_index(self) -> u8 {
        ((self.raw >> 18) & 0x7f) as u8
    }

    pub const fn feedback_type_and_ng_encoding(self) -> u8 {
        ((self.raw >> 25) & 0x03) as u8
    }

    pub const fn disambiguation(self) -> bool {
        self.raw & (1 << 27) != 0
    }

    pub const fn codebook_size(self) -> bool {
        self.raw & (1 << 28) != 0
    }

    pub const fn nc_encoding(self) -> u8 {
        ((self.raw >> 29) & 0x07) as u8
    }
}

/// One owned HE NDPA STA Info field.
///
/// SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: monitor capture
/// `esp32s31-he-oracle-fixed-ch11.pcapng`, SHA-256
/// `d50289842bd3cddbcebf3080c049cf6d6b387908b501b6b7333fbfb250e7abde`.
/// Frame 1374 is a complete vendor AP-to-S31 HE20 NDPA. Its STA Info word
/// `0x0820001d` selects AID 29, RU indices 0..8, feedback/Ng encoding zero,
/// disambiguation one, codebook zero and Nc zero. Frame 1376, 14.39 us later,
/// is the S31's HE Compressed Beamforming and CQI Action-No-Ack response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaStationEncoding {
    pub association_id: u16,
    pub resource_unit_start_index: u8,
    pub resource_unit_end_index: u8,
    pub feedback_type_and_ng_encoding: u8,
    pub disambiguation: bool,
    pub codebook_size: bool,
    pub nc_encoding: u8,
}

impl HeNdpaStationEncoding {
    pub const fn encode(self) -> Result<u32, HeNdpaEncodingError> {
        if self.association_id > AID11_MASK as u16 {
            return Err(HeNdpaEncodingError::AssociationIdOutOfRange);
        }
        if self.resource_unit_start_index > 0x7f || self.resource_unit_end_index > 0x7f {
            return Err(HeNdpaEncodingError::ResourceUnitIndexOutOfRange);
        }
        if self.resource_unit_start_index > self.resource_unit_end_index {
            return Err(HeNdpaEncodingError::ReversedResourceUnitRange);
        }
        if self.feedback_type_and_ng_encoding > 0x03 {
            return Err(HeNdpaEncodingError::FeedbackTypeAndNgOutOfRange);
        }
        if self.nc_encoding > 0x07 {
            return Err(HeNdpaEncodingError::NcOutOfRange);
        }
        Ok(self.association_id as u32
            | ((self.resource_unit_start_index as u32) << 11)
            | ((self.resource_unit_end_index as u32) << 18)
            | ((self.feedback_type_and_ng_encoding as u32) << 25)
            | ((self.disambiguation as u32) << 27)
            | ((self.codebook_size as u32) << 28)
            | ((self.nc_encoding as u32) << 29))
    }
}

/// Owned, allocation-free HE NDPA encoder input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaEncoding<'a> {
    pub duration: u16,
    pub receiver_address: [u8; 6],
    pub transmitter_address: [u8; 6],
    pub dialog_token: u8,
    pub stations: &'a [HeNdpaStationEncoding],
}

impl HeNdpaEncoding<'_> {
    pub const fn encoded_len(self) -> usize {
        NDPA_HEADER_SIZE + self.stations.len() * HE_STA_INFO_SIZE
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, HeNdpaEncodingError> {
        if self.stations.is_empty() {
            return Err(HeNdpaEncodingError::NoStations);
        }
        if self.dialog_token > 0x3f {
            return Err(HeNdpaEncodingError::DialogTokenOutOfRange);
        }
        let length = self.encoded_len();
        if output.len() < length {
            return Err(HeNdpaEncodingError::OutputTooShort);
        }

        output[..length].fill(0);
        output[0..2].copy_from_slice(&NDPA_FRAME_CONTROL.to_le_bytes());
        output[2..4].copy_from_slice(&self.duration.to_le_bytes());
        output[4..10].copy_from_slice(&self.receiver_address);
        output[10..16].copy_from_slice(&self.transmitter_address);
        output[16] = (self.dialog_token << 2) | HE_MARKER;
        for (index, station) in self.stations.iter().copied().enumerate() {
            let raw = station.encode()?;
            let offset = NDPA_HEADER_SIZE + index * HE_STA_INFO_SIZE;
            output[offset..offset + HE_STA_INFO_SIZE].copy_from_slice(&raw.to_le_bytes());
        }
        Ok(length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaStationIterator<'a> {
    remaining: &'a [u8],
}

impl Iterator for HeNdpaStationIterator<'_> {
    type Item = HeNdpaStationInfo;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.len() < HE_STA_INFO_SIZE {
            return None;
        }
        let raw = u32::from_le_bytes([
            self.remaining[0],
            self.remaining[1],
            self.remaining[2],
            self.remaining[3],
        ]);
        self.remaining = &self.remaining[HE_STA_INFO_SIZE..];
        Some(HeNdpaStationInfo { raw })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining.len() / HE_STA_INFO_SIZE;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for HeNdpaStationIterator<'_> {}

/// One FCS-stripped HE Compressed Beamforming and CQI Action-No-Ack MPDU.
///
/// SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: the same complete vendor
/// sounding exchange as [`HeNdpaStationEncoding`]. Frame 1376 carries HE MIMO
/// Control `0x0dc4008208`, one average-SNR byte and the complete borrowed
/// feedback matrix. This parser deliberately does not interpret matrix angles;
/// it owns only the fixed control word needed to qualify the hardware report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeCompressedBeamformingReport<'a> {
    mpdu: &'a [u8],
    mimo_control: u64,
    average_snr_end: usize,
}

impl<'a> HeCompressedBeamformingReport<'a> {
    pub fn parse(mpdu: &'a [u8]) -> Result<Self, HeCompressedBeamformingReportError> {
        let fixed_end = MANAGEMENT_HEADER_SIZE + 2 + HE_MIMO_CONTROL_SIZE;
        if mpdu.len() < fixed_end {
            return Err(HeCompressedBeamformingReportError::TooShort);
        }
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        if frame_control & 0x00fc != ACTION_NO_ACK_FRAME_CONTROL {
            return Err(HeCompressedBeamformingReportError::NotActionNoAck);
        }
        if mpdu[MANAGEMENT_HEADER_SIZE] != HE_ACTION_CATEGORY
            || mpdu[MANAGEMENT_HEADER_SIZE + 1] != HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION
        {
            return Err(HeCompressedBeamformingReportError::NotHeCompressedBeamformingAndCqi);
        }
        let control = &mpdu[MANAGEMENT_HEADER_SIZE + 2..fixed_end];
        let mimo_control = u64::from_le_bytes([
            control[0], control[1], control[2], control[3], control[4], 0, 0, 0,
        ]);
        let average_snr_end = fixed_end + ((mimo_control & 0x07) as usize + 1);
        if mpdu.len() < average_snr_end {
            return Err(HeCompressedBeamformingReportError::MissingAverageSnr);
        }
        Ok(Self {
            mpdu,
            mimo_control,
            average_snr_end,
        })
    }

    pub fn receiver_address(self) -> &'a [u8] {
        &self.mpdu[4..10]
    }

    pub fn transmitter_address(self) -> &'a [u8] {
        &self.mpdu[10..16]
    }

    pub const fn sequence_number(self) -> u16 {
        u16::from_le_bytes([self.mpdu[22], self.mpdu[23]]) >> 4
    }

    pub const fn mimo_control(self) -> u64 {
        self.mimo_control
    }

    pub const fn column_count(self) -> u8 {
        (self.mimo_control & 0x07) as u8 + 1
    }

    pub const fn row_count(self) -> u8 {
        ((self.mimo_control >> 3) & 0x07) as u8 + 1
    }

    pub const fn bandwidth_encoding(self) -> u8 {
        ((self.mimo_control >> 6) & 0x03) as u8
    }

    pub const fn grouping(self) -> bool {
        self.mimo_control & (1 << 8) != 0
    }

    pub const fn codebook_information(self) -> bool {
        self.mimo_control & (1 << 9) != 0
    }

    pub const fn feedback_type_encoding(self) -> u8 {
        ((self.mimo_control >> 10) & 0x03) as u8
    }

    pub const fn remaining_feedback_segments(self) -> u8 {
        ((self.mimo_control >> 12) & 0x07) as u8
    }

    pub const fn first_feedback_segment(self) -> bool {
        self.mimo_control & (1 << 15) != 0
    }

    pub const fn resource_unit_start_index(self) -> u8 {
        ((self.mimo_control >> 16) & 0x7f) as u8
    }

    pub const fn resource_unit_end_index(self) -> u8 {
        ((self.mimo_control >> 23) & 0x7f) as u8
    }

    pub const fn sounding_dialog_token(self) -> u8 {
        ((self.mimo_control >> 30) & 0x3f) as u8
    }

    pub const fn reserved(self) -> u8 {
        ((self.mimo_control >> 36) & 0x0f) as u8
    }

    pub fn average_snr(self) -> &'a [u8] {
        &self.mpdu[MANAGEMENT_HEADER_SIZE + 2 + HE_MIMO_CONTROL_SIZE..self.average_snr_end]
    }

    pub fn feedback_matrices(self) -> &'a [u8] {
        &self.mpdu[self.average_snr_end..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RA: [u8; 6] = [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    const TA: [u8; 6] = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];

    fn two_station_frame() -> [u8; 25] {
        let mut frame = [0_u8; 25];
        frame[0] = NDPA_FRAME_CONTROL as u8;
        frame[1] = 0x08;
        frame[2..4].copy_from_slice(&0x1234_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&RA);
        frame[10..16].copy_from_slice(&TA);
        frame[16] = (0x2a << 2) | HE_MARKER;
        frame[17..21].copy_from_slice(&0xa5a5_8123_u32.to_le_bytes());
        frame[21..25].copy_from_slice(&0x5a5a_8456_u32.to_le_bytes());
        frame
    }

    #[test]
    fn decodes_the_complete_blob_geometry_and_aid_membership() {
        let frame = two_station_frame();
        let ndpa = HeNdpa::parse(&frame).unwrap();

        assert_eq!(ndpa.frame_control(), 0x0854);
        assert_eq!(ndpa.duration(), 0x1234);
        assert_eq!(ndpa.receiver_address(), &RA);
        assert_eq!(ndpa.transmitter_address(), &TA);
        assert_eq!(ndpa.dialog_token(), 0x2a);
        assert_eq!(ndpa.stations().len(), 2);
        let mut stations = ndpa.stations();
        assert_eq!(
            stations.next(),
            Some(HeNdpaStationInfo { raw: 0xa5a5_8123 })
        );
        assert_eq!(
            stations.next(),
            Some(HeNdpaStationInfo { raw: 0x5a5a_8456 })
        );
        assert_eq!(stations.next(), None);
        assert!(ndpa.contains_association_id(0x123));
        assert!(ndpa.contains_association_id(0x456));
        assert!(!ndpa.contains_association_id(0x124));
        assert!(!ndpa.contains_association_id(0x0800));
    }

    #[test]
    fn fails_closed_on_non_he_or_partial_station_info() {
        let mut frame = two_station_frame();
        frame[16] &= !HE_MARKER;
        assert_eq!(HeNdpa::parse(&frame), Err(HeNdpaError::NotHe));

        frame[16] |= HE_MARKER;
        frame[0] = 0x44;
        assert_eq!(HeNdpa::parse(&frame), Err(HeNdpaError::NotNdpa));

        frame[0] = NDPA_FRAME_CONTROL as u8;
        assert_eq!(
            HeNdpa::parse(&frame[..24]),
            Err(HeNdpaError::MisalignedStationInfo)
        );
        assert_eq!(HeNdpa::parse(&frame[..20]), Err(HeNdpaError::TooShort));
    }

    #[test]
    fn encodes_the_complete_vendor_he20_sounding_exchange_ndpa() {
        let station = HeNdpaStationEncoding {
            association_id: 29,
            resource_unit_start_index: 0,
            resource_unit_end_index: 8,
            feedback_type_and_ng_encoding: 0,
            disambiguation: true,
            codebook_size: false,
            nc_encoding: 0,
        };
        assert_eq!(station.encode(), Ok(0x0820_001d));

        let encoding = HeNdpaEncoding {
            duration: 100,
            receiver_address: RA,
            transmitter_address: [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e],
            dialog_token: 0x37,
            stations: core::slice::from_ref(&station),
        };
        let mut frame = [0_u8; 21];
        assert_eq!(encoding.encode(&mut frame), Ok(frame.len()));
        assert_eq!(
            frame,
            [
                0x54, 0x00, 0x64, 0x00, 0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0, 0xdc, 0x15, 0xc8, 0x54,
                0xbc, 0x1e, 0xde, 0x1d, 0x00, 0x20, 0x08,
            ]
        );

        let decoded = HeNdpa::parse(&frame).unwrap();
        let decoded_station = decoded.stations().next().unwrap();
        assert_eq!(decoded_station.association_id(), 29);
        assert_eq!(decoded_station.resource_unit_start_index(), 0);
        assert_eq!(decoded_station.resource_unit_end_index(), 8);
        assert_eq!(decoded_station.feedback_type_and_ng_encoding(), 0);
        assert!(decoded_station.disambiguation());
        assert!(!decoded_station.codebook_size());
        assert_eq!(decoded_station.nc_encoding(), 0);
    }

    #[test]
    fn ndpa_encoder_rejects_every_unowned_boundary() {
        let valid = HeNdpaStationEncoding {
            association_id: 1,
            resource_unit_start_index: 0,
            resource_unit_end_index: 8,
            feedback_type_and_ng_encoding: 0,
            disambiguation: true,
            codebook_size: false,
            nc_encoding: 0,
        };
        assert_eq!(
            HeNdpaStationEncoding {
                association_id: 0x0800,
                ..valid
            }
            .encode(),
            Err(HeNdpaEncodingError::AssociationIdOutOfRange)
        );
        assert_eq!(
            HeNdpaStationEncoding {
                resource_unit_start_index: 9,
                resource_unit_end_index: 8,
                ..valid
            }
            .encode(),
            Err(HeNdpaEncodingError::ReversedResourceUnitRange)
        );
        assert_eq!(
            HeNdpaStationEncoding {
                resource_unit_end_index: 0x80,
                ..valid
            }
            .encode(),
            Err(HeNdpaEncodingError::ResourceUnitIndexOutOfRange)
        );
        assert_eq!(
            HeNdpaStationEncoding {
                feedback_type_and_ng_encoding: 4,
                ..valid
            }
            .encode(),
            Err(HeNdpaEncodingError::FeedbackTypeAndNgOutOfRange)
        );
        assert_eq!(
            HeNdpaStationEncoding {
                nc_encoding: 8,
                ..valid
            }
            .encode(),
            Err(HeNdpaEncodingError::NcOutOfRange)
        );

        let encoding = HeNdpaEncoding {
            duration: 100,
            receiver_address: RA,
            transmitter_address: TA,
            dialog_token: 64,
            stations: core::slice::from_ref(&valid),
        };
        assert_eq!(
            encoding.encode(&mut [0_u8; 21]),
            Err(HeNdpaEncodingError::DialogTokenOutOfRange)
        );
        assert_eq!(
            HeNdpaEncoding {
                dialog_token: 1,
                stations: &[],
                ..encoding
            }
            .encode(&mut [0_u8; 21]),
            Err(HeNdpaEncodingError::NoStations)
        );
        assert_eq!(
            HeNdpaEncoding {
                dialog_token: 1,
                stations: core::slice::from_ref(&valid),
                ..encoding
            }
            .encode(&mut [0_u8; 20]),
            Err(HeNdpaEncodingError::OutputTooShort)
        );
    }

    #[test]
    fn decodes_the_complete_vendor_he20_compressed_feedback_header() {
        let mut report = [0_u8; 112];
        report[0..2].copy_from_slice(&ACTION_NO_ACK_FRAME_CONTROL.to_le_bytes());
        report[4..10].copy_from_slice(&[0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e]);
        report[10..16].copy_from_slice(&RA);
        report[16..22].copy_from_slice(&[0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e]);
        report[22..24].copy_from_slice(&0x00a0_u16.to_le_bytes());
        report[24] = HE_ACTION_CATEGORY;
        report[25] = HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION;
        report[26..31].copy_from_slice(&[0x08, 0x82, 0x00, 0xc4, 0x0d]);
        report[31] = 0x14;
        report[32..].copy_from_slice(&[
            0x51, 0x47, 0x1d, 0x75, 0xd4, 0x51, 0x43, 0x1d, 0x75, 0xd4, 0x50, 0x47, 0x0d, 0x75,
            0xd4, 0x51, 0x47, 0x0d, 0x35, 0xd4, 0x51, 0x43, 0x1d, 0x75, 0xd4, 0x51, 0x43, 0x1d,
            0x75, 0xd4, 0x52, 0x47, 0x1d, 0x75, 0xd4, 0x51, 0x47, 0x0d, 0xb5, 0xd4, 0x51, 0x4b,
            0x2d, 0x75, 0xd4, 0x51, 0x47, 0x1d, 0xb5, 0xd4, 0x52, 0x47, 0x2d, 0x75, 0xd4, 0x52,
            0x47, 0x2d, 0x75, 0xd4, 0x52, 0x4b, 0x2d, 0xb5, 0xd4, 0x52, 0x4b, 0x2d, 0xb5, 0xd4,
            0x52, 0x4b, 0x2d, 0x75, 0xd4, 0x52, 0x4b, 0x1d, 0xb5, 0xe4,
        ]);

        let decoded = HeCompressedBeamformingReport::parse(&report).unwrap();
        assert_eq!(
            decoded.receiver_address(),
            &[0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e]
        );
        assert_eq!(decoded.transmitter_address(), &RA);
        assert_eq!(decoded.sequence_number(), 10);
        assert_eq!(decoded.mimo_control(), 0x0dc4_0082_08);
        assert_eq!(decoded.column_count(), 1);
        assert_eq!(decoded.row_count(), 2);
        assert_eq!(decoded.bandwidth_encoding(), 0);
        assert!(!decoded.grouping());
        assert!(decoded.codebook_information());
        assert_eq!(decoded.feedback_type_encoding(), 0);
        assert_eq!(decoded.remaining_feedback_segments(), 0);
        assert!(decoded.first_feedback_segment());
        assert_eq!(decoded.resource_unit_start_index(), 0);
        assert_eq!(decoded.resource_unit_end_index(), 8);
        assert_eq!(decoded.sounding_dialog_token(), 0x37);
        assert_eq!(decoded.reserved(), 0);
        assert_eq!(decoded.average_snr(), &[0x14]);
        assert_eq!(decoded.feedback_matrices().len(), 80);
    }

    #[test]
    fn compressed_feedback_parser_rejects_other_actions_and_missing_snr() {
        let mut report = [0_u8; 32];
        report[0..2].copy_from_slice(&ACTION_NO_ACK_FRAME_CONTROL.to_le_bytes());
        report[24] = HE_ACTION_CATEGORY;
        report[25] = HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION;
        report[26] = 1;
        assert_eq!(
            HeCompressedBeamformingReport::parse(&report[..31]),
            Err(HeCompressedBeamformingReportError::MissingAverageSnr)
        );
        report[25] = 1;
        assert_eq!(
            HeCompressedBeamformingReport::parse(&report),
            Err(HeCompressedBeamformingReportError::NotHeCompressedBeamformingAndCqi)
        );
        report[25] = HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION;
        report[0] = 0xd0;
        assert_eq!(
            HeCompressedBeamformingReport::parse(&report),
            Err(HeCompressedBeamformingReportError::NotActionNoAck)
        );
    }
}
