//! Shared bounded storage and delivery contracts for Event and Diagnostic reports.
use crate::exchange::Exchange;
use crate::{
    Body, Error, LinkIdentity, OperationId, Pending, RequestEvent, Transmission, TxOutcome,
    TxPhase, deadline,
};
use oer_ieee80211_mac::roaming as wire;
use oer_ieee80211_mac::roaming::{
    ACTION_HEADER_LEN, DestinationUri, ELEMENT_HEADER_LEN, Elements, WNM_CATEGORY, WireError,
    WnmAction,
};
use oer_time::{Duration, Instant};

const SECONDS_PER_MINUTE: u32 = 60;

/// The integration changes this identity when the station leaves its ESS/IBSS.
/// An ordinary BSS transition inside that network preserves it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkIdentity(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestReplacement {
    pub id: OperationId,
    pub superseded: Option<OperationId>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportError {
    Protocol(Error),
    Full,
    UnexpectedToken(u8),
    UnexpectedType(u8),
    UnsupportedStatus(u8),
    WrongNetwork,
    InvalidJournalCapacity,
    IncompleteResults,
    WrongWork,
    ExpiredWork,
    InvalidResult,
    HistoryTooShort,
    UnsupportedRequest,
}
impl From<Error> for ReportError {
    fn from(v: Error) -> Self {
        Self::Protocol(v)
    }
}
impl From<WireError> for ReportError {
    fn from(v: WireError) -> Self {
        Self::Protocol(v.into())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportWindowEvent {
    Ignored,
    AutonomousReceived,
    MonitoringReportReceived { id: OperationId },
    ReportReceived { id: OperationId },
    WindowClosed { id: OperationId, frames: usize },
    TxFailed { id: OperationId },
    TimedOut { id: OperationId },
    Cancelled { id: OperationId },
}
/// A supplied observation of connectivity to the original requesting AP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReportConnectivity {
    pub requester_reachable: bool,
    /// Start of the continuous absence of this requesting AP's Beacon frames.
    pub beacon_absent_since: Instant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportRoute<'a> {
    Action { destination: wire::MacAddress },
    Uri { uri: &'a [u8] },
}
/// Copy the complete action body into transport-owned storage before admission.
/// URI transport framing and execution belong to the integration.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ReportDelivery<'a> {
    pub id: OperationId,
    pub route: ReportRoute<'a>,
    pub body: &'a [u8],
}
impl core::fmt::Debug for ReportDelivery<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReportDelivery")
            .field("id", &self.id)
            .field("body_length", &self.body.len())
            .finish_non_exhaustive()
    }
}
pub(crate) fn route<'a>(
    tx: Transmission<'a>,
    uri: Option<DestinationUri<'a>>,
    connectivity: ReportConnectivity,
    now: Instant,
) -> Result<Option<ReportDelivery<'a>>, ReportError> {
    if connectivity.requester_reachable {
        return Ok(Some(ReportDelivery {
            id: tx.id,
            route: ReportRoute::Action {
                destination: tx.destination,
            },
            body: tx.body,
        }));
    }
    if now < connectivity.beacon_absent_since {
        return Err(Error::TimeBeforeOperation.into());
    }
    let Some(uri) = uri else {
        return Ok(None);
    };
    let wait = Duration::from_secs(u32::from(uri.ess_detection_minutes) * SECONDS_PER_MINUTE);
    if now.as_micros() - connectivity.beacon_absent_since.as_micros() < wait.as_micros() {
        return Ok(None);
    }
    Ok(Some(ReportDelivery {
        id: tx.id,
        route: ReportRoute::Uri { uri: uri.uri },
        body: tx.body,
    }))
}

