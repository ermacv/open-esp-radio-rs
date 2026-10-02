//! Current-AP over-the-DS relay. Backhaul framing/authentication is external.
use super::protocol::Packet;
use super::session::{Pending, Session};
use super::*;

pub struct DsForward<'a> {
    pub id: OperationId,
    pub current_ap: MacAddress,
    pub station: MacAddress,
    pub target: MacAddress,
    pub action_body: &'a [u8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayEvent {
    None,
    ResponseReady,
    Delivered,
    TimedOut,
}

/// One associated peer's outstanding DS exchange; no secret key is retained.
pub struct DsRelay<const N: usize> {
    session: Session,
    domain: MobilityDomainId,
    request: Option<Packet<N>>,
    operation: Option<OperationId>,
    target: MacAddress,
    deadline: u64,
    forwarded: bool,
    response: Option<Pending<N>>,
}
impl<const N: usize> DsRelay<N> {
    pub fn new(
        identity: SessionIdentity,
        domain: MobilityDomainId,
        timeouts: Timeouts,
        now_us: u64,
    ) -> Result<Self, Error> {
        Ok(Self {
            session: Session::new(identity, timeouts, now_us)?,
            domain,
            request: None,
            operation: None,
            target: [0; oer_ieee80211_mac::management::MAC_ADDRESS_LEN],
            deadline: 0,
            forwarded: false,
            response: None,
        })
    }
    /// `authorized_target` comes from the current AP's mobility-domain policy,
    /// not directly from the incoming frame's target address.
    pub fn request(
        &mut self,
        source: MacAddress,
        action_body: &[u8],
        authorized_target: MacAddress,
        now_us: u64,
    ) -> Result<(), Error> {
        self.session.observe(now_us)?;
        if source != self.session.identity.station {
            return Err(Error::WrongPeer);
        }
        let action = wire::Action::parse(action_body)?;
        if action.status.is_some()
            || action.station != source
            || action.target != authorized_target
            || action.target == [0; oer_ieee80211_mac::management::MAC_ADDRESS_LEN]
            || action.target == self.session.identity.current_ap
            || oer_ieee80211_mac::management::is_group_address(action.target)
        {
            return Err(Error::WrongPeer);
        }
        let elements = wire::FtElements::parse(action.elements.as_bytes())?;
        if wire::MobilityDomain::parse(elements.md)?.id != self.domain {
            return Err(Error::WrongKeyContext);
        }
        if let Some(request) = &self.request {
            if now_us >= self.deadline {
                return Err(Error::ExpiredOperation);
            }
            return if request.bytes() == action_body {
                Ok(())
            } else {
                Err(Error::Busy)
            };
        }
        let request = Packet::copy(action_body)?;
        let deadline = now_us
            .checked_add(self.session.timeouts.authentication_us)
            .ok_or(Error::DeadlineOverflow)?;
        let id = self.session.issue()?;
        self.request = Some(request);
        self.operation = Some(id);
        self.target = action.target;
        self.deadline = deadline;
        self.forwarded = false;
        Ok(())
    }
    pub fn forward(&self) -> Option<DsForward<'_>> {
        if self.forwarded {
            return None;
        }
        Some(DsForward {
            id: self.operation?,
            current_ap: self.session.identity.current_ap,
            station: self.session.identity.station,
            target: self.target,
            action_body: self.request.as_ref()?.bytes(),
        })
    }
    pub fn forward_admitted(&mut self, id: OperationId, now_us: u64) -> Result<(), Error> {
        self.session.observe(now_us)?;
        if self.operation != Some(id) || self.forwarded {
            return Err(Error::StaleOperation);
        }
        if now_us >= self.deadline {
            return Err(Error::ExpiredOperation);
        }
        self.forwarded = true;
        Ok(())
    }
    /// The transport authenticates the originating target AP and operation.
    pub fn authenticated_response(
        &mut self,
        id: OperationId,
        target: MacAddress,
        action_body: &[u8],
        now_us: u64,
    ) -> Result<RelayEvent, Error> {
        self.session.observe(now_us)?;
        if self.operation != Some(id) || !self.forwarded || self.response.is_some() {
            return Err(Error::StaleOperation);
        }
        if now_us >= self.deadline {
            return Err(Error::ExpiredOperation);
        }
        if target != self.target {
            return Err(Error::WrongPeer);
        }
        let action = wire::Action::parse(action_body)?;
        if action.status.is_none()
            || action.station != self.session.identity.station
            || action.target != self.target
        {
            return Err(Error::WrongPeer);
        }
        if action.status == Some(0) {
            let response = wire::FtElements::parse(action.elements.as_bytes())?;
            let request =
                wire::Action::parse(self.request.as_ref().ok_or(Error::WrongPhase)?.bytes())?;
            let requested = wire::FtElements::parse(request.elements.as_bytes())?;
            if response.ft.snonce() != requested.ft.snonce()
                || response.ft.r0kh()? != requested.ft.r0kh()?
                || wire::MobilityDomain::parse(response.md)?.id != self.domain
            {
                return Err(Error::WrongKeyContext);
            }
        }
        let packet = Packet::copy(action_body)?;
        let destination = self.session.identity.station;
        self.response = Some(Pending::new(
            &mut self.session,
            packet,
            destination,
            FrameKind::Action,
            self.deadline,
        )?);
        Ok(RelayEvent::ResponseReady)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.response.as_ref()?.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now_us: u64) -> Result<(), Error> {
        self.response
            .as_mut()
            .ok_or(Error::WrongPhase)?
            .admit(&mut self.session, id, now_us)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now_us: u64,
    ) -> Result<RelayEvent, Error> {
        self.session.observe(now_us)?;
        if now_us >= self.deadline {
            self.cancel();
            return Ok(RelayEvent::TimedOut);
        }
        if self
            .response
            .as_mut()
            .is_some_and(|pending| pending.complete(id, outcome))
        {
            self.cancel();
            return Ok(RelayEvent::Delivered);
        }
        Ok(RelayEvent::None)
    }
    pub fn poll(&mut self, now_us: u64) -> Result<RelayEvent, Error> {
        self.session.observe(now_us)?;
        if self.request.is_none() {
            return Ok(RelayEvent::None);
        }
        if now_us >= self.deadline {
            self.cancel();
            return Ok(RelayEvent::TimedOut);
        }
        if let Some(pending) = &mut self.response {
            pending.poll(&mut self.session, now_us)?;
        }
        Ok(RelayEvent::None)
    }
    pub fn cancel(&mut self) {
        self.request = None;
        self.operation = None;
        self.response = None;
        self.forwarded = false;
    }
    pub fn next_deadline_us(&self) -> Option<u64> {
        self.request.as_ref().map(|_| {
            self.response
                .as_ref()
                .map_or(self.deadline, Pending::next_deadline)
        })
    }
}
