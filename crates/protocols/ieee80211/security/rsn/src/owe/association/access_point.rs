//! Per-peer AP admission, retained response and one-time RSN handoff.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointPhase {
    NeedsKey(Group),
    Responding,
    Established,
    /// Reply retention ended after the PMK was handed to the RSN owner.
    /// This does not terminate that owner's association or installed keys.
    Retired,
    Failed,
}

/// One admitted request, bounded by the caller's peer table. Construction
/// validates OWE policy/group before asking for entropy or doing ECC. Retries
/// are correlated by the selected security and DH transcript, not MAC sequence
/// numbers; duplicate requests never regenerate a scalar or reinstall a PMK.
pub struct AccessPoint<'a, const N: usize> {
    exchange: Exchange,
    advertisement: SecurityProfile<'a>,
    selected: SecurityProfile<'a>,
    dh: DhParameter<'a>,
    scope: CacheScope,
    phase: AccessPointPhase,
    response: Option<Packet<N>>,
    association: Option<InitialAssociation<'a>>,
    cache_expiry: Option<u64>,
    duplicate_pending: bool,
}
impl<'a, const N: usize> AccessPoint<'a, N> {
    pub fn new(
        context: AssociationContext<'a>,
        request: Elements<'a>,
        groups: GroupPolicy<'_>,
        cached: Option<CachedPmk>,
        now_us: u64,
    ) -> Result<Self, Error> {
        let AssociationContext {
            id,
            ssid,
            advertisement,
            retry,
        } = context;
        let exchange = Exchange::new(id, crate::RsnInterface::AccessPoint, retry, now_us)?;
        let selected = SecurityProfile::from_elements(request)?;
        let scope = CacheScope {
            addresses: id.addresses,
            ssid,
            management_protection: advertisement.negotiate(selected)?,
        };
        let dh = DhParameter::from_elements(request)
            .map_err(Error::Wire)?
            .ok_or(Error::InvalidKey)?;
        let group = dh.supported_group().map_err(Error::Wire)?;
        if !groups.contains(group) {
            return Err(Error::Wire(
                oer_ieee80211_mac::owe::WireError::UnsupportedGroup(dh.group),
            ));
        }
        let mut value = Self {
            exchange,
            advertisement,
            selected,
            dh,
            scope,
            phase: AccessPointPhase::NeedsKey(group),
            response: None,
            association: None,
            cache_expiry: None,
            duplicate_pending: false,
        };
        if let Some(cache) = cached {
            cache.validate_at(now_us)?;
            if cache.scope() != scope || !groups.contains(cache.group()) {
                return Err(Error::WrongContext);
            }
            let offered =
                RsnElement::parse(selected.rsn()).map_err(|_| Error::UnsupportedSecurity)?;
            if offered.pmkids().any(|name| name == cache.pmkid()) {
                value.response = Some(Packet::new(advertisement, Some(cache.pmkid()), None)?);
                value.cache_expiry = Some(cache.expires_at_us());
                value.association = Some(InitialAssociation::new(
                    cache.into_key(),
                    ssid,
                    advertisement,
                    selected,
                )?);
                value.phase = AccessPointPhase::Responding;
            }
        }
        Ok(value)
    }
    pub const fn phase(&self) -> AccessPointPhase {
        self.phase
    }
    /// Retention/retry wakeup. Expiry releases the retained response without
    /// changing a four-way/peer owner to which the key was already handed off.
    pub fn next_deadline_us(&self) -> Option<u64> {
        (!matches!(
            self.phase,
            AccessPointPhase::Failed | AccessPointPhase::Retired
        ))
        .then(|| {
            let deadline = self.exchange.next_deadline(
                self.phase == AccessPointPhase::Responding || self.duplicate_pending,
            );
            self.cache_expiry
                .map_or(deadline, |expiry| expiry.min(deadline))
        })
    }
    fn fail(&mut self) {
        self.association = None;
        self.response = None;
        self.phase = AccessPointPhase::Failed;
    }
    fn observe(&mut self, now: u64) -> Result<(), Error> {
        if matches!(
            self.phase,
            AccessPointPhase::Failed | AccessPointPhase::Retired
        ) {
            return Err(Error::WrongPhase);
        }
        let result = self.exchange.observe(now);
        if result == Err(Error::TimedOut) {
            self.expire();
        }
        result?;
        if self.cache_expiry.is_some_and(|expiry| now >= expiry) {
            self.expire();
            return Err(Error::KeyExpired);
        }
        Ok(())
    }
    fn expire(&mut self) {
        if self.phase == AccessPointPhase::Established {
            self.response = None;
            self.phase = AccessPointPhase::Retired;
        } else {
            self.fail();
        }
    }
    pub fn provide_key(&mut self, key: KeyPair, now_us: u64) -> Result<(), Error> {
        self.observe(now_us)?;
        if self.phase != AccessPointPhase::NeedsKey(key.group()) {
            return Err(Error::WrongPhase);
        }
        let response = Packet::new(self.advertisement, None, Some(key.parameter()))?;
        let key = key.finish(
            crate::RsnInterface::AccessPoint,
            self.scope.addresses,
            self.dh,
        );
        let key = match key {
            Ok(key) => key,
            Err(error) => {
                self.fail();
                return Err(error);
            }
        };
        self.association = Some(InitialAssociation::new(
            key,
            self.scope.ssid,
            self.advertisement,
            self.selected,
        )?);
        self.response = Some(response);
        self.phase = AccessPointPhase::Responding;
        Ok(())
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.exchange.transmission(self.response.as_ref()?.bytes())
    }
    pub fn admitted(&mut self, ticket: TxTicket, now_us: u64) -> Result<(), Error> {
        self.observe(now_us)?;
        if !matches!(
            self.phase,
            AccessPointPhase::Responding | AccessPointPhase::Established
        ) {
            return Err(Error::WrongPhase);
        }
        self.exchange.admitted(ticket, now_us)
    }
    /// Release the PMK/four-way binding once, only after the successful
    /// Association Response reached a matching acknowledged TX completion.
    pub fn tx_completed(
        &mut self,
        ticket: TxTicket,
        acknowledged: bool,
        now_us: u64,
    ) -> Result<Option<InitialAssociation<'a>>, Error> {
        self.observe(now_us)?;
        if !matches!(
            self.phase,
            AccessPointPhase::Responding | AccessPointPhase::Established
        ) {
            return Err(Error::WrongPhase);
        }
        self.exchange.completed(ticket, now_us)?;
        if acknowledged && self.phase == AccessPointPhase::Responding {
            self.phase = AccessPointPhase::Established;
            return Ok(self.association.take());
        }
        Ok(None)
    }
    pub fn tick(&mut self, now_us: u64) -> Result<bool, Error> {
        if matches!(
            self.phase,
            AccessPointPhase::Failed | AccessPointPhase::Retired
        ) {
            return Ok(false);
        }
        self.observe(now_us)?;
        if self.phase != AccessPointPhase::Responding && !self.duplicate_pending {
            return Ok(false);
        }
        let ready = self.exchange.retry(false, now_us)?;
        if ready {
            self.duplicate_pending = false;
        }
        Ok(ready)
    }
    /// MLME has separately checked SSID, addressing and peer generation.
    /// Unrelated capabilities can change on retries; security/DH cannot.
    pub fn duplicate_request(
        &mut self,
        id: ExchangeId,
        request: Elements<'_>,
        now_us: u64,
    ) -> Result<bool, Error> {
        self.observe(now_us)?;
        if id != self.exchange.id {
            return Err(Error::WrongContext);
        }
        let selected = SecurityProfile::from_elements(request)?;
        if selected != self.selected
            || DhParameter::from_elements(request).map_err(Error::Wire)? != Some(self.dh)
        {
            return Err(Error::WrongContext);
        }
        if self.response.is_none() || self.transmission().is_some() {
            return Ok(false);
        }
        self.duplicate_pending = true;
        // Already admitted DMA storage must finish before a duplicate queues.
        let ready = self.exchange.retry(false, now_us)?;
        if ready {
            self.duplicate_pending = false;
        }
        Ok(ready)
    }
    pub fn cancel(&mut self) {
        self.fail();
    }
}
