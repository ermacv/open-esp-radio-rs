use super::*;
use crate::management::{PeerDialog, Reception};
use crate::{Body, DialogEvent, LinkIdentity, OperationId, Transmission, TxOutcome};
use oer_ieee80211_mac::roaming::{TfsRequestFrame, TfsResponseFrame};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TfsApEvent {
    Ignored,
    Requested {
        id: OperationId,
        superseded: Option<OperationId>,
    },
    ReplayQueued {
        id: OperationId,
    },
    TimedOut {
        id: OperationId,
    },
}
/// AP owns both the negotiated filters and the separate request/response dialog.
/// A prepared replacement is validated before its reply or filters are published.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-traffic-filtering-service-802-11v
pub struct TfsAccessPoint<const BYTES: usize, const FILTERS: usize> {
    dialog: PeerDialog<BYTES>,
    filters: TrafficFilters<BYTES, FILTERS>,
}
impl<const B: usize, const F: usize> TfsAccessPoint<B, F> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, TfsError> {
        Ok(Self {
            dialog: PeerDialog::new(link, timeout)?,
            filters: TrafficFilters::new(link),
        })
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<TfsApEvent, TfsError> {
        if link != self.dialog.ids.link {
            return Ok(TfsApEvent::Ignored);
        }
        TfsRequestFrame::parse(bytes)?;
        Ok(match self.dialog.receive(link, bytes, now)? {
            Reception::Ignored => TfsApEvent::Ignored,
            Reception::Requested { id, superseded } => TfsApEvent::Requested { id, superseded },
            Reception::ReplayQueued { id } => TfsApEvent::ReplayQueued { id },
        })
    }
    pub fn request(&self) -> Option<(OperationId, TfsRequestFrame<'_>)> {
        self.dialog.incoming.as_ref().map(|p| {
            (
                p.id,
                TfsRequestFrame::parse(p.body.bytes()).expect("validated request"),
            )
        })
    }
    /// The supplied response contains exactly one status per requested filter.
    /// A denied alternative is retained as a hint, never installed implicitly.
    pub fn respond(
        &mut self,
        id: OperationId,
        response: TfsResponseFrame<'_>,
        now: Instant,
    ) -> Result<OperationId, TfsError> {
        let (current, request) = self.request().ok_or(Error::NoPendingOperation)?;
        if current != id || request.dialog_token != response.dialog_token {
            return Err(Error::WrongOperation.into());
        }
        let plan = self.filters.prepare_install(
            self.dialog.ids.link,
            request.elements,
            response.elements,
        )?;
        let body = Body::encode(|out| response.encode(out))?;
        let tx = self.dialog.reply(id, body, now, true)?;
        self.filters.apply_install(plan);
        Ok(tx)
    }
    pub fn negotiation(
        &self,
    ) -> (
        oer_ieee80211_mac::roaming::Elements<'_>,
        oer_ieee80211_mac::roaming::Elements<'_>,
    ) {
        self.filters.negotiation()
    }
    pub fn evaluate(
        &mut self,
        link: LinkIdentity,
        input: TrafficInput<'_>,
    ) -> Result<TrafficDecision<F>, TfsError> {
        Ok(self.filters.evaluate(link, input)?)
    }
    pub fn traffic_applied(
        &mut self,
        id: OperationId,
        notification_queued: bool,
        matched_frame_queued: bool,
    ) -> Result<(), TfsError> {
        Ok(self
            .filters
            .applied(id, notification_queued, matched_frame_queued)?)
    }
    pub fn cancel_traffic(&mut self, id: OperationId) -> Result<(), TfsError> {
        Ok(self.filters.cancel(id)?)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.dialog.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), TfsError> {
        Ok(self.dialog.admitted(id, now)?)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DialogEvent, TfsError> {
        Ok(self.dialog.completed(id, outcome, now)?)
    }
    pub fn poll(&mut self, now: Instant) -> Result<TfsApEvent, TfsError> {
        let (request, tx) = self.dialog.poll(now)?;
        Ok(request
            .or(tx)
            .map_or(TfsApEvent::Ignored, |id| TfsApEvent::TimedOut { id }))
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.dialog.next_deadline()
    }
    pub fn cancel_request(&mut self) -> Option<OperationId> {
        self.dialog.cancel()
    }
}
