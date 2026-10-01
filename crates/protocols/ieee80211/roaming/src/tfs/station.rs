use super::*;
use crate::exchange::Exchange;
use crate::{
    Body, Error, LinkIdentity, OperationId, RequestEvent, Transmission, TxOutcome, TxPhase,
};
use oer_ieee80211_mac::roaming as wire;
use oer_ieee80211_mac::roaming::{TfsAction, TfsStatusCode, element_id};
use oer_ieee80211_mac::roaming::{TfsNotify, TfsRequest, TfsRequestFrame, TfsResponseFrame};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TfsNotifications {
    ids: [u8; wire::MAX_ELEMENT_BODY_LEN],
    len: u8,
}
impl TfsNotifications {
    pub fn ids(&self) -> &[u8] {
        &self.ids[..usize::from(self.len)]
    }
}
/// STA records the AP's accepted full replacement. Lost admitted requests leave
/// membership uncertain until an explicit new full replacement is acknowledged.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-traffic-filtering-service-802-11v
pub struct TfsStation<const BYTES: usize> {
    exchange: Exchange<BYTES>,
    request: Body<BYTES>,
    response: Body<BYTES>,
    accepted: [bool; crate::OCTET_VALUE_COUNT],
    state: TfsState,
    last_update: Instant,
}
impl<const B: usize> TfsStation<B> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, TfsError> {
        if !crate::valid_peer_address(link.peer) {
            return Err(Error::InvalidTarget.into());
        }
        Ok(Self {
            exchange: Exchange::new(link, timeout)?,
            request: Body::empty(),
            response: Body::empty(),
            accepted: [false; crate::OCTET_VALUE_COUNT],
            state: TfsState::Unconfigured,
            last_update: Instant::EPOCH,
        })
    }
    fn time(&self, now: Instant) -> Result<(), TfsError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub const fn state(&self) -> TfsState {
        self.state
    }
    pub fn request(
        &mut self,
        request: TfsRequestFrame<'_>,
        now: Instant,
    ) -> Result<OperationId, TfsError> {
        self.time(now)?;
        for element in request
            .elements
            .iter()
            .filter(|e| e.id == element_id::TFS_REQUEST)
        {
            super::traffic::admit_filter(TfsRequest::parse(element.body)?)?;
        }
        let body = Body::encode(|out| request.encode(out))?;
        let id = self.exchange.start(body, now)?;
        self.last_update = now;
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.exchange.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), TfsError> {
        self.time(now)?;
        self.exchange.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<TfsStationEvent, TfsError> {
        if link != self.exchange.ids.link {
            return Ok(TfsStationEvent::Ignored);
        }
        self.time(now)?;
        let response = TfsResponseFrame::parse(bytes)?;
        if !self.exchange.matches(link, response.dialog_token, now)? {
            return Ok(TfsStationEvent::Ignored);
        }
        let pending = self.exchange.pending.as_ref().expect("matched request");
        let request = TfsRequestFrame::parse(pending.body.bytes())?;
        let count = validate_tfs_negotiation(request.elements, response.elements)?;
        let retained_request = Body::copy(pending.body.bytes())?;
        let retained_response = Body::copy(bytes)?;
        let mut accepted = [false; crate::OCTET_VALUE_COUNT];
        for element in response
            .elements
            .iter()
            .filter(|e| e.id == element_id::TFS_RESPONSE)
        {
            for status in
                oer_ieee80211_mac::roaming::TfsResponse::parse(element.body)?.statuses()?
            {
                accepted[usize::from(status.id)] = status.status == TfsStatusCode::ACCEPT;
            }
        }
        let RequestEvent::ResponseReceived { id } = self.exchange.accept(bytes)? else {
            unreachable!("response accepted");
        };
        self.request = retained_request;
        self.response = retained_response;
        self.accepted = accepted;
        self.state = if count == 0 {
            TfsState::Unconfigured
        } else {
            TfsState::Configured
        };
        self.last_update = now;
        Ok(TfsStationEvent::ResponseReceived { id })
    }
    pub fn negotiation(&self) -> Option<(TfsRequestFrame<'_>, TfsResponseFrame<'_>)> {
        if self.request.len == 0 {
            return None;
        }
        Some((
            TfsRequestFrame::parse(self.request.bytes()).expect("retained request"),
            TfsResponseFrame::parse(self.response.bytes()).expect("retained response"),
        ))
    }
    pub fn receive_notify(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Option<TfsNotifications>, TfsError> {
        if link != self.exchange.ids.link {
            return Ok(None);
        }
        self.time(now)?;
        let notice = TfsNotify::parse(bytes)?;
        if self.state == TfsState::Uncertain {
            return Err(Error::NoPendingOperation.into());
        }
        for &id in notice.ids {
            if !self.accepted[usize::from(id)] {
                return Err(TfsError::UnknownNotification(id));
            }
        }
        let request = self.negotiation().ok_or(Error::NoPendingOperation)?.0;
        let mut delete = [false; crate::OCTET_VALUE_COUNT];
        for filter in request
            .elements
            .iter()
            .filter(|e| e.id == element_id::TFS_REQUEST)
        {
            let filter = TfsRequest::parse(filter.body)?;
            if notice.ids.contains(&filter.id)
                && filter.action.contains(TfsAction::DELETE_AFTER_MATCH)
            {
                delete[usize::from(filter.id)] = true;
            }
        }
        let mut ids = [0; wire::MAX_ELEMENT_BODY_LEN];
        ids[..notice.ids.len()].copy_from_slice(notice.ids);
        for (index, delete) in delete.iter().enumerate() {
            if *delete {
                self.accepted[index] = false;
            }
        }
        self.last_update = now;
        Ok(Some(TfsNotifications {
            ids,
            len: notice.ids.len() as u8,
        }))
    }
    fn terminal(&mut self, event: RequestEvent, admitted: bool) -> TfsStationEvent {
        let id = match event {
            RequestEvent::TxFailed { id }
            | RequestEvent::TimedOut { id }
            | RequestEvent::Cancelled { id } => Some(id),
            _ => None,
        };
        if admitted && let Some(id) = id {
            self.state = TfsState::Uncertain;
            TfsStationEvent::RecoveryRequired { id }
        } else if event == RequestEvent::Ignored {
            TfsStationEvent::Ignored
        } else {
            TfsStationEvent::Request(event)
        }
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<TfsStationEvent, TfsError> {
        if self.exchange.pending.as_ref().is_none_or(|p| p.id != id) {
            return Ok(TfsStationEvent::Ignored);
        }
        self.time(now)?;
        let admitted = self
            .exchange
            .pending
            .as_ref()
            .is_some_and(|p| p.phase != TxPhase::Ready);
        let event = self.exchange.completed(id, outcome, now)?;
        self.last_update = now;
        Ok(self.terminal(event, admitted))
    }
    pub fn poll(&mut self, now: Instant) -> Result<TfsStationEvent, TfsError> {
        self.time(now)?;
        let admitted = self
            .exchange
            .pending
            .as_ref()
            .is_some_and(|p| p.phase != TxPhase::Ready);
        let event = self.exchange.poll(now)?;
        self.last_update = now;
        Ok(self.terminal(event, admitted))
    }
    pub fn cancel(&mut self) -> TfsStationEvent {
        let admitted = self
            .exchange
            .pending
            .as_ref()
            .is_some_and(|p| p.phase != TxPhase::Ready);
        let event = self.exchange.cancel();
        self.terminal(event, admitted)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.exchange.next_deadline()
    }
}
