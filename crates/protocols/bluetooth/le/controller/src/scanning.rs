//! Legacy passive and active scanning.
//!
//! Enable configures the backend's scanner, which an active scanner makes
//! answer scannable advertising with `SCAN_REQ`, and then listens once per scan
//! interval for up to one scan window, rotating the primary channels 37, 38
//! and 39. A window yields to every other reservation: it starts after the
//! busy ones and ends before the next one, and a window shorter than
//! [`MINIMUM_SCAN_WINDOW`] is not scheduled. Received advertising PDUs become
//! LE Advertising Reports, passed through the duplicate filter when the Host
//! asked for it. Disable cancels the window in progress, waits for it to end
//! and removes the scanner before it completes. An active scanner also reports
//! the scan responses it receives.

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
    AdvertisingChannel, EventId, LeInstant, LePhy, LeWindow, RadioDuration, RadioOutcome,
    RadioRequest, RadioTiming, ScanFilterPolicy, ScanType, ScanWindow, ScannerConfiguration,
    ScannerId, TxPower,
};

use crate::RadioWork;
use crate::planning::{
    PlanningCalculation as C, PlanningError, PlanningOperation as O, PlanningRole as R,
};
use oer_bluetooth_radio::TimingError;

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
    reservation: LeWindow,
}

// CAPABILITY: bluetooth-legacy-passive-scanning, bluetooth-active-scanning
pub(crate) struct Scanner {
    phase: Phase,
    scan_type: ScanType,
    filter_policy: ScanFilterPolicy,
    interval: RadioDuration,
    window: RadioDuration,
    filter_duplicates: bool,
    duplicates: LegacyAdvertisingDuplicateFilter<DUPLICATE_FILTER_CAPACITY>,
    channel: AdvertisingChannel,
    next_anchor: Option<LeInstant>,
    outstanding: Option<Outstanding>,
    completion: Option<Status>,
}

impl Scanner {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            scan_type: ScanType::Passive,
            filter_policy: ScanFilterPolicy::AcceptAll,
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

    /// Whether the running scanner reports only the filter accept list.
    pub(crate) fn uses_accept_list(&self) -> bool {
        self.is_active() && self.filter_policy == ScanFilterPolicy::AcceptListOnly
    }

