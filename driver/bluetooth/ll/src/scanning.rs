//! Portable legacy passive-scanning policy and PDU parsing.
//!
//! This module understands Bluetooth air-interface fields only. It contains no
//! ESP32 descriptor layout, MMIO, HCI packet framing, executor, or allocator.

use crate::{LeDeviceAddress, LeDeviceAddressKind};

/// HCI legacy scan interval/window unit: 0.625 milliseconds.
pub const LEGACY_SCAN_UNIT_MICROS: u32 = 625;
pub const LEGACY_SCAN_MIN_UNITS: u16 = 0x0004;
pub const LEGACY_SCAN_MAX_UNITS: u16 = 0x4000;
pub const LEGACY_SCAN_DATA_CAPACITY: usize = 31;

const LEGACY_HEADER_BYTES: usize = 2;
const DEVICE_ADDRESS_BYTES: usize = 6;
const MAX_LEGACY_PAYLOAD_BYTES: usize = DEVICE_ADDRESS_BYTES + LEGACY_SCAN_DATA_CAPACITY;
const PDU_TYPE_MASK: u8 = 0x0f;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RX_ADD_RANDOM: u8 = 1 << 7;
const PAYLOAD_LENGTH_MASK: u8 = 0x3f;

/// Validated legacy scan interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanInterval(u16);

impl LegacyScanInterval {
    pub const fn new(units_625_us: u16) -> Result<Self, LegacyScanTimingError> {
        if units_625_us < LEGACY_SCAN_MIN_UNITS || units_625_us > LEGACY_SCAN_MAX_UNITS {
            Err(LegacyScanTimingError::IntervalOutsideRange)
        } else {
            Ok(Self(units_625_us))
        }
    }

    pub const fn units_625_us(self) -> u16 {
        self.0
    }

    pub const fn micros(self) -> u32 {
        self.0 as u32 * LEGACY_SCAN_UNIT_MICROS
    }
}

/// Validated non-empty legacy receive window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanWindow(u16);

impl LegacyScanWindow {
    pub const fn new(units_625_us: u16) -> Result<Self, LegacyScanTimingError> {
        if units_625_us < LEGACY_SCAN_MIN_UNITS || units_625_us > LEGACY_SCAN_MAX_UNITS {
            Err(LegacyScanTimingError::WindowOutsideRange)
        } else {
            Ok(Self(units_625_us))
        }
    }

    pub const fn units_625_us(self) -> u16 {
        self.0
    }

    pub const fn micros(self) -> u32 {
        self.0 as u32 * LEGACY_SCAN_UNIT_MICROS
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanTimingError {
    IntervalOutsideRange,
    WindowOutsideRange,
    WindowExceedsInterval,
}

/// Immutable passive LE 1M scanning parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPassiveScanParameters {
    interval: LegacyScanInterval,
    window: LegacyScanWindow,
}

impl LegacyPassiveScanParameters {
    pub const fn new(
        interval: LegacyScanInterval,
        window: LegacyScanWindow,
    ) -> Result<Self, LegacyScanTimingError> {
        if window.units_625_us() > interval.units_625_us() {
            Err(LegacyScanTimingError::WindowExceedsInterval)
        } else {
            Ok(Self { interval, window })
        }
    }

    pub const fn interval(self) -> LegacyScanInterval {
        self.interval
    }

    pub const fn window(self) -> LegacyScanWindow {
        self.window
    }
}

/// Primary advertising channel selected for one receive window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryScanChannel {
    Channel37,
    Channel38,
    Channel39,
}

impl PrimaryScanChannel {
    pub const fn next(self) -> Self {
        match self {
            Self::Channel37 => Self::Channel38,
            Self::Channel38 => Self::Channel39,
            Self::Channel39 => Self::Channel37,
        }
    }
}

/// Legacy advertising PDU classes reportable by a passive scanner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingReportKind {
    ConnectableUndirected,
    ConnectableDirected,
    NonconnectableUndirected,
    ScanResponse,
    ScannableUndirected,
}

/// Owned protocol report formed from one validated on-air PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingReport {
    kind: LegacyAdvertisingReportKind,
    advertiser: LeDeviceAddress,
    directed_target: Option<LeDeviceAddress>,
    data: [u8; LEGACY_SCAN_DATA_CAPACITY],
    data_len: u8,
    channel: PrimaryScanChannel,
    rssi_dbm: i8,
}

