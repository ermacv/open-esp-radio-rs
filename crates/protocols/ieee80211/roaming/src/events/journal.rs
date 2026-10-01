use super::*;
use crate::{Body, Error};
use oer_ieee80211_mac::roaming as wire;
use oer_time::Instant;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalEventId {
    pub network: NetworkIdentity,
    pub kind: EventType,
    serial: u64,
}
impl JournalEventId {
    pub const fn sequence(self) -> u64 {
        self.serial
    }
}
struct Logged<const B: usize> {
    id: JournalEventId,
    recorded: Instant,
    body: Body<B>,
}
const JOURNAL_TYPES: &[EventType] = &[
    EventType::TRANSITION,
    EventType::RSNA,
    EventType::PEER_LINK,
    EventType::WNM_LOG,
    EventType::VENDOR,
];
const JOURNAL_TYPE_COUNT: usize = JOURNAL_TYPES.len();
/// Independently bounded rings guarantee at least the last five records of
/// each original event type. No BSS transition or report consumes the journal.
// CAPABILITY: wifi-roaming-and-service-discovery-event-reporting-802-11v
pub struct EventJournal<const PER_TYPE: usize, const BYTES: usize> {
    network: NetworkIdentity,
    rows: [[Option<Logged<BYTES>>; PER_TYPE]; JOURNAL_TYPE_COUNT],
    count: [usize; JOURNAL_TYPE_COUNT],
    serial: u64,
    last_update: Instant,
}
fn row(kind: EventType) -> Option<usize> {
    JOURNAL_TYPES.iter().position(|&stored| stored == kind)
}
impl<const N: usize, const B: usize> EventJournal<N, B> {
    pub fn new(network: NetworkIdentity) -> Result<Self, ReportError> {
        if N < MINIMUM_EVENT_HISTORY {
            return Err(ReportError::InvalidJournalCapacity);
        }
        Ok(Self {
            network,
            rows: core::array::from_fn(|_| core::array::from_fn(|_| None)),
            count: [0; JOURNAL_TYPE_COUNT],
            serial: 0,
            last_update: Instant::EPOCH,
        })
    }
    pub const fn network(&self) -> NetworkIdentity {
        self.network
    }
    pub fn record(
        &mut self,
        network: NetworkIdentity,
        report: EventReport<'_>,
        now: Instant,
    ) -> Result<JournalEventId, ReportError> {
        if network != self.network {
            return Err(ReportError::WrongNetwork);
        }
        if now < self.last_update {
            return Err(Error::TimeBeforeOperation.into());
        }
        if report.token != AUTONOMOUS_EVENT_TOKEN
            || report.status != EventReportStatus::SUCCESSFUL
            || report.timing.is_none()
            || report.data == EventData::Empty
        {
            return Err(ReportError::InvalidResult);
        }
        let index = row(report.kind).ok_or(ReportError::UnexpectedType(report.kind.0))?;
        if let EventData::WnmLog(bytes) = report.data
            && !bytes.is_ascii()
        {
            return Err(ReportError::InvalidResult);
        }
        if let EventData::PeerLink(peer) = report.data
            && (!peer.status.is_known() || (peer.status.active() && peer.connection_seconds != 0))
        {
            return Err(ReportError::InvalidResult);
        }
        let body = Body::encode(|out| report.encode(out))?;
        let serial = self.serial.checked_add(1).ok_or(Error::IdentityExhausted)?;
        let id = JournalEventId {
            network,
            kind: report.kind,
            serial,
        };
        if let EventData::PeerLink(peer) = report.data
            && peer.status.terminated_initiation().is_some()
        {
            let mut keep = 0;
            for old in 0..self.count[index] {
                let item = self.rows[index][old].take().expect("retained entry");
                let previous = EventReport::parse(&item.body.bytes()[wire::ELEMENT_HEADER_LEN..])
                    .expect("validated entry");
                let remove = matches!(previous.data,EventData::PeerLink(v) if v.peer==peer.peer && Some(v.status)==peer.status.terminated_initiation());
                if !remove {
                    self.rows[index][keep] = Some(item);
                    keep += 1;
                }
            }
            self.count[index] = keep;
        }
        if self.count[index] == N {
            for old in 1..N {
                self.rows[index][old - 1] = self.rows[index][old].take();
            }
            self.count[index] -= 1;
        }
        self.rows[index][self.count[index]] = Some(Logged {
            id,
            recorded: now,
            body,
        });
        self.count[index] += 1;
        self.serial = serial;
        self.last_update = now;
        Ok(id)
    }
    /// Most recent first, without converting TSF timestamps into host time.
    pub fn records(
        &self,
        kind: EventType,
    ) -> impl Iterator<Item = (JournalEventId, Instant, EventReport<'_>)> {
        let index = row(kind);
        let count = index.map_or(0, |i| self.count[i]);
        (0..count).map(move |offset| {
            let index = index.expect("known row");
            let slot = self.count[index] - 1 - offset;
            let item = self.rows[index][slot].as_ref().expect("retained ring slot");
            (
                item.id,
                item.recorded,
                EventReport::parse(&item.body.bytes()[wire::ELEMENT_HEADER_LEN..])
                    .expect("validated event"),
            )
        })
    }
    pub fn get(&self, id: JournalEventId) -> Option<(Instant, EventReport<'_>)> {
        if id.network != self.network {
            return None;
        }
        self.records(id.kind)
            .find(|(candidate, _, _)| *candidate == id)
            .map(|(_, when, event)| (when, event))
    }
    pub(crate) fn time(&self, now: Instant) -> Result<(), ReportError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn transition_count(&self, since: Instant) -> usize {
        self.records(EventType::TRANSITION)
            .filter(|(_, when, _)| *when >= since)
            .count()
    }
    /// Leaving the network clears records while preserving local sequence identity.
    pub fn change_network(
        &mut self,
        network: NetworkIdentity,
        now: Instant,
    ) -> Result<(), ReportError> {
        self.time(now)?;
        if network == self.network {
            self.last_update = now;
            return Ok(());
        }
        self.rows = core::array::from_fn(|_| core::array::from_fn(|_| None));
        self.count = [0; JOURNAL_TYPE_COUNT];
        self.network = network;
        self.last_update = now;
        Ok(())
    }
}