pub(crate) struct ReportWindow<const B: usize, const F: usize> {
    pub(crate) exchange: Exchange<B>,
    frames: [Option<Body<B>>; F],
    count: usize,
    last_update: Instant,
    timeout: Duration,
}
impl<const B: usize, const F: usize> ReportWindow<B, F> {
    pub(crate) fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, ReportError> {
        if !crate::valid_peer_address(link.peer) {
            return Err(Error::InvalidTarget.into());
        }
        if F == 0 {
            return Err(ReportError::Full);
        }
        Ok(Self {
            exchange: Exchange::new(link, timeout)?,
            frames: core::array::from_fn(|_| None),
            count: 0,
            last_update: Instant::EPOCH,
            timeout,
        })
    }
    pub(crate) fn time(&self, now: Instant) -> Result<(), ReportError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn start(
        &mut self,
        body: Body<B>,
        now: Instant,
    ) -> Result<RequestReplacement, ReportError> {
        self.time(now)?;
        let until = deadline(now, self.timeout)?;
        if let Some(old) = &self.exchange.pending
            && old.live(now)?
            && old.body.bytes()[wire::DIALOG_TOKEN_OFFSET]
                == body.bytes()[wire::DIALOG_TOKEN_OFFSET]
        {
            return Err(Error::ConflictingDialog.into());
        }
        let id = self.exchange.ids.issue()?;
        let superseded = self.exchange.pending.as_ref().map(|p| p.id);
        self.exchange.pending = Some(Pending::new(id, body, now, until));
        self.exchange.clear_response();
        self.frames = core::array::from_fn(|_| None);
        self.count = 0;
        self.last_update = now;
        Ok(RequestReplacement { id, superseded })
    }
    /// Whole-frame capacity is checked before publishing any report history.
    pub(crate) fn push(
        &mut self,
        link: LinkIdentity,
        token: u8,
        bytes: &[u8],
        now: Instant,
    ) -> Result<ReportWindowEvent, ReportError> {
        if link != self.exchange.ids.link {
            return Ok(ReportWindowEvent::Ignored);
        }
        self.time(now)?;
        if !self.exchange.matches(link, token, now)? {
            return Ok(ReportWindowEvent::Ignored);
        }
        if self.frames.iter().flatten().any(|b| b.bytes() == bytes) {
            self.last_update = now;
            return Ok(ReportWindowEvent::Ignored);
        }
        if self.count == F {
            return Err(ReportError::Full);
        }
        let body = Body::copy(bytes)?;
        let pending = self.exchange.pending.as_mut().expect("matched request");
        let id = pending.id;
        pending.phase = TxPhase::Waiting;
        self.frames[self.count] = Some(body);
        self.count += 1;
        self.last_update = now;
        Ok(ReportWindowEvent::ReportReceived { id })
    }
    pub(crate) fn frames(&self) -> impl Iterator<Item = &[u8]> {
        self.frames[..self.count].iter().flatten().map(Body::bytes)
    }
    pub(crate) fn transmission(&self) -> Option<Transmission<'_>> {
        self.exchange.transmission()
    }
    pub(crate) fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), ReportError> {
        self.time(now)?;
        self.exchange.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    fn terminal(&self, event: RequestEvent) -> ReportWindowEvent {
        match event {
            RequestEvent::TimedOut { id } if self.count != 0 => ReportWindowEvent::WindowClosed {
                id,
                frames: self.count,
            },
            RequestEvent::TimedOut { id } => ReportWindowEvent::TimedOut { id },
            RequestEvent::TxFailed { id } => ReportWindowEvent::TxFailed { id },
            RequestEvent::Cancelled { id } => ReportWindowEvent::Cancelled { id },
            _ => ReportWindowEvent::Ignored,
        }
    }
    pub(crate) fn completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<ReportWindowEvent, ReportError> {
        if self.exchange.pending.as_ref().is_none_or(|p| p.id != id) {
            return Ok(ReportWindowEvent::Ignored);
        }
        self.time(now)?;
        let event = self.exchange.completed(id, outcome, now)?;
        self.last_update = now;
        Ok(self.terminal(event))
    }
    pub(crate) fn poll(&mut self, now: Instant) -> Result<ReportWindowEvent, ReportError> {
        self.time(now)?;
        let event = self.exchange.poll(now)?;
        self.last_update = now;
        Ok(self.terminal(event))
    }
    pub(crate) fn cancel(&mut self) -> ReportWindowEvent {
        let event = self.exchange.cancel();
        self.terminal(event)
    }
}
/// Whole IEs are packed into bounded complete Action bodies. No element is
/// truncated or split across frames. Acknowledged snapshots may be replayed.
#[derive(Clone)]
pub(crate) struct ReportBatch<const B: usize, const F: usize> {
    frames: [Option<Body<B>>; F],
    count: usize,
    cursor: usize,
}
impl<const B: usize, const F: usize> ReportBatch<B, F> {
    pub(crate) fn new() -> Self {
        Self {
            frames: core::array::from_fn(|_| None),
            count: 0,
            cursor: 0,
        }
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }
    pub(crate) fn append(
        &mut self,
        action: WnmAction,
        dialog: u8,
        elements: Elements<'_>,
    ) -> Result<(), ReportError> {
        if B < ACTION_HEADER_LEN {
            return Err(Error::FrameTooLarge {
                required: ACTION_HEADER_LEN,
                capacity: B,
            }
            .into());
        }
        let mut needed = 1;
        let mut used = ACTION_HEADER_LEN;
        for e in elements.iter() {
            let n = e.body.len() + ELEMENT_HEADER_LEN;
            if n > B - ACTION_HEADER_LEN {
                return Err(Error::FrameTooLarge {
                    required: n + ACTION_HEADER_LEN,
                    capacity: B,
                }
                .into());
            }
            if used + n > B {
                needed += 1;
                used = ACTION_HEADER_LEN;
            }
            used += n;
        }
        if needed > F - self.count {
            return Err(ReportError::Full);
        }
        let mut index = self.count;
        let mut body = Body::<B>::empty();
        body.bytes[..ACTION_HEADER_LEN].copy_from_slice(&[WNM_CATEGORY, action as u8, dialog]);
        body.len = ACTION_HEADER_LEN;
        for e in elements.iter() {
            let n = e.body.len() + ELEMENT_HEADER_LEN;
            if body.len + n > B {
                self.frames[index] = Some(body);
                index += 1;
                body = Body::empty();
                body.bytes[..ACTION_HEADER_LEN].copy_from_slice(&[
                    WNM_CATEGORY,
                    action as u8,
                    dialog,
                ]);
                body.len = ACTION_HEADER_LEN;
            }
            body.bytes[body.len..body.len + ELEMENT_HEADER_LEN]
                .copy_from_slice(&[e.id, e.body.len() as u8]);
            body.bytes[body.len + ELEMENT_HEADER_LEN..body.len + n].copy_from_slice(e.body);
            body.len += n;
        }
        self.frames[index] = Some(body);
        self.count += needed;
        Ok(())
    }
    /// Used while preparing a fresh snapshot, before it can be admitted.
    pub(crate) fn pack_element(
        &mut self,
        action: WnmAction,
        dialog: u8,
        element: &[u8],
    ) -> Result<(), ReportError> {
        let view = Elements::parse(element)?;
        if view.iter().count() != 1 {
            return Err(ReportError::InvalidResult);
        }
        if B >= ACTION_HEADER_LEN && element.len() <= B - ACTION_HEADER_LEN && self.count > 0 {
            let last = self.frames[self.count - 1].as_mut().expect("stored frame");
            if last.len + element.len() <= B {
                last.bytes[last.len..last.len + element.len()].copy_from_slice(element);
                last.len += element.len();
                return Ok(());
            }
        }
        self.append(action, dialog, view)
    }
    pub(crate) fn head(&self) -> Option<&Body<B>> {
        self.frames.get(self.cursor)?.as_ref()
    }
    pub(crate) fn acknowledge(&mut self) {
        if self.cursor < self.count {
            self.cursor += 1;
        }
    }
    pub(crate) fn rewind(&mut self) {
        self.cursor = 0;
    }
    pub(crate) fn done(&self) -> bool {
        self.cursor == self.count
    }
    /// Drop already delivered snapshots only at the caller's explicit boundary.
    pub(crate) fn retire_delivered(&mut self) {
        let remaining = self.count - self.cursor;
        for index in 0..remaining {
            self.frames[index] = self.frames[index + self.cursor].take();
        }
        for slot in &mut self.frames[remaining..] {
            *slot = None;
        }
        self.count = remaining;
        self.cursor = 0;
    }
}
