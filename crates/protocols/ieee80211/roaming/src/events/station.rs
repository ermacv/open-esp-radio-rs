use super::*;
use crate::management::{PeerDialog, Reception};
use crate::reports::{ReportBatch, ReportConnectivity, ReportDelivery, route};
use crate::{Body, DialogEvent, Error, LinkIdentity, OperationId, TxOutcome};
use oer_ieee80211_mac::management::IEEE_TIME_UNIT_MICROS;
use oer_ieee80211_mac::roaming as wire;
use oer_time::{Duration, Instant};
const ASSOCIATION_END: Instant = Instant::from_micros(u64::MAX);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventStationEvent {
    Ignored,
    Requested {
        id: OperationId,
        superseded: Option<OperationId>,
    },
    ReplayReady {
        id: OperationId,
    },
    Transmitted {
        id: OperationId,
    },
    TxFailed {
        id: OperationId,
    },
    TimedOut {
        id: OperationId,
    },
}
/// Per-element resource/power policy supplied before publishing a response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventAdmission {
    Accept,
    Failed,
    Refused,
    Incapable,
}
#[derive(Clone, Copy)]
enum TxSource {
    History,
    Alerts,
    Autonomous,
}
/// Independent initial-report snapshots and a bounded alert outbox. The journal
/// remains externally owned so another BSS dialog can use it after roaming.
// CAPABILITY: wifi-roaming-and-service-discovery-event-reporting-802-11v
pub struct EventStation<const BYTES: usize, const FRAMES: usize> {
    dialog: PeerDialog<BYTES>,
    network: NetworkIdentity,
    format: FrequentTransitionFormat,
    history: ReportBatch<BYTES, FRAMES>,
    alerts: ReportBatch<BYTES, FRAMES>,
    prepared: bool,
    accepted: [bool; crate::OCTET_VALUE_COUNT],
    alerted: [u64; crate::OCTET_VALUE_COUNT],
    tx: Option<(OperationId, TxSource)>,
}
impl<const B: usize, const F: usize> EventStation<B, F> {
    pub fn new(
        link: LinkIdentity,
        network: NetworkIdentity,
        tx_timeout: Duration,
        format: FrequentTransitionFormat,
    ) -> Result<Self, ReportError> {
        if F == 0 {
            return Err(ReportError::Full);
        }
        Ok(Self {
            dialog: PeerDialog::new(link, tx_timeout)?,
            network,
            format,
            history: ReportBatch::new(),
            alerts: ReportBatch::new(),
            prepared: false,
            accepted: [false; crate::OCTET_VALUE_COUNT],
            alerted: [0; crate::OCTET_VALUE_COUNT],
            tx: None,
        })
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<EventStationEvent, ReportError> {
        if link != self.dialog.ids.link {
            return Ok(EventStationEvent::Ignored);
        }
        EventRequestFrame::parse(bytes)?;
        let reception = self
            .dialog
            .receive_until(link, bytes, now, ASSOCIATION_END)?;
        match reception {
            Reception::Requested { id, superseded } => {
                self.history.clear();
                self.alerts.clear();
                self.prepared = false;
                self.accepted = [false; crate::OCTET_VALUE_COUNT];
                self.alerted = [0; crate::OCTET_VALUE_COUNT];
                self.tx = None;
                Ok(EventStationEvent::Requested { id, superseded })
            }
            Reception::Ignored if self.prepared && self.history.done() && self.tx.is_none() => {
                self.history.rewind();
                Ok(EventStationEvent::ReplayReady {
                    id: self.dialog.incoming.as_ref().expect("live request").id,
                })
            }
            _ => Ok(EventStationEvent::Ignored),
        }
    }
    pub fn request(&self) -> Option<(OperationId, EventRequestFrame<'_>)> {
        self.dialog.incoming.as_ref().map(|p| {
            (
                p.id,
                EventRequestFrame::parse(p.body.bytes()).expect("validated request"),
            )
        })
    }
    /// Prepare the entire response before publishing any frames or changing the
    /// alert subscription. Limits select the most recent matching journal entries.
    pub fn prepare_reports<const N: usize, const J: usize>(
        &mut self,
        id: OperationId,
        journal: &EventJournal<N, J>,
        now: Instant,
    ) -> Result<(), ReportError> {
        self.prepare_with_admission(id, journal, now, |_| EventAdmission::Accept)
    }
    /// Preflight supplied admission policy for the whole response. The callback
    /// must only inspect facts and must not change external resources.
    pub fn prepare_with_admission<const N: usize, const J: usize>(
        &mut self,
        id: OperationId,
        journal: &EventJournal<N, J>,
        now: Instant,
        mut admission: impl FnMut(EventRequest<'_>) -> EventAdmission,
    ) -> Result<(), ReportError> {
        self.dialog.time(now)?;
        journal.time(now)?;
        if journal.network() != self.network {
            return Err(ReportError::WrongNetwork);
        }
        if self.prepared {
            return Err(Error::Busy.into());
        }
        let (current, frame) = self.request().ok_or(Error::NoPendingOperation)?;
        if current != id {
            return Err(Error::WrongOperation.into());
        }
        let mut batch = ReportBatch::new();
        let mut requests = 0;
        let mut accepted = [false; crate::OCTET_VALUE_COUNT];
        for request in frame.requests()? {
            requests += 1;
            let incapable = !request.kind.original()
                || request
                    .frequent_transition()?
                    .is_some_and(|condition| usize::from(condition.minimum_count) > N);
            let status = if incapable {
                EventReportStatus::INCAPABLE
            } else {
                match admission(request) {
                    EventAdmission::Accept => EventReportStatus::SUCCESSFUL,
                    EventAdmission::Failed => EventReportStatus::FAILED,
                    EventAdmission::Refused => EventReportStatus::REFUSED,
                    EventAdmission::Incapable => EventReportStatus::INCAPABLE,
                }
            };
            accepted[usize::from(request.token)] = status == EventReportStatus::SUCCESSFUL;
            let mut count = 0;
            if status == EventReportStatus::SUCCESSFUL {
                for (_, recorded, mut report) in journal.records(request.kind) {
                    if !super::matches(request, report)? {
                        continue;
                    }
                    if request.response_limit != ALL_RETAINED_EVENTS
                        && count >= usize::from(request.response_limit)
                    {
                        break;
                    }
                    if let EventData::PeerLink(mut peer) = report.data
                        && peer.status.active()
                    {
                        let elapsed = (now.as_micros() - recorded.as_micros())
                            / Duration::from_secs(1).as_micros();
                        let value = u64::from(peer.connection_seconds)
                            .checked_add(elapsed)
                            .ok_or(Error::TimeOverflow)?;
                        if value > u64::from(MAX_PEER_CONNECTION_SECONDS) {
                            return Err(ReportError::InvalidResult);
                        }
                        peer.connection_seconds = value as u32;
                        report.data = EventData::PeerLink(peer);
                    }
                    report.token = request.token;
                    let mut bytes = [0; wire::MAX_ENCODED_ELEMENT_LEN];
                    let n = report.encode(&mut bytes)?;
                    batch.pack_element(WnmAction::EventReport, frame.dialog_token, &bytes[..n])?;
                    count += 1;
                }
            }
            if count == 0 {
                let report = EventReport {
                    token: request.token,
                    kind: request.kind,
                    status,
                    timing: None,
                    data: EventData::Empty,
                };
                let mut bytes = [0; wire::MAX_ENCODED_ELEMENT_LEN];
                let n = report.encode(&mut bytes)?;
                batch.pack_element(WnmAction::EventReport, frame.dialog_token, &bytes[..n])?;
            }
        }
        if requests == 0 {
            batch.append(WnmAction::EventReport, frame.dialog_token, Elements::EMPTY)?;
        }
        self.history = batch;
        self.accepted = accepted;
        self.prepared = true;
        self.dialog.last_update = now;
        Ok(())
    }
    /// Feed each newly logged transition. A repeated event identity emits no
    /// second alert. Thresholds larger than journal capacity were reported incapable.
    pub fn transition_logged<const N: usize, const J: usize>(
        &mut self,
        journal: &EventJournal<N, J>,
        event: JournalEventId,
        now: Instant,
    ) -> Result<usize, ReportError> {
        self.dialog.time(now)?;
        journal.time(now)?;
        if journal.network() != self.network || event.network != self.network {
            return Err(ReportError::WrongNetwork);
        }
        if event.kind != EventType::TRANSITION {
            return Err(ReportError::UnexpectedType(event.kind.0));
        }
        let (recorded, last) = journal.get(event).ok_or(ReportError::HistoryTooShort)?;
        let Some(current) = &self.dialog.incoming else {
            return Ok(0);
        };
        if !self.prepared || recorded < current.received {
            return Ok(0);
        }
        let frame = EventRequestFrame::parse(current.body.bytes())?;
        let mut batch = self.alerts.clone();
        let mut alerted = self.alerted;
        let mut count = 0;
        for request in frame.requests()? {
            if !self.accepted[usize::from(request.token)]
                || request.kind != EventType::TRANSITION
                || alerted[usize::from(request.token)] >= event.sequence()
            {
                continue;
            }
            let Some(frequent) = request.frequent_transition()? else {
                continue;
            };
            if usize::from(frequent.minimum_count) > N {
                continue;
            }
            let interval = u64::from(frequent.interval_tu) * IEEE_TIME_UNIT_MICROS;
            let since = Instant::from_micros(recorded.as_micros().saturating_sub(interval));
            // The field definition specifies a minimum count, hence >=.
            if journal.transition_count(since) < usize::from(frequent.minimum_count)
                || !super::matches(request, last)?
            {
                continue;
            }
            let mut report = last;
            report.token = request.token;
            report.status = EventReportStatus::FREQUENT_TRANSITION;
            if self.format == FrequentTransitionFormat::StatusOnly {
                report.timing = None;
                report.data = EventData::Empty;
            }
            let mut bytes = [0; wire::MAX_ENCODED_ELEMENT_LEN];
            let n = report.encode_frequent(self.format, &mut bytes)?;
            batch.append(
                WnmAction::EventReport,
                frame.dialog_token,
                Elements::parse(&bytes[..n])?,
            )?;
            alerted[usize::from(request.token)] = event.sequence();
            count += 1;
        }
        self.alerts = batch;
        self.alerted = alerted;
        self.dialog.last_update = now;
        Ok(count)
    }
    pub fn send_next(&mut self, now: Instant) -> Result<Option<OperationId>, ReportError> {
        self.dialog.time(now)?;
        if self.tx.is_some() {
            return Err(Error::Busy.into());
        }
        let (current, _) = self.request().ok_or(Error::NoPendingOperation)?;
        let (body, source) = if let Some(body) = self.history.head() {
            (body.clone(), TxSource::History)
        } else if let Some(body) = self.alerts.head() {
            (body.clone(), TxSource::Alerts)
        } else {
            return Ok(None);
        };
        let id = self.dialog.reply(current, body, now, false)?;
        self.tx = Some((id, source));
        Ok(Some(id))
    }
    pub fn send_autonomous(
        &mut self,
        reports: Elements<'_>,
        now: Instant,
    ) -> Result<OperationId, ReportError> {
        let frame = EventReportFrame {
            dialog_token: AUTONOMOUS_DIALOG_TOKEN,
            elements: reports,
        };
        for report in frame.reports()? {
            if report.status == EventReportStatus::FREQUENT_TRANSITION
                && (report.timing.is_some()
                    != (self.format == FrequentTransitionFormat::WithLastEvent))
            {
                return Err(ReportError::InvalidResult);
            }
        }
        let body = Body::encode(|out| frame.encode_frequent(self.format, out))?;
        if self.tx.is_some() {
            return Err(Error::Busy.into());
        }
        let id = self.dialog.send(body, now)?;
        self.tx = Some((id, TxSource::Autonomous));
        Ok(id)
    }
    pub fn delivery(
        &self,
        connectivity: ReportConnectivity,
        now: Instant,
    ) -> Result<Option<ReportDelivery<'_>>, ReportError> {
        self.dialog.time(now)?;
        let Some(tx) = self.dialog.transmission() else {
            return Ok(None);
        };
        let uri = if tx.body[wire::DIALOG_TOKEN_OFFSET] == AUTONOMOUS_DIALOG_TOKEN {
            None
        } else {
            self.request()
                .map(|(_, r)| DestinationUri::parse(r.elements))
                .transpose()?
                .flatten()
        };
        route(tx, uri, connectivity, now)
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), ReportError> {
        Ok(self.dialog.admitted(id, now)?)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<EventStationEvent, ReportError> {
        let Some((active, source)) = self.tx else {
            return Ok(EventStationEvent::Ignored);
        };
        if active != id {
            return Ok(EventStationEvent::Ignored);
        }
        let event = self.dialog.completed(id, outcome, now)?;
        let result = match event {
            DialogEvent::Transmitted => {
                match source {
                    TxSource::History => self.history.acknowledge(),
                    TxSource::Alerts => {
                        self.alerts.acknowledge();
                        self.alerts.retire_delivered();
                    }
                    TxSource::Autonomous => {}
                }
                EventStationEvent::Transmitted { id }
            }
            DialogEvent::TxFailed => EventStationEvent::TxFailed { id },
            DialogEvent::TimedOut => EventStationEvent::TimedOut { id },
            _ => return Ok(EventStationEvent::Ignored),
        };
        self.tx = None;
        Ok(result)
    }
    pub fn poll(&mut self, now: Instant) -> Result<EventStationEvent, ReportError> {
        let (_, expired) = self.dialog.poll(now)?;
        if let Some(id) = expired {
            self.tx = None;
            Ok(EventStationEvent::TimedOut { id })
        } else {
            Ok(EventStationEvent::Ignored)
        }
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.dialog
            .next_deadline()
            .filter(|i| *i != ASSOCIATION_END)
    }
    /// On a BSS transition, retire the dialog; keep `EventJournal` for the same network.
    pub fn cancel(&mut self) -> Option<OperationId> {
        self.history.clear();
        self.alerts.clear();
        self.tx = None;
        self.prepared = false;
        self.dialog.cancel()
    }
}
