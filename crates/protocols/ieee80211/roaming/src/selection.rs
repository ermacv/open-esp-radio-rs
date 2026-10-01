//! Candidate admission, scoring and bounded roaming decisions without I/O.
//!
//! A report is advice; only a live observation can be selected. Scoring has
//! explicit weights, with no guessed PHY throughput. The caller supplies the
//! local PHY admission and, optionally, its independently derived estimate.

use oer_ieee80211_mac::{
    channel::{Band, Channel},
    roaming::{BtmRequest, BtmRequestMode, MacAddress, NEIGHBOR_REPORT_ELEMENT_ID, NeighborReport},
    security::StaSecurityPolicy,
    ssid::WifiSsid,
    station::{SelectedRsn, select_association_rsn},
};
use oer_time::{Duration, Instant};

use crate::{
    Error, Identities, LinkIdentity, OperationId,
    btm::ProposalTiming,
    database::{DatabaseError, NeighborDatabase, Observation, ReportSource},
};

const MINIMUM_RSSI_DBM: i8 = i8::MIN + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BandPolicy {
    pub minimum_rssi_dbm: i8,
    /// Stronger RSSI stops adding score above this level.
    pub sufficient_rssi_dbm: i8,
}

/// Weights are score points per dB, or maximum bonus points for the other
/// terms. Unknown load/throughput/preference adds zero bonus. AP preference
/// never overrides eligibility, and preference zero excludes a candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScoreWeights {
    pub rssi: u16,
    pub available_channel: u16,
    pub ap_preference: u16,
    pub throughput: u16,
    pub throughput_limit_kbps: u32,
}

