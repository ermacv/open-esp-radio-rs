//! Protected QoS A-MSDU sizing and encoding from complete Ethernet frames.

use super::*;

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
