//! Shared request transport lifecycle, private to typed protocol owners.
use crate::{
    Body, Error, Identities, LinkIdentity, OperationId, Pending, RequestEvent, Transmission,
    TxOutcome, deadline,
};
use oer_time::{Duration, Instant};

pub(crate) struct Exchange<const BYTES: usize> {
    pub(crate) ids: Identities,
    timeout: Duration,
    pub(crate) pending: Option<Pending<BYTES>>,
    response: Option<Body<BYTES>>,
}
impl<const BYTES: usize> Exchange<BYTES> {
    pub(crate) fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        deadline(Instant::EPOCH, timeout)?;
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            pending: None,
            response: None,
        })
    }
    pub(crate) fn start(&mut self, body: Body<BYTES>, now: Instant) -> Result<OperationId, Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let until = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.pending = Some(Pending::new(id, body, now, until));
        self.response = None;
        Ok(id)
    }
    pub(crate) fn transmission(&self) -> Option<Transmission<'_>> {
        self.pending.as_ref()?.transmission()
    }
    pub(crate) fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        self.pending
            .as_mut()
            .ok_or(Error::NoPendingOperation)?
            .admit(id, now)
    }
    pub(crate) fn matches(
        &self,
        link: LinkIdentity,
        token: u8,
        now: Instant,
    ) -> Result<bool, Error> {
        if link != self.ids.link {
            return Ok(false);
        }
        let Some(pending) = &self.pending else {
            return Ok(false);
        };
        Ok(pending.live(now)? && pending.received(token))
    }
    pub(crate) fn accept(&mut self, bytes: &[u8]) -> Result<RequestEvent, Error> {
        let body = Body::copy(bytes)?;
        let pending = self.pending.take().ok_or(Error::NoPendingOperation)?;
        self.response = Some(body);
        Ok(RequestEvent::ResponseReceived { id: pending.id })
    }
    pub(crate) fn response(&self) -> Option<&[u8]> {
        self.response.as_ref().map(Body::bytes)
    }
    pub(crate) fn remember_response(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.response = Some(Body::copy(bytes)?);
        Ok(())
    }
    pub(crate) fn clear_response(&mut self) {
        self.response = None;
    }
    pub(crate) fn completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<RequestEvent, Error> {
        let Some(pending) = &mut self.pending else {
            return Ok(RequestEvent::Ignored);
        };
        if pending.id != id {
            return Ok(RequestEvent::Ignored);
        }
        if !pending.live(now)? {
            self.pending = None;
            return Ok(RequestEvent::TimedOut { id });
        }
        if pending.complete(id, outcome)? && outcome == TxOutcome::Failed {
            self.pending = None;
            return Ok(RequestEvent::TxFailed { id });
        }
        Ok(RequestEvent::Ignored)
    }
    pub(crate) fn poll(&mut self, now: Instant) -> Result<RequestEvent, Error> {
        if let Some(pending) = &self.pending
            && !pending.live(now)?
        {
            let id = pending.id;
            self.pending = None;
            return Ok(RequestEvent::TimedOut { id });
        }
        Ok(RequestEvent::Ignored)
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.deadline)
    }
    pub(crate) fn cancel(&mut self) -> RequestEvent {
        self.pending
            .take()
            .map_or(RequestEvent::Ignored, |pending| RequestEvent::Cancelled {
                id: pending.id,
            })
    }
}