impl LegacyAdvertisingReport {
    pub const fn kind(&self) -> LegacyAdvertisingReportKind {
        self.kind
    }

    pub const fn advertiser(&self) -> LeDeviceAddress {
        self.advertiser
    }

    pub const fn directed_target(&self) -> Option<LeDeviceAddress> {
        self.directed_target
    }

    pub const fn data(&self) -> &[u8] {
        self.data.split_at(self.data_len as usize).0
    }

    pub const fn channel(&self) -> PrimaryScanChannel {
        self.channel
    }

    pub const fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }

    const fn duplicate_identity(self) -> LegacyAdvertisingDuplicateIdentity {
        LegacyAdvertisingDuplicateIdentity {
            kind: self.kind,
            advertiser: self.advertiser,
            directed_target: self.directed_target,
            data: self.data,
            data_len: self.data_len,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingReportParseError {
    HeaderTruncated,
    DeclaredLengthMismatch,
    LegacyPayloadTooLong,
    UnsupportedPduType(u8),
    AdvertiserAddressMissing,
    DirectedAddressLengthInvalid,
}

/// Parse one complete legacy advertising-channel PDU plus receive metadata.
pub const fn parse_legacy_advertising_report(
    pdu: &[u8],
    channel: PrimaryScanChannel,
    rssi_dbm: i8,
) -> Result<LegacyAdvertisingReport, LegacyAdvertisingReportParseError> {
    if pdu.len() < LEGACY_HEADER_BYTES {
        return Err(LegacyAdvertisingReportParseError::HeaderTruncated);
    }
    let payload_len = (pdu[1] & PAYLOAD_LENGTH_MASK) as usize;
    if payload_len > MAX_LEGACY_PAYLOAD_BYTES {
        return Err(LegacyAdvertisingReportParseError::LegacyPayloadTooLong);
    }
    if pdu.len() != LEGACY_HEADER_BYTES + payload_len {
        return Err(LegacyAdvertisingReportParseError::DeclaredLengthMismatch);
    }
    if payload_len < DEVICE_ADDRESS_BYTES {
        return Err(LegacyAdvertisingReportParseError::AdvertiserAddressMissing);
    }

    let header = pdu[0];
    let kind = match header & PDU_TYPE_MASK {
        0 => LegacyAdvertisingReportKind::ConnectableUndirected,
        1 => LegacyAdvertisingReportKind::ConnectableDirected,
        2 => LegacyAdvertisingReportKind::NonconnectableUndirected,
        4 => LegacyAdvertisingReportKind::ScanResponse,
        6 => LegacyAdvertisingReportKind::ScannableUndirected,
        unsupported => {
            return Err(LegacyAdvertisingReportParseError::UnsupportedPduType(
                unsupported,
            ));
        }
    };
    if matches!(kind, LegacyAdvertisingReportKind::ConnectableDirected) && payload_len != 12 {
        return Err(LegacyAdvertisingReportParseError::DirectedAddressLengthInvalid);
    }

    let advertiser = LeDeviceAddress::from_wire_bytes(
        copy_address(pdu, LEGACY_HEADER_BYTES),
        if header & TX_ADD_RANDOM == 0 {
            LeDeviceAddressKind::Public
        } else {
            LeDeviceAddressKind::Random
        },
    );
    let directed_target = if matches!(kind, LegacyAdvertisingReportKind::ConnectableDirected) {
        Some(LeDeviceAddress::from_wire_bytes(
            copy_address(pdu, LEGACY_HEADER_BYTES + DEVICE_ADDRESS_BYTES),
            if header & RX_ADD_RANDOM == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        ))
    } else {
        None
    };
    let data_start = LEGACY_HEADER_BYTES + DEVICE_ADDRESS_BYTES;
    let data_len = if directed_target.is_some() {
        0
    } else {
        pdu.len() - data_start
    };
    let mut data = [0; LEGACY_SCAN_DATA_CAPACITY];
    let mut index = 0;
    while index < data_len {
        data[index] = pdu[data_start + index];
        index += 1;
    }
    Ok(LegacyAdvertisingReport {
        kind,
        advertiser,
        directed_target,
        data,
        data_len: data_len as u8,
        channel,
        rssi_dbm,
    })
}

const fn copy_address(pdu: &[u8], start: usize) -> [u8; DEVICE_ADDRESS_BYTES] {
    [
        pdu[start],
        pdu[start + 1],
        pdu[start + 2],
        pdu[start + 3],
        pdu[start + 4],
        pdu[start + 5],
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LegacyAdvertisingDuplicateIdentity {
    kind: LegacyAdvertisingReportKind,
    advertiser: LeDeviceAddress,
    directed_target: Option<LeDeviceAddress>,
    data: [u8; LEGACY_SCAN_DATA_CAPACITY],
    data_len: u8,
}

/// Fixed exact-match duplicate cache. RSSI and receive channel intentionally do
/// not affect identity; changed advertising data does.
pub struct LegacyAdvertisingDuplicateFilter<const CAPACITY: usize> {
    entries: [Option<LegacyAdvertisingDuplicateIdentity>; CAPACITY],
    replacement: usize,
}

impl<const CAPACITY: usize> LegacyAdvertisingDuplicateFilter<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            replacement: 0,
        }
    }

    /// Return true exactly once for a new identity while retaining it.
    pub fn accept(&mut self, report: LegacyAdvertisingReport) -> bool {
        let identity = report.duplicate_identity();
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| *entry == identity)
        {
            return false;
        }
        if CAPACITY != 0 {
            self.entries[self.replacement] = Some(identity);
            self.replacement = (self.replacement + 1) % CAPACITY;
        }
        true
    }

    pub fn clear(&mut self) {
        self.entries.fill(None);
        self.replacement = 0;
    }
}

