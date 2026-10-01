//! Association-scoped neighbor data and independently observed BSS records.
//!
//! Advertisements never manufacture a scan observation. A report and an
//! observation expire independently, and an oversized batch changes nothing.

use oer_ieee80211_mac::roaming::ELEMENT_HEADER_LEN;
use oer_ieee80211_mac::{
    channel::Channel,
    roaming::{
        BssLoad, Elements, MacAddress, NEIGHBOR_REPORT_ELEMENT_ID, NeighborReport,
        PeerCapabilities, WireError,
    },
    scan::ScanRecord,
    ssid::WifiSsid,
};
use oer_time::{Duration, Instant};

use crate::{Body, Error, LinkIdentity, deadline};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseError {
    Protocol(Error),
    Full,
    InvalidBssid,
    MalformedObservation,
    ChannelMismatch,
    DuplicateBssid(MacAddress),
}
impl From<Error> for DatabaseError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
impl From<WireError> for DatabaseError {
    fn from(value: WireError) -> Self {
        Self::Protocol(Error::Wire(value))
    }
}

/// One normalized beacon/probe observation. Lengths are checked once before
/// existing MAC security/PHY selectors can inspect the retained ScanRecord.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    record: ScanRecord,
    channel: Channel,
    capabilities: PeerCapabilities,
    load: Option<BssLoad>,
}
impl Observation {
    /// `elements` is the same observation's complete information-element
    /// sequence, used for peer capabilities and BSS Load. The integration owns
    /// receiver timing and channel/regulatory admission, not this value.
    pub fn new(
        record: ScanRecord,
        channel: Channel,
        elements: Elements<'_>,
    ) -> Result<Self, DatabaseError> {
        valid_bssid(record.bssid)?;
        if record.channel != channel.number() {
            return Err(DatabaseError::ChannelMismatch);
        }
        if usize::from(record.ssid_len) > record.ssid.len()
            || usize::from(record.supported_rates_len) > record.supported_rates.len()
            || usize::from(record.extended_supported_rates_len)
                > record.extended_supported_rates.len()
            || usize::from(record.he_capability_ie_len) > record.he_capability_ie.len()
            || usize::from(record.he_operation_ie_len) > record.he_operation_ie.len()
            || usize::from(record.wmm_ie_len) > record.wmm_ie.len()
            || usize::from(record.rsn_ie_len) > record.rsn_ie.len()
            || usize::from(record.rsnxe_len) > record.rsnxe.len()
        {
            return Err(DatabaseError::MalformedObservation);
        }
        Ok(Self {
            record,
            channel,
            capabilities: PeerCapabilities::parse(elements)?,
            load: BssLoad::parse(elements)?,
        })
    }
    pub const fn record(&self) -> &ScanRecord {
        &self.record
    }
    pub const fn channel(&self) -> Channel {
        self.channel
    }
    pub const fn capabilities(&self) -> &PeerCapabilities {
        &self.capabilities
    }
    pub const fn load(&self) -> Option<BssLoad> {
        self.load
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportSource {
    NeighborResponse { token: u8 },
    BssTransition { token: u8 },
    LocalDatabase,
}

/// The SSID is context supplied with the initiating request or local database;
/// Neighbor Report itself does not contain an SSID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReportContext {
    pub source: ReportSource,
    pub ssid: Option<WifiSsid>,
}

#[derive(Clone)]
struct Timed<T> {
    value: T,
    observed: Instant,
    expires: Instant,
}
#[derive(Clone)]
struct StoredReport<const BYTES: usize> {
    body: Body<BYTES>,
    context: ReportContext,
}
#[derive(Clone)]
struct Entry<const BYTES: usize> {
    bssid: MacAddress,
    report: Option<Timed<StoredReport<BYTES>>>,
    observation: Option<Timed<Observation>>,
}
impl<const BYTES: usize> Entry<BYTES> {
    fn live(&self, now: Instant) -> bool {
        self.report
            .as_ref()
            .is_some_and(|report| now < report.expires)
            || self
                .observation
                .as_ref()
                .is_some_and(|observation| now < observation.expires)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseEvent {
    Ignored,
    Updated { entries: usize },
    Expired { reports: usize, observations: usize },
}

pub struct ReportView<'a> {
    pub report: NeighborReport<'a>,
    pub context: ReportContext,
    pub observed: Instant,
    pub expires: Instant,
}
pub struct ObservationView<'a> {
    pub observation: &'a Observation,
    pub observed: Instant,
    pub expires: Instant,
}
pub struct NeighborView<'a> {
    pub bssid: MacAddress,
    pub report: Option<ReportView<'a>>,
    pub observation: Option<ObservationView<'a>>,
}

