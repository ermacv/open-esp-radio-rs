//! Legacy passive scanning.
//!
//! Enable configures the backend's scanner and then listens once per scan
//! interval for up to one scan window, rotating the primary channels 37, 38
//! and 39. A window yields to every other reservation: it starts after the
//! busy ones and ends before the next one, and a window shorter than
//! [`MINIMUM_SCAN_WINDOW`] is not scheduled. Received advertising PDUs become
//! LE Advertising Reports, passed through the duplicate filter when the Host
//! asked for it. Disable cancels the window in progress, waits for it to end
//! and removes the scanner before it completes.

use bt_hci::param::{AddrKind, BdAddr, Error as HciError, LeAdvEventKind, Status};
use oer_bluetooth_hci::{
    LeLegacyAdvertisingReportEvent, LeLegacyScanningDuplicatePolicy, LeLegacyScanningEnableRequest,
};
use oer_bluetooth_ll::{
    LeDeviceAddressKind,
    scanning::{
        LegacyAdvertisingDuplicateFilter, LegacyAdvertisingReportKind, PrimaryScanChannel,
        parse_legacy_advertising_report,
    },
};
use oer_bluetooth_radio::{
    AdvertisingChannel, EventId, RadioDuration, RadioInstant, RadioOutcome, RadioRequest,
    RadioTiming, RadioWindow, ScanWindow, ScannerConfiguration, ScannerId, TxPower,
};

use crate::arbiter::{Proposal, place};

/// Shortest scan window worth scheduling.
pub const MINIMUM_SCAN_WINDOW: RadioDuration = RadioDuration::from_micros(1_250);
/// Distinct advertisements the duplicate filter remembers.
pub const DUPLICATE_FILTER_CAPACITY: usize = 16;
const SCANNER: ScannerId = ScannerId::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Configuring { sent: bool },
    Running,
    Cancelling { sent: bool },
    Removing { sent: bool },
}

#[derive(Clone, Copy, Debug)]
struct Outstanding {
    id: EventId,
    channel: AdvertisingChannel,
    reservation: RadioWindow,
}

pub(crate) struct Scanner {
    phase: Phase,
    interval: RadioDuration,
    window: RadioDuration,
    filter_duplicates: bool,
    duplicates: LegacyAdvertisingDuplicateFilter<DUPLICATE_FILTER_CAPACITY>,
    channel: AdvertisingChannel,
    next_anchor: Option<RadioInstant>,
    outstanding: Option<Outstanding>,
    completion: Option<Status>,
}

