//! Allocation-free HE NDP Announcement parsing.

const NDPA_FRAME_CONTROL: u16 = 0x0054;
const NDPA_HEADER_SIZE: usize = 17;
const HE_STA_INFO_SIZE: usize = 4;
const HE_MARKER: u8 = 1 << 1;
const AID11_MASK: u32 = 0x07ff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeNdpaError {
    TooShort,
    NotNdpa,
    NotHe,
    MisalignedStationInfo,
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
}
