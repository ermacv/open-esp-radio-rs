use crate::{
    Body, DialogEvent, Error, Identities, LinkIdentity, OperationId, Pending, Sender, Transmission,
    TxOutcome, TxPhase, deadline,
};
use core::mem::size_of;
use oer_ieee80211_mac::management::{BROADCAST_ADDRESS, IEEE_TIME_UNIT_MICROS};
use oer_ieee80211_mac::roaming as wire;
use oer_ieee80211_mac::roaming::*;
use oer_time::{Duration, Instant};
const REQUESTS_PERMITTED: u8 = 1 << 0;
const REPORTS_PERMITTED: u8 = 1 << 1;
const PEER_INCAPABLE: u8 = 1 << 2;
const REQUEST_PERMISSION_MASK: u8 = REQUESTS_PERMITTED | PEER_INCAPABLE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasurementError {
    Protocol(Error),
    ReportStorageFull,
    TaskStorageFull,
    PeerIncapable(MeasurementType),
    PeerDisabled(MeasurementType),
    UnexpectedMeasurement { token: u8, kind: MeasurementType },
    UnsupportedMode(u8),
    UnsupportedCondition(u8),
    MissingReference,
    MissingSsid,
    InvalidDuration,
    InvalidStartTime,
    WrongWork,
    WorkNotStarted,
    WorkAlreadyStarted,
    ChannelStorageFull,
    ConflictingChannels,
    UnsupportedBeaconMode(u8),
    MissingChannelReports,
    NoPermittedChannels,
    UnsupportedDetail(u8),
    UnsupportedExtension(u8),
    ReportBodyFull,
    UnexpectedChannel,
    MissingMeasurement { token: u8, kind: MeasurementType },
}
impl From<Error> for MeasurementError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
impl From<WireError> for MeasurementError {
    fn from(value: WireError) -> Self {
        Self::Protocol(Error::Wire(value))
    }
}

