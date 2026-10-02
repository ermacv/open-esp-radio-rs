//! One target-AP peer. Peer-table admission remains with the AP service.
use super::protocol::{IeRequest, Packet, build_ies};
use super::session::{Pending, Session};
use super::*;
use oer_ieee80211_mac::management::elements::Elements;
use oer_ieee80211_mac::security::rsn::{RSN_PMKID_LEN, RsnElement};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointProfile<'a> {
    pub bssid: MacAddress,
    pub ssid: WifiSsid,
    pub domain: MobilityDomain,
    pub r1kh: R1khId,
    pub security: SecurityProfile<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointPhase {
    Idle,
    LookingUpKey,
    Rejecting,
    Authenticating,
    Resources,
    CommitReady,
    Committing,
    Replying,
    Authorized,
    RollingBack,
    Failed,
}

/// A request to a trusted local cache or authenticated inter-AP key service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyLookup {
    pub id: OperationId,
    pub root: RootContext,
    pub root_name: [u8; RSN_PMKID_LEN],
    pub r1kh: R1khId,
    pub akm: FtAkm,
    pub target: MacAddress,
    pub deadline_us: u64,
}
pub struct AccessPointCommit<'a> {
    pub id: OperationId,
    pub station: MacAddress,
    pub pairwise: &'a FtPtk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointEvent {
    None,
    KeyLookupReady,
    ResourcesReady,
    CommitReady,
    Authorized,
    Failed,
    RollbackRequired(Rollback),
}

