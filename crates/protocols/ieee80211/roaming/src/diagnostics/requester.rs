use super::*;
use crate::reports::ReportWindow;
use crate::{Body, LinkIdentity, OperationId, Transmission, TxOutcome};
use oer_time::{Duration, Instant};
/// Complete response windows permit multiple configuration profiles and frames.
pub struct DiagnosticRequester<const BYTES: usize, const FRAMES: usize> {
    window: ReportWindow<BYTES, FRAMES>,
    incapable: [bool; crate::OCTET_VALUE_COUNT],
}
impl<const B: usize, const F: usize> DiagnosticRequester<B, F> {
    pub fn new(link: LinkIdentity, window: Duration) -> Result<Self, ReportError> {
        Ok(Self {
            window: ReportWindow::new(link, window)?,
            incapable: [false; crate::OCTET_VALUE_COUNT],
        })
    }
    pub fn peer_incapable(&self, kind: DiagnosticType) -> bool {
        self.incapable[usize::from(kind.0)]
    }
    pub fn request(
        &mut self,
        request: DiagnosticRequestFrame<'_>,
        now: Instant,
    ) -> Result<RequestReplacement, ReportError> {
        for r in request.requests()? {
            if self.peer_incapable(r.kind) {
                return Err(ReportError::UnsupportedRequest);
            }
        }
        let body = Body::encode(|out| request.encode(out))?;
        self.window.start(body, now)
    }
    pub fn cancel_remote(
        &mut self,
        dialog_token: u8,
        now: Instant,
    ) -> Result<RequestReplacement, ReportError> {
        let mut element = [0; DiagnosticRequest::FIXED_ENCODED_LEN];
        DiagnosticRequest {
            token: 0,
            kind: DiagnosticType::CANCEL,
            timeout_seconds: 0,
            information: Elements::EMPTY,
        }
        .encode(&mut element)?;
        self.request(
            DiagnosticRequestFrame {
                dialog_token,
                elements: Elements::parse(&element)?,
            },
            now,
        )
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
        self.window.time(now)?;
        let frame = DiagnosticReportFrame::parse(bytes)?;
        if !self
            .window
            .exchange
            .matches(link, frame.dialog_token, now)?
        {
            return Ok(ReportWindowEvent::Ignored);
        }
        let pending = self
            .window
            .exchange
            .pending
            .as_ref()
            .expect("matched request");
        let request = DiagnosticRequestFrame::parse(pending.body.bytes())?;
        let mut incapable = self.incapable;
        for report in frame.reports()? {
            let requested = request
                .requests()?
                .find(|r| r.token == report.token)
                .ok_or(ReportError::UnexpectedToken(report.token))?;
            super::validate_result(requested, report)?;
            if report.status == DiagnosticReportStatus::INCAPABLE {
                incapable[usize::from(report.kind.0)] = true;
            }
            if report.kind == DiagnosticType::CONFIGURATION
                && report.status == DiagnosticReportStatus::SUCCESSFUL
            {
                let profile = report
                    .information
                    .unique(diagnostic_information_id::PROFILE_ID)?
                    .expect("validated profile");
                for other in frame.reports()? {
                    if other.token == report.token
                        && other.kind == report.kind
                        && other.status == DiagnosticReportStatus::SUCCESSFUL
                        && other
                            .information
                            .unique(diagnostic_information_id::PROFILE_ID)?
                            == Some(profile)
                        && other.information != report.information
                    {
                        return Err(ReportError::InvalidResult);
                    }
                }
                for old in self.window.frames() {
                    for old in DiagnosticReportFrame::parse(old)?.reports()? {
                        if old.token == report.token
                            && old.kind == report.kind
                            && old.status == DiagnosticReportStatus::SUCCESSFUL
                            && old
                                .information
                                .unique(diagnostic_information_id::PROFILE_ID)?
                                == Some(profile)
                            && old.information != report.information
                        {
                            return Err(ReportError::InvalidResult);
                        }
                    }
                }
            }
        }
        let event = self.window.push(link, frame.dialog_token, bytes, now)?;
        self.incapable = incapable;
        Ok(event)
    }
    pub fn reports(&self) -> impl Iterator<Item = DiagnosticReportFrame<'_>> {
        self.window
            .frames()
            .map(|b| DiagnosticReportFrame::parse(b).expect("retained report"))
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.window.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), ReportError> {
        self.window.admitted(id, now)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<ReportWindowEvent, ReportError> {
        self.window.completed(id, outcome, now)
    }
    pub fn poll(&mut self, now: Instant) -> Result<ReportWindowEvent, ReportError> {
        self.window.poll(now)
    }
    pub fn cancel(&mut self) -> ReportWindowEvent {
        self.window.cancel()
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.window.exchange.next_deadline()
    }
}
