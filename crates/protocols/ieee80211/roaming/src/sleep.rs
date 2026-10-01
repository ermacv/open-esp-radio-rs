//! WNM Sleep transactions and explicit application of key/filter/power services.
use oer_ieee80211_mac::management::IEEE_TIME_UNIT_MICROS;
use oer_ieee80211_mac::roaming as wire;
use oer_ieee80211_mac::roaming::element_id;
mod traffic;
use crate::{
    Body, DialogEvent, Error, Identities, LinkIdentity, OperationId, RequestEvent, Sender,
    Transmission, TxOutcome, TxPhase, deadline, exchange::Exchange,
};
use oer_ieee80211_mac::{
    roaming::*,
    security::{AssociationSecurity, BIP_CMAC_128_KEY_LEN, CCMP_128_KEY_LEN, IGTK_KEY_IDS},
};
use oer_time::{Duration, Instant};
const KEY_ID_COUNT: usize = *IGTK_KEY_IDS.end() as usize + 1;
pub use traffic::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepError {
    Protocol(Error),
    InvalidState,
    UnsupportedAction(u8),
    UnsupportedStatus(u8),
    InvalidTiming,
    IdleConflict,
    KeyDataWithoutProtection,
    MissingKeys,
    UnsupportedKey(u8),
    InvalidKey,
    ConflictingKeys,
    ServicesNotApplied,
    Traffic(TrafficError),
    RecoveryRequired,
}
impl From<Error> for SleepError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
impl From<WireError> for SleepError {
    fn from(value: WireError) -> Self {
        Self::Protocol(Error::Wire(value))
    }
}
impl From<TrafficError> for SleepError {
    fn from(value: TrafficError) -> Self {
        Self::Traffic(value)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepState {
    Awake,
    Asleep { interval_dtim: u16 },
    Unknown,
}

/// Services required before the local transition. An RSN without PMF exits via
/// the ordinary group-key handshake, never via keys in an unprotected response.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SleepServices(u8);
impl SleepServices {
    pub const RETIRE_GTK: Self = Self(1);
    pub const RETIRE_IGTK: Self = Self(2);
    pub const INSTALL_GTK: Self = Self(4);
    pub const INSTALL_IGTK: Self = Self(8);
    pub const GROUP_REKEY: Self = Self(16);
    pub const FILTERS_AND_POWER: Self = Self(32);
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SleepTiming {
    pub beacon_interval_tu: u16,
    pub dtim_period: u8,
    /// Measured next DTIM at/after request reception, on monotonic time.
    pub next_dtim: Instant,
    pub maximum_idle: Option<BssMaxIdle>,
}
impl SleepTiming {
    fn dtim_micros(self) -> u64 {
        u64::from(self.beacon_interval_tu) * u64::from(self.dtim_period) * IEEE_TIME_UNIT_MICROS
    }
    fn validate(self, sleep: WnmSleepElement, now: Instant) -> Result<(), SleepError> {
        if self.beacon_interval_tu == 0 || self.dtim_period == 0 {
            return Err(SleepError::InvalidTiming);
        }
        let period = self.dtim_micros();
        if self.next_dtim < now
            || self.next_dtim
                > now
                    .checked_add(Duration::from_micros(period))
                    .ok_or(Error::TimeOverflow)?
        {
            return Err(SleepError::InvalidTiming);
        }
        if let Some(idle) = self.maximum_idle {
            idle.validate()?;
            if sleep.action == SleepAction::ENTER
                && sleep.interval_dtim != 0
                && period * u64::from(sleep.interval_dtim)
                    >= u64::from(idle.period) * BssMaxIdle::PERIOD_UNIT_TU * IEEE_TIME_UNIT_MICROS
            {
                return Err(SleepError::IdleConflict);
            }
        }
        Ok(())
    }
    fn wake(self, interval: u16, now: Instant) -> Result<Option<(Instant, Duration)>, SleepError> {
        if interval == 0 {
            return Ok(None);
        }
        let dtim = self.dtim_micros();
        let skipped = if self.next_dtim < now {
            (now.as_micros() - self.next_dtim.as_micros()).div_ceil(dtim)
        } else {
            0
        };
        let offset = skipped
            .checked_add(u64::from(interval - 1))
            .and_then(|count| count.checked_mul(dtim))
            .ok_or(Error::TimeOverflow)?;
        let next = self
            .next_dtim
            .checked_add(Duration::from_micros(offset))
            .ok_or(Error::TimeOverflow)?;
        Ok(Some((
            next,
            Duration::from_micros(dtim * u64::from(interval)),
        )))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepEvent {
    Ignored,
    ApplyRequired {
        id: OperationId,
        services: SleepServices,
        target: SleepState,
    },
    Applied {
        id: OperationId,
        state: SleepState,
    },
    Rejected {
        id: OperationId,
        status: SleepStatus,
    },
    RecoveryRequired {
        id: OperationId,
    },
    CheckTim {
        link: LinkIdentity,
    },
    RequestCancelled {
        id: OperationId,
    },
}
struct Application {
    id: OperationId,
    target: SleepState,
    services: SleepServices,
    until: Instant,
    timing: SleepTiming,
}

/// Accepted responses await service application. Uncertain delivery/timeouts
/// produce Unknown, and an explicit exit can resynchronize that state.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-wnm-sleep-mode-802-11v
pub struct WnmSleepStation<const BYTES: usize> {
    exchange: Exchange<BYTES>,
    security: AssociationSecurity,
    state: SleepState,
    timing: Option<SleepTiming>,
    application: Option<Application>,
    wake: Option<(Instant, Duration)>,
    last_update: Instant,
}
impl<const BYTES: usize> WnmSleepStation<BYTES> {
    pub fn new(
        link: LinkIdentity,
        security: AssociationSecurity,
        timeout: Duration,
    ) -> Result<Self, SleepError> {
        Ok(Self {
            exchange: Exchange::new(link, timeout)?,
            security,
            state: SleepState::Awake,
            timing: None,
            application: None,
            wake: None,
            last_update: Instant::EPOCH,
        })
    }
    pub const fn state(&self) -> SleepState {
        self.state
    }
    fn time(&self, now: Instant) -> Result<(), SleepError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub fn request(
        &mut self,
        request: WnmSleepRequest<'_>,
        timing: SleepTiming,
        now: Instant,
    ) -> Result<OperationId, SleepError> {
        self.time(now)?;
        if self.application.is_some() {
            return Err(Error::Busy.into());
        }
        let sleep = request.sleep()?;
        if sleep.action == SleepAction::ENTER
            && !request
                .elements
                .iter()
                .any(|element| element.id == element_id::TFS_REQUEST)
        {
            return Err(TrafficError::NoAcceptedFilters.into());
        }
        match sleep.action {
            SleepAction::ENTER if self.state != SleepState::Awake => {
                return Err(SleepError::InvalidState);
            }
            SleepAction::EXIT if self.state == SleepState::Awake => {
                return Err(SleepError::InvalidState);
            }
            SleepAction::ENTER | SleepAction::EXIT => {}
            SleepAction(other) => return Err(SleepError::UnsupportedAction(other)),
        }
        if sleep.status != SleepStatus::ACCEPT {
            return Err(SleepError::UnsupportedStatus(sleep.status.0));
        }
        timing.validate(sleep, now)?;
        let body = Body::encode(|out| request.encode(out))?;
        let id = self.exchange.start(body, now)?;
        self.timing = Some(timing);
        self.last_update = now;
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.exchange.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), SleepError> {
        self.time(now)?;
        self.exchange.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    fn terminal(&mut self, event: RequestEvent, admitted: bool) -> SleepEvent {
        match event {
            RequestEvent::TimedOut { id }
            | RequestEvent::TxFailed { id }
            | RequestEvent::Cancelled { id } => {
                self.timing = None;
                if admitted {
                    self.state = SleepState::Unknown;
                    self.wake = None;
                    SleepEvent::RecoveryRequired { id }
                } else {
                    SleepEvent::RequestCancelled { id }
                }
            }
            _ => SleepEvent::Ignored,
        }
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<SleepEvent, SleepError> {
        if self
            .exchange
            .pending
            .as_ref()
            .is_none_or(|pending| pending.id != id)
        {
            return Ok(SleepEvent::Ignored);
        }
        self.time(now)?;
        let admitted = self
            .exchange
            .pending
            .as_ref()
            .is_some_and(|pending| pending.phase != TxPhase::Ready);
        let event = self.exchange.completed(id, outcome, now)?;
        self.last_update = now;
        Ok(self.terminal(event, admitted))
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<SleepEvent, SleepError> {
        if link != self.exchange.ids.link || self.exchange.pending.is_none() {
            return Ok(SleepEvent::Ignored);
        }
        self.time(now)?;
        let response = WnmSleepResponse::parse(bytes)?;
        if !self.exchange.matches(link, response.dialog_token, now)? {
            return Ok(SleepEvent::Ignored);
        }
        let pending = self.exchange.pending.as_ref().expect("matching request");
        let requested = WnmSleepRequest::parse(pending.body.bytes())?.sleep()?;
        let sleep = response.sleep()?;
        if sleep.action != requested.action
            || (sleep.action == SleepAction::ENTER
                && sleep.interval_dtim != requested.interval_dtim)
        {
            return Err(Error::ConflictingDialog.into());
        }
        validate_response(sleep, response.keys, self.security)?;
        if sleep.status.accepted() && sleep.action == SleepAction::ENTER {
            validate_sleep_filters(
                WnmSleepRequest::parse(pending.body.bytes())?.elements,
                response.elements,
            )?;
        }
        let id = pending.id;
        let until = pending.deadline;
        let services = requirements(sleep, self.security);
        let target = if sleep.action == SleepAction::ENTER {
            SleepState::Asleep {
                interval_dtim: sleep.interval_dtim,
            }
        } else {
            SleepState::Awake
        };
        let timing = self.timing.expect("request timing");
        if sleep.status.accepted() && sleep.action == SleepAction::ENTER {
            timing.wake(sleep.interval_dtim, now)?;
        }
        self.exchange.accept(bytes)?;
        self.last_update = now;
        if !sleep.status.accepted() {
            self.timing = None;
            return Ok(SleepEvent::Rejected {
                id,
                status: sleep.status,
            });
        }
        self.application = Some(Application {
            id,
            target,
            services,
            until,
            timing,
        });
        Ok(SleepEvent::ApplyRequired {
            id,
            services,
            target,
        })
    }
    /// Pass these retained keys to the existing security owner, preserving its
    /// nonce/replay and anti-reinstall rules. Duplicate responses cannot apply twice.
    pub fn application(&self) -> Option<(OperationId, SleepServices, WnmSleepResponse<'_>)> {
        let application = self.application.as_ref()?;
        Some((
            application.id,
            application.services,
            WnmSleepResponse::parse(self.exchange.response()?).expect("validated response"),
        ))
    }
    pub fn services_applied(
        &mut self,
        id: OperationId,
        applied: SleepServices,
        now: Instant,
    ) -> Result<SleepEvent, SleepError> {
        let Some(application) = self
            .application
            .as_ref()
            .filter(|application| application.id == id)
        else {
            return Ok(SleepEvent::Ignored);
        };
        self.time(now)?;
        if now >= application.until {
            return self.poll(now);
        }
        if !applied.contains(application.services) {
            return Err(SleepError::ServicesNotApplied);
        }
        let wake = match application.target {
            SleepState::Asleep { interval_dtim } => application.timing.wake(interval_dtim, now)?,
            _ => None,
        };
        self.state = application.target;
        self.wake = wake;
        self.application = None;
        self.exchange.clear_response();
        self.timing = None;
        self.last_update = now;
        Ok(SleepEvent::Applied {
            id,
            state: self.state,
        })
    }
    pub fn poll(&mut self, now: Instant) -> Result<SleepEvent, SleepError> {
        self.time(now)?;
        if let Some(application) = &self.application
            && now >= application.until
        {
            let id = application.id;
            self.application = None;
            self.exchange.clear_response();
            self.timing = None;
            self.state = SleepState::Unknown;
            self.wake = None;
            self.last_update = now;
            return Ok(SleepEvent::RecoveryRequired { id });
        }
        let admitted = self
            .exchange
            .pending
            .as_ref()
            .is_some_and(|pending| pending.phase != TxPhase::Ready);
        let event = self.exchange.poll(now)?;
        if event != RequestEvent::Ignored {
            self.last_update = now;
            return Ok(self.terminal(event, admitted));
        }
        if let Some((wake, period)) = self.wake
            && now >= wake
        {
            let count = (now.as_micros() - wake.as_micros()) / period.as_micros() + 1;
            let elapsed = period
                .as_micros()
                .checked_mul(count)
                .ok_or(Error::TimeOverflow)?;
            let next = wake
                .checked_add(Duration::from_micros(elapsed))
                .ok_or(Error::TimeOverflow)?;
            self.wake = Some((next, period));
            self.last_update = now;
            return Ok(SleepEvent::CheckTim {
                link: self.exchange.ids.link,
            });
        }
        self.last_update = now;
        Ok(SleepEvent::Ignored)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.exchange
            .next_deadline()
            .into_iter()
            .chain(
                self.application
                    .as_ref()
                    .map(|application| application.until),
            )
            .chain(self.wake.map(|(wake, _)| wake))
            .min()
    }
    pub fn cancel(&mut self, now: Instant) -> Result<SleepEvent, SleepError> {
        self.time(now)?;
        if let Some(application) = self.application.take() {
            self.exchange.clear_response();
            self.state = SleepState::Unknown;
            self.wake = None;
            self.timing = None;
            self.last_update = now;
            return Ok(SleepEvent::RecoveryRequired { id: application.id });
        }
        let admitted = self
            .exchange
            .pending
            .as_ref()
            .is_some_and(|pending| pending.phase != TxPhase::Ready);
        let event = self.exchange.cancel();
        self.last_update = now;
        Ok(self.terminal(event, admitted))
    }
}

fn validate_response(
    sleep: WnmSleepElement,
    keys: SleepKeyData<'_>,
    security: AssociationSecurity,
) -> Result<(), SleepError> {
    if sleep.action != SleepAction::ENTER && sleep.action != SleepAction::EXIT {
        return Err(SleepError::UnsupportedAction(sleep.action.0));
    }
    if !sleep.status.is_known()
        || (sleep.status == SleepStatus::ACCEPT_KEY_UPDATE && sleep.action != SleepAction::EXIT)
    {
        return Err(SleepError::UnsupportedStatus(sleep.status.0));
    }
    if !keys.as_bytes().is_empty() && !security.protects_management() {
        return Err(SleepError::KeyDataWithoutProtection);
    }
    if !keys.as_bytes().is_empty()
        && (!sleep.status.accepted() || sleep.action != SleepAction::EXIT)
    {
        return Err(SleepError::InvalidKey);
    }
    let mut gtk = false;
    let mut igtk = false;
    let mut used = [false; KEY_ID_COUNT];
    for key in keys.keys() {
        let id = match key {
            SleepKey::Gtk { key_info, key, .. } => {
                if key.len() != CCMP_128_KEY_LEN || key_info & !GTK_KEY_INFO_MASK != 0 {
                    return Err(SleepError::InvalidKey);
                }
                gtk = true;
                usize::from(key_info & GTK_KEY_ID_MASK)
            }
            SleepKey::Integrity {
                kind: sleep_key_id::IGTK,
                key_id,
                key,
                ..
            } => {
                if !IGTK_KEY_IDS.contains(&key_id) || key.len() != BIP_CMAC_128_KEY_LEN {
                    return Err(SleepError::InvalidKey);
                }
                igtk = true;
                usize::from(key_id)
            }
            SleepKey::Integrity { kind, .. } | SleepKey::Other { kind, .. } => {
                return Err(SleepError::UnsupportedKey(kind));
            }
        };
        if core::mem::replace(&mut used[id], true) {
            return Err(SleepError::ConflictingKeys);
        }
    }
    if sleep.action == SleepAction::EXIT
        && sleep.status.accepted()
        && security.protects_management()
        && (!gtk || !igtk)
    {
        return Err(SleepError::MissingKeys);
    }
    if sleep.status == SleepStatus::ACCEPT_KEY_UPDATE
        && matches!(security, AssociationSecurity::Open)
    {
        return Err(SleepError::InvalidKey);
    }
    Ok(())
}
fn requirements(sleep: WnmSleepElement, security: AssociationSecurity) -> SleepServices {
    let mut services = SleepServices::FILTERS_AND_POWER;
    if matches!(security, AssociationSecurity::Open) {
        return services;
    }
    if sleep.action == SleepAction::ENTER {
        services = services.union(SleepServices::RETIRE_GTK);
        if security.protects_management() {
            services = services.union(SleepServices::RETIRE_IGTK);
        }
    } else if security.protects_management() {
        services = services
            .union(SleepServices::INSTALL_GTK)
            .union(SleepServices::INSTALL_IGTK);
    } else {
        services = services.union(SleepServices::GROUP_REKEY);
    }
    services
}

struct ApRequest<const BYTES: usize> {
    id: OperationId,
    body: Body<BYTES>,
    until: Instant,
    applying: bool,
    changes_services: bool,
}
/// Fully validated, owned AP response. Inspect `target` and apply AP filter,
/// queue and key-handshake services before returning this receipt to its owner.
pub struct SleepResponsePlan<const BYTES: usize> {
    id: OperationId,
    body: Body<BYTES>,
    target: Option<SleepState>,
    security: AssociationSecurity,
}
impl<const B: usize> SleepResponsePlan<B> {
    pub const fn id(&self) -> OperationId {
        self.id
    }
    pub const fn target(&self) -> Option<SleepState> {
        self.target
    }
    pub fn response(&self) -> WnmSleepResponse<'_> {
        WnmSleepResponse::parse(self.body.bytes()).expect("prepared response")
    }
}
struct ApHistory<const BYTES: usize> {
    request: Body<BYTES>,
    response: Body<BYTES>,
    until: Instant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepApEvent {
    Ignored,
    Requested { id: OperationId },
    ReplayQueued { id: OperationId },
    ResponseSent { id: OperationId, state: SleepState },
    TxFailed { id: OperationId },
    TimedOut { id: OperationId },
    RecoveryRequired { id: OperationId },
}

/// AP state changes when its caller applies services, before sending acceptance.
/// Retransmission never reapplies services or extends the original dialog lease.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-wnm-sleep-mode-802-11v
pub struct WnmSleepAccessPoint<const BYTES: usize> {
    ids: Identities,
    timeout: Duration,
    security: AssociationSecurity,
    state: SleepState,
    request: Option<ApRequest<BYTES>>,
    history: Option<ApHistory<BYTES>>,
    sender: Sender<BYTES>,
    last_update: Instant,
}
impl<const BYTES: usize> WnmSleepAccessPoint<BYTES> {
    pub fn new(
        link: LinkIdentity,
        security: AssociationSecurity,
        timeout: Duration,
    ) -> Result<Self, SleepError> {
        deadline(Instant::EPOCH, timeout)?;
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            security,
            state: SleepState::Awake,
            request: None,
            history: None,
            sender: Sender::new(link, timeout)?,
            last_update: Instant::EPOCH,
        })
    }
    pub const fn state(&self) -> SleepState {
        self.state
    }
    fn time(&self, now: Instant) -> Result<(), SleepError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<SleepApEvent, SleepError> {
        if link != self.ids.link {
            return Ok(SleepApEvent::Ignored);
        }
        self.time(now)?;
        let request = WnmSleepRequest::parse(bytes)?;
        let sleep = request.sleep()?;
        if sleep.status != SleepStatus::ACCEPT {
            return Err(SleepError::UnsupportedStatus(sleep.status.0));
        }
        if sleep.action == SleepAction::ENTER
            && !request
                .elements
                .iter()
                .any(|element| element.id == element_id::TFS_REQUEST)
        {
            return Err(TrafficError::NoAcceptedFilters.into());
        }
        if sleep.action != SleepAction::ENTER && sleep.action != SleepAction::EXIT {
            return Err(SleepError::UnsupportedAction(sleep.action.0));
        }
        if let Some(request) = &self.request
            && request.applying
        {
            return if request.body.bytes() == bytes {
                Ok(SleepApEvent::Ignored)
            } else {
                Err(Error::Busy.into())
            };
        }
        if let Some(history) = &self.history
            && now < history.until
            && history.request.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token
        {
            if history.request.bytes() != bytes {
                return Err(Error::ConflictingDialog.into());
            }
            if self.sender.pending.is_some() {
                return Ok(SleepApEvent::Ignored);
            }
            let id = self.sender.start_using_until(
                now,
                history.response.clone(),
                &mut self.ids,
                history.until,
            )?;
            self.last_update = now;
            return Ok(SleepApEvent::ReplayQueued { id });
        }
        if let Some(active) = &self.request
            && now < active.until
        {
            if active.body.bytes() == bytes {
                return Ok(SleepApEvent::Ignored);
            }
            return Err(
                if active.body.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token {
                    Error::ConflictingDialog
                } else {
                    Error::Busy
                }
                .into(),
            );
        }
        if self.sender.pending.is_some() {
            return Err(Error::Busy.into());
        }
        let body = Body::copy(bytes)?;
        let until = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.request = Some(ApRequest {
            id,
            body,
            until,
            applying: false,
            changes_services: false,
        });
        self.last_update = now;
        Ok(SleepApEvent::Requested { id })
    }
    pub fn request(&self) -> Option<(OperationId, WnmSleepRequest<'_>)> {
        self.request.as_ref().map(|request| {
            (
                request.id,
                WnmSleepRequest::parse(request.body.bytes()).expect("validated request"),
            )
        })
    }
    /// Validate timing, security, TFS coverage and complete output capacity
    /// before the caller changes AP services. Only one live plan can exist.
    pub fn prepare_response(
        &mut self,
        id: OperationId,
        response: WnmSleepResponse<'_>,
        timing: SleepTiming,
        now: Instant,
    ) -> Result<SleepResponsePlan<BYTES>, SleepError> {
        self.time(now)?;
        let request = self.request.as_ref().ok_or(Error::NoPendingOperation)?;
        if request.id != id {
            return Err(Error::WrongOperation.into());
        }
        if now >= request.until {
            return Err(Error::NoPendingOperation.into());
        }
        if request.applying || self.sender.pending.is_some() {
            return Err(Error::Busy.into());
        }
        self.ids
            .next
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        let source = WnmSleepRequest::parse(request.body.bytes())?;
        let requested = source.sleep()?;
        let sleep = response.sleep()?;
        if source.dialog_token != response.dialog_token
            || requested.action != sleep.action
            || (sleep.action == SleepAction::ENTER
                && sleep.interval_dtim != requested.interval_dtim)
        {
            return Err(Error::ConflictingDialog.into());
        }
        validate_response(sleep, response.keys, self.security)?;
        if sleep.status.accepted() && sleep.action == SleepAction::ENTER {
            validate_sleep_filters(source.elements, response.elements)?;
            timing.validate(requested, now)?;
        }
        let body = Body::encode(|out| response.encode(out))?;
        let target = if sleep.status.accepted() {
            Some(if sleep.action == SleepAction::ENTER {
                SleepState::Asleep {
                    interval_dtim: sleep.interval_dtim,
                }
            } else {
                SleepState::Awake
            })
        } else {
            None
        };
        self.request.as_mut().expect("live request").applying = true;
        self.request
            .as_mut()
            .expect("live request")
            .changes_services = target.is_some();
        self.last_update = now;
        Ok(SleepResponsePlan {
            id,
            body,
            target,
            security: self.security,
        })
    }
    /// Returning the owned plan confirms AP services have been applied for its
    /// accepted target. Rejections require no service changes. If publication
    /// becomes impossible after application, state becomes Unknown and requires
    /// recovery; expiry cannot roll back already applied services.
    pub fn services_applied(
        &mut self,
        plan: SleepResponsePlan<BYTES>,
        now: Instant,
    ) -> Result<OperationId, SleepError> {
        if plan.id.link != self.ids.link || plan.security != self.security {
            return Err(Error::WrongOperation.into());
        }
        if plan.target.is_some() {
            self.state = SleepState::Unknown;
        }
        self.time(now)?;
        let request = self
            .request
            .as_ref()
            .filter(|request| request.id == plan.id && request.applying)
            .ok_or(SleepError::RecoveryRequired)?;
        if now >= request.until {
            self.request = None;
            self.last_update = now;
            return Err(SleepError::RecoveryRequired);
        }
        let original = request.body.clone();
        let until = request.until;
        let tx = self
            .sender
            .start_using_until(now, plan.body.clone(), &mut self.ids, until)?;
        if let Some(target) = plan.target {
            self.state = target;
        }
        self.history = Some(ApHistory {
            request: original,
            response: plan.body,
            until,
        });
        self.request = None;
        self.last_update = now;
        Ok(tx)
    }
    /// Return a prepared plan before changing services. This releases the
    /// reservation without inventing an uncertain AP state.
    pub fn cancel_response(
        &mut self,
        plan: SleepResponsePlan<BYTES>,
        now: Instant,
    ) -> Result<SleepApEvent, SleepError> {
        self.time(now)?;
        if plan.id.link != self.ids.link || plan.security != self.security {
            return Err(Error::WrongOperation.into());
        }
        let request = self
            .request
            .as_mut()
            .filter(|request| request.id == plan.id && request.applying)
            .ok_or(Error::WrongOperation)?;
        request.applying = false;
        request.changes_services = false;
        self.last_update = now;
        self.poll(now)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.sender.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), SleepError> {
        self.time(now)?;
        self.sender.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<SleepApEvent, SleepError> {
        if self
            .sender
            .pending
            .as_ref()
            .is_none_or(|pending| pending.id != id)
        {
            return Ok(SleepApEvent::Ignored);
        }
        self.time(now)?;
        let event = self.sender.completed(id, outcome, now)?;
        self.last_update = now;
        Ok(match event {
            DialogEvent::Transmitted => SleepApEvent::ResponseSent {
                id,
                state: self.state,
            },
            DialogEvent::TxFailed => SleepApEvent::TxFailed { id },
            DialogEvent::TimedOut => SleepApEvent::TimedOut { id },
            _ => SleepApEvent::Ignored,
        })
    }
    pub fn poll(&mut self, now: Instant) -> Result<SleepApEvent, SleepError> {
        self.time(now)?;
        if let Some(request) = &self.request
            && now >= request.until
        {
            let id = request.id;
            let applying = request.changes_services;
            self.request = None;
            self.last_update = now;
            return Ok(if applying {
                self.state = SleepState::Unknown;
                SleepApEvent::RecoveryRequired { id }
            } else {
                SleepApEvent::TimedOut { id }
            });
        }
        if let Some(pending) = &self.sender.pending
            && now >= pending.deadline
        {
            let id = pending.id;
            self.sender.poll(now)?;
            self.last_update = now;
            return Ok(SleepApEvent::TimedOut { id });
        }
        if self
            .history
            .as_ref()
            .is_some_and(|history| now >= history.until)
        {
            self.history = None;
        }
        self.last_update = now;
        Ok(SleepApEvent::Ignored)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.request
            .as_ref()
            .map(|request| request.until)
            .into_iter()
            .chain(self.sender.next_deadline())
            .chain(self.history.as_ref().map(|history| history.until))
            .min()
    }
}

#[cfg(test)]
mod tests;