pub struct AccessPoint<'a, const N: usize> {
    session: Session,
    profile: AccessPointProfile<'a>,
    phase: AccessPointPhase,
    transport: Transport,
    lookup: Option<KeyLookup>,
    snonce: [u8; RSN_NONCE_LEN],
    anonce: [u8; RSN_NONCE_LEN],
    key_expiry: u64,
    r1: Option<PmkR1>,
    ptk: Option<FtPtk>,
    pending: Option<Pending<N>>,
    reassociation: Packet<N>,
    response: Option<Packet<N>>,
    commit_id: Option<OperationId>,
    deadline: u64,
}
impl<'a, const N: usize> AccessPoint<'a, N> {
    pub fn new(
        profile: AccessPointProfile<'a>,
        identity: SessionIdentity,
        timeouts: Timeouts,
        now_us: u64,
    ) -> Result<Self, Error> {
        if oer_ieee80211_mac::management::is_group_address(profile.bssid)
            || profile.bssid == [0; oer_ieee80211_mac::management::MAC_ADDRESS_LEN]
        {
            return Err(Error::WrongPeer);
        }
        Ok(Self {
            session: Session::new(identity, timeouts, now_us)?,
            profile,
            phase: AccessPointPhase::Idle,
            transport: Transport::OverAir,
            lookup: None,
            snonce: [0; RSN_NONCE_LEN],
            anonce: [0; RSN_NONCE_LEN],
            key_expiry: 0,
            r1: None,
            ptk: None,
            pending: None,
            reassociation: Packet::empty(),
            response: None,
            commit_id: None,
            deadline: 0,
        })
    }
    pub const fn phase(&self) -> AccessPointPhase {
        self.phase
    }
    pub const fn transport(&self) -> Transport {
        self.transport
    }
    pub fn authentication_request(
        &mut self,
        source: MacAddress,
        body: &[u8],
        anonce: [u8; RSN_NONCE_LEN],
        now_us: u64,
    ) -> Result<AccessPointEvent, Error> {
        if source != self.session.identity.station {
            return Err(Error::WrongPeer);
        }
        let request = wire::Authentication::parse(body)?;
        if request.transaction != wire::AuthenticationTransaction::Request || request.status != 0 {
            return Err(Error::WrongPhase);
        }
        self.begin(
            request.elements.as_bytes(),
            Transport::OverAir,
            anonce,
            now_us,
        )
    }
    /// Input has been authenticated/authorized by the caller's backhaul.
    pub fn authenticated_ds_request(
        &mut self,
        current_ap: MacAddress,
        body: &[u8],
        anonce: [u8; RSN_NONCE_LEN],
        now_us: u64,
    ) -> Result<AccessPointEvent, Error> {
        if !self.profile.domain.over_ds || current_ap != self.session.identity.current_ap {
            return Err(Error::WrongPeer);
        }
        let request = wire::Action::parse(body)?;
        if request.status.is_some()
            || request.station != self.session.identity.station
            || request.target != self.profile.bssid
        {
            return Err(Error::WrongPeer);
        }
        self.begin(
            request.elements.as_bytes(),
            Transport::OverDs,
            anonce,
            now_us,
        )
    }
    fn begin(
        &mut self,
        ies: &[u8],
        transport: Transport,
        anonce: [u8; RSN_NONCE_LEN],
        now_us: u64,
    ) -> Result<AccessPointEvent, Error> {
        self.session.observe(now_us)?;
        let elements = wire::FtElements::parse(ies)?;
        self.profile.security.accept_rsn(elements.rsn)?;
        let rsn = RsnElement::parse(elements.rsn).map_err(|_| Error::UnsupportedSecurity)?;
        if rsn.akm_suites().len() != 1 || rsn.pmkid_count() != Some(1) {
            return Err(Error::UnsupportedSecurity);
        }
        if wire::MobilityDomain::parse(elements.md)?.id != self.profile.domain.id
            || elements.ft.element_count() != 0
            || elements.ft.anonce() != &[0; RSN_NONCE_LEN]
            || elements.ft.snonce() == &[0; RSN_NONCE_LEN]
            || !elements.ric.is_empty()
        {
            return Err(Error::WrongKeyContext);
        }
        let r0kh = elements.ft.r0kh()?.ok_or(Error::WrongKeyContext)?;
        let root_name = rsn.pmkids().next().ok_or(Error::WrongKeyContext)?;
        if let Some(lookup) = self.lookup
            && lookup.root_name == root_name
            && lookup.root.r0kh == r0kh
            && self.snonce == *elements.ft.snonce()
            && self.transport == transport
            && matches!(
                self.phase,
                AccessPointPhase::LookingUpKey | AccessPointPhase::Authenticating
            )
        {
            if now_us >= self.deadline {
                return Err(Error::ExpiredOperation);
            }
            return Ok(AccessPointEvent::None);
        }
        if self.phase != AccessPointPhase::Idle {
            return Err(Error::Busy);
        }
        if anonce == [0; RSN_NONCE_LEN] {
            return Err(Error::ZeroNonce);
        }
        let deadline = now_us
            .checked_add(self.session.timeouts.authentication_us)
            .ok_or(Error::DeadlineOverflow)?;
        let lookup = KeyLookup {
            id: self.session.issue()?,
            root: RootContext {
                ssid: self.profile.ssid,
                mobility_domain: self.profile.domain.id,
                r0kh,
                station: self.session.identity.station,
            },
            root_name,
            r1kh: self.profile.r1kh,
            akm: self.profile.security.akm(),
            target: self.profile.bssid,
            deadline_us: deadline,
        };
        self.lookup = Some(lookup);
        self.snonce = *elements.ft.snonce();
        self.anonce = anonce;
        self.transport = transport;
        self.deadline = deadline;
        self.phase = AccessPointPhase::LookingUpKey;
        Ok(AccessPointEvent::KeyLookupReady)
    }
    pub fn key_lookup(&self) -> Option<KeyLookup> {
        if self.phase == AccessPointPhase::LookingUpKey {
            self.lookup
        } else {
            None
        }
    }
    pub fn key_delivered(&mut self, delivery: KeyDelivery, now_us: u64) -> Result<(), Error> {
        if delivery.target != self.profile.bssid {
            return Err(Error::WrongPeer);
        }
        let KeyDelivery {
            request: id,
            target: _,
            expires_at_us,
            key,
        } = delivery;
        self.session.observe(now_us)?;
        let lookup = self.lookup.ok_or(Error::WrongPhase)?;
        if self.phase != AccessPointPhase::LookingUpKey || lookup.id != id {
            return Err(Error::StaleOperation);
        }
        if now_us >= lookup.deadline_us || now_us >= expires_at_us {
            return Err(Error::KeyExpired);
        }
        if key.root_context() != lookup.root
            || key.root_name() != lookup.root_name
            || key.r1kh() != lookup.r1kh
            || key.akm() != lookup.akm
            || key.sae_pwe() != self.profile.security.sae_pwe()
        {
            return Err(Error::WrongKeyContext);
        }
        let ptk = key.derive_ptk(FtPtkContext {
            addresses: wire::FtAddresses {
                station: self.session.identity.station,
                access_point: self.profile.bssid,
            },
            snonce: self.snonce,
            anonce: self.anonce,
        })?;
        // FT Authentication Response keeps PMKR0Name. PMKR1Name belongs to
        // the subsequent Reassociation exchange. RSNXE is not in this response.
        let security = self.profile.security.without_rsnxe();
        let ies = build_ies::<N>(IeRequest {
            security,
            selected_rsn: false,
            md: self.profile.domain,
            root_name: lookup.root_name,
            r0kh: lookup.root.r0kh,
            r1kh: Some(self.profile.r1kh),
            snonce: self.snonce,
            anonce: self.anonce,
            ric: &[],
            groups: None,
            protected: None,
            reassociation_deadline_tu: None,
        })?;
        let mut packet = Packet::empty();
        let (length, destination, kind) = match self.transport {
            Transport::OverAir => (
                wire::Authentication {
                    transaction: wire::AuthenticationTransaction::Response,
                    status: 0,
                    elements: Elements::parse(ies.bytes()).map_err(wire::WireError::from)?,
                }
                .encode(packet.bytes_for_encoding())?,
                self.session.identity.station,
                FrameKind::Authentication,
            ),
            Transport::OverDs => (
                wire::Action {
                    station: self.session.identity.station,
                    target: self.profile.bssid,
                    status: Some(0),
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
            self.session.timeouts.reassociation_us,
            expires_at_us,
        )?;
        let pending = Pending::new(&mut self.session, packet, destination, kind, deadline)?;
        self.deadline = deadline;
        self.key_expiry = expires_at_us;
        self.r1 = Some(key);
        self.ptk = Some(ptk);
        self.pending = Some(pending);
        self.phase = AccessPointPhase::Authenticating;
        Ok(())
    }
    /// Explicit refusal from the key-service/policy owner. Never substitute a
    /// different cached key or leave a refused STA waiting for a success frame.
    pub fn key_unavailable(
        &mut self,
        id: OperationId,
        status: u16,
        now_us: u64,
    ) -> Result<(), Error> {
        self.live(now_us)?;
        if status == 0 {
            return Err(Error::InvalidConfiguration);
        }
        if self.phase != AccessPointPhase::LookingUpKey
            || self.lookup.map(|lookup| lookup.id) != Some(id)
        {
            return Err(Error::StaleOperation);
        }
        let elements = Elements::parse(&[]).map_err(wire::WireError::from)?;
        let mut packet = Packet::empty();
        let (length, destination, kind) = match self.transport {
            Transport::OverAir => (
                wire::Authentication {
                    transaction: wire::AuthenticationTransaction::Response,
                    status,
                    elements,
                }
                .encode(packet.bytes_for_encoding())?,
                self.session.identity.station,
                FrameKind::Authentication,
            ),
            Transport::OverDs => (
                wire::Action {
                    station: self.session.identity.station,
                    target: self.profile.bssid,
                    status: Some(status),
                    elements,
                }
                .encode(packet.bytes_for_encoding())?,
                self.session.identity.current_ap,
                FrameKind::Action,
            ),
        };
        packet.set_encoded_length(length);
        let pending = Pending::new(&mut self.session, packet, destination, kind, self.deadline)?;
        self.pending = Some(pending);
        self.lookup = None;
        self.phase = AccessPointPhase::Rejecting;
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
    ) -> Result<AccessPointEvent, Error> {
        self.session.observe(now_us)?;
        let delivered = self
            .pending
            .as_mut()
            .is_some_and(|pending| pending.complete(id, outcome));
        if delivered && self.phase == AccessPointPhase::Rejecting {
            self.fail();
            return Ok(AccessPointEvent::Failed);
        }
        if delivered && self.phase == AccessPointPhase::Authorized {
            self.pending = None;
        }
        if delivered && self.phase == AccessPointPhase::Replying {
            if now_us >= self.deadline || now_us >= self.key_expiry {
                return Ok(self.require_rollback());
            }
            self.phase = AccessPointPhase::Authorized;
            self.pending = None;
            return Ok(AccessPointEvent::Authorized);
        }
        Ok(AccessPointEvent::None)
    }
    pub fn reassociation_request(
        &mut self,
        source: MacAddress,
        ies: &[u8],
        now_us: u64,
    ) -> Result<AccessPointEvent, Error> {
        self.live(now_us)?;
        if source != self.session.identity.station {
            return Err(Error::WrongPeer);
        }
        if matches!(
            self.phase,
            AccessPointPhase::Resources
                | AccessPointPhase::CommitReady
                | AccessPointPhase::Committing
                | AccessPointPhase::Replying
                | AccessPointPhase::Authorized
        ) {
            if ies != self.reassociation.bytes() {
                return Err(Error::Busy);
            }
            // Never reinstall a key on a retransmitted authenticated request.
            if self.phase == AccessPointPhase::Authorized && self.pending.is_none() {
                let packet =
                    Packet::copy(self.response.as_ref().ok_or(Error::WrongPhase)?.bytes())?;
                let pending = Pending::new(
                    &mut self.session,
                    packet,
                    source,
                    FrameKind::ReassociationElements,
                    self.deadline,
                )?;
                self.pending = Some(pending);
            }
            return Ok(AccessPointEvent::None);
        }
        if self.phase != AccessPointPhase::Authenticating
            || !self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.ever_admitted)
        {
            return Err(Error::WrongPhase);
        }
        let elements = wire::FtElements::parse(ies)?;
        let key = self.r1.as_ref().ok_or(Error::WrongPhase)?;
        self.profile.security.accept(elements, key.name(), false)?;
        if wire::MobilityDomain::parse(elements.md)?.id != self.profile.domain.id
            || elements.ft.r0kh()? != Some(key.root_context().r0kh)
            || elements.ft.r1kh()? != Some(self.profile.r1kh)
            || elements.ft.snonce() != &self.snonce
            || elements.ft.anonce() != &self.anonce
        {
            return Err(Error::WrongKeyContext);
        }
        if !elements.ric.is_empty() && !self.profile.domain.resource_request {
            return Err(Error::UnsupportedSecurity);
        }
        if elements.ft.rsnxe_used() != self.profile.security.rsnxe_used()
            || (elements.ft.rsnxe_used() && elements.rsnxe.is_empty())
        {
            return Err(Error::SecurityMismatch);
        }
        self.ptk
            .as_ref()
            .ok_or(Error::WrongPhase)?
            .verify_mic(wire::MicTransaction::ReassociationRequest, elements)?;
        let retained = Packet::copy(ies)?;
        let id = self.session.issue()?;
        self.reassociation = retained;
        self.pending = None;
        self.commit_id = Some(id);
        self.phase = AccessPointPhase::Resources;
        Ok(AccessPointEvent::ResourcesReady)
    }
    /// Resource owner must answer every requested RDIE. Empty is valid only
    /// for a request with no RIC; rejected resources return an explicit error.
    pub fn resources(&self) -> Option<(OperationId, &[u8])> {
        if self.phase != AccessPointPhase::Resources {
            return None;
        }
        Some((
            self.commit_id?,
            wire::FtElements::parse(self.reassociation.bytes())
                .ok()?
                .ric,
        ))
    }
    pub fn prepare_response(
        &mut self,
        id: OperationId,
        ric_response: &[u8],
        groups: &TransitionGroupKeys,
        now_us: u64,
    ) -> Result<AccessPointEvent, Error> {
        self.live(now_us)?;
        if self.phase != AccessPointPhase::Resources || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        let request = wire::FtElements::parse(self.reassociation.bytes())?;
        super::station::validate_resource_response(request.ric, ric_response)?;
        if groups.igtk.is_some() != self.profile.security.protects_management() {
            return Err(Error::InvalidGroupKeys);
        }
        let key = self.r1.as_ref().ok_or(Error::WrongPhase)?;
        let ptk = self.ptk.as_ref().ok_or(Error::WrongPhase)?;
        let response = build_ies(IeRequest {
            security: self.profile.security,
            selected_rsn: false,
            md: self.profile.domain,
            root_name: key.name(),
            r0kh: key.root_context().r0kh,
            r1kh: Some(self.profile.r1kh),
            snonce: self.snonce,
            anonce: self.anonce,
            ric: ric_response,
            groups: Some(groups),
            protected: Some((ptk, wire::MicTransaction::ReassociationResponse)),
            reassociation_deadline_tu: None,
        })?;
        let deadline = self.session.deadline(
            now_us,
            self.session.timeouts.commit_us,
            self.deadline.min(self.key_expiry),
        )?;
        self.response = Some(response);
        self.deadline = deadline;
        self.phase = AccessPointPhase::CommitReady;
        Ok(AccessPointEvent::CommitReady)
    }
    pub fn commit(&self) -> Option<AccessPointCommit<'_>> {
        (self.phase == AccessPointPhase::CommitReady).then(|| AccessPointCommit {
            id: self.commit_id.expect("commit identity"),
            station: self.session.identity.station,
            pairwise: self.ptk.as_ref().expect("authenticated pairwise key"),
        })
    }
    pub fn commit_admitted(&mut self, id: OperationId, now_us: u64) -> Result<(), Error> {
        self.live(now_us)?;
        if self.phase != AccessPointPhase::CommitReady || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        self.phase = AccessPointPhase::Committing;
        Ok(())
    }
    pub fn commit_completed(
        &mut self,
        id: OperationId,
        outcome: CommitOutcome,
        now_us: u64,
    ) -> Result<AccessPointEvent, Error> {
        self.session.observe(now_us)?;
        if self.phase != AccessPointPhase::Committing || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        if outcome == CommitOutcome::RolledBack {
            self.fail();
            return Ok(AccessPointEvent::Failed);
        }
        if now_us >= self.deadline || now_us >= self.key_expiry {
            return Ok(self.require_rollback());
        }
        let packet = Packet::copy(self.response.as_ref().ok_or(Error::WrongPhase)?.bytes())?;
        let destination = self.session.identity.station;
        let pending = match Pending::new(
            &mut self.session,
            packet,
            destination,
            FrameKind::ReassociationElements,
            self.deadline,
        ) {
            Ok(pending) => pending,
            Err(_) => return Ok(self.require_rollback()),
        };
        self.pending = Some(pending);
        self.phase = AccessPointPhase::Replying;
        Ok(AccessPointEvent::None)
    }
    /// After response delivery the AP service takes the connected key owner
    /// once; retained response bytes still answer retries without reinstall.
    pub fn take_authorized_keys(&mut self) -> Option<(FtPtk, PmkR1)> {
        if self.phase != AccessPointPhase::Authorized {
            return None;
        }
        Some((self.ptk.take()?, self.r1.take()?))
    }
    pub fn cancel(&mut self) -> AccessPointEvent {
        if matches!(
            self.phase,
            AccessPointPhase::Committing
                | AccessPointPhase::Replying
                | AccessPointPhase::RollingBack
        ) {
            self.require_rollback()
        } else if self.phase == AccessPointPhase::Authorized {
            AccessPointEvent::None
        } else {
            self.fail();
            AccessPointEvent::Failed
        }
    }
    pub fn rollback(&self) -> Option<Rollback> {
        (self.phase == AccessPointPhase::RollingBack).then(|| Rollback {
            id: self.commit_id.expect("rollback identity"),
            target: self.profile.bssid,
        })
    }
    pub fn rollback_completed(&mut self, id: OperationId) -> Result<(), Error> {
        if self.phase != AccessPointPhase::RollingBack || self.commit_id != Some(id) {
            return Err(Error::StaleOperation);
        }
        self.fail();
        Ok(())
    }
    pub fn poll(&mut self, now_us: u64) -> Result<AccessPointEvent, Error> {
        self.session.observe(now_us)?;
        if self.phase == AccessPointPhase::Authorized {
            if let Some(pending) = &mut self.pending {
                if now_us >= pending.deadline {
                    self.pending = None;
                } else {
                    pending.poll(&mut self.session, now_us)?;
                }
            }
            return Ok(AccessPointEvent::None);
        }
        if matches!(
            self.phase,
            AccessPointPhase::Idle | AccessPointPhase::Failed
        ) {
            return Ok(AccessPointEvent::None);
        }
        if self.phase == AccessPointPhase::RollingBack {
            return Ok(self.require_rollback());
        }
        if now_us >= self.deadline || (self.key_expiry != 0 && now_us >= self.key_expiry) {
            return Ok(self.cancel());
        }
        if let Some(pending) = &mut self.pending {
            pending.poll(&mut self.session, now_us)?;
        }
        Ok(AccessPointEvent::None)
    }
    pub fn next_deadline_us(&self) -> Option<u64> {
        if self.phase == AccessPointPhase::Authorized {
            return self.pending.as_ref().map(Pending::next_deadline);
        }
        if matches!(
            self.phase,
            AccessPointPhase::Idle
                | AccessPointPhase::Failed
                | AccessPointPhase::Authorized
                | AccessPointPhase::RollingBack
        ) {
            return None;
        }
        Some(
            self.pending
                .as_ref()
                .map_or(self.deadline, Pending::next_deadline),
        )
    }
    fn live(&mut self, now_us: u64) -> Result<(), Error> {
        self.session.observe(now_us)?;
        if now_us >= self.deadline {
            return Err(Error::ExpiredOperation);
        }
        if self.key_expiry != 0 && now_us >= self.key_expiry {
            return Err(Error::KeyExpired);
        }
        Ok(())
    }
    fn fail(&mut self) {
        self.lookup = None;
        self.r1 = None;
        self.ptk = None;
        self.pending = None;
        self.response = None;
        self.commit_id = None;
        self.phase = AccessPointPhase::Failed;
    }
    fn require_rollback(&mut self) -> AccessPointEvent {
        self.phase = AccessPointPhase::RollingBack;
        self.pending = None;
        AccessPointEvent::RollbackRequired(self.rollback().expect("admitted key transaction"))
    }
}
