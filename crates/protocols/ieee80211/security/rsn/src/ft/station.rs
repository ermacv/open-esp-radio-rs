//! Station FT authentication/reassociation, including explicit commit/rollback.
use super::protocol::{IeRequest, Packet, build_ies, unwrap_groups};
use super::session::{Pending, Session};
use super::*;
use oer_ieee80211_mac::management::elements::Elements;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target<'a> {
    pub bssid: MacAddress,
    pub ssid: WifiSsid,
    pub domain: MobilityDomain,
    pub security: SecurityProfile<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationPhase {
    Idle,
    Authenticating,
    Reassociating,
    CommitReady,
    Committing,
    RollingBack,
    Completed,
    Failed,
}

pub struct StationKeys {
    pub pairwise: FtPtk,
    pub groups: TransitionGroupKeys,
    pub r1: PmkR1,
}
pub struct StationCommit<'a> {
    pub id: OperationId,
    pub target: MacAddress,
    pub keys: &'a StationKeys,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationEvent {
    None,
    ReassociationReady,
    CommitReady,
    Completed,
    Failed,
    RollbackRequired(Rollback),
}

// CAPABILITY: wifi-roaming-and-service-discovery-fast-bss-transition-802-11r
pub struct Station<'a, const N: usize> {
    session: Session,
    root: &'a PmkR0,
    root_expiry: u64,
    target: Option<Target<'a>>,
    transport: Transport,
    phase: StationPhase,
    snonce: [u8; RSN_NONCE_LEN],
    pending: Option<Pending<N>>,
    r1: Option<PmkR1>,
    ptk: Option<FtPtk>,
    resources: Packet<N>,
    keys: Option<StationKeys>,
    commit_id: Option<OperationId>,
    commit_deadline: u64,
}
impl<'a, const N: usize> Station<'a, N> {
    pub fn new(
        root: &'a PmkR0,
        root_expiry_us: u64,
        identity: SessionIdentity,
        timeouts: Timeouts,
        now_us: u64,
    ) -> Result<Self, Error> {
        if root.context().station != identity.station || now_us >= root_expiry_us {
            return Err(Error::WrongKeyContext);
        }
        Ok(Self {
            session: Session::new(identity, timeouts, now_us)?,
            root,
            root_expiry: root_expiry_us,
            target: None,
            transport: Transport::OverAir,
            phase: StationPhase::Idle,
            snonce: [0; RSN_NONCE_LEN],
            pending: None,
            r1: None,
            ptk: None,
            resources: Packet::empty(),
            keys: None,
            commit_id: None,
            commit_deadline: 0,
        })
    }
    pub const fn phase(&self) -> StationPhase {
        self.phase
    }
    pub fn start(
        &mut self,
        target: Target<'a>,
        transport: Transport,
        snonce: [u8; RSN_NONCE_LEN],
        resources: &[u8],
        now_us: u64,
    ) -> Result<(), Error> {
        self.session.observe(now_us)?;
        if !matches!(self.phase, StationPhase::Idle | StationPhase::Failed) {
            return Err(Error::Busy);
        }
        if target.domain.id != self.root.context().mobility_domain
            || target.ssid != self.root.context().ssid
            || target.security.akm() != self.root.akm()
            || target.security.sae_pwe() != self.root.sae_pwe()
        {
            return Err(Error::WrongKeyContext);
        }
        if target.bssid == self.session.identity.current_ap
            || oer_ieee80211_mac::management::is_group_address(target.bssid)
            || target.bssid == [0; oer_ieee80211_mac::management::MAC_ADDRESS_LEN]
        {
            return Err(Error::WrongPeer);
        }
        if snonce == [0; RSN_NONCE_LEN] {
            return Err(Error::ZeroNonce);
        }
        if transport == Transport::OverDs && !target.domain.over_ds {
            return Err(Error::UnsupportedSecurity);
        }
        if !resources.is_empty() && !target.domain.resource_request {
            return Err(Error::UnsupportedSecurity);
        }
        wire::RicElements::parse(resources)?;
        let resources = Packet::copy(resources)?;
        let ies = build_ies::<N>(IeRequest {
            security: target.security,
            selected_rsn: true,
            md: target.domain,
            root_name: self.root.name(),
            r0kh: self.root.context().r0kh,
            r1kh: None,
            snonce,
            anonce: [0; RSN_NONCE_LEN],
            ric: &[],
            groups: None,
            protected: None,
            reassociation_deadline_tu: None,
        })?;
        let mut packet = Packet::empty();
        let (length, destination, kind) = match transport {
            Transport::OverAir => (
                wire::Authentication {
                    transaction: wire::AuthenticationTransaction::Request,
                    status: 0,
                    elements: Elements::parse(ies.bytes()).map_err(wire::WireError::from)?,
                }
                .encode(packet.bytes_for_encoding())?,
                target.bssid,
                FrameKind::Authentication,
            ),
            Transport::OverDs => (
                wire::Action {
                    station: self.session.identity.station,
                    target: target.bssid,
                    status: None,
                    elements: Elements::parse(ies.bytes()).map_err(wire::WireError::from)?,
                }
                .encode(packet.bytes_for_encoding())?,
                self.session.identity.current_ap,
                FrameKind::Action,
            ),
        };
        packet.set_encoded_length(length);
        let deadline = self.session.deadline(
            now_us,
            self.session.timeouts.authentication_us,
            self.root_expiry,
        )?;
        let pending = Pending::new(&mut self.session, packet, destination, kind, deadline)?;
        self.pending = Some(pending);
        self.target = Some(target);
        self.transport = transport;
        self.resources = resources;
        self.snonce = snonce;
        self.phase = StationPhase::Authenticating;
        Ok(())
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.pending.as_ref()?.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now_us: u64) -> Result<(), Error> {
        self.pending
            .as_mut()
            .ok_or(Error::WrongPhase)?
            .admit(&mut self.session, id, now_us)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now_us: u64,
    ) -> Result<(), Error> {
        self.session.observe(now_us)?;
        if let Some(pending) = &mut self.pending {
            pending.complete(id, outcome);
        }
        Ok(())
    }
    pub fn authentication_response(
        &mut self,
        source: MacAddress,
        body: &[u8],
        now_us: u64,
    ) -> Result<StationEvent, Error> {
        self.live(now_us)?;
        if self.phase != StationPhase::Authenticating
            || !self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.ever_admitted)
        {
            return Err(Error::WrongPhase);
        }
        let target = self.target.ok_or(Error::WrongPhase)?;
        let (status, bytes) = match self.transport {
            Transport::OverAir => {
                if source != target.bssid {
                    return Err(Error::WrongPeer);
                }
                let response = wire::Authentication::parse(body)?;
                if response.transaction != wire::AuthenticationTransaction::Response {
                    return Err(Error::WrongPhase);
                }
                (response.status, response.elements.as_bytes())
            }
            Transport::OverDs => {
                if source != self.session.identity.current_ap {
                    return Err(Error::WrongPeer);
                }
                let response = wire::Action::parse(body)?;
                if response.station != self.session.identity.station
                    || response.target != target.bssid
                {
                    return Err(Error::WrongPeer);
                }
                (
                    response.status.ok_or(Error::WrongPhase)?,
                    response.elements.as_bytes(),
                )
            }
        };
        if status != 0 {
            self.fail();
            return Err(Error::PeerRejected(status));
        }
        let elements = wire::FtElements::parse(bytes)?;
        let r1kh = elements.ft.r1kh()?.ok_or(Error::WrongKeyContext)?;
        let r1 = self.root.derive_r1(r1kh);
        target.security.accept(elements, self.root.name(), false)?;
        self.check_context(elements, None)?;
        if elements.ft.snonce() != &self.snonce
            || elements.ft.anonce() == &[0; RSN_NONCE_LEN]
            || elements.ft.element_count() != 0
        {
            return Err(Error::WrongKeyContext);
        }
        let ptk = r1.derive_ptk(FtPtkContext {
            addresses: wire::FtAddresses {
                station: self.session.identity.station,
                access_point: target.bssid,
            },
            snonce: self.snonce,
            anonce: *elements.ft.anonce(),
        })?;
        let ies = build_ies(IeRequest {
            security: target.security,
            selected_rsn: true,
            md: target.domain,
            root_name: r1.name(),
            r0kh: self.root.context().r0kh,
            r1kh: Some(r1kh),
            snonce: self.snonce,
            anonce: ptk.context().anonce,
            ric: self.resources.bytes(),
            groups: None,
            protected: Some((&ptk, wire::MicTransaction::ReassociationRequest)),
            reassociation_deadline_tu: None,
        })?;
        let mut deadline = self.session.deadline(
            now_us,
            self.session.timeouts.reassociation_us,
            self.root_expiry,
        )?;
        if let Some(tu) = elements.reassociation_deadline_tu {
            deadline = deadline.min(
                now_us
                    .checked_add(
                        u64::from(tu) * oer_ieee80211_mac::management::IEEE_TIME_UNIT_MICROS,
                    )
                    .ok_or(Error::DeadlineOverflow)?,
            );
        }
        if let Some(seconds) = elements.key_lifetime_seconds {
            let micros = core::time::Duration::from_secs(u64::from(seconds)).as_micros() as u64;
            deadline = deadline.min(now_us.checked_add(micros).ok_or(Error::DeadlineOverflow)?);
        }
        if deadline <= now_us {
            return Err(Error::ExpiredOperation);
        }
        let pending = Pending::new(
            &mut self.session,
            ies,
            target.bssid,
            FrameKind::ReassociationElements,
            deadline,
        )?;
        self.pending = Some(pending);
        self.r1 = Some(r1);
        self.ptk = Some(ptk);
        self.phase = StationPhase::Reassociating;
        Ok(StationEvent::ReassociationReady)
    }
    pub fn reassociation_response(
        &mut self,
        source: MacAddress,
        status: u16,
        ies: &[u8],
        now_us: u64,
    ) -> Result<StationEvent, Error> {
        self.live(now_us)?;
        if self.phase != StationPhase::Reassociating
            || !self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.ever_admitted)
        {
            return Err(Error::WrongPhase);
        }
        let target = self.target.ok_or(Error::WrongPhase)?;
        if source != target.bssid {
            return Err(Error::WrongPeer);
        }
        if status != 0 {
            self.fail();
            return Err(Error::PeerRejected(status));
        }
        let elements = wire::FtElements::parse(ies)?;
        let r1 = self.r1.as_ref().ok_or(Error::WrongPhase)?;
        target.security.accept(elements, r1.name(), true)?;
        self.check_context(elements, Some(r1.r1kh()))?;
        let ptk = self.ptk.as_ref().ok_or(Error::WrongPhase)?;
        if elements.ft.snonce() != &ptk.context().snonce
            || elements.ft.anonce() != &ptk.context().anonce
            || elements.ft.rsnxe_used() != target.security.rsnxe_used()
        {
            return Err(Error::WrongKeyContext);
        }
        ptk.verify_mic(wire::MicTransaction::ReassociationResponse, elements)?;
        validate_resource_response(self.resources.bytes(), elements.ric)?;
        let groups = unwrap_groups(ptk, elements.ft, target.security.protects_management())?;
        let deadline = self.session.deadline(
            now_us,
            self.session.timeouts.commit_us,
            self.pending
                .as_ref()
                .ok_or(Error::WrongPhase)?
                .deadline
                .min(self.root_expiry),
        )?;
        let id = self.session.issue()?;
        self.keys = Some(StationKeys {
            pairwise: self.ptk.take().ok_or(Error::WrongPhase)?,
            groups,
            r1: self.r1.take().ok_or(Error::WrongPhase)?,
        });
        self.pending = None;
        self.commit_id = Some(id);
        self.commit_deadline = deadline;
        self.phase = StationPhase::CommitReady;
        Ok(StationEvent::CommitReady)
    }
    pub fn commit(&self) -> Option<StationCommit<'_>> {
        (self.phase == StationPhase::CommitReady).then(|| StationCommit {
            id: self.commit_id.expect("commit identity"),
            target: self.target.expect("transition target").bssid,
            keys: self.keys.as_ref().expect("authenticated keys"),
        })
    }
    pub fn commit_admitted(&mut self, id: OperationId, now_us: u64) -> Result<(), Error> {
        self.live(now_us)?;
        if self.phase != StationPhase::CommitReady || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        self.phase = StationPhase::Committing;
        Ok(())
    }
    pub fn commit_completed(
        &mut self,
        id: OperationId,
        outcome: CommitOutcome,
        now_us: u64,
    ) -> Result<StationEvent, Error> {
        self.session.observe(now_us)?;
        if self.phase != StationPhase::Committing || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        if outcome == CommitOutcome::RolledBack {
            self.fail();
            return Ok(StationEvent::Failed);
        }
        if now_us >= self.commit_deadline {
            return Ok(self.require_rollback());
        }
        self.phase = StationPhase::Completed;
        Ok(StationEvent::Completed)
    }
    /// Transfer connected keys once, after the complete driver transaction.
    pub fn take_completed_keys(&mut self) -> Option<StationKeys> {
        if self.phase == StationPhase::Completed {
            self.keys.take()
        } else {
            None
        }
    }
    pub fn cancel(&mut self) -> StationEvent {
        if matches!(
            self.phase,
            StationPhase::Committing | StationPhase::RollingBack
        ) {
            self.require_rollback()
        } else if self.phase == StationPhase::Completed {
            StationEvent::None
        } else {
            self.fail();
            StationEvent::Failed
        }
    }
    pub fn rollback(&self) -> Option<Rollback> {
        (self.phase == StationPhase::RollingBack).then(|| Rollback {
            id: self.commit_id.expect("rollback identity"),
            target: self.target.expect("target").bssid,
        })
    }
    pub fn rollback_completed(&mut self, id: OperationId) -> Result<(), Error> {
        if self.phase != StationPhase::RollingBack || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        self.fail();
        Ok(())
    }
    pub fn poll(&mut self, now_us: u64) -> Result<StationEvent, Error> {
        self.session.observe(now_us)?;
        if matches!(
            self.phase,
            StationPhase::Completed | StationPhase::Failed | StationPhase::Idle
        ) {
            return Ok(StationEvent::None);
        }
        if self.phase == StationPhase::RollingBack {
            return Ok(self.require_rollback());
        }
        let expired = now_us >= self.root_expiry
            || (matches!(
                self.phase,
                StationPhase::CommitReady | StationPhase::Committing
            ) && now_us >= self.commit_deadline);
        if expired {
            return Ok(self.cancel());
        }
        if let Some(pending) = &mut self.pending {
            match pending.poll(&mut self.session, now_us) {
                Err(Error::ExpiredOperation) => {
                    self.fail();
                    return Ok(StationEvent::Failed);
                }
                other => other?,
            }
        }
        Ok(StationEvent::None)
    }
    pub fn next_deadline_us(&self) -> Option<u64> {
        self.pending
            .as_ref()
            .map(Pending::next_deadline)
            .or_else(|| {
                matches!(
                    self.phase,
                    StationPhase::CommitReady | StationPhase::Committing
                )
                .then_some(self.commit_deadline)
            })
    }
    fn live(&mut self, now_us: u64) -> Result<(), Error> {
        self.session.observe(now_us)?;
        if now_us >= self.root_expiry {
            return Err(Error::KeyExpired);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now_us >= pending.deadline)
            || (matches!(
                self.phase,
                StationPhase::CommitReady | StationPhase::Committing
            ) && now_us >= self.commit_deadline)
        {
            return Err(Error::ExpiredOperation);
        }
        Ok(())
    }
    fn check_context(
        &self,
        elements: wire::FtElements<'_>,
        r1kh: Option<R1khId>,
    ) -> Result<(), Error> {
        if wire::MobilityDomain::parse(elements.md)?.id != self.root.context().mobility_domain
            || elements.ft.r0kh()? != Some(self.root.context().r0kh)
            || r1kh.is_some_and(|expected| elements.ft.r1kh().ok().flatten() != Some(expected))
        {
            return Err(Error::WrongKeyContext);
        }
        Ok(())
    }
    fn fail(&mut self) {
        self.pending = None;
        self.r1 = None;
        self.ptk = None;
        self.keys = None;
        self.commit_id = None;
        self.phase = StationPhase::Failed;
    }
    fn require_rollback(&mut self) -> StationEvent {
        self.phase = StationPhase::RollingBack;
        StationEvent::RollbackRequired(self.rollback().expect("retained admitted transaction"))
    }
}

pub(crate) fn validate_resource_response(request: &[u8], response: &[u8]) -> Result<(), Error> {
    let request = wire::RicElements::parse(request)?.elements();
    let response = wire::RicElements::parse(response)?.elements();
    let mut requested = request
        .iter()
        .filter(|element| element.id == wire::RIC_DATA_ELEMENT_ID);
    let mut answered = response
        .iter()
        .filter(|element| element.id == wire::RIC_DATA_ELEMENT_ID);
    loop {
        match (requested.next(), answered.next()) {
            (None, None) => return Ok(()),
            (Some(request), Some(response))
                if request.body.len() == 4
                    && response.body.len() == 4
                    && request.body[0] == response.body[0] =>
            {
                let status = u16::from_le_bytes([response.body[2], response.body[3]]);
                if status != 0 {
                    return Err(Error::PeerRejected(status));
                }
            }
            _ => return Err(Error::Wire(wire::WireError::InconsistentFields)),
        }
    }
}
