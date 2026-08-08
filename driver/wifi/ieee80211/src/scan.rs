//! Allocation-free scan records and management-frame parsing.
//!
//! This is the maintained source-owned passive-scan frontier. Channel changes,
//! dwell timers, and RX ownership remain explicit responsibilities of the
//! radio owner; this module owns only bounded observations and selection. The
//! earlier hybrid-runtime copy was removed after hardware qualification. The
//! implementation is chip-independent and now forms the first maintained
//! upper-stack extraction from the migration archive.

pub const SCAN_RECORD_CAPACITY: usize = 32;
pub const RSN_IE_CAPACITY: usize = 64;
pub const RSNXE_CAPACITY: usize = 16;
pub const EXTENDED_RATES_CAPACITY: usize = 16;
pub const HT_CAPABILITY_IE_LEN: usize = 28;
pub const HT_OPERATION_IE_LEN: usize = 24;
pub const HE_CAPABILITY_IE_CAPACITY: usize = 64;
pub const HE_OPERATION_IE_CAPACITY: usize = 32;
pub const WMM_IE_CAPACITY: usize = 26;

const HE_CAPABILITIES_EXTENSION_ID: u8 = 35;
const HE_OPERATION_EXTENSION_ID: u8 = 36;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtSecondaryChannel {
    Above,
    Below,
}

/// Recover the Default PE Duration advertised in an HE Operation IE.
///
/// A stored extension IE includes `[element_id, length, extension_id]`
/// before its payload, so HE Operation Parameters byte zero is byte three.
/// The field is its low three bits. This layout and its direct use by the
/// ESP32-S31 MAC are independently confirmed by complete
/// `libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr` and
/// `libpp.a[hal_mac_ctl.o]::hal_he_set_default_pe`.
pub fn he_default_packet_extension_duration(he_operation_ie: &[u8]) -> Option<u8> {
    if he_operation_ie.len() < 4
        || he_operation_ie[0] != 255
        || he_operation_ie[2] != HE_OPERATION_EXTENSION_ID
    {
        return None;
    }
    Some(he_operation_ie[3] & 0x07)
}

/// Recover the HE Operation `ER-SU-Disable` bit consumed by the ESP32-S31.
///
/// Complete
/// `libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr`
/// logs complete-IE byte five bit zero as `ER-SU-Disable`, stores it at
/// peer-state bit 10 and passes it unchanged to `hal_he_set_ersu`.
pub fn he_extended_range_single_user_disabled(he_operation_ie: &[u8]) -> Option<bool> {
    if he_operation_ie.len() < 6
        || he_operation_ie[0] != 255
        || he_operation_ie[2] != HE_OPERATION_EXTENSION_ID
    {
        return None;
    }
    Some(he_operation_ie[5] & 0x01 != 0)
}

/// One bounded, owned observation from a beacon or probe response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanRecord {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8,
    pub privacy: bool,
    pub rsn: bool,
    pub legacy_wpa: bool,
    pub information_elements_truncated: bool,
    pub capability_info: u16,
    pub beacon_interval_tu: u16,
    pub supported_rates: [u8; 8],
    pub supported_rates_len: u8,
    pub extended_supported_rates: [u8; EXTENDED_RATES_CAPACITY],
    pub extended_supported_rates_len: u8,
    pub ht_capability_ie: [u8; HT_CAPABILITY_IE_LEN],
    pub ht_capability_ie_present: bool,
    pub ht_operation_ie: [u8; HT_OPERATION_IE_LEN],
    pub ht_operation_ie_present: bool,
    pub he_capability_ie: [u8; HE_CAPABILITY_IE_CAPACITY],
    pub he_capability_ie_len: u8,
    pub he_operation_ie: [u8; HE_OPERATION_IE_CAPACITY],
    pub he_operation_ie_len: u8,
    pub wmm_ie: [u8; WMM_IE_CAPACITY],
    pub wmm_ie_len: u8,
    pub rsn_ie: [u8; RSN_IE_CAPACITY],
    pub rsn_ie_len: u8,
    pub rsnxe: [u8; RSNXE_CAPACITY],
    pub rsnxe_len: u8,
}

