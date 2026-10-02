//! Station DH/group selection and exact association-to-RSN handoff.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationPhase {
    NeedsKey(Group),
    RequestReady,
    AwaitingResponse,
    Completed,
    Failed,
}

/// One attempt at one BSS. All retries retain one scalar/public transcript;
/// status 77 explicitly advances the configured group preference and asks the
/// caller for a fresh key. Invalid peer keys terminate and wipe the attempt.
pub struct Station<'a, const N: usize> {
    exchange: Exchange,
    groups: GroupPolicy<'a>,
    advertisement: SecurityProfile<'a>,
    selected: SecurityProfile<'a>,
    scope: CacheScope,
    phase: StationPhase,
    key: Option<KeyPair>,
    cache: Option<CachedPmk>,
    request: Option<Packet<N>>,
}
impl<'a, const N: usize> Station<'a, N> {
    pub fn new(
        context: AssociationContext<'a>,
        selected: SecurityProfile<'a>,
        groups: GroupPolicy<'a>,
        cached: Option<CachedPmk>,
        now_us: u64,
    ) -> Result<Self, Error> {
        let AssociationContext {
            id,
            ssid,
            advertisement,
            retry,
        } = context;
        let exchange = Exchange::new(id, crate::RsnInterface::Station, retry, now_us)?;
        let scope = CacheScope {
            addresses: id.addresses,
            ssid,
            management_protection: advertisement.negotiate(selected)?,
        };
        let rsn = RsnElement::parse(selected.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
        if let Some(cache) = &cached {
            cache.validate_at(now_us)?;
            if cache.scope() != scope
                || !groups.contains(cache.group())
                || rsn.pmkid_count() != Some(1)
                || rsn.pmkids().next() != Some(cache.pmkid())
            {
                return Err(Error::WrongContext);
            }
        } else if rsn.pmkid_count().unwrap_or(0) != 0 {
            return Err(Error::WrongContext);
        }
        Ok(Self {
            exchange,
            groups,
            advertisement,
            selected,
            scope,
            phase: StationPhase::NeedsKey(groups.first()),
            key: None,
            cache: cached,
            request: None,
        })
    }
    pub const fn phase(&self) -> StationPhase {
        self.phase
    }
    /// Next monotonic wakeup; the caller drives `tick` and owns the clock.
    pub fn next_deadline_us(&self) -> Option<u64> {
        (!matches!(self.phase, StationPhase::Completed | StationPhase::Failed)).then(|| {
            self.exchange
                .next_deadline(self.phase == StationPhase::AwaitingResponse)
        })
    }
    fn fail(&mut self) {
        self.key = None;
        self.cache = None;
        self.request = None;
        self.phase = StationPhase::Failed;
    }
    fn observe(&mut self, now: u64) -> Result<(), Error> {
        if matches!(self.phase, StationPhase::Completed | StationPhase::Failed) {
            return Err(Error::WrongPhase);
        }
        let result = self.exchange.observe(now);
        if result == Err(Error::TimedOut) {
            self.fail();
        }
        result
    }
    pub fn provide_key(&mut self, key: KeyPair, now_us: u64) -> Result<(), Error> {
        self.observe(now_us)?;
        if self.phase != StationPhase::NeedsKey(key.group()) {
            return Err(Error::WrongPhase);
        }
        let packet = Packet::request(self.selected, key.parameter())?;
        self.key = Some(key);
        self.request = Some(packet);
        self.phase = StationPhase::RequestReady;
        Ok(())
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        if matches!(self.phase, StationPhase::Completed | StationPhase::Failed) {
            return None;
        }
        self.exchange.transmission(self.request.as_ref()?.bytes())
    }
    pub fn admitted(&mut self, ticket: TxTicket, now_us: u64) -> Result<(), Error> {
        self.observe(now_us)?;
        if !matches!(
            self.phase,
            StationPhase::RequestReady | StationPhase::AwaitingResponse
        ) {
            return Err(Error::WrongPhase);
        }
        self.exchange.admitted(ticket, now_us)?;
        self.phase = StationPhase::AwaitingResponse;
        Ok(())
    }
    pub fn tx_completed(&mut self, ticket: TxTicket, now_us: u64) -> Result<(), Error> {
        self.observe(now_us)?;
        if self.phase != StationPhase::AwaitingResponse {
            return Err(Error::WrongPhase);
        }
        // An ACK is not an Association Response: both outcomes await/retry it.
        self.exchange.completed(ticket, now_us)
    }
    pub fn tick(&mut self, now_us: u64) -> Result<bool, Error> {
        if matches!(self.phase, StationPhase::Completed | StationPhase::Failed) {
            return Ok(false);
        }
        self.observe(now_us)?;
        if self.phase != StationPhase::AwaitingResponse {
            return Ok(false);
        }
        self.exchange.retry(false, now_us)
    }
    /// Context is captured by MLME when dispatching the frame, not read from
    /// untrusted IE data. `Ok(None)` with NeedsKey reports group-77 negotiation;
    /// other peer rejection codes are returned without an implicit fallback.
    pub fn response(
        &mut self,
        request: TxTicket,
        status: u16,
        elements: Elements<'_>,
        now_us: u64,
    ) -> Result<Option<InitialAssociation<'a>>, Error> {
        self.observe(now_us)?;
        if self.phase != StationPhase::AwaitingResponse {
            return Err(Error::WrongPhase);
        }
        self.exchange.accepts_response(request)?;
        if status == STATUS_UNSUPPORTED_FINITE_CYCLIC_GROUP {
            let group = self.key.as_ref().ok_or(Error::WrongPhase)?.group();
            let next = self.groups.next(group).ok_or(Error::UnsupportedSecurity);
            let retry = next.and_then(|next| self.exchange.retry(true, now_us).map(|_| next));
            return match retry {
                Ok(next) => {
                    self.key = None;
                    self.request = None;
                    self.exchange.new_transcript();
                    self.phase = StationPhase::NeedsKey(next);
                    Ok(None)
                }
                Err(error) => {
                    self.fail();
                    Err(error)
                }
            };
        }
        if status != STATUS_SUCCESS {
            self.fail();
            return Err(Error::AssociationRejected(status));
        }
        let result = self.finish(elements, now_us);
        match result {
            Ok(value) => {
                self.key = None;
                self.cache = None;
                self.request = None;
                self.phase = StationPhase::Completed;
                Ok(Some(value))
            }
            Err(error) => {
                self.fail();
                Err(error)
            }
        }
    }
    fn finish(
        &mut self,
        elements: Elements<'_>,
        now: u64,
    ) -> Result<InitialAssociation<'a>, Error> {
        let response = SecurityProfile::from_elements(elements)?;
        if response.negotiate(self.selected)? != self.scope.management_protection {
            return Err(Error::SecurityMismatch);
        }
        let rsn = RsnElement::parse(response.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
        if let Some(cache) = &self.cache
            && rsn.pmkid_count() == Some(1)
            && rsn.pmkids().next() == Some(cache.pmkid())
        {
            return InitialAssociation::resume(
                self.cache.take().ok_or(Error::WrongPhase)?,
                self.scope,
                self.advertisement,
                self.selected,
                elements,
                now,
            );
        }
        // Unsolicited/mismatching PMKID does not select cache resumption.
        let dh = DhParameter::from_elements(elements)
            .map_err(Error::Wire)?
            .ok_or(Error::InvalidKey)?;
        let key = self.key.take().ok_or(Error::WrongPhase)?.finish(
            crate::RsnInterface::Station,
            self.scope.addresses,
            dh,
        )?;
        InitialAssociation::new(key, self.scope.ssid, self.advertisement, self.selected)
    }
    pub fn cancel(&mut self) {
        self.fail();
    }
}
