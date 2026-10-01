//! Neighbor Report dialogs. A received response is retained in its entirety.

use crate::{
    Body, DialogEvent, Error, Identities, LinkIdentity, OperationId, Pending, Sender, Transmission,
    TxOutcome, deadline,
};
use oer_ieee80211_mac::roaming::{Elements, NeighborReportRequest, NeighborReportResponse};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NeighborEvent {
    Ignored,
    ReportsReceived,
    CacheExpired,
    TxFailed,
    TimedOut,
    Cancelled,
}

/// One station request and one bounded response cache in an association epoch.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct NeighborReportRequester<const BYTES: usize> {
    ids: Identities,
    timeout: Duration,
    cache_lifetime: Duration,
    pending: Option<Pending<BYTES>>,
    cache: Option<(Body<BYTES>, Instant)>,
}
impl<const BYTES: usize> NeighborReportRequester<BYTES> {
    pub fn new(
        link: LinkIdentity,
        timeout: Duration,
        cache_lifetime: Duration,
    ) -> Result<Self, Error> {
        deadline(Instant::EPOCH, timeout)?;
        deadline(Instant::EPOCH, cache_lifetime)?;
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            cache_lifetime,
            pending: None,
            cache: None,
        })
    }
    pub fn request(
        &mut self,
        now: Instant,
        request: NeighborReportRequest<'_>,
    ) -> Result<OperationId, Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let body = Body::encode(|output| request.encode(output))?;
        let expires = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.pending = Some(Pending::new(id, body, now, expires));
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.pending.as_ref()?.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        self.pending
            .as_mut()
            .ok_or(Error::NoPendingOperation)?
            .admit(id, now)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<NeighborEvent, Error> {
        if let Some(pending) = &mut self.pending {
            if id != pending.id {
                return Ok(NeighborEvent::Ignored);
            }
            if !pending.live(now)? {
                self.pending = None;
                return Ok(NeighborEvent::TimedOut);
            }
            if pending.complete(id, outcome)? && outcome == TxOutcome::Failed {
                self.pending = None;
                return Ok(NeighborEvent::TxFailed);
            }
        }
        Ok(NeighborEvent::Ignored)
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<NeighborEvent, Error> {
        if link != self.ids.link {
            return Ok(NeighborEvent::Ignored);
        }
        let Some(pending) = &self.pending else {
            return Ok(NeighborEvent::Ignored);
        };
        if !pending.live(now)? {
            self.pending = None;
            return Ok(NeighborEvent::TimedOut);
        }
        let response = NeighborReportResponse::parse(bytes)?;
        if !pending.received(response.dialog_token) {
            return Ok(NeighborEvent::Ignored);
        }
        let body = Body::copy(bytes)?;
        let expires = deadline(now, self.cache_lifetime)?;
        self.cache = Some((body, expires));
        self.pending = None;
        Ok(NeighborEvent::ReportsReceived)
    }
    pub fn reports(&self, now: Instant) -> Option<NeighborReportResponse<'_>> {
        let (body, expires) = self.cache.as_ref()?;
        (now < *expires).then(|| {
            NeighborReportResponse::parse(body.bytes()).expect("validated cached response")
        })
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .map(|pending| pending.deadline)
            .into_iter()
            .chain(self.cache.as_ref().map(|(_, expires)| *expires))
            .min()
    }
    pub fn poll(&mut self, now: Instant) -> Result<NeighborEvent, Error> {
        if let Some(pending) = &self.pending
            && !pending.live(now)?
        {
            self.pending = None;
            return Ok(NeighborEvent::TimedOut);
        }
        if self
            .cache
            .as_ref()
            .is_some_and(|(_, expires)| now >= *expires)
        {
            self.cache = None;
            return Ok(NeighborEvent::CacheExpired);
        }
        Ok(NeighborEvent::Ignored)
    }
    pub fn cancel(&mut self) -> NeighborEvent {
        if self.pending.take().is_some() {
            NeighborEvent::Cancelled
        } else {
            NeighborEvent::Ignored
        }
    }
}

/// AP-side response owner for one peer. The caller supplies its neighbor
/// database and optional measurement responses after inspecting the complete
/// request (including SSID and extension elements); no input is filtered here.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct NeighborReportResponder<const BYTES: usize>(Sender<BYTES>);
impl<const BYTES: usize> NeighborReportResponder<BYTES> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        Ok(Self(Sender::new(link, timeout)?))
    }
    pub fn receive<'a>(
        &self,
        link: LinkIdentity,
        bytes: &'a [u8],
    ) -> Result<Option<NeighborReportRequest<'a>>, Error> {
        if link != self.0.ids.link {
            return Ok(None);
        }
        Ok(Some(NeighborReportRequest::parse(bytes)?))
    }
    pub fn respond(
        &mut self,
        now: Instant,
        request: NeighborReportRequest<'_>,
        elements: Elements<'_>,
    ) -> Result<OperationId, Error> {
        request.validate()?;
        let body = Body::encode(|output| {
            NeighborReportResponse {
                dialog_token: request.dialog_token,
                elements,
            }
            .encode(output)
        })?;
        self.0.start(now, body)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.0.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        self.0.admitted(id, now)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DialogEvent, Error> {
        self.0.completed(id, outcome, now)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.0.next_deadline()
    }
    pub fn poll(&mut self, now: Instant) -> Result<DialogEvent, Error> {
        self.0.poll(now)
    }
    pub fn cancel(&mut self) -> DialogEvent {
        self.0.cancel()
    }
}

#[cfg(test)]
mod tests;