impl<const CAPACITY: usize> Default for LegacyAdvertisingDuplicateFilter<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LegacyAdvertisingDuplicateFilter, LegacyAdvertisingReportKind, LegacyPassiveScanParameters,
        LegacyScanInterval, LegacyScanTimingError, LegacyScanWindow, PrimaryScanChannel,
        parse_legacy_advertising_report,
    };
    use crate::LeDeviceAddressKind;

    const ADV_NONCONN: [u8; 11] = [2, 9, 1, 2, 3, 4, 5, 6, 2, 1, 6];

    #[test]
    fn timing_rejects_a_window_larger_than_its_interval() {
        let interval = LegacyScanInterval::new(16).expect("the interval is valid");
        let window = LegacyScanWindow::new(17).expect("the window is valid");
        assert_eq!(
            LegacyPassiveScanParameters::new(interval, window),
            Err(LegacyScanTimingError::WindowExceedsInterval)
        );
    }

    #[test]
    fn passive_report_retains_protocol_fields_and_metadata() {
        let report =
            parse_legacy_advertising_report(&ADV_NONCONN, PrimaryScanChannel::Channel37, -71)
                .expect("the legacy PDU is valid");
        assert_eq!(
            report.kind(),
            LegacyAdvertisingReportKind::NonconnectableUndirected
        );
        assert_eq!(report.advertiser().wire_bytes(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(report.advertiser().kind(), LeDeviceAddressKind::Public);
        assert_eq!(report.data(), [2, 1, 6]);
        assert_eq!(report.channel(), PrimaryScanChannel::Channel37);
        assert_eq!(report.rssi_dbm(), -71);
    }

    #[test]
    fn exact_duplicate_filter_ignores_channel_and_rssi_but_not_data() {
        let first =
            parse_legacy_advertising_report(&ADV_NONCONN, PrimaryScanChannel::Channel37, -50)
                .expect("the first PDU is valid");
        let duplicate =
            parse_legacy_advertising_report(&ADV_NONCONN, PrimaryScanChannel::Channel39, -90)
                .expect("the duplicate PDU is valid");
        let changed = [2, 9, 1, 2, 3, 4, 5, 6, 2, 1, 5];
        let changed = parse_legacy_advertising_report(&changed, PrimaryScanChannel::Channel38, -60)
            .expect("the changed PDU is valid");

        let mut filter = LegacyAdvertisingDuplicateFilter::<2>::new();
        assert!(filter.accept(first));
        assert!(!filter.accept(duplicate));
        assert!(filter.accept(changed));
        filter.clear();
        assert!(filter.accept(first));
    }
}
