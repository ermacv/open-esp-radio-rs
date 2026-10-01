use super::*;
use crate::management::{PeerDialog, Reception};
use crate::reports::{ReportBatch, ReportConnectivity, ReportDelivery, route};
use crate::{Body, DialogEvent, Error, LinkIdentity, OperationId, TxOutcome, deadline};
use oer_ieee80211_mac::roaming as wire;
use oer_time::{Duration, Instant};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticWorkId {
    request: OperationId,
    token: u8,
}
impl DiagnosticWorkId {
    pub const fn request(self) -> OperationId {
        self.request
    }
    pub const fn token(self) -> u8 {
        self.token
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticWork<'a> {
    pub id: DiagnosticWorkId,
    pub request: DiagnosticRequest<'a>,
    pub until: Instant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticAdmission {
    /// Facts/policy only: confirm backend support, current-network membership,
    /// permitted channel and availability of the requested profile/credentials.
    Accept,
    Failed,
    Refused,
    Incapable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticStationEvent {
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
    WorkExpired {
        id: DiagnosticWorkId,
        admitted: bool,
    },
    ReportExpired {
        id: DiagnosticWorkId,
    },
    Cancelled {
        id: OperationId,
    },
    DiagnosticTransition {
        id: DiagnosticWorkId,
    },
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Ready,
    Admitted,
    Completed,
    Expired,
}
#[derive(Clone, Copy)]
struct Slot {
    id: DiagnosticWorkId,
    kind: DiagnosticType,
    until: Instant,
    phase: Phase,
}
/// Each test has the exact requested timeout and local work identity. Active
/// tests are admitted separately, and their causal BSS transition preserves work.
// CAPABILITY: wifi-roaming-and-service-discovery-diagnostic-reporting-802-11v
pub struct DiagnosticStation<const BYTES: usize, const JOBS: usize, const FRAMES: usize> {
    dialog: PeerDialog<BYTES>,
    network: NetworkIdentity,
    tx_timeout: Duration,
    jobs: [Option<Slot>; JOBS],
    outbox: ReportBatch<BYTES, FRAMES>,
    tx: Option<OperationId>,
    away: Option<DiagnosticWorkId>,
}
impl<const B: usize, const J: usize, const F: usize> DiagnosticStation<B, J, F> {
    pub fn new(
        link: LinkIdentity,
        network: NetworkIdentity,
        tx_timeout: Duration,
    ) -> Result<Self, ReportError> {
        if J == 0 || F == 0 {
            return Err(ReportError::Full);
        }
        Ok(Self {
            dialog: PeerDialog::new(link, tx_timeout)?,
            network,
            tx_timeout,
            jobs: [None; J],
            outbox: ReportBatch::new(),
            tx: None,
            away: None,
        })
    }
    /// Admission callbacks return supplied facts/policy; they must not start work
    /// or change external resources while the whole frame is being preflighted.
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
        mut admission: impl FnMut(DiagnosticRequest<'_>) -> DiagnosticAdmission,
    ) -> Result<DiagnosticStationEvent, ReportError> {
        if link != self.dialog.ids.link || self.away.is_some() {
            return Ok(DiagnosticStationEvent::Ignored);
        }
        self.dialog.time(now)?;
        let frame = DiagnosticRequestFrame::parse(bytes)?;
        if let Some(current) = &self.dialog.incoming
            && now < current.until
            && current.body.bytes()[wire::DIALOG_TOKEN_OFFSET] == frame.dialog_token
        {
            if current.body.bytes() != bytes {
                return Err(Error::ConflictingDialog.into());
            }
            if self.outbox.done()
                && self.tx.is_none()
                && self
                    .jobs
                    .iter()
                    .flatten()
                    .all(|s| matches!(s.phase, Phase::Completed | Phase::Expired))
            {
                self.outbox.rewind();
                return Ok(DiagnosticStationEvent::ReplayReady { id: current.id });
            }
            return Ok(DiagnosticStationEvent::Ignored);
        }
        Body::<B>::copy(bytes)?;
        let count = frame.requests()?.count();
        if count > J {
            return Err(ReportError::Full);
        }
        let serial = self
            .dialog
            .ids
            .next
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        let cancelled = frame.requests()?.any(|r| r.kind == DiagnosticType::CANCEL);
        let request_id = OperationId {
            link: self.dialog.ids.link,
            serial,
        };
        let mut jobs = [None; J];
        let mut outbox = ReportBatch::new();
        let mut lease = deadline(now, self.tx_timeout)?;
        for (index, request) in frame.requests()?.enumerate() {
            let until = if cancelled {
                deadline(now, self.tx_timeout)?
            } else {
                now.checked_add(Duration::from_secs(u32::from(request.timeout_seconds)))
                    .ok_or(Error::TimeOverflow)?
            };
            lease = lease.max(until);
            let status = if cancelled {
                Some(DiagnosticReportStatus::CANCELLED)
            } else {
                if let Some(target) = request.target()?
                    && (!crate::valid_peer_address(target.bssid) || target.channel == 0)
                {
                    return Err(Error::InvalidTarget.into());
                }
                match admission(request) {
                    DiagnosticAdmission::Accept => None,
                    DiagnosticAdmission::Failed => Some(DiagnosticReportStatus::FAILED),
                    DiagnosticAdmission::Refused => Some(DiagnosticReportStatus::REFUSED),
                    DiagnosticAdmission::Incapable => Some(DiagnosticReportStatus::INCAPABLE),
                }
            };
            jobs[index] = Some(Slot {
                id: DiagnosticWorkId {
                    request: request_id,
                    token: request.token,
                },
                kind: request.kind,
                until,
                phase: if status.is_some() {
                    Phase::Completed
                } else {
                    Phase::Ready
                },
            });
            if let Some(status) = status {
                let mut encoded = [0; wire::MAX_ENCODED_ELEMENT_LEN];
                let n = DiagnosticReport {
                    token: request.token,
                    kind: request.kind,
                    status,
                    information: Elements::EMPTY,
                }
                .encode(&mut encoded)?;
                outbox.append(
                    WnmAction::DiagnosticReport,
                    frame.dialog_token,
                    Elements::parse(&encoded[..n])?,
                )?;
            }
        }
        let reception = self.dialog.receive_until(link, bytes, now, lease)?;
        let Reception::Requested { id, superseded } = reception else {
            return Ok(DiagnosticStationEvent::Ignored);
        };
        self.jobs = jobs;
        self.outbox = outbox;
        self.tx = None;
        self.away = None;
        Ok(DiagnosticStationEvent::Requested { id, superseded })
    }
    pub fn request(&self) -> Option<(OperationId, DiagnosticRequestFrame<'_>)> {
        self.dialog.incoming.as_ref().map(|p| {
            (
                p.id,
                DiagnosticRequestFrame::parse(p.body.bytes()).expect("validated request"),
            )
        })
    }
    pub fn works(&self) -> impl Iterator<Item = DiagnosticWork<'_>> {
        self.jobs
            .iter()
            .flatten()
            .filter(|s| s.phase == Phase::Ready)
            .map(|slot| {
                let (_, frame) = self.request().expect("work owns a request");
                let request = frame
                    .requests()
                    .expect("validated request")
                    .find(|r| r.token == slot.id.token)
                    .expect("retained job");
                DiagnosticWork {
                    id: slot.id,
                    request,
                    until: slot.until,
                }
            })
    }
    pub fn admitted_work(&mut self, id: DiagnosticWorkId, now: Instant) -> Result<(), ReportError> {
        self.dialog.time(now)?;
        let slot = self
            .jobs
            .iter_mut()
            .flatten()
            .find(|s| s.id == id)
            .ok_or(ReportError::WrongWork)?;
        if now >= slot.until {
            return Err(ReportError::ExpiredWork);
        }
        if slot.phase != Phase::Ready {
            return Err(Error::AlreadyAdmitted.into());
        }
        slot.phase = Phase::Admitted;
        self.dialog.last_update = now;
        Ok(())
    }
    /// Complete elements may describe several profiles; every element belongs
    /// to this exact task. Active successes require prior execution admission.
    pub fn complete_work(
        &mut self,
        id: DiagnosticWorkId,
        reports: Elements<'_>,
        now: Instant,
    ) -> Result<(), ReportError> {
        self.dialog.time(now)?;
        let slot = self
            .jobs
            .iter()
            .flatten()
            .find(|s| s.id == id)
            .ok_or(ReportError::WrongWork)?;
        if now >= slot.until {
            return Err(ReportError::ExpiredWork);
        }
        if matches!(slot.phase, Phase::Completed | Phase::Expired) {
            return Err(ReportError::WrongWork);
        }
        let (_, frame) = self.request().ok_or(Error::NoPendingOperation)?;
        let request = frame
            .requests()?
            .find(|r| r.token == id.token)
            .ok_or(ReportError::WrongWork)?;
        let mut count = 0;
        let mut profiles = [false; crate::OCTET_VALUE_COUNT];
        for e in reports.iter() {
            if e.id != element_id::DIAGNOSTIC_REPORT {
                return Err(ReportError::InvalidResult);
            }
            let report = DiagnosticReport::parse(e.body)?;
            super::validate_result(request, report)?;
            if report.status == DiagnosticReportStatus::SUCCESSFUL
                && slot.kind.active()
                && slot.phase != Phase::Admitted
            {
                return Err(Error::NotAdmitted.into());
            }
            if report.status == DiagnosticReportStatus::SUCCESSFUL
                && slot.kind == DiagnosticType::CONFIGURATION
            {
                let profile = usize::from(
                    report
                        .information
                        .unique(diagnostic_information_id::PROFILE_ID)?
                        .expect("validated profile")[0],
                );
                if profiles[profile] {
                    return Err(ReportError::InvalidResult);
                }
                profiles[profile] = true;
            }
            count += 1;
        }
        if count == 0 {
            return Err(ReportError::IncompleteResults);
        }
        let mut outbox = self.outbox.clone();
        outbox.append(WnmAction::DiagnosticReport, frame.dialog_token, reports)?;
        self.outbox = outbox;
        self.jobs
            .iter_mut()
            .flatten()
            .find(|s| s.id == id)
            .expect("checked slot")
            .phase = Phase::Completed;
        self.dialog.last_update = now;
        Ok(())
    }
    pub fn cancel_work(&mut self, id: DiagnosticWorkId, now: Instant) -> Result<(), ReportError> {
        let slot = self
            .jobs
            .iter()
            .flatten()
            .find(|s| s.id == id)
            .ok_or(ReportError::WrongWork)?;
        let mut bytes = [0; DiagnosticReport::FIXED_ENCODED_LEN];
        DiagnosticReport {
            token: id.token,
            kind: slot.kind,
            status: DiagnosticReportStatus::CANCELLED,
            information: Elements::EMPTY,
        }
        .encode(&mut bytes)?;
        self.complete_work(id, Elements::parse(&bytes)?, now)
    }
    fn report_deadline(&self, body: &[u8]) -> Result<(DiagnosticWorkId, Instant), ReportError> {
        let frame = DiagnosticReportFrame::parse(body)?;
        let mut selected = None;
        for report in frame.reports()? {
            let slot = self
                .jobs
                .iter()
                .flatten()
                .find(|s| s.id.token == report.token && s.kind == report.kind)
                .ok_or(ReportError::WrongWork)?;
            if selected.is_none_or(|(_, until)| slot.until < until) {
                selected = Some((slot.id, slot.until));
            }
        }
        selected.ok_or(ReportError::IncompleteResults)
    }
    pub fn send_next(&mut self, now: Instant) -> Result<Option<OperationId>, ReportError> {
        self.dialog.time(now)?;
        if self.tx.is_some() {
            return Err(Error::Busy.into());
        }
        let Some(body) = self.outbox.head() else {
            return Ok(None);
        };
        let (_, until) = self.report_deadline(body.bytes())?;
        if now >= until {
            return Err(ReportError::ExpiredWork);
        }
        let (current, _) = self.request().ok_or(Error::NoPendingOperation)?;
        let tx = self.dialog.reply_until(current, body.clone(), now, until)?;
        self.tx = Some(tx);
        Ok(Some(tx))
    }
    pub fn delivery(
        &self,
        mut connectivity: ReportConnectivity,
        now: Instant,
    ) -> Result<Option<ReportDelivery<'_>>, ReportError> {
        self.dialog.time(now)?;
        let Some(tx) = self.dialog.transmission() else {
            return Ok(None);
        };
        let (_, request) = self.request().ok_or(Error::NoPendingOperation)?;
        if self.away.is_some() {
            connectivity.requester_reachable = false;
        }
        route(
            tx,
            DestinationUri::parse(request.elements)?,
            connectivity,
            now,
        )
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), ReportError> {
        Ok(self.dialog.admitted(id, now)?)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DiagnosticStationEvent, ReportError> {
        if self.tx != Some(id) {
            return Ok(DiagnosticStationEvent::Ignored);
        }
        let event = self.dialog.completed(id, outcome, now)?;
        let event = match event {
            DialogEvent::Transmitted => {
                self.outbox.acknowledge();
                DiagnosticStationEvent::Transmitted { id }
            }
            DialogEvent::TxFailed => DiagnosticStationEvent::TxFailed { id },
            DialogEvent::TimedOut => DiagnosticStationEvent::TimedOut { id },
            _ => return Ok(DiagnosticStationEvent::Ignored),
        };
        self.tx = None;
        Ok(event)
    }
    pub fn poll(&mut self, now: Instant) -> Result<DiagnosticStationEvent, ReportError> {
        self.dialog.time(now)?;
        if let Some(slot) = self
            .jobs
            .iter_mut()
            .flatten()
            .find(|s| now >= s.until && matches!(s.phase, Phase::Ready | Phase::Admitted))
        {
            let admitted = slot.phase == Phase::Admitted;
            slot.phase = Phase::Expired;
            self.dialog.last_update = now;
            return Ok(DiagnosticStationEvent::WorkExpired {
                id: slot.id,
                admitted,
            });
        }
        let (request, tx) = self.dialog.poll(now)?;
        if let Some(id) = request {
            self.jobs = [None; J];
            self.outbox.clear();
            self.tx = None;
            self.away = None;
            return Ok(DiagnosticStationEvent::TimedOut { id });
        }
        if let Some(id) = tx {
            self.tx = None;
            return Ok(DiagnosticStationEvent::TimedOut { id });
        }
        if self.tx.is_none()
            && let Some(body) = self.outbox.head()
        {
            let (id, until) = self.report_deadline(body.bytes())?;
            if now >= until {
                self.outbox.acknowledge();
                return Ok(DiagnosticStationEvent::ReportExpired { id });
            }
        }
        Ok(DiagnosticStationEvent::Ignored)
    }
    /// Only an admitted active test may preserve work during its own BSS transition.
    pub fn association_changed(
        &mut self,
        network: NetworkIdentity,
        cause: Option<DiagnosticWorkId>,
        now: Instant,
    ) -> Result<DiagnosticStationEvent, ReportError> {
        self.dialog.time(now)?;
        if network == self.network
            && let Some(id) = cause
            && self.jobs.iter().flatten().any(|s| {
                s.id == id && s.phase == Phase::Admitted && s.kind.active() && now < s.until
            })
        {
            if self.tx.is_some() {
                return Err(Error::Busy.into());
            }
            self.away = Some(id);
            self.dialog.last_update = now;
            return Ok(DiagnosticStationEvent::DiagnosticTransition { id });
        }
        let id = self.cancel();
        self.dialog.last_update = now;
        self.network = network;
        Ok(id.map_or(DiagnosticStationEvent::Ignored, |id| {
            DiagnosticStationEvent::Cancelled { id }
        }))
    }
    /// Return to the original requester with the new association generation.
    pub fn reattached(
        &mut self,
        link: LinkIdentity,
        network: NetworkIdentity,
        now: Instant,
    ) -> Result<(), ReportError> {
        self.dialog.time(now)?;
        if network != self.network {
            return Err(ReportError::WrongNetwork);
        }
        let cause = self.away.ok_or(ReportError::WrongWork)?;
        if !self.jobs.iter().flatten().any(|s| {
            s.id == cause && now < s.until && matches!(s.phase, Phase::Admitted | Phase::Completed)
        }) {
            return Err(ReportError::ExpiredWork);
        }
        self.dialog.rebind(link, now)?;
        self.away = None;
        Ok(())
    }
    pub fn cancel(&mut self) -> Option<OperationId> {
        self.jobs = [None; J];
        self.outbox.clear();
        self.tx = None;
        self.away = None;
        self.dialog.cancel()
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.jobs
            .iter()
            .flatten()
            .filter(|s| matches!(s.phase, Phase::Ready | Phase::Admitted))
            .map(|s| s.until)
            .chain(
                self.outbox
                    .head()
                    .and_then(|b| self.report_deadline(b.bytes()).ok().map(|(_, until)| until)),
            )
            .chain(self.dialog.next_deadline())
            .min()
    }
}
