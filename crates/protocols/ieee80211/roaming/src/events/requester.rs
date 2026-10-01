use super::*;
use crate::reports::ReportWindow;
use crate::{Body, Error, LinkIdentity, OperationId, Transmission, TxOutcome};
use oer_time::{Duration, Instant};

/// Initial response windows retain complete frames. Frequent-transition alerts
/// remain correlated until replacement/cancellation, independently of that window.
/// Autonomous and monitoring histories are bounded and explicitly drained.
pub struct EventRequester<const BYTES: usize, const FRAMES: usize> {
    window: ReportWindow<BYTES, FRAMES>,
    autonomous: [Option<Body<BYTES>>; FRAMES],
    monitoring: [Option<Body<BYTES>>; FRAMES],
    subscription: Option<(OperationId, Body<BYTES>)>,
    incapable: [bool; crate::OCTET_VALUE_COUNT],
    monitor_active: [bool; crate::OCTET_VALUE_COUNT],
    last_update: Instant,
}
impl<const B: usize, const F: usize> EventRequester<B, F> {
    pub fn new(link: LinkIdentity, window: Duration) -> Result<Self, ReportError> {
        Ok(Self {
            window: ReportWindow::new(link, window)?,
            autonomous: core::array::from_fn(|_| None),
            monitoring: core::array::from_fn(|_| None),
            subscription: None,
            incapable: [false; crate::OCTET_VALUE_COUNT],
            monitor_active: [false; crate::OCTET_VALUE_COUNT],
            last_update: Instant::EPOCH,
        })
    }
    fn time(&self, now: Instant) -> Result<(), ReportError> {
        self.window.time(now)?;
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub fn peer_incapable(&self, kind: EventType) -> bool {
        self.incapable[usize::from(kind.0)]
    }
    pub fn request(
        &mut self,
        request: EventRequestFrame<'_>,
        now: Instant,
    ) -> Result<RequestReplacement, ReportError> {
        self.time(now)?;
        for r in request.requests()? {
            if self.peer_incapable(r.kind) {
                return Err(ReportError::UnsupportedRequest);
            }
        }
        let body = Body::encode(|out| request.encode(out))?;
        let monitor = request.requests()?.any(|r| {
            r.kind == EventType::TRANSITION
                && r.conditions
                    .iter()
                    .any(|e| e.id == transition_condition_id::FREQUENT_TRANSITION)
        });
        let retained = monitor.then(|| body.clone());
        let replacement = self.window.start(body, now)?;
        self.monitor_active.fill(false);
        self.subscription = retained.map(|body| (replacement.id, body));
        self.monitoring = core::array::from_fn(|_| None);
        self.last_update = now;
        Ok(replacement)
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<ReportWindowEvent, ReportError> {
        if link != self.window.exchange.ids.link {
            return Ok(ReportWindowEvent::Ignored);
        }
        self.time(now)?;
        let frame = EventReportFrame::parse(bytes)?;
        for report in frame.reports()? {
            if report.status == EventReportStatus::FREQUENT_TRANSITION
                && report.kind != EventType::TRANSITION
            {
                return Err(ReportError::InvalidResult);
            }
            if !report.status.is_known() {
                return Err(ReportError::UnsupportedStatus(report.status.0));
            }
        }
        if frame.dialog_token == AUTONOMOUS_DIALOG_TOKEN {
            let added = store(&mut self.autonomous, bytes)?;
            self.last_update = now;
            return Ok(if added {
                ReportWindowEvent::AutonomousReceived
            } else {
                ReportWindowEvent::Ignored
            });
        }
        let live = self
            .window
            .exchange
            .matches(link, frame.dialog_token, now)?;
        let (id, request) = if live {
            let p = self
                .window
                .exchange
                .pending
                .as_ref()
                .expect("matched request");
            (p.id, EventRequestFrame::parse(p.body.bytes())?)
        } else if self.monitor_active.iter().any(|active| *active)
            && let Some((id, body)) = &self.subscription
        {
            let request = EventRequestFrame::parse(body.bytes())?;
            if request.dialog_token != frame.dialog_token {
                return Ok(ReportWindowEvent::Ignored);
            }
            (*id, request)
        } else {
            return Ok(ReportWindowEvent::Ignored);
        };
        let mut incapable = self.incapable;
        let mut count = 0;
        let mut monitor_active = self.monitor_active;
        for report in frame.reports()? {
            let requested = request
                .requests()?
                .find(|r| r.token == report.token)
                .ok_or(ReportError::UnexpectedToken(report.token))?;
            if report.kind != requested.kind {
                return Err(ReportError::UnexpectedType(report.kind.0));
            }
            if !live
                && (!self.monitor_active[usize::from(report.token)]
                    || report.status != EventReportStatus::FREQUENT_TRANSITION
                    || requested.kind != EventType::TRANSITION
                    || requested.frequent_transition()?.is_none())
            {
                return Ok(ReportWindowEvent::Ignored);
            }
            if requested.kind == EventType::TRANSITION && requested.frequent_transition()?.is_some()
            {
                monitor_active[usize::from(report.token)] = matches!(
                    report.status,
                    EventReportStatus::SUCCESSFUL | EventReportStatus::FREQUENT_TRANSITION
                );
            }
            if report.status == EventReportStatus::INCAPABLE {
                incapable[usize::from(report.kind.0)] = true;
            }
            count += 1;
        }
        let event = if live {
            self.window.push(link, frame.dialog_token, bytes, now)?
        } else if count != 0 && store(&mut self.monitoring, bytes)? {
            ReportWindowEvent::MonitoringReportReceived { id }
        } else {
            ReportWindowEvent::Ignored
        };
        self.incapable = incapable;
        self.monitor_active = monitor_active;
        self.last_update = now;
        Ok(event)
    }
    pub fn reports(&self) -> impl Iterator<Item = EventReportFrame<'_>> {
        self.window
            .frames()
            .map(|b| EventReportFrame::parse(b).expect("retained report"))
    }
    pub fn autonomous_reports(&self) -> impl Iterator<Item = EventReportFrame<'_>> {
        self.autonomous
            .iter()
            .flatten()
            .map(|b| EventReportFrame::parse(b.bytes()).expect("retained autonomous report"))
    }
    pub fn monitoring_reports(&self) -> impl Iterator<Item = EventReportFrame<'_>> {
        self.monitoring
            .iter()
            .flatten()
            .map(|b| EventReportFrame::parse(b.bytes()).expect("retained monitoring report"))
    }
    pub fn clear_autonomous_reports(&mut self) {
        self.autonomous = core::array::from_fn(|_| None);
    }
    pub fn clear_monitoring_reports(&mut self) {
        self.monitoring = core::array::from_fn(|_| None);
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.window.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), ReportError> {
        self.time(now)?;
        self.window.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<ReportWindowEvent, ReportError> {
        if self
            .window
            .exchange
            .pending
            .as_ref()
            .is_none_or(|p| p.id != id)
        {
            return Ok(ReportWindowEvent::Ignored);
        }
        self.time(now)?;
        let event = self.window.completed(id, outcome, now)?;
        if matches!(
            event,
            ReportWindowEvent::TxFailed { .. } | ReportWindowEvent::TimedOut { .. }
        ) {
            self.subscription = None;
            self.monitor_active.fill(false);
        }
        self.last_update = now;
        Ok(event)
    }
    pub fn poll(&mut self, now: Instant) -> Result<ReportWindowEvent, ReportError> {
        self.time(now)?;
        let event = self.window.poll(now)?;
        if matches!(event, ReportWindowEvent::TimedOut { .. }) {
            self.subscription = None;
            self.monitor_active.fill(false);
        }
        self.last_update = now;
        Ok(event)
    }
    pub fn cancel(&mut self) -> ReportWindowEvent {
        self.subscription = None;
        self.monitor_active.fill(false);
        self.monitoring = core::array::from_fn(|_| None);
        self.window.cancel()
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.window.exchange.next_deadline()
    }
}
fn store<const B: usize, const F: usize>(
    history: &mut [Option<Body<B>>; F],
    bytes: &[u8],
) -> Result<bool, ReportError> {
    if history.iter().flatten().any(|b| b.bytes() == bytes) {
        return Ok(false);
    }
    let index = history
        .iter()
        .position(Option::is_none)
        .ok_or(ReportError::Full)?;
    let body = Body::copy(bytes)?;
    history[index] = Some(body);
    Ok(true)
}