impl ScanRecord {
    pub const EMPTY: Self = Self {
        ssid: [0; 32],
        ssid_len: 0,
        bssid: [0; 6],
        channel: 0,
        rssi: i8::MIN,
        privacy: false,
        rsn: false,
        legacy_wpa: false,
        information_elements_truncated: false,
        capability_info: 0,
        beacon_interval_tu: 0,
        supported_rates: [0; 8],
        supported_rates_len: 0,
        extended_supported_rates: [0; EXTENDED_RATES_CAPACITY],
        extended_supported_rates_len: 0,
        ht_capability_ie: [0; HT_CAPABILITY_IE_LEN],
        ht_capability_ie_present: false,
        ht_operation_ie: [0; HT_OPERATION_IE_LEN],
        ht_operation_ie_present: false,
        he_capability_ie: [0; HE_CAPABILITY_IE_CAPACITY],
        he_capability_ie_len: 0,
        he_operation_ie: [0; HE_OPERATION_IE_CAPACITY],
        he_operation_ie_len: 0,
        wmm_ie: [0; WMM_IE_CAPACITY],
        wmm_ie_len: 0,
        rsn_ie: [0; RSN_IE_CAPACITY],
        rsn_ie_len: 0,
        rsnxe: [0; RSNXE_CAPACITY],
        rsnxe_len: 0,
    };

    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }

    pub fn supported_rates_bytes(&self) -> &[u8] {
        &self.supported_rates[..usize::from(self.supported_rates_len)]
    }

    pub fn extended_supported_rates_bytes(&self) -> &[u8] {
        &self.extended_supported_rates[..usize::from(self.extended_supported_rates_len)]
    }

    pub fn ht_capability_ie_bytes(&self) -> Option<&[u8; HT_CAPABILITY_IE_LEN]> {
        self.ht_capability_ie_present
            .then_some(&self.ht_capability_ie)
    }

    pub fn ht_operation_ie_bytes(&self) -> Option<&[u8; HT_OPERATION_IE_LEN]> {
        self.ht_operation_ie_present
            .then_some(&self.ht_operation_ie)
    }

    /// Whether the AP advertises reception with a 400 ns guard interval on a
    /// 20 MHz HT channel.
    pub fn supports_ht_short_guard_interval_20mhz(&self) -> bool {
        self.ht_capability_ie_bytes().is_some_and(|capability| {
            u16::from_le_bytes([capability[2], capability[3]]) & (1 << 5) != 0
        })
    }

    /// Whether the AP advertises reception with a 400 ns guard interval on a
    /// 40 MHz HT channel.
    pub fn supports_ht_short_guard_interval_40mhz(&self) -> bool {
        self.ht_capability_ie_bytes().is_some_and(|capability| {
            u16::from_le_bytes([capability[2], capability[3]]) & (1 << 6) != 0
        })
    }

    /// Return the AP's usable 40-MHz secondary-channel geometry.
    ///
    /// The HT Capabilities Supported Channel Width bit must be present, the
    /// HT Operation primary channel must match this record, and the operation
    /// must permit any channel width. The secondary offset uses IEEE 802.11
    /// values one (above) and three (below).
    ///
    /// SOURCE: the complete IEs copied by this module from Beacon/Probe
    /// Response frames; the ESP32-S31 mapping from above/below to CBW 2/3 is
    /// independently recovered in
    /// `open-esp-radio-esp32s31-registers/src/frequency.rs::bss_tx_offset` from the
    /// complete rev0 ROM `phy_bb_bss_cbw40`.
    pub fn ht40_secondary_channel(&self) -> Option<HtSecondaryChannel> {
        let capability = self.ht_capability_ie_bytes()?;
        let operation = self.ht_operation_ie_bytes()?;
        let capability_info = u16::from_le_bytes([capability[2], capability[3]]);
        if capability_info & (1 << 1) == 0
            || operation[2] != self.channel
            || operation[3] & (1 << 2) == 0
        {
            return None;
        }
        match operation[3] & 0x03 {
            1 if self.channel <= 9 => Some(HtSecondaryChannel::Above),
            3 if self.channel >= 5 => Some(HtSecondaryChannel::Below),
            _ => None,
        }
    }

    pub fn he_capability_ie_bytes(&self) -> &[u8] {
        &self.he_capability_ie[..usize::from(self.he_capability_ie_len)]
    }

    pub fn he_operation_ie_bytes(&self) -> &[u8] {
        &self.he_operation_ie[..usize::from(self.he_operation_ie_len)]
    }

    pub fn wmm_ie_bytes(&self) -> &[u8] {
        &self.wmm_ie[..usize::from(self.wmm_ie_len)]
    }

    pub fn wmm_parameters(&self) -> Option<crate::wmm::WmmParameterSet> {
        crate::wmm::parse_wmm_parameter_element(self.wmm_ie_bytes())
    }

    pub fn rsn_ie_bytes(&self) -> &[u8] {
        &self.rsn_ie[..usize::from(self.rsn_ie_len)]
    }

    pub fn rsnxe_bytes(&self) -> &[u8] {
        &self.rsnxe[..usize::from(self.rsnxe_len)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSummary {
    pub records: usize,
    pub observed_frames: u32,
    pub dropped_unique_bss: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanObservation {
    Ignored,
    Inserted { index: usize },
    Updated { index: usize },
    Duplicate { index: usize },
    Dropped,
}

/// Fixed-capacity BSS table owned by one radio task.
pub struct ScanTable<const N: usize = SCAN_RECORD_CAPACITY> {
    records: [ScanRecord; N],
    length: usize,
    observed_frames: u32,
    dropped_unique_bss: u32,
}

impl<const N: usize> ScanTable<N> {
    pub const fn new() -> Self {
        Self {
            records: [ScanRecord::EMPTY; N],
            length: 0,
            observed_frames: 0,
            dropped_unique_bss: 0,
        }
    }

    pub fn clear(&mut self) {
        self.records.fill(ScanRecord::EMPTY);
        self.length = 0;
        self.observed_frames = 0;
        self.dropped_unique_bss = 0;
    }

    pub fn records(&self) -> &[ScanRecord] {
        &self.records[..self.length]
    }

    pub fn summary(&self) -> ScanSummary {
        ScanSummary {
            records: self.length,
            observed_frames: self.observed_frames,
            dropped_unique_bss: self.dropped_unique_bss,
        }
    }

    /// Parse and merge one beacon/probe response by BSSID.
    pub fn observe_management(
        &mut self,
        frame: &[u8],
        fallback_channel: u8,
        rssi: i8,
    ) -> ScanObservation {
        let Some(record) = parse_management(frame, fallback_channel, rssi) else {
            return ScanObservation::Ignored;
        };
        self.observed_frames = self.observed_frames.saturating_add(1);

        for (index, existing) in self.records[..self.length].iter_mut().enumerate() {
            if existing.bssid == record.bssid {
                if record.rssi > existing.rssi || existing.ssid_len == 0 {
                    *existing = record;
                    return ScanObservation::Updated { index };
                }
                return ScanObservation::Duplicate { index };
            }
        }

        if self.length == N {
            self.dropped_unique_bss = self.dropped_unique_bss.saturating_add(1);
            return ScanObservation::Dropped;
        }
        let index = self.length;
        self.records[index] = record;
        self.length += 1;
        ScanObservation::Inserted { index }
    }
}

impl<const N: usize> Default for ScanTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Select the strongest complete observation with an exact SSID match.
pub fn best_matching_ssid<'a>(records: &'a [ScanRecord], ssid: &[u8]) -> Option<&'a ScanRecord> {
    records
        .iter()
        .filter(|record| record.ssid_bytes() == ssid && (1..=13).contains(&record.channel))
        .max_by_key(|record| record.rssi)
}

/// Decode one beacon or probe response into an owned bounded record.
pub fn parse_management(frame: &[u8], fallback_channel: u8, rssi: i8) -> Option<ScanRecord> {
    if frame.len() < 36 {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    let subtype = (frame_control >> 4) & 0x0f;
    if frame_control & 0x000c != 0 || !matches!(subtype, 5 | 8) {
        return None;
    }

    let mut record = ScanRecord::EMPTY;
    record.bssid.copy_from_slice(&frame[16..22]);
    record.channel = fallback_channel;
    record.rssi = rssi;
    record.beacon_interval_tu = u16::from_le_bytes([frame[32], frame[33]]);
    record.capability_info = u16::from_le_bytes([frame[34], frame[35]]);
    record.privacy = record.capability_info & 0x0010 != 0;

    let mut offset = 36;
    while offset + 2 <= frame.len() {
        let id = frame[offset];
        let length = usize::from(frame[offset + 1]);
        offset += 2;
        let Some(end) = offset.checked_add(length) else {
            record.information_elements_truncated = true;
            break;
        };
        if end > frame.len() {
            record.information_elements_truncated = true;
            break;
        }
        let value = &frame[offset..end];
        match id {
            0 if length <= record.ssid.len() => {
                record.ssid[..length].copy_from_slice(value);
                record.ssid_len = length as u8;
            }
            1 if length <= record.supported_rates.len() => {
                record.supported_rates[..length].copy_from_slice(value);
                record.supported_rates_len = length as u8;
            }
            3 if length == 1 => record.channel = value[0],
            45 if length + 2 == HT_CAPABILITY_IE_LEN => {
                record
                    .ht_capability_ie
                    .copy_from_slice(&frame[offset - 2..end]);
                record.ht_capability_ie_present = true;
            }
            48 => {
                record.rsn = true;
                let total = length + 2;
                if total <= record.rsn_ie.len() {
                    record.rsn_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.rsn_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            50 => {
                let copied = length.min(record.extended_supported_rates.len());
                record.extended_supported_rates[..copied].copy_from_slice(&value[..copied]);
                record.extended_supported_rates_len = copied as u8;
                record.information_elements_truncated |= copied != length;
            }
            61 if length + 2 == HT_OPERATION_IE_LEN => {
                record
                    .ht_operation_ie
                    .copy_from_slice(&frame[offset - 2..end]);
                record.ht_operation_ie_present = true;
            }
            244 => {
                let total = length + 2;
                if total <= record.rsnxe.len() {
                    record.rsnxe[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.rsnxe_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            255 if value.first().copied() == Some(HE_CAPABILITIES_EXTENSION_ID) => {
                let total = length + 2;
                if total <= record.he_capability_ie.len() {
                    record.he_capability_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.he_capability_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            255 if value.first().copied() == Some(HE_OPERATION_EXTENSION_ID) => {
                let total = length + 2;
                if total <= record.he_operation_ie.len() {
                    record.he_operation_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.he_operation_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            221 if length >= 4 && value[..4] == [0x00, 0x50, 0xf2, 0x01] => {
                record.legacy_wpa = true;
            }
            221 if length >= 6 && value[..4] == [0x00, 0x50, 0xf2, 0x02] => {
                let total = length + 2;
                if total <= record.wmm_ie.len() {
                    record.wmm_ie[..total].copy_from_slice(&frame[offset - 2..end]);
                    record.wmm_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            _ => {}
        }
        offset = end;
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::{
        HtSecondaryChannel, ScanObservation, ScanRecord, ScanTable, best_matching_ssid,
        he_default_packet_extension_duration, he_extended_range_single_user_disabled,
        parse_management,
    };

    #[test]
    fn extracts_default_pe_duration_from_complete_he_operation_ie() {
        assert_eq!(
            he_default_packet_extension_duration(&[255, 4, 36, 0b1010_1100, 0, 0]),
            Some(4)
        );
        assert_eq!(
            he_default_packet_extension_duration(&[255, 4, 35, 4, 0, 0]),
            None
        );
        assert_eq!(he_default_packet_extension_duration(&[255, 1, 36]), None);
    }

    #[test]
    fn extracts_ersu_argument_from_complete_he_operation_ie() {
        assert_eq!(
            he_extended_range_single_user_disabled(&[255, 4, 36, 0, 0, 1]),
            Some(true)
        );
        assert_eq!(
            he_extended_range_single_user_disabled(&[255, 4, 36, 0, 0, 0]),
            Some(false)
        );
        assert_eq!(
            he_extended_range_single_user_disabled(&[255, 2, 36, 0]),
            None
        );
    }

    #[test]
    fn parses_beacon_into_owned_bounded_record() {
        let mut frame = [0_u8; 64];
        frame[0] = 0x80;
        frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[34] = 0x10;
        frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
        frame[42..45].copy_from_slice(&[3, 1, 11]);
        frame[45..47].copy_from_slice(&[48, 0]);
        let record = parse_management(&frame[..47], 3, -42).unwrap();
        assert_eq!(record.ssid_bytes(), b"test");
        assert_eq!(record.bssid, [1, 2, 3, 4, 5, 6]);
        assert_eq!(record.channel, 11);
        assert_eq!(record.rssi, -42);
        assert!(record.privacy);
        assert!(record.rsn);
    }

    #[test]
    fn ht40_geometry_requires_matching_capability_and_operation() {
        let mut record = ScanRecord {
            channel: 6,
            ht_capability_ie_present: true,
            ht_operation_ie_present: true,
            ..ScanRecord::EMPTY
        };
        record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
        record.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
        assert_eq!(
            record.ht40_secondary_channel(),
            Some(HtSecondaryChannel::Above)
        );

        record.ht_operation_ie[3] = 0x07;
        assert_eq!(
            record.ht40_secondary_channel(),
            Some(HtSecondaryChannel::Below)
        );
        record.ht_capability_ie[2] = 0;
        assert_eq!(record.ht40_secondary_channel(), None);
    }

    #[test]
    fn ht_short_guard_intervals_are_read_from_capability_info() {
        let mut record = ScanRecord {
            ht_capability_ie_present: true,
            ..ScanRecord::EMPTY
        };
        record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 1 << 5, 0]);
        assert!(record.supports_ht_short_guard_interval_20mhz());
        assert!(!record.supports_ht_short_guard_interval_40mhz());

        record.ht_capability_ie[2] = 1 << 6;
        assert!(!record.supports_ht_short_guard_interval_20mhz());
        assert!(record.supports_ht_short_guard_interval_40mhz());
    }

    #[test]
    fn table_deduplicates_by_bssid_and_retains_strongest_record() {
        let mut table = ScanTable::<1>::new();
        let mut frame = [0_u8; 42];
        frame[0] = 0x80;
        frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
        assert_eq!(
            table.observe_management(&frame, 1, -70),
            ScanObservation::Inserted { index: 0 }
        );
        assert_eq!(
            table.observe_management(&frame, 1, -30),
            ScanObservation::Updated { index: 0 }
        );
        assert_eq!(table.records()[0].rssi, -30);
        assert_eq!(table.summary().observed_frames, 2);
    }

    #[test]
    fn strongest_exact_ssid_ignores_invalid_channel() {
        let mut records = [ScanRecord::EMPTY; 3];
        for (record, rssi, channel) in [(-70, 1), (-25, 6), (-10, 0)]
            .into_iter()
            .zip(&mut records)
            .map(|((rssi, channel), record)| (record, rssi, channel))
        {
            record.ssid[..4].copy_from_slice(b"test");
            record.ssid_len = 4;
            record.rssi = rssi;
            record.channel = channel;
        }
        assert_eq!(best_matching_ssid(&records, b"test").unwrap().channel, 6);
    }
}