    pub(crate) fn enable(&mut self, request: LeLegacyScanningEnableRequest) {
        let parameters = request.parameters();
        self.scan_type = if parameters.is_active() {
            ScanType::Active
        } else {
            ScanType::Passive
        };
        self.filter_policy = if parameters.is_accept_list_only() {
            ScanFilterPolicy::AcceptListOnly
        } else {
            ScanFilterPolicy::AcceptAll
        };
        self.interval =
            RadioDuration::from_micros(u64::from(parameters.interval_units_625_us()) * 625);
        self.window = RadioDuration::from_micros(u64::from(parameters.window_units_625_us()) * 625);
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

    pub(crate) fn control_request(&mut self) -> Option<RadioWork<'static>> {
        match &mut self.phase {
            Phase::Configuring { sent: sent @ false } => {
                *sent = true;
                Some(RadioWork::Submit(RadioRequest::ConfigureScanner(
                    ScannerConfiguration {
                        scanner: SCANNER,
                        scan_type: self.scan_type,
                        filter_policy: self.filter_policy,
                        tx_power: TxPower::from_dbm(0),
                        phy: LePhy::Le1M,
                    },
                )))
            }
            Phase::Cancelling { sent: sent @ false } => {
                *sent = true;
                let id = self.outstanding.expect("a window is cancelled").id;
                Some(RadioWork::Cancel(id))
            }
            Phase::Removing { sent: sent @ false } => {
                *sent = true;
                Some(RadioWork::Submit(RadioRequest::RemoveScanner(SCANNER)))
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
        &self,
        next_anchor: Option<LeInstant>,
        earliest: LeInstant,
        timing: RadioTiming,
        busy: &[Option<LeWindow>],
    ) -> Result<Placement, PlanningError> {
        assert!(self.wants_window(), "only an eligible scanner is planned");
        let anchor = next_anchor.map_or(earliest, |anchor| anchor.max(earliest));
        let interval_end = anchor
            .checked_add(self.interval)
            .ok_or(error(C::IntervalEnd, TimingError::BeyondEpoch))?;
        let Some(slack) = self.interval.checked_sub(MINIMUM_SCAN_WINDOW) else {
            return Ok(Placement::Skipped {
                next_anchor: interval_end,
            });
        };
        let latest = anchor
            .checked_add(slack)
            .ok_or(error(C::MinimumWindow, TimingError::BeyondEpoch))?;
        let start = place(
            Proposal {
                earliest: anchor,
                latest,
                duration: MINIMUM_SCAN_WINDOW,
            },
            timing,
            busy,
        )
        .map_err(|cause| error(C::Reservation, cause))?;
        let Some(start) = start else {
            return Ok(Placement::Skipped {
                next_anchor: interval_end,
            });
        };
        // Clip the required duration before adding it to a late start. The
        // unrestricted requested end need not exist beyond this interval.
        let mut duration = interval_end
            .checked_duration_since(start)
            .ok_or(error(C::WindowClipping, TimingError::ReversedTime))?
            .min(self.window);
        for window in busy.iter().flatten() {
            if window.start() >= start {
                duration = duration.min(
                    window
                        .start()
                        .checked_duration_since(start)
                        .ok_or(error(C::WindowClipping, TimingError::ReversedTime))?,
                );
            }
        }
        if duration < MINIMUM_SCAN_WINDOW {
            return Ok(Placement::Skipped {
                next_anchor: interval_end,
            });
        }
        let window = LeWindow::new(start, duration)
            .map_err(|cause| error(C::WindowClipping, TimingError::Window(cause)))?;
        let reservation = timing
            .reservation(window)
            .map_err(|cause| error(C::Reservation, cause))?;
        Ok(Placement::Ready {
            window,
            reservation,
            next_anchor: interval_end,
        })
    }

    pub(crate) fn next_anchor(&self) -> Option<LeInstant> {
        self.next_anchor
    }

    pub(crate) fn commit_skip(&mut self, next_anchor: Option<LeInstant>) {
        self.next_anchor = next_anchor;
    }

    pub(crate) fn build(
        &mut self,
        id: EventId,
        window: LeWindow,
        reservation: LeWindow,
        next_anchor: LeInstant,
    ) -> RadioRequest<'static> {
        let channel = self.channel;
        self.outstanding = Some(Outstanding {
            id,
            channel,
            reservation,
        });
        self.next_anchor = Some(next_anchor);
        self.channel = next_channel(channel);
        RadioRequest::Scan(ScanWindow {
            id,
            scanner: SCANNER,
            channel,
            window,
        })
    }

    pub(crate) fn busy(&self) -> Option<LeWindow> {
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
                    // Only an active scanner asked for scan responses.
                    LegacyAdvertisingReportKind::ScanResponse
                        if self.scan_type == ScanType::Active =>
                    {
                        LeAdvEventKind::ScanRsp
                    }
                    // Without a filter policy for directed advertising the
                    // scanner reports no directed PDUs.
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

fn error(calculation: C, cause: TimingError) -> PlanningError {
    PlanningError::timing(R::Scanning, O::Event, calculation, cause)
}

pub(crate) enum Placement {
    Skipped {
        next_anchor: LeInstant,
    },
    Ready {
        window: LeWindow,
        reservation: LeWindow,
        next_anchor: LeInstant,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_window_clips_before_epoch_addition_and_commits_only_on_build() {
        let mut scanner = Scanner::new();
        scanner.phase = Phase::Running;
        scanner.interval = RadioDuration::from_micros(10_000);
        scanner.window = scanner.interval;
        let timing = crate::tests::TIMING;
        let earliest = LeInstant::from_micros(u64::MAX - 10_200);
        let busy = LeWindow::new(earliest, RadioDuration::from_micros(3_000)).unwrap();
        let Placement::Ready {
            window,
            reservation,
            next_anchor,
        } = scanner
            .place(None, earliest, timing, &[Some(busy)])
            .unwrap()
        else {
            panic!("a clipped window fits")
        };
        assert!(window.start().checked_add(scanner.window).is_none());
        assert_eq!(window.end(), next_anchor);
        assert_eq!(next_anchor, LeInstant::from_micros(u64::MAX - 200));
        assert_eq!(reservation.end(), next_anchor);
        assert_eq!(scanner.next_anchor(), None);
        assert_eq!(scanner.channel, AdvertisingChannel::Channel37);
        assert!(scanner.outstanding.is_none());
        let id = EventId::new(9);
        assert!(matches!(
            scanner.build(id, window, reservation, next_anchor),
            RadioRequest::Scan(_)
        ));
        assert!(scanner.owns(id));
        assert_eq!(scanner.next_anchor(), Some(next_anchor));
        assert_eq!(scanner.channel, AdvertisingChannel::Channel38);
    }

    #[test]
    fn interval_overflow_is_an_error_without_advancing_the_scanner() {
        let mut scanner = Scanner::new();
        scanner.phase = Phase::Running;
        scanner.interval = RadioDuration::from_micros(10_000);
        scanner.window = scanner.interval;
        let earliest = LeInstant::from_micros(u64::MAX - 9_999);
        for _ in 0..2 {
            assert!(matches!(
                scanner.place(None, earliest, crate::tests::TIMING, &[]),
                Err(PlanningError {
                    calculation: C::IntervalEnd,
                    cause: crate::PlanningCause::Timing(TimingError::BeyondEpoch),
                    ..
                })
            ));
            assert!(scanner.outstanding.is_none());
            assert_eq!(scanner.next_anchor(), None);
            assert_eq!(scanner.channel, AdvertisingChannel::Channel37);
        }
        scanner.disable();
        assert!(matches!(
            scanner.control_request(),
            Some(RadioWork::Submit(RadioRequest::RemoveScanner(_)))
        ));
    }
}