impl Scanner {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            interval: RadioDuration::from_micros(0),
            window: RadioDuration::from_micros(0),
            filter_duplicates: false,
            duplicates: LegacyAdvertisingDuplicateFilter::new(),
            channel: AdvertisingChannel::Channel37,
            next_anchor: None,
            outstanding: None,
            completion: None,
        }
    }

    pub(crate) const fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    pub(crate) fn enable(&mut self, request: LeLegacyScanningEnableRequest) {
        let parameters = request.parameters();
        self.interval =
            RadioDuration::from_micros(u32::from(parameters.interval_units_625_us()) * 625);
        self.window = RadioDuration::from_micros(u32::from(parameters.window_units_625_us()) * 625);
        self.filter_duplicates = matches!(
            request.duplicate_policy(),
            LeLegacyScanningDuplicatePolicy::FilterDuplicates
        );
        self.duplicates.clear();
        self.channel = AdvertisingChannel::Channel37;
        self.next_anchor = None;
        self.outstanding = None;
        self.phase = Phase::Configuring { sent: false };
    }

    pub(crate) fn disable(&mut self) {
        self.phase = match self.outstanding {
            Some(_) => Phase::Cancelling { sent: false },
            None => Phase::Removing { sent: false },
        };
    }

    pub(crate) fn control_request(&mut self) -> Option<RadioRequest<'static>> {
        match &mut self.phase {
            Phase::Configuring { sent: sent @ false } => {
                *sent = true;
                Some(RadioRequest::ConfigureScanner(ScannerConfiguration {
                    scanner: SCANNER,
                    tx_power: TxPower::from_dbm(0),
                }))
            }
            Phase::Cancelling { sent: sent @ false } => {
                *sent = true;
                let id = self.outstanding.expect("a window is cancelled").id;
                Some(RadioRequest::Cancel(id))
            }
            Phase::Removing { sent: sent @ false } => {
                *sent = true;
                Some(RadioRequest::RemoveScanner(SCANNER))
            }
            _ => None,
        }
    }

    pub(crate) fn control_done(&mut self, accepted: bool) {
        match self.phase {
            Phase::Configuring { .. } if accepted => {
                self.phase = Phase::Running;
                self.completion = Some(Status::SUCCESS);
            }
            Phase::Configuring { .. } => {
                self.phase = Phase::Idle;
                self.completion = Some(HciError::HARDWARE_FAILURE.to_status());
            }
            Phase::Cancelling { .. } => {}
            Phase::Removing { .. } => {
                self.phase = Phase::Idle;
                self.completion = Some(if accepted {
                    Status::SUCCESS
                } else {
                    HciError::HARDWARE_FAILURE.to_status()
                });
            }
            Phase::Idle | Phase::Running => {}
        }
    }

    /// Whether the role has a request for the radio.
    pub(crate) fn wants_radio(&self) -> bool {
        self.wants_window()
            || matches!(
                self.phase,
                Phase::Configuring { sent: false }
                    | Phase::Cancelling { sent: false }
                    | Phase::Removing { sent: false }
            )
    }

    /// Whether the scanner wants its next window.
    pub(crate) fn wants_window(&self) -> bool {
        self.phase == Phase::Running && self.outstanding.is_none()
    }

    /// Place the next window around `busy` reservations, or `None` when no
    /// useful window fits before the next interval.
    pub(crate) fn place(
        &mut self,
        id: EventId,
        earliest: RadioInstant,
        timing: RadioTiming,
        busy: &[Option<RadioWindow>],
    ) -> Option<RadioRequest<'static>> {
        if !self.wants_window() {
            return None;
        }
        let anchor = self
            .next_anchor
            .map_or(earliest, |anchor| anchor.max(earliest));
        // The window may start late within its interval while a minimum
        // window still fits, and never runs into the next interval.
        let interval_end = anchor.checked_add(self.interval)?;
        let slack = self
            .interval
            .as_micros()
            .saturating_sub(MINIMUM_SCAN_WINDOW.as_micros());
        let start = place(
            Proposal {
                earliest: anchor,
                latest: anchor.checked_add(RadioDuration::from_micros(slack))?,
                duration: MINIMUM_SCAN_WINDOW,
            },
            timing.preparation_lead,
            busy,
        );
        let Some(start) = start else {
            self.next_anchor = Some(interval_end);
            return None;
        };
        // End before the next reservation.
        let mut end =
            (start.as_micros() + u64::from(self.window.as_micros())).min(interval_end.as_micros());
        for window in busy.iter().flatten() {
            if window.start() >= start {
                end = end.min(window.start().as_micros());
            }
        }
        let duration = RadioDuration::from_micros((end - start.as_micros()) as u32);
        if duration < MINIMUM_SCAN_WINDOW {
            self.next_anchor = Some(interval_end);
            return None;
        }
        let window = RadioWindow::new(start, duration).ok()?;
        self.outstanding = Some(Outstanding {
            id,
            channel: self.channel,
            reservation: timing.reservation(window)?,
        });
        self.next_anchor = Some(interval_end);
        let channel = self.channel;
        self.channel = next_channel(channel);
        Some(RadioRequest::Scan(ScanWindow {
            id,
            scanner: SCANNER,
            channel,
            window,
        }))
    }

    pub(crate) fn busy(&self) -> Option<RadioWindow> {
        self.outstanding.map(|window| window.reservation)
    }

    pub(crate) fn owns(&self, id: EventId) -> bool {
        self.outstanding.is_some_and(|window| window.id == id)
    }

    /// The backend's answer to the window request.
    pub(crate) fn window_done(&mut self, accepted: bool) {
        if !accepted {
            self.outstanding = None;
        }
    }

    /// Account one outcome; a received advertisement becomes a report.
    pub(crate) fn outcome(
        &mut self,
        outcome: RadioOutcome<'_>,
    ) -> Option<LeLegacyAdvertisingReportEvent> {
        match outcome {
            RadioOutcome::Received { id, pdu } => {
                let window = self.outstanding.filter(|window| window.id == id)?;
                if self.phase != Phase::Running {
                    return None;
                }
                let report = parse_legacy_advertising_report(
                    pdu.pdu,
                    scan_channel(window.channel),
                    pdu.rssi_dbm,
                )
                .ok()?;
                let event_kind = match report.kind() {
                    LegacyAdvertisingReportKind::ConnectableUndirected => LeAdvEventKind::AdvInd,
                    LegacyAdvertisingReportKind::NonconnectableUndirected => {
                        LeAdvEventKind::AdvNonconnInd
                    }
                    LegacyAdvertisingReportKind::ScannableUndirected => LeAdvEventKind::AdvScanInd,
                    // A passive scanner without a filter policy for directed
                    // advertising reports neither directed PDUs nor scan
                    // responses it never asked for.
                    LegacyAdvertisingReportKind::ConnectableDirected
                    | LegacyAdvertisingReportKind::ScanResponse => return None,
                };
                if self.filter_duplicates && !self.duplicates.accept(report) {
                    return None;
                }
                let address_kind = match report.advertiser().kind() {
                    LeDeviceAddressKind::Public => AddrKind::PUBLIC,
                    LeDeviceAddressKind::Random => AddrKind::RANDOM,
                };
                LeLegacyAdvertisingReportEvent::new(
                    event_kind,
                    address_kind,
                    BdAddr::new(report.advertiser().wire_bytes()),
                    report.data(),
                    report.rssi_dbm(),
                )
                .ok()
            }
            RadioOutcome::EventEnded { id, .. } if self.owns(id) => {
                self.outstanding = None;
                if let Phase::Cancelling { .. } = self.phase {
                    self.phase = Phase::Removing { sent: false };
                }
                None
            }
            _ => None,
        }
    }

    pub(crate) fn take_completion(&mut self) -> Option<Status> {
        self.completion.take()
    }
}

const fn next_channel(channel: AdvertisingChannel) -> AdvertisingChannel {
    match channel {
        AdvertisingChannel::Channel37 => AdvertisingChannel::Channel38,
        AdvertisingChannel::Channel38 => AdvertisingChannel::Channel39,
        AdvertisingChannel::Channel39 => AdvertisingChannel::Channel37,
    }
}

const fn scan_channel(channel: AdvertisingChannel) -> PrimaryScanChannel {
    match channel {
        AdvertisingChannel::Channel37 => PrimaryScanChannel::Channel37,
        AdvertisingChannel::Channel38 => PrimaryScanChannel::Channel38,
        AdvertisingChannel::Channel39 => PrimaryScanChannel::Channel39,
    }
}