/// Per-association peer permissions, including permanent Incapable reports.
/// Defaults permit requests and disallow unsolicited reports (IEEE 10.11.6).
#[derive(Clone)]
pub struct MeasurementPermissions {
    values: [u8; crate::OCTET_VALUE_COUNT],
}
impl Default for MeasurementPermissions {
    fn default() -> Self {
        Self {
            values: [REQUESTS_PERMITTED; crate::OCTET_VALUE_COUNT],
        }
    }
}
impl MeasurementPermissions {
    pub fn requests_allowed(&self, kind: MeasurementType) -> bool {
        self.values[usize::from(kind.0)] & REQUEST_PERMISSION_MASK == REQUESTS_PERMITTED
    }
    pub fn reports_allowed(&self, kind: MeasurementType) -> bool {
        self.values[usize::from(kind.0)] & REPORTS_PERMITTED != 0
    }
    fn controls(&mut self, request: RadioMeasurementRequest<'_>) -> Result<(), MeasurementError> {
        for item in request.measurements()? {
            if item.mode.0 & !MeasurementRequestMode::KNOWN_BITS != 0 {
                return Err(MeasurementError::UnsupportedMode(item.mode.0));
            }
            if item.mode.control() {
                let flags = if item.mode.requests_allowed() {
                    REQUESTS_PERMITTED
                } else {
                    0
                } | if item.mode.reports_allowed() {
                    REPORTS_PERMITTED
                } else {
                    0
                };
                let value = &mut self.values[usize::from(item.measurement_type.0)];
                *value = (*value & PEER_INCAPABLE) | flags;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasurementEvent {
    Ignored,
    ReportsReceived { id: OperationId, frame: usize },
    WindowClosed { id: OperationId, frames: usize },
    TxFailed { id: OperationId },
    Cancelled { id: OperationId },
}

/// A bounded multi-frame result window. A report does not prove that all
/// reports have arrived: only the supplied measurement window closes it.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct RadioMeasurementRequester<const BYTES: usize, const FRAMES: usize> {
    ids: Identities,
    timeout: Duration,
    pending: Option<Pending<BYTES>>,
    frames: [Option<Body<BYTES>>; FRAMES],
    permissions: MeasurementPermissions,
}
impl<const BYTES: usize, const FRAMES: usize> RadioMeasurementRequester<BYTES, FRAMES> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, MeasurementError> {
        deadline(Instant::EPOCH, timeout)?;
        if FRAMES == 0 {
            return Err(MeasurementError::ReportStorageFull);
        }
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            pending: None,
            frames: core::array::from_fn(|_| None),
            permissions: MeasurementPermissions::default(),
        })
    }
    pub fn permissions(&self) -> &MeasurementPermissions {
        &self.permissions
    }
    /// Receive peer control bits from its complete request. The measuring owner
    /// independently handles the measurement work in the same incoming body.
    pub fn peer_controls(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
    ) -> Result<(), MeasurementError> {
        if link != self.ids.link {
            return Ok(());
        }
        let request = RadioMeasurementRequest::parse(bytes)?;
        let mut prepared = self.permissions.clone();
        prepared.controls(request)?;
        self.permissions = prepared;
        Ok(())
    }
    pub fn request(
        &mut self,
        now: Instant,
        request: RadioMeasurementRequest<'_>,
    ) -> Result<OperationId, MeasurementError> {
        if self.pending.is_some() {
            return Err(Error::Busy.into());
        }
        for item in request.measurements()? {
            if item.mode.0 & !MeasurementRequestMode::KNOWN_BITS != 0 {
                return Err(MeasurementError::UnsupportedMode(item.mode.0));
            }
            if !item.mode.control() && !self.permissions.requests_allowed(item.measurement_type) {
                return Err(
                    if self.permissions.values[usize::from(item.measurement_type.0)]
                        & PEER_INCAPABLE
                        != 0
                    {
                        MeasurementError::PeerIncapable(item.measurement_type)
                    } else {
                        MeasurementError::PeerDisabled(item.measurement_type)
                    },
                );
            }
        }
        let body = Body::encode(|out| request.encode(out))?;
        let until = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.pending = Some(Pending::new(id, body, now, until));
        self.frames.fill(None);
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.pending.as_ref()?.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), MeasurementError> {
        self.pending
            .as_mut()
            .ok_or(Error::NoPendingOperation)?
            .admit(id, now)?;
        Ok(())
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<MeasurementEvent, MeasurementError> {
        let Some(pending) = &mut self.pending else {
            return Ok(MeasurementEvent::Ignored);
        };
        if id != pending.id {
            return Ok(MeasurementEvent::Ignored);
        }
        if !pending.live(now)? {
            return self.poll(now);
        }
        if pending.complete(id, outcome)? && outcome == TxOutcome::Failed {
            self.pending = None;
            return Ok(MeasurementEvent::TxFailed { id });
        }
        Ok(MeasurementEvent::Ignored)
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<MeasurementEvent, MeasurementError> {
        if link != self.ids.link {
            return Ok(MeasurementEvent::Ignored);
        }
        let Some(pending) = &self.pending else {
            return Ok(MeasurementEvent::Ignored);
        };
        if !pending.live(now)? {
            return self.poll(now);
        }
        let report = RadioMeasurementReport::parse(bytes)?;
        if !pending.received(report.dialog_token) {
            return Ok(MeasurementEvent::Ignored);
        }
        let request = RadioMeasurementRequest::parse(pending.body.bytes())?;
        let mut permissions = self.permissions.clone();
        for item in report.reports()? {
            if item.mode.0
                & !(MeasurementReportMode::INCAPABLE.0 | MeasurementReportMode::REFUSED.0)
                != 0
            {
                return Err(MeasurementError::UnsupportedMode(item.mode.0));
            }
            if !request.measurements()?.any(|requested| {
                requested.measurement_token == item.measurement_token
                    && requested.measurement_type == item.measurement_type
                    && (!requested.mode.control() || item.mode.rejected())
            }) {
                return Err(MeasurementError::UnexpectedMeasurement {
                    token: item.measurement_token,
                    kind: item.measurement_type,
                });
            }
            if item.mode.0 & MeasurementReportMode::INCAPABLE.0 != 0 {
                permissions.values[usize::from(item.measurement_type.0)] |= PEER_INCAPABLE;
            }
        }
        if self
            .frames
            .iter()
            .flatten()
            .any(|body| body.bytes() == bytes)
        {
            return Ok(MeasurementEvent::Ignored);
        }
        let index = self
            .frames
            .iter()
            .position(Option::is_none)
            .ok_or(MeasurementError::ReportStorageFull)?;
        let body = Body::copy(bytes)?;
        let id = pending.id;
        self.frames[index] = Some(body);
        self.permissions = permissions;
        self.pending.as_mut().expect("live request").phase = TxPhase::Waiting;
        Ok(MeasurementEvent::ReportsReceived { id, frame: index })
    }
    /// Retained history of complete frames, including Action-level extensions.
    /// A new successfully queued request replaces this historical snapshot.
    pub fn reports(&self) -> impl Iterator<Item = RadioMeasurementReport<'_>> {
        self.frames
            .iter()
            .flatten()
            .map(|body| RadioMeasurementReport::parse(body.bytes()).expect("validated reports"))
    }
    pub fn poll(&mut self, now: Instant) -> Result<MeasurementEvent, MeasurementError> {
        if let Some(pending) = &self.pending
            && !pending.live(now)?
        {
            let id = pending.id;
            self.pending = None;
            return Ok(MeasurementEvent::WindowClosed {
                id,
                frames: self.frames.iter().flatten().count(),
            });
        }
        Ok(MeasurementEvent::Ignored)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.deadline)
    }
    pub fn cancel(&mut self) -> MeasurementEvent {
        self.pending
            .take()
            .map_or(MeasurementEvent::Ignored, |pending| {
                MeasurementEvent::Cancelled { id: pending.id }
            })
    }
}