/// Explicit product policy. These are tunable decisions, not IEEE defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionPolicy {
    pub ghz2_4: BandPolicy,
    pub ghz5: BandPolicy,
    pub weights: ScoreWeights,
    pub minimum_score_improvement: u32,
    pub confirmation_time: Duration,
    pub cooldown: Duration,
    pub attempt_timeout: Duration,
    pub failure_threshold: u16,
    pub failure_block: Duration,
    pub maximum_failure_block: Duration,
    pub failure_memory: Duration,
    /// Permit a safe but lower-scoring target when the AP announces imminent
    /// disassociation. It still needs fresh observations and confirmation.
    pub imminent_btm_bypasses_improvement: bool,
}
impl SelectionPolicy {
    fn validate(self) -> Result<(), SelectionError> {
        for band in [self.ghz2_4, self.ghz5] {
            if band.minimum_rssi_dbm == i8::MIN
                || band.minimum_rssi_dbm > band.sufficient_rssi_dbm
                || band.sufficient_rssi_dbm > 0
            {
                return Err(SelectionError::InvalidPolicy);
            }
        }
        if self.weights.rssi == 0
            || self.weights.throughput_limit_kbps == 0
            || self.attempt_timeout == Duration::ZERO
            || self.failure_threshold == 0
            || self.failure_block == Duration::ZERO
            || self.maximum_failure_block < self.failure_block
            || self.failure_memory < self.maximum_failure_block
        {
            return Err(SelectionError::InvalidPolicy);
        }
        Ok(())
    }
    fn band(self, band: Band) -> BandPolicy {
        match band {
            Band::Ghz2_4 => self.ghz2_4,
            Band::Ghz5 => self.ghz5,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionError {
    Protocol(Error),
    Database(DatabaseError),
    InvalidPolicy,
    FailureTableFull,
    InvalidNewLink,
}
impl From<Error> for SelectionError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
impl From<DatabaseError> for SelectionError {
    fn from(value: DatabaseError) -> Self {
        Self::Database(value)
    }
}

/// The integration's local radio/PHY admission, evaluated for each observed
/// BSS. `None` throughput means unavailable, and is never synthesized from RSSI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CandidateAssessment {
    pub phy_supported: bool,
    pub estimated_throughput_kbps: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub enum SelectionTrigger<'a> {
    Autonomous,
    Btm {
        proposal: OperationId,
        request: BtmRequest<'a>,
        timing: ProposalTiming,
    },
}
impl SelectionTrigger<'_> {
    fn proposal(self) -> Option<OperationId> {
        match self {
            Self::Autonomous => None,
            Self::Btm { proposal, .. } => Some(proposal),
        }
    }
    fn validate(self, link: LinkIdentity) -> Result<(), SelectionError> {
        let Self::Btm {
            proposal, request, ..
        } = self
        else {
            return Ok(());
        };
        if proposal.link != link {
            return Err(Error::WrongOperation.into());
        }
        request.validate().map_err(Error::from)?;
        if request.mode.0 & !BtmRequestMode::KNOWN_BITS != 0 {
            return Err(Error::UnsupportedRequestMode(request.mode.0).into());
        }
        // Conflicts must fail even when neither duplicate would be selected.
        for (index, element) in request.elements.iter().enumerate() {
            if element.id != NEIGHBOR_REPORT_ELEMENT_ID {
                continue;
            }
            let report = NeighborReport::parse(element.body).map_err(Error::from)?;
            for earlier in request.elements.iter().take(index) {
                if earlier.id == NEIGHBOR_REPORT_ELEMENT_ID
                    && NeighborReport::parse(earlier.body)
                        .map_err(Error::from)?
                        .bssid
                        == report.bssid
                {
                    return Err(Error::ConflictingCandidates.into());
                }
            }
        }
        Ok(())
    }
    fn preference(self, bssid: MacAddress, now: Instant) -> Option<Option<u8>> {
        let Self::Btm {
            request, timing, ..
        } = self
        else {
            return Some(None);
        };
        let abridged = request.mode.contains(BtmRequestMode::ABRIDGED);
        if now >= timing.candidates_valid_until {
            return if abridged { None } else { Some(None) };
        }
        let report = request
            .elements
            .iter()
            .filter(|element| element.id == NEIGHBOR_REPORT_ELEMENT_ID)
            .map(|element| NeighborReport::parse(element.body).expect("validated request"))
            .find(|report| report.bssid == bssid);
        match report {
            Some(report) => match report.preference().expect("validated preference") {
                Some(0) => None,
                preference => Some(preference),
            },
            None if abridged => None,
            None => Some(None),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StayReason {
    NoEligibleCandidate,
    InsufficientImprovement,
    Cooldown,
    ProposalExpired,
    CandidateListExpired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanReason {
    CurrentBssUnobserved,
    AdvertisedCandidateUnobserved,
}

/// A decision to hand to the association owner, or to the BTM decision owner.
/// For BTM, use `CandidateSource::Scan` and the original `proposal` identity;
/// call `admit` only after the BTM owner emits `TransitionReady`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub id: OperationId,
    pub proposal: Option<OperationId>,
    pub target: MacAddress,
    pub channel: Channel,
    pub security: SelectedRsn,
    pub score: i64,
    pub observed: Instant,
    /// Latest admission time, bounded by observation and BTM validity.
    pub valid_until: Instant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionEvent {
    Ignored,
    Stay(StayReason),
    ScanNeeded(ScanReason),
    Confirming {
        target: MacAddress,
        earliest: Instant,
    },
    Selected(Selection),
    InProgress {
        id: OperationId,
    },
    Expired {
        id: OperationId,
    },
    Failed {
        id: OperationId,
    },
    Connected {
        id: OperationId,
        link: LinkIdentity,
    },
    Cancelled {
        id: OperationId,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationOutcome {
    Failed,
    Connected(LinkIdentity),
}

#[derive(Clone, Copy)]
struct Stable {
    target: MacAddress,
    proposal: Option<OperationId>,
    since: Instant,
    first_observation: Instant,
}
#[derive(Clone, Copy)]
struct Attempt {
    selection: Selection,
    deadline: Instant,
    admitted: bool,
}
#[derive(Clone, Copy)]
struct Failure {
    target: MacAddress,
    count: u16,
    streak: u16,
    blocked_until: Instant,
    forget_at: Instant,
}

/// Immutable SSID and security requirements for one association's roaming.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkProfile {
    pub ssid: WifiSsid,
    pub security: StaSecurityPolicy,
}

/// One association's candidate policy and terminal association feedback.
/// Pending decisions are emitted once. Failure storage has explicit capacity
/// and never evicts a live record to admit another failed target.
// CAPABILITY: wifi-roaming-and-service-discovery-bss-transition-management-802-11v
pub struct RoamingSelector<const FAILURES: usize> {
    network: NetworkProfile,
    ids: Identities,
    policy: SelectionPolicy,
    stable: Option<Stable>,
    attempt: Option<Attempt>,
    cooldown_until: Instant,
    failures: [Option<Failure>; FAILURES],
    last_update: Instant,
}
impl<const FAILURES: usize> RoamingSelector<FAILURES> {
    pub fn new(
        link: LinkIdentity,
        network: NetworkProfile,
        policy: SelectionPolicy,
    ) -> Result<Self, SelectionError> {
        policy.validate()?;
        if FAILURES == 0 || !crate::valid_peer_address(link.peer) {
            return Err(SelectionError::InvalidPolicy);
        }
        Ok(Self {
            network,
            ids: Identities::new(link),
            policy,
            stable: None,
            attempt: None,
            cooldown_until: Instant::EPOCH,
            failures: [None; FAILURES],
            last_update: Instant::EPOCH,
        })
    }
    pub const fn link(&self) -> LinkIdentity {
        self.ids.link
    }
    fn time(&self, now: Instant) -> Result<(), SelectionError> {
        if now < self.last_update {
            return Err(Error::TimeBeforeOperation.into());
        }
        Ok(())
    }
    pub fn pending(&self) -> Option<Selection> {
        self.attempt.map(|attempt| attempt.selection)
    }
    pub fn blocked(&self, target: MacAddress, now: Instant) -> Result<bool, SelectionError> {
        self.time(now)?;
        Ok(self
            .failures
            .iter()
            .flatten()
            .any(|failure| failure.target == target && now < failure.blocked_until))
    }
    /// Evaluate fresh observations of the same SSID with the MAC's existing
    /// RSN selector and the caller's exact allowed channels and PHY admission.
    /// Confirmation requires a second observation; advancing time alone cannot
    /// turn a single scan result into a confirmed candidate.
    pub fn evaluate<const ENTRIES: usize, const REPORT_BYTES: usize>(
        &mut self,
        database: &NeighborDatabase<ENTRIES, REPORT_BYTES>,
        allowed_channels: &[Channel],
        trigger: SelectionTrigger<'_>,
        now: Instant,
        mut assess: impl FnMut(&Observation) -> CandidateAssessment,
    ) -> Result<SelectionEvent, SelectionError> {
        let NetworkProfile { ssid, security } = self.network;
        if database.link() != self.link() {
            return Ok(SelectionEvent::Ignored);
        }
        self.time(now)?;
        trigger.validate(self.link())?;
        if let Some(attempt) = self.attempt {
            if now >= attempt.deadline {
                return self.poll(now);
            }
            self.last_update = now;
            return Ok(SelectionEvent::InProgress {
                id: attempt.selection.id,
            });
        }
        if let SelectionTrigger::Btm {
            request, timing, ..
        } = trigger
        {
            if now >= timing.decision_deadline {
                return Ok(self.stay(now, StayReason::ProposalExpired));
            }
            if request.mode.contains(BtmRequestMode::ABRIDGED)
                && now >= timing.candidates_valid_until
            {
                return Ok(self.stay(now, StayReason::CandidateListExpired));
            }
        }
        let current = database.get(self.link().peer, now)?;
        let Some(current) = current.and_then(|entry| entry.observation) else {
            self.stable = None;
            self.last_update = now;
            return Ok(SelectionEvent::ScanNeeded(ScanReason::CurrentBssUnobserved));
        };
        let record = current.observation.record();
        if record.ssid_bytes() != ssid.as_bytes() || record.rssi == i8::MIN || record.rssi > 0 {
            self.stable = None;
            self.last_update = now;
            return Ok(SelectionEvent::ScanNeeded(ScanReason::CurrentBssUnobserved));
        }
        let current_score = self.score(current.observation, assess(current.observation), None);
        let mut best: Option<Selection> = None;
        let mut unobserved = false;
        for entry in database.entries(now)? {
            if entry.bssid == self.link().peer || self.blocked(entry.bssid, now)? {
                continue;
            }
            let Some(mut preference) = trigger.preference(entry.bssid, now) else {
                continue;
            };
            if matches!(trigger, SelectionTrigger::Autonomous)
                && let Some(report) = &entry.report
                && report.context.ssid == Some(ssid)
                && !matches!(report.context.source, ReportSource::BssTransition { .. })
            {
                preference = report.report.preference().map_err(Error::from)?;
                if preference == Some(0) {
                    continue;
                }
            }
            let Some(observed) = entry.observation else {
                unobserved |= entry.report.is_some_and(|report| {
                    report.context.ssid.is_none_or(|reported| reported == ssid)
                });
                continue;
            };
            let observation = observed.observation;
            let record = observation.record();
            if record.ssid_bytes() != ssid.as_bytes()
                || !allowed_channels.contains(&observation.channel())
                || record.rssi
                    < self
                        .policy
                        .band(observation.channel().band())
                        .minimum_rssi_dbm
                || record.rssi > 0
            {
                continue;
            }
            let Ok(selected_security) = select_association_rsn(record, security) else {
                continue;
            };
            let assessment = assess(observation);
            if !assessment.phy_supported {
                continue;
            }
            let mut valid_until = observed.expires.min(current.expires);
            if let SelectionTrigger::Btm { timing, .. } = trigger {
                valid_until = valid_until.min(timing.decision_deadline);
                if now < timing.candidates_valid_until {
                    valid_until = valid_until.min(timing.candidates_valid_until);
                }
            }
            let candidate = Selection {
                id: OperationId {
                    link: self.link(),
                    serial: 0,
                },
                proposal: trigger.proposal(),
                target: entry.bssid,
                channel: observation.channel(),
                security: selected_security,
                score: self.score(observation, assessment, preference),
                observed: observed.observed,
                valid_until,
            };
            if best.is_none_or(|best| {
                candidate.score > best.score
                    || (candidate.score == best.score && candidate.target < best.target)
            }) {
                best = Some(candidate);
            }
        }
        let Some(mut best) = best else {
            if unobserved {
                self.stable = None;
                self.last_update = now;
                return Ok(SelectionEvent::ScanNeeded(
                    ScanReason::AdvertisedCandidateUnobserved,
                ));
            }
            return Ok(self.stay(now, StayReason::NoEligibleCandidate));
        };
        let imminent = matches!(trigger,
            SelectionTrigger::Btm { request, .. }
            if self.policy.imminent_btm_bypasses_improvement
                && (request.mode.contains(BtmRequestMode::DISASSOCIATION_IMMINENT)
                    || request.mode.contains(BtmRequestMode::ESS_DISASSOCIATION_IMMINENT))
        );
        if !imminent
            && (best.score <= current_score
                || best.score - current_score < i64::from(self.policy.minimum_score_improvement))
        {
            return Ok(self.stay(now, StayReason::InsufficientImprovement));
        }
        if now < self.cooldown_until && !imminent {
            return Ok(self.stay(now, StayReason::Cooldown));
        }
        let stable = self
            .stable
            .filter(|stable| stable.target == best.target && stable.proposal == best.proposal)
            .unwrap_or(Stable {
                target: best.target,
                proposal: best.proposal,
                since: now,
                first_observation: best.observed,
            });
        let earliest = add(stable.since, self.policy.confirmation_time)?;
        if now < earliest
            || (self.policy.confirmation_time != Duration::ZERO
                && best.observed <= stable.first_observation)
        {
            self.stable = Some(stable);
            self.last_update = now;
            return Ok(SelectionEvent::Confirming {
                target: best.target,
                earliest,
            });
        }
        best.id = self.ids.issue()?;
        self.attempt = Some(Attempt {
            selection: best,
            deadline: best.valid_until,
            admitted: false,
        });
        self.stable = None;
        self.last_update = now;
        Ok(SelectionEvent::Selected(best))
    }
    fn stay(&mut self, now: Instant, reason: StayReason) -> SelectionEvent {
        self.stable = None;
        self.last_update = now;
        SelectionEvent::Stay(reason)
    }
    fn score(
        &self,
        observation: &Observation,
        assessment: CandidateAssessment,
        preference: Option<u8>,
    ) -> i64 {
        let weights = self.policy.weights;
        let rssi = observation.record().rssi.min(
            self.policy
                .band(observation.channel().band())
                .sufficient_rssi_dbm,
        );
        let mut score = (i64::from(rssi) - i64::from(MINIMUM_RSSI_DBM)) * i64::from(weights.rssi);
        if let Some(load) = observation.load() {
            score += i64::from(u8::MAX - load.channel_utilization)
                * i64::from(weights.available_channel)
                / i64::from(u8::MAX);
        }
        score += i64::from(preference.unwrap_or(0)) * i64::from(weights.ap_preference)
            / i64::from(u8::MAX);
        if let Some(throughput) = assessment.estimated_throughput_kbps {
            score += i64::from(throughput.min(weights.throughput_limit_kbps))
                * i64::from(weights.throughput)
                / i64::from(weights.throughput_limit_kbps);
        }
        score
    }
    /// The caller has accepted the decision for an actual association attempt.
    pub fn admit(&mut self, id: OperationId, now: Instant) -> Result<(), SelectionError> {
        self.time(now)?;
        let attempt = self.attempt.as_mut().ok_or(Error::NoPendingOperation)?;
        if attempt.selection.id != id {
            return Err(Error::WrongOperation.into());
        }
        if attempt.admitted {
            return Err(Error::AlreadyAdmitted.into());
        }
        if now >= attempt.deadline {
            return Err(Error::CandidateExpired.into());
        }
        let until = add(now, self.policy.attempt_timeout)?;
        attempt.admitted = true;
        attempt.deadline = until;
        self.last_update = now;
        Ok(())
    }
    fn prepare_failure(
        &self,
        target: MacAddress,
        now: Instant,
    ) -> Result<(usize, Failure), SelectionError> {
        let existing = self.failures.iter().position(|failure| {
            failure.is_some_and(|failure| failure.target == target && now < failure.forget_at)
        });
        let index = existing
            .or_else(|| {
                self.failures
                    .iter()
                    .position(|failure| failure.is_none_or(|failure| now >= failure.forget_at))
            })
            .ok_or(SelectionError::FailureTableFull)?;
        let mut failure = existing
            .and_then(|index| self.failures[index])
            .unwrap_or(Failure {
                target,
                count: 0,
                streak: 0,
                blocked_until: now,
                forget_at: now,
            });
        failure.count = failure.count.saturating_add(1);
        if failure.count >= self.policy.failure_threshold || failure.streak > 0 {
            let multiplier = 1_u64
                .checked_shl(u32::from(failure.streak))
                .unwrap_or(u64::MAX);
            let block = self
                .policy
                .failure_block
                .as_micros()
                .saturating_mul(multiplier)
                .min(self.policy.maximum_failure_block.as_micros());
            failure.blocked_until = add(now, Duration::from_micros(block))?;
            failure.streak = failure.streak.saturating_add(1);
            failure.count = 0;
        }
        failure.forget_at = add(now, self.policy.failure_memory)?;
        Ok((index, failure))
    }
    pub fn finish(
        &mut self,
        id: OperationId,
        outcome: AssociationOutcome,
        now: Instant,
    ) -> Result<SelectionEvent, SelectionError> {
        let Some(attempt) = self.attempt.filter(|attempt| attempt.selection.id == id) else {
            return Ok(SelectionEvent::Ignored);
        };
        self.time(now)?;
        if !attempt.admitted {
            return Err(Error::NotAdmitted.into());
        }
        if now >= attempt.deadline {
            return self.poll(now);
        }
        let event = match outcome {
            AssociationOutcome::Failed => {
                let (index, failure) = self.prepare_failure(attempt.selection.target, now)?;
                self.failures[index] = Some(failure);
                SelectionEvent::Failed { id }
            }
            AssociationOutcome::Connected(link) => {
                if link.peer != attempt.selection.target
                    || link.generation == self.link().generation
                {
                    return Err(SelectionError::InvalidNewLink);
                }
                let cooldown = add(now, self.policy.cooldown)?;
                for failure in &mut self.failures {
                    if failure.is_some_and(|failure| failure.target == link.peer) {
                        *failure = None;
                    }
                }
                self.ids.link = link;
                self.cooldown_until = cooldown;
                SelectionEvent::Connected { id, link }
            }
        };
        self.attempt = None;
        self.last_update = now;
        Ok(event)
    }
    pub fn poll(&mut self, now: Instant) -> Result<SelectionEvent, SelectionError> {
        self.time(now)?;
        let Some(attempt) = self.attempt.filter(|attempt| now >= attempt.deadline) else {
            self.last_update = now;
            return Ok(SelectionEvent::Ignored);
        };
        if attempt.admitted {
            let (index, failure) = self.prepare_failure(attempt.selection.target, now)?;
            self.failures[index] = Some(failure);
        }
        self.attempt = None;
        self.stable = None;
        self.last_update = now;
        Ok(SelectionEvent::Expired {
            id: attempt.selection.id,
        })
    }
    pub fn cancel(
        &mut self,
        id: OperationId,
        now: Instant,
    ) -> Result<SelectionEvent, SelectionError> {
        if !self
            .attempt
            .is_some_and(|attempt| attempt.selection.id == id)
        {
            return Ok(SelectionEvent::Ignored);
        }
        self.time(now)?;
        self.attempt = None;
        self.stable = None;
        self.last_update = now;
        Ok(SelectionEvent::Cancelled { id })
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.attempt
            .map(|attempt| attempt.deadline)
            .into_iter()
            .chain(
                self.stable
                    .and_then(|stable| stable.since.checked_add(self.policy.confirmation_time))
                    .filter(|deadline| *deadline > self.last_update),
            )
            .chain((self.cooldown_until > self.last_update).then_some(self.cooldown_until))
            .chain(self.failures.iter().flatten().filter_map(|failure| {
                (failure.blocked_until > self.last_update).then_some(failure.blocked_until)
            }))
            .min()
    }
}

fn add(now: Instant, duration: Duration) -> Result<Instant, SelectionError> {
    now.checked_add(duration).ok_or(Error::TimeOverflow.into())
}

#[cfg(test)]
mod tests;
