//! Sleep requires at least one accepted filter; matching is shared with standalone TFS.
pub use crate::tfs::{TrafficDecision, TrafficError, TrafficInput, validate_sleep_filters};
use crate::{LinkIdentity, OperationId};
use oer_ieee80211_mac::roaming::Elements;

/// Sleep-specific admission over the common per-association TFS filter owner.
pub struct SleepTrafficFilters<const BYTES: usize, const FILTERS: usize> {
    inner: crate::tfs::TrafficFilters<BYTES, FILTERS>,
}
impl<const B: usize, const F: usize> SleepTrafficFilters<B, F> {
    pub fn new(link: LinkIdentity) -> Self {
        Self {
            inner: crate::tfs::TrafficFilters::new(link),
        }
    }
    pub fn install(
        &mut self,
        link: LinkIdentity,
        request: Elements<'_>,
        response: Elements<'_>,
    ) -> Result<(), TrafficError> {
        validate_sleep_filters(request, response)?;
        self.inner.install(link, request, response)
    }
    pub fn clear(&mut self, link: LinkIdentity) -> Result<(), TrafficError> {
        self.inner.clear(link)
    }
    pub fn negotiation(&self) -> (Elements<'_>, Elements<'_>) {
        self.inner.negotiation()
    }
    pub fn evaluate(
        &mut self,
        link: LinkIdentity,
        input: TrafficInput<'_>,
    ) -> Result<TrafficDecision<F>, TrafficError> {
        self.inner.evaluate(link, input)
    }
    pub fn applied(
        &mut self,
        id: OperationId,
        notification_queued: bool,
        matched_frame_queued: bool,
    ) -> Result<(), TrafficError> {
        self.inner
            .applied(id, notification_queued, matched_frame_queued)
    }
    pub fn cancel(&mut self, id: OperationId) -> Result<(), TrafficError> {
        self.inner.cancel(id)
    }
}
#[cfg(test)]
pub(super) mod tests;