/// Fixed-capacity storage for one association epoch. Live entries are never
/// silently evicted: an input exceeding available capacity returns `Full`.
// CAPABILITY: wifi-roaming-and-service-discovery-radio-resource-measurement-802-11k
pub struct NeighborDatabase<const ENTRIES: usize, const REPORT_BYTES: usize> {
    link: LinkIdentity,
    entries: [Option<Entry<REPORT_BYTES>>; ENTRIES],
    last_update: Instant,
}
impl<const ENTRIES: usize, const REPORT_BYTES: usize> NeighborDatabase<ENTRIES, REPORT_BYTES> {
    pub fn new(link: LinkIdentity) -> Self {
        Self {
            link,
            entries: core::array::from_fn(|_| None),
            last_update: Instant::EPOCH,
        }
    }
    pub const fn link(&self) -> LinkIdentity {
        self.link
    }
    fn time(&self, now: Instant) -> Result<(), DatabaseError> {
        if now < self.last_update {
            return Err(Error::TimeBeforeOperation.into());
        }
        Ok(())
    }
    fn slot(
        &self,
        bssid: MacAddress,
        now: Instant,
        reserved: impl Fn(usize) -> bool,
    ) -> Result<usize, DatabaseError> {
        if let Some((index, _)) = self.entries.iter().enumerate().find(|(index, entry)| {
            !reserved(*index) && entry.as_ref().is_some_and(|entry| entry.bssid == bssid)
        }) {
            return Ok(index);
        }
        self.entries
            .iter()
            .enumerate()
            .find(|(index, entry)| {
                !reserved(*index) && entry.as_ref().is_none_or(|entry| !entry.live(now))
            })
            .map(|(index, _)| index)
            .ok_or(DatabaseError::Full)
    }
    /// Atomically retain all Neighbor Report elements. Optional Action-level
    /// elements remain with the dialog owner; each neighbor retains every
    /// subelement. A duplicate BSSID in one batch is an explicit conflict.
    pub fn update_reports(
        &mut self,
        link: LinkIdentity,
        elements: Elements<'_>,
        context: ReportContext,
        now: Instant,
        lifetime: Duration,
    ) -> Result<DatabaseEvent, DatabaseError> {
        if link != self.link {
            return Ok(DatabaseEvent::Ignored);
        }
        self.time(now)?;
        let expires = deadline(now, lifetime)?;
        struct Update<const BYTES: usize> {
            index: usize,
            bssid: MacAddress,
            report: StoredReport<BYTES>,
        }
        let mut updates: [Option<Update<REPORT_BYTES>>; ENTRIES] = core::array::from_fn(|_| None);
        let mut count = 0;
        for element in elements
            .iter()
            .filter(|element| element.id == NEIGHBOR_REPORT_ELEMENT_ID)
        {
            let report = NeighborReport::parse(element.body)?;
            valid_bssid(report.bssid)?;
            if updates
                .iter()
                .flatten()
                .any(|update| update.bssid == report.bssid)
            {
                return Err(DatabaseError::DuplicateBssid(report.bssid));
            }
            let index = self.slot(report.bssid, now, |index| {
                updates.iter().flatten().any(|update| update.index == index)
            })?;
            let body = Body::encode(|output| report.encode(output))?;
            let update = updates.get_mut(count).ok_or(DatabaseError::Full)?;
            *update = Some(Update {
                index,
                bssid: report.bssid,
                report: StoredReport { body, context },
            });
            count += 1;
        }
        for update in updates.into_iter().flatten() {
            let slot = &mut self.entries[update.index];
            if slot
                .as_ref()
                .is_none_or(|entry| entry.bssid != update.bssid)
            {
                *slot = Some(Entry {
                    bssid: update.bssid,
                    report: None,
                    observation: None,
                });
            }
            slot.as_mut().expect("prepared entry").report = Some(Timed {
                value: update.report,
                observed: now,
                expires,
            });
        }
        self.last_update = now;
        Ok(DatabaseEvent::Updated { entries: count })
    }
    pub fn observe(
        &mut self,
        link: LinkIdentity,
        observation: Observation,
        now: Instant,
        lifetime: Duration,
    ) -> Result<DatabaseEvent, DatabaseError> {
        if link != self.link {
            return Ok(DatabaseEvent::Ignored);
        }
        self.time(now)?;
        let expires = deadline(now, lifetime)?;
        let index = self.slot(observation.record.bssid, now, |_| false)?;
        let slot = &mut self.entries[index];
        if slot
            .as_ref()
            .is_none_or(|entry| entry.bssid != observation.record.bssid)
        {
            *slot = Some(Entry {
                bssid: observation.record.bssid,
                report: None,
                observation: None,
            });
        }
        slot.as_mut().expect("prepared entry").observation = Some(Timed {
            value: observation,
            observed: now,
            expires,
        });
        self.last_update = now;
        Ok(DatabaseEvent::Updated { entries: 1 })
    }
    pub fn entries(
        &self,
        now: Instant,
    ) -> Result<impl Iterator<Item = NeighborView<'_>>, DatabaseError> {
        self.time(now)?;
        Ok(self
            .entries
            .iter()
            .flatten()
            .filter(move |entry| entry.live(now))
            .map(move |entry| NeighborView {
                bssid: entry.bssid,
                report: entry
                    .report
                    .as_ref()
                    .filter(|report| now < report.expires)
                    .map(|report| ReportView {
                        report: NeighborReport::parse(
                            &report.value.body.bytes()[ELEMENT_HEADER_LEN..],
                        )
                        .expect("validated report"),
                        context: report.value.context,
                        observed: report.observed,
                        expires: report.expires,
                    }),
                observation: entry
                    .observation
                    .as_ref()
                    .filter(|observation| now < observation.expires)
                    .map(|observation| ObservationView {
                        observation: &observation.value,
                        observed: observation.observed,
                        expires: observation.expires,
                    }),
            }))
    }
    pub fn get(
        &self,
        bssid: MacAddress,
        now: Instant,
    ) -> Result<Option<NeighborView<'_>>, DatabaseError> {
        Ok(self.entries(now)?.find(|entry| entry.bssid == bssid))
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.entries
            .iter()
            .flatten()
            .flat_map(|entry| {
                entry
                    .report
                    .as_ref()
                    .map(|report| report.expires)
                    .into_iter()
                    .chain(
                        entry
                            .observation
                            .as_ref()
                            .map(|observation| observation.expires),
                    )
            })
            .min()
    }
    pub fn expire(&mut self, now: Instant) -> Result<DatabaseEvent, DatabaseError> {
        self.time(now)?;
        let mut reports = 0;
        let mut observations = 0;
        for slot in &mut self.entries {
            let Some(entry) = slot else {
                continue;
            };
            if entry
                .report
                .as_ref()
                .is_some_and(|report| now >= report.expires)
            {
                entry.report = None;
                reports += 1;
            }
            if entry
                .observation
                .as_ref()
                .is_some_and(|observation| now >= observation.expires)
            {
                entry.observation = None;
                observations += 1;
            }
            if entry.report.is_none() && entry.observation.is_none() {
                *slot = None;
            }
        }
        self.last_update = now;
        Ok(DatabaseEvent::Expired {
            reports,
            observations,
        })
    }
}

fn valid_bssid(bssid: MacAddress) -> Result<(), DatabaseError> {
    if !crate::valid_peer_address(bssid) {
        return Err(DatabaseError::InvalidBssid);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