/// Addressing precedence for a received request, already admitted by the MAC.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RequestAddressing {
    Broadcast,
    Multicast,
    Individual,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementWorkId {
    pub operation: OperationId,
    round: u32,
    group: usize,
}
impl MeasurementWorkId {
    pub const fn round(self) -> u32 {
        self.round
    }
}
pub struct MeasurementWork<'a, const TASKS: usize> {
    pub id: MeasurementWorkId,
    pub earliest_start: Instant,
    pub latest_start: Instant,
    pub requests: [Option<MeasurementRequestElement<'a>>; TASKS],
    pub addressing: RequestAddressing,
}
struct Plan<const BYTES: usize> {
    id: OperationId,
    body: Body<BYTES>,
    received: Instant,
    until: Instant,
    addressing: RequestAddressing,
    round: u32,
    cursor: usize,
    ready: Instant,
    started: Option<Instant>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanEvent {
    Ignored,
    Accepted {
        id: OperationId,
        superseded: Option<OperationId>,
    },
    ControlsApplied,
    Cancelled {
        id: OperationId,
    },
    GroupCompleted {
        more: bool,
        response: Option<OperationId>,
    },
    TimedOut {
        id: OperationId,
    },
    Transmission {
        event: DialogEvent,
    },
}

/// RRM sequencing and reporting owner. The integration admits scans and
/// supplies measured reports; the owner retains the full request, schedules
/// sequential/parallel groups and repetitions, and applies reporting rules.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct RadioMeasurementResponder<const BYTES: usize, const TASKS: usize> {
    ids: Identities,
    lease: Duration,
    plan: Option<Plan<BYTES>>,
    sender: Sender<BYTES>,
    permissions: MeasurementPermissions,
}
impl<const BYTES: usize, const TASKS: usize> RadioMeasurementResponder<BYTES, TASKS> {
    pub fn new(
        link: LinkIdentity,
        lease: Duration,
        tx_timeout: Duration,
    ) -> Result<Self, MeasurementError> {
        deadline(Instant::EPOCH, lease)?;
        if TASKS == 0 {
            return Err(MeasurementError::TaskStorageFull);
        }
        Ok(Self {
            ids: Identities::new(link),
            lease,
            plan: None,
            sender: Sender::new(link, tx_timeout)?,
            permissions: MeasurementPermissions::default(),
        })
    }
    pub fn permissions(&self) -> &MeasurementPermissions {
        &self.permissions
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        addressing: RequestAddressing,
        now: Instant,
    ) -> Result<PlanEvent, MeasurementError> {
        if link != self.ids.link {
            return Ok(PlanEvent::Ignored);
        }
        let request = RadioMeasurementRequest::parse(bytes)?;
        let mut permissions = self.permissions.clone();
        permissions.controls(request)?;
        let count = request
            .measurements()?
            .filter(|item| !item.mode.control() || !item.body.is_empty())
            .count();
        if count > TASKS {
            return Err(MeasurementError::TaskStorageFull);
        }
        let body = Body::copy(bytes)?;
        let until = deadline(now, self.lease)?;
        if let Some(plan) = &self.plan {
            if now < plan.received {
                return Err(Error::TimeBeforeOperation.into());
            }
            if now < plan.until {
                if plan.body.bytes() == bytes && plan.addressing == addressing {
                    return Ok(PlanEvent::Ignored);
                }
                if addressing < plan.addressing {
                    self.permissions = permissions;
                    return Ok(PlanEvent::ControlsApplied);
                }
                if plan.body.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token {
                    return Err(Error::ConflictingDialog.into());
                }
            }
        }
        let superseded = self.plan.as_ref().map(|plan| plan.id);
        if count == 0 {
            self.plan = None;
            self.permissions = permissions;
            return Ok(
                superseded.map_or(PlanEvent::ControlsApplied, |id| PlanEvent::Cancelled { id })
            );
        }
        let id = self.ids.issue()?;
        self.plan = Some(Plan {
            id,
            body,
            received: now,
            until,
            addressing,
            round: 0,
            cursor: 0,
            ready: now,
            started: None,
        });
        self.permissions = permissions;
        Ok(PlanEvent::Accepted { id, superseded })
    }
    pub fn work(
        &self,
        now: Instant,
    ) -> Result<Option<MeasurementWork<'_, TASKS>>, MeasurementError> {
        let Some(plan) = &self.plan else {
            return Ok(None);
        };
        if now < plan.received || now < plan.ready {
            return Err(Error::TimeBeforeOperation.into());
        }
        if now >= plan.until {
            return Ok(None);
        }
        let request = RadioMeasurementRequest::parse(plan.body.bytes())?;
        let mut requests = [None; TASKS];
        let mut latest = plan.until;
        for item in request
            .measurements()?
            .filter(|item| !item.mode.control() || !item.body.is_empty())
            .skip(plan.cursor)
            .enumerate()
        {
            let (count, item) = item;
            let window_tu = match item.data()? {
                MeasurementRequestData::ChannelLoad(data)
                | MeasurementRequestData::NoiseHistogram(data) => data.randomization_tu,
                MeasurementRequestData::Beacon(data) => data.channel.randomization_tu,
                _ => 0,
            };
            latest = latest.min(
                plan.ready
                    .checked_add(Duration::from_micros(
                        u64::from(window_tu) * IEEE_TIME_UNIT_MICROS,
                    ))
                    .ok_or(Error::TimeOverflow)?,
            );
            requests[count] = Some(item);
            if !item.mode.parallel() {
                break;
            }
        }
        Ok(Some(MeasurementWork {
            id: MeasurementWorkId {
                operation: plan.id,
                round: plan.round,
                group: plan.cursor,
            },
            earliest_start: plan.ready,
            latest_start: latest,
            requests,
            addressing: plan.addressing,
        }))
    }
    pub fn start_work(
        &mut self,
        id: MeasurementWorkId,
        now: Instant,
    ) -> Result<(), MeasurementError> {
        let work = self.work(now)?.ok_or(Error::NoPendingOperation)?;
        if work.id != id {
            return Err(MeasurementError::WrongWork);
        }
        if now > work.latest_start {
            return Err(MeasurementError::InvalidStartTime);
        }
        let plan = self.plan.as_mut().expect("live work");
        if plan.started.is_some() {
            return Err(MeasurementError::WorkAlreadyStarted);
        }
        plan.started = Some(now);
        Ok(())
    }
    /// Complete a whole group, including an empty Beacon Report for no
    /// observations. Supply a measured or rejected result for every request;
    /// the owner suppresses results whose reporting condition is false. Reports may contain several
    /// Beacon elements for one token. All validation precedes state mutation.
    pub fn complete_work<'a>(
        &mut self,
        id: MeasurementWorkId,
        reports: Elements<'a>,
        mut metadata: impl FnMut(MeasurementReportElement<'a>) -> ReportMetadata,
        now: Instant,
    ) -> Result<PlanEvent, MeasurementError> {
        let work = self.work(now)?.ok_or(Error::NoPendingOperation)?;
        if work.id != id {
            return Err(MeasurementError::WrongWork);
        }
        let plan = self.plan.as_ref().expect("live work");
        let source = RadioMeasurementReport {
            dialog_token: plan.body.bytes()[wire::DIALOG_TOKEN_OFFSET],
            elements: reports,
        };
        source.validate()?;
        let started = match plan.started {
            Some(started) if now >= started => started,
            Some(_) => return Err(Error::TimeBeforeOperation.into()),
            None if source.reports()?.next().is_some()
                && source.reports()?.all(|report| report.mode.rejected()) =>
            {
                now
            }
            None => return Err(MeasurementError::WorkNotStarted),
        };
        for request in work.requests.iter().flatten() {
            if !source.reports()?.any(|report| {
                report.measurement_token == request.measurement_token
                    && report.measurement_type == request.measurement_type
            }) {
                return Err(MeasurementError::MissingMeasurement {
                    token: request.measurement_token,
                    kind: request.measurement_type,
                });
            }
        }
        let mut body = Body::<BYTES>::empty();
        if BYTES < ACTION_HEADER_LEN {
            return Err(Error::FrameTooLarge {
                required: ACTION_HEADER_LEN,
                capacity: BYTES,
            }
            .into());
        }
        body.bytes[..ACTION_HEADER_LEN].copy_from_slice(&[
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::MeasurementReport as u8,
            source.dialog_token,
        ]);
        body.len = ACTION_HEADER_LEN;
        let mut emitted = false;
        for element in reports.iter() {
            let include = if element.id == MEASUREMENT_REPORT_ELEMENT_ID {
                let report = MeasurementReportElement::parse(element.body)?;
                let request = work
                    .requests
                    .iter()
                    .flatten()
                    .find(|request| {
                        request.measurement_token == report.measurement_token
                            && request.measurement_type == report.measurement_type
                    })
                    .ok_or(MeasurementError::UnexpectedMeasurement {
                        token: report.measurement_token,
                        kind: report.measurement_type,
                    })?;
                if report.mode.0
                    & !(MeasurementReportMode::INCAPABLE.0 | MeasurementReportMode::REFUSED.0)
                    != 0
                {
                    return Err(MeasurementError::UnsupportedMode(report.mode.0));
                }
                if request.mode.control() && !report.mode.rejected() {
                    return Err(MeasurementError::UnsupportedMode(request.mode.0));
                }
                let header = match report.data()? {
                    MeasurementReportData::ChannelLoad(data) => Some(data.header),
                    MeasurementReportData::NoiseHistogram(data) => Some(data.header),
                    MeasurementReportData::Beacon(data) if !matches!(request.data()?,MeasurementRequestData::Beacon(data) if data.measurement_mode==2) => {
                        Some(data.header)
                    }
                    _ => None,
                };
                if let Some(header) = header
                    && started
                        .checked_add(Duration::from_micros(
                            u64::from(header.duration_tu) * IEEE_TIME_UNIT_MICROS,
                        ))
                        .ok_or(Error::TimeOverflow)?
                        > now
                {
                    return Err(MeasurementError::InvalidDuration);
                }
                if report.mode.rejected() {
                    work.addressing == RequestAddressing::Individual
                } else {
                    report_allowed(*request, report, metadata(report))?
                }
            } else {
                true
            };
            if include {
                let end = body.len + ELEMENT_HEADER_LEN + element.body.len();
                if end > BYTES {
                    return Err(Error::FrameTooLarge {
                        required: end,
                        capacity: BYTES,
                    }
                    .into());
                }
                body.bytes[body.len..body.len + ELEMENT_HEADER_LEN]
                    .copy_from_slice(&[element.id, element.body.len() as u8]);
                body.bytes[body.len + ELEMENT_HEADER_LEN..end].copy_from_slice(element.body);
                body.len = end;
                emitted |= element.id == MEASUREMENT_REPORT_ELEMENT_ID;
            }
        }
        let group_len = work.requests.iter().flatten().count();
        let request = RadioMeasurementRequest::parse(plan.body.bytes())?;
        let total = request
            .measurements()?
            .filter(|item| !item.mode.control() || !item.body.is_empty())
            .count();
        let next_cursor = plan.cursor + group_len;
        let repeat = next_cursor == total
            && (request.repetitions == INDEFINITE_MEASUREMENT_REPETITIONS
                || plan.round < u32::from(request.repetitions));
        let next_round = if repeat {
            plan.round.checked_add(1).ok_or(Error::IdentityExhausted)?
        } else {
            plan.round
        };
        let response = if emitted {
            let until = plan.until;
            let tx = self
                .sender
                .start_using_until(now, body, &mut self.ids, until)?;
            Some(tx)
        } else {
            None
        };
        let more = next_cursor < total || repeat;
        if more {
            let plan = self.plan.as_mut().expect("live plan");
            plan.cursor = if repeat { 0 } else { next_cursor };
            plan.round = next_round;
            plan.ready = now;
            plan.started = None;
        } else {
            self.plan = None;
        }
        Ok(PlanEvent::GroupCompleted { more, response })
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.sender.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), MeasurementError> {
        self.sender.admitted(id, now)?;
        Ok(())
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DialogEvent, MeasurementError> {
        Ok(self.sender.completed(id, outcome, now)?)
    }
    pub fn poll(&mut self, now: Instant) -> Result<PlanEvent, MeasurementError> {
        if let Some(plan) = &self.plan {
            if now < plan.received {
                return Err(Error::TimeBeforeOperation.into());
            }
            if now >= plan.until {
                let id = plan.id;
                self.plan = None;
                return Ok(PlanEvent::TimedOut { id });
            }
        }
        let event = self.sender.poll(now)?;
        Ok(if event == DialogEvent::Ignored {
            PlanEvent::Ignored
        } else {
            PlanEvent::Transmission { event }
        })
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.plan
            .as_ref()
            .map(|plan| plan.until)
            .into_iter()
            .chain(self.sender.next_deadline())
            .min()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReportMetadata {
    pub beacon_ssid: Option<oer_ieee80211_mac::ssid::WifiSsid>,
    pub serving_rcpi: Option<u8>,
    pub serving_rsni: Option<u8>,
}

/// Apply channel/noise thresholds and all ten Beacon conditions to real
/// supplied values. Missing reference measurements are explicit errors.
pub fn report_allowed(
    request: MeasurementRequestElement<'_>,
    report: MeasurementReportElement<'_>,
    metadata: ReportMetadata,
) -> Result<bool, MeasurementError> {
    if request.measurement_token != report.measurement_token
        || request.measurement_type != report.measurement_type
    {
        return Err(MeasurementError::UnexpectedMeasurement {
            token: report.measurement_token,
            kind: report.measurement_type,
        });
    }
    if report.mode.rejected() {
        return Ok(true);
    }
    let subelements = match request.data()? {
        MeasurementRequestData::ChannelLoad(data)
        | MeasurementRequestData::NoiseHistogram(data) => Some(data.subelements),
        MeasurementRequestData::Beacon(data) => Some(data.channel.subelements),
        _ => None,
    };
    if let Some(subelements) = subelements {
        if subelements
            .unique(measurement_subelement_id::REPORTING_INFORMATION)?
            .is_some_and(|body| body.len() != 2 * size_of::<u8>())
        {
            return Err(MeasurementError::UnsupportedExtension(
                measurement_subelement_id::REPORTING_INFORMATION,
            ));
        }
        if subelements
            .unique(measurement_subelement_id::REPORTING_DETAIL)?
            .is_some_and(|body| body.len() != size_of::<u8>())
        {
            return Err(MeasurementError::UnsupportedExtension(
                measurement_subelement_id::REPORTING_DETAIL,
            ));
        }
    }
    let mandatory = request.mode.duration_mandatory();
    let (request_channel, header, condition, value) = match (request.data()?, report.data()?) {
        (
            MeasurementRequestData::ChannelLoad(request),
            MeasurementReportData::ChannelLoad(report),
        ) => (
            request,
            report.header,
            request.reporting_condition()?,
            report.load,
        ),
        (
            MeasurementRequestData::NoiseHistogram(request),
            MeasurementReportData::NoiseHistogram(report),
        ) => (
            request,
            report.header,
            request.reporting_condition()?,
            report.anpi,
        ),
        (MeasurementRequestData::Beacon(request), MeasurementReportData::Beacon(report)) => {
            if request.bssid != BROADCAST_ADDRESS && request.bssid != report.bssid {
                return Ok(false);
            }
            if let Some(ssid) = request.channel.subelements.unique(element_id::SSID)?
                && !ssid.is_empty()
            {
                let observed = metadata.beacon_ssid.ok_or(MeasurementError::MissingSsid)?;
                if observed.as_bytes() != ssid {
                    return Ok(false);
                }
            }
            if request.measurement_mode != beacon_measurement_mode::TABLE {
                duration(request.channel, report.header, mandatory)?;
            }
            let condition = request
                .channel
                .reporting_condition()?
                .unwrap_or((beacon_reporting_condition::ALWAYS, 0));
            return beacon_condition(condition.0, condition.1, report.rcpi, report.rsni, metadata);
        }
        (MeasurementRequestData::Beacon(_), MeasurementReportData::EmptyBeacon) => return Ok(true),
        _ => return Err(MeasurementError::PeerIncapable(request.measurement_type)),
    };
    duration(request_channel, header, request.mode.duration_mandatory())?;
    if request_channel.operating_class != header.operating_class
        || request_channel.channel != header.channel
    {
        return Err(MeasurementError::UnexpectedChannel);
    }
    match condition.unwrap_or((channel_reporting_condition::ALWAYS, 0)) {
        (channel_reporting_condition::ALWAYS, _) => Ok(true),
        (channel_reporting_condition::AT_LEAST, reference) => Ok(value >= reference),
        (channel_reporting_condition::AT_MOST, reference) => Ok(value <= reference),
        (condition, _) => Err(MeasurementError::UnsupportedCondition(condition)),
    }
}
fn duration(
    request: ChannelMeasurementRequest<'_>,
    report: MeasurementReportHeader,
    mandatory: bool,
) -> Result<(), MeasurementError> {
    if report.duration_tu > request.duration_tu
        || (mandatory && report.duration_tu != request.duration_tu)
    {
        return Err(MeasurementError::InvalidDuration);
    }
    Ok(())
}
fn beacon_condition(
    condition: u8,
    offset: u8,
    rcpi: u8,
    rsni: u8,
    metadata: ReportMetadata,
) -> Result<bool, MeasurementError> {
    use beacon_reporting_condition::*;
    if condition == ALWAYS {
        return Ok(true);
    }
    let value = if matches!(
        condition,
        RSNI_ABOVE
            | RSNI_BELOW
            | RSNI_ABOVE_SERVING_OFFSET
            | RSNI_BELOW_SERVING_OFFSET
            | RSNI_BETWEEN_SERVING_AND_OFFSET
    ) {
        rsni
    } else {
        rcpi
    };
    if value == SIGNAL_MEASUREMENT_UNAVAILABLE {
        return Err(MeasurementError::MissingReference);
    }
    match condition {
        RCPI_ABOVE | RSNI_ABOVE => Ok(value > offset),
        RCPI_BELOW | RSNI_BELOW => Ok(value < offset),
        RCPI_ABOVE_SERVING_OFFSET
        | RCPI_BELOW_SERVING_OFFSET
        | RSNI_ABOVE_SERVING_OFFSET
        | RSNI_BELOW_SERVING_OFFSET
        | RCPI_BETWEEN_SERVING_AND_OFFSET
        | RSNI_BETWEEN_SERVING_AND_OFFSET => {
            let reference = if matches!(
                condition,
                RSNI_ABOVE_SERVING_OFFSET
                    | RSNI_BELOW_SERVING_OFFSET
                    | RSNI_BETWEEN_SERVING_AND_OFFSET
            ) {
                metadata.serving_rsni
            } else {
                metadata.serving_rcpi
            }
            .filter(|value| *value != SIGNAL_MEASUREMENT_UNAVAILABLE)
            .ok_or(MeasurementError::MissingReference)?;
            let base = i16::from(reference);
            let threshold = base + i16::from(offset as i8);
            let value = i16::from(value);
            match condition {
                RCPI_ABOVE_SERVING_OFFSET | RSNI_ABOVE_SERVING_OFFSET => Ok(value > threshold),
                RCPI_BELOW_SERVING_OFFSET | RSNI_BELOW_SERVING_OFFSET => Ok(value < threshold),
                _ => Ok(value >= base.min(threshold) && value <= base.max(threshold)),
            }
        }
        _ => Err(MeasurementError::UnsupportedCondition(condition)),
    }
}

#[cfg(test)]
mod tests;
