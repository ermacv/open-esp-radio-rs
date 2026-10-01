//! RRM transactions and measurement execution plans for either peer role.
mod radio;
pub use radio::*;
mod beacon;
pub use beacon::*;

use crate::{
    Body, DialogEvent, Error, LinkIdentity, OperationId, RequestEvent, Sender, Transmission,
    TxOutcome, exchange::Exchange,
};
use oer_ieee80211_mac::roaming::{LinkMeasurementReport, LinkMeasurementRequest};
use oer_time::{Duration, Instant};

/// Solicited Link Measurement reports, including every optional subelement.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct LinkMeasurementRequester<const BYTES: usize>(Exchange<BYTES>);
impl<const BYTES: usize> LinkMeasurementRequester<BYTES> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        Ok(Self(Exchange::new(link, timeout)?))
    }
    pub fn request(
        &mut self,
        now: Instant,
        request: LinkMeasurementRequest<'_>,
    ) -> Result<OperationId, Error> {
        let body = Body::encode(|out| request.encode(out))?;
        self.0.start(body, now)
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
    ) -> Result<RequestEvent, Error> {
        self.0.completed(id, outcome, now)
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<RequestEvent, Error> {
        if link != self.0.ids.link || self.0.pending.is_none() {
            return Ok(RequestEvent::Ignored);
        }
        let report = LinkMeasurementReport::parse(bytes)?;
        if !self.0.matches(link, report.dialog_token, now)? {
            return Ok(RequestEvent::Ignored);
        }
        self.0.accept(bytes)
    }
    /// Completed result, retained until the next request. This is a historical
    /// measurement snapshot, not a claim that the measured link remains fresh.
    pub fn report(&self) -> Option<LinkMeasurementReport<'_>> {
        self.0
            .response()
            .map(|bytes| LinkMeasurementReport::parse(bytes).expect("validated response"))
    }
    pub fn poll(&mut self, now: Instant) -> Result<RequestEvent, Error> {
        self.0.poll(now)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.0.next_deadline()
    }
    pub fn cancel(&mut self) -> RequestEvent {
        self.0.cancel()
    }
}

/// The caller supplies measured RX power/SNR, antennas and actual TX power.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct LinkMeasurementResponder<const BYTES: usize>(Sender<BYTES>);
impl<const BYTES: usize> LinkMeasurementResponder<BYTES> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        Ok(Self(Sender::new(link, timeout)?))
    }
    pub fn receive<'a>(
        &self,
        link: LinkIdentity,
        bytes: &'a [u8],
    ) -> Result<Option<LinkMeasurementRequest<'a>>, Error> {
        if link != self.0.ids.link {
            return Ok(None);
        }
        Ok(Some(LinkMeasurementRequest::parse(bytes)?))
    }
    pub fn respond(
        &mut self,
        now: Instant,
        request: LinkMeasurementRequest<'_>,
        report: LinkMeasurementReport<'_>,
    ) -> Result<OperationId, Error> {
        if request.dialog_token != report.dialog_token {
            return Err(Error::WrongOperation);
        }
        let body = Body::encode(|out| report.encode(out))?;
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
    pub fn poll(&mut self, now: Instant) -> Result<DialogEvent, Error> {
        self.0.poll(now)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.0.next_deadline()
    }
    pub fn cancel(&mut self) -> DialogEvent {
        self.0.cancel()
    }
}

#[cfg(test)]
mod tests;
