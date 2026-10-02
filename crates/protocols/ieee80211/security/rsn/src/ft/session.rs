//! Bounded operation ownership shared by FT role procedures, not k/v dialogs.
use super::protocol::Packet;
use super::*;

/// Caller-issued association/peer generation; it must not be reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionIdentity {
    pub station: MacAddress,
    pub current_ap: MacAddress,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId {
    pub session: SessionIdentity,
    serial: u32,
}

/// All durations are explicit, nonzero monotonic microseconds. Zero retries
/// means one original transmission, followed by its complete response window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeouts {
    pub authentication_us: u64,
    pub reassociation_us: u64,
    pub commit_us: u64,
    pub retry_interval_us: u64,
    pub max_retries: u8,
}
impl Timeouts {
    pub fn validate(self) -> Result<Self, Error> {
        if [
            self.authentication_us,
            self.reassociation_us,
            self.commit_us,
            self.retry_interval_us,
        ]
        .contains(&0)
        {
            return Err(Error::InvalidConfiguration);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    OverAir,
    OverDs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameKind {
    Authentication,
    Action,
    /// Only the IE list; the association owner supplies rates, capabilities,
    /// listen interval/current AP or response status/AID and the MAC header.
    ReassociationElements,
}

pub struct Transmission<'a> {
    pub id: OperationId,
    pub destination: MacAddress,
    pub kind: FrameKind,
    pub bytes: &'a [u8],
    pub retransmission: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOutcome {
    Acknowledged,
    Failed,
}

/// Result of a caller-owned atomic key/association transaction. A failed
/// transaction must have already rolled back every side effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    Committed,
    RolledBack,
}

pub(crate) struct Session {
    pub(crate) identity: SessionIdentity,
    pub(crate) timeouts: Timeouts,
    serial: u32,
    now: u64,
}
impl Session {
    pub(crate) fn new(
        identity: SessionIdentity,
        timeouts: Timeouts,
        now: u64,
    ) -> Result<Self, Error> {
        if identity.generation == 0
            || identity.current_ap == identity.station
            || identity.current_ap == [0; oer_ieee80211_mac::management::MAC_ADDRESS_LEN]
            || oer_ieee80211_mac::management::is_group_address(identity.current_ap)
            || oer_ieee80211_mac::management::is_group_address(identity.station)
            || identity.station == [0; oer_ieee80211_mac::management::MAC_ADDRESS_LEN]
        {
            return Err(Error::WrongPeer);
        }
        Ok(Self {
            identity,
            timeouts: timeouts.validate()?,
            serial: 0,
            now,
        })
    }
    pub(crate) fn observe(&mut self, now: u64) -> Result<(), Error> {
        if now < self.now {
            return Err(Error::TimeWentBackwards);
        }
        self.now = now;
        Ok(())
    }
    pub(crate) fn issue(&mut self) -> Result<OperationId, Error> {
        let serial = self
            .serial
            .checked_add(1)
            .ok_or(Error::OperationExhausted)?;
        self.serial = serial;
        Ok(OperationId {
            session: self.identity,
            serial,
        })
    }
    pub(crate) fn deadline(&self, now: u64, duration: u64, key_expiry: u64) -> Result<u64, Error> {
        if now >= key_expiry {
            return Err(Error::KeyExpired);
        }
        Ok(now
            .checked_add(duration)
            .ok_or(Error::DeadlineOverflow)?
            .min(key_expiry))
    }
}

pub(crate) struct Pending<const N: usize> {
    pub(crate) id: OperationId,
    pub(crate) packet: Packet<N>,
    pub(crate) destination: MacAddress,
    pub(crate) kind: FrameKind,
    pub(crate) deadline: u64,
    pub(crate) ever_admitted: bool,
    admitted: bool,
    completed: bool,
    acknowledged: bool,
    retry_at: Option<u64>,
    retries: u8,
    retransmission: bool,
}
impl<const N: usize> Pending<N> {
    pub(crate) fn new(
        session: &mut Session,
        packet: Packet<N>,
        destination: MacAddress,
        kind: FrameKind,
        deadline: u64,
    ) -> Result<Self, Error> {
        Ok(Self {
            id: session.issue()?,
            packet,
            destination,
            kind,
            deadline,
            ever_admitted: false,
            admitted: false,
            completed: false,
            acknowledged: false,
            retry_at: None,
            retries: session.timeouts.max_retries,
            retransmission: false,
        })
    }
    pub(crate) fn transmission(&self) -> Option<Transmission<'_>> {
        (!self.admitted).then_some(Transmission {
            id: self.id,
            destination: self.destination,
            kind: self.kind,
            bytes: self.packet.bytes(),
            retransmission: self.retransmission,
        })
    }
    pub(crate) fn admit(
        &mut self,
        session: &mut Session,
        id: OperationId,
        now: u64,
    ) -> Result<(), Error> {
        session.observe(now)?;
        if self.id != id {
            return Err(Error::StaleOperation);
        }
        if self.admitted {
            return Err(Error::WrongPhase);
        }
        if now >= self.deadline {
            return Err(Error::ExpiredOperation);
        }
        let retry_at = now
            .checked_add(session.timeouts.retry_interval_us)
            .ok_or(Error::DeadlineOverflow)?
            .min(self.deadline);
        self.admitted = true;
        self.ever_admitted = true;
        self.retry_at = Some(retry_at);
        Ok(())
    }
    pub(crate) fn complete(&mut self, id: OperationId, outcome: TxOutcome) -> bool {
        if self.id != id || !self.admitted || self.completed {
            return false;
        }
        self.completed = true;
        if outcome == TxOutcome::Acknowledged {
            self.acknowledged = true;
            self.retry_at = None;
        }
        self.acknowledged
    }
    pub(crate) fn poll(&mut self, session: &mut Session, now: u64) -> Result<(), Error> {
        session.observe(now)?;
        if now >= self.deadline {
            return Err(Error::ExpiredOperation);
        }
        if self.completed
            && !self.acknowledged
            && self.retry_at.is_some_and(|deadline| now >= deadline)
            && self.retries != 0
        {
            let id = session.issue()?;
            self.id = id;
            self.admitted = false;
            self.completed = false;
            self.retry_at = None;
            self.retries -= 1;
            self.retransmission = true;
        }
        Ok(())
    }
    pub(crate) fn next_deadline(&self) -> u64 {
        if self.retries == 0 || !self.completed {
            self.deadline
        } else {
            self.retry_at.unwrap_or(self.deadline).min(self.deadline)
        }
    }
}

/// Side effects may have been admitted. The owner stays in rollback until the
/// caller confirms their removal; it must not begin another transition yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rollback {
    pub id: OperationId,
    pub target: MacAddress,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serial_exhaustion_never_reuses_an_old_operation() {
        let identity = SessionIdentity {
            station: [2, 0, 0, 0, 0, 1],
            current_ap: [2, 0, 0, 0, 0, 2],
            generation: 1,
        };
        let timeouts = Timeouts {
            authentication_us: 100,
            reassociation_us: 100,
            commit_us: 100,
            retry_interval_us: 10,
            max_retries: 1,
        };
        let mut session = Session::new(identity, timeouts, 0).unwrap();
        session.serial = u32::MAX - 1;
        let last = session.issue().unwrap();
        assert_eq!(last.serial, u32::MAX);
        assert_eq!(session.issue(), Err(Error::OperationExhausted));
        assert_eq!(session.issue(), Err(Error::OperationExhausted));
    }
    #[test]
    fn retry_waits_for_terminal_io_completion_without_extending_the_deadline() {
        let identity = SessionIdentity {
            station: [2, 0, 0, 0, 0, 1],
            current_ap: [2, 0, 0, 0, 0, 2],
            generation: 1,
        };
        let timeouts = Timeouts {
            authentication_us: 100,
            reassociation_us: 100,
            commit_us: 100,
            retry_interval_us: 10,
            max_retries: 1,
        };
        let mut session = Session::new(identity, timeouts, 0).unwrap();
        let mut pending = Pending::<8>::new(
            &mut session,
            Packet::copy(b"frame").unwrap(),
            identity.station,
            FrameKind::Authentication,
            100,
        )
        .unwrap();
        let id = pending.id;
        pending.admit(&mut session, id, 0).unwrap();
        pending.poll(&mut session, 20).unwrap();
        assert!(pending.transmission().is_none());
        assert_eq!(pending.next_deadline(), 100);
        assert!(!pending.complete(id, TxOutcome::Failed));
        pending.poll(&mut session, 20).unwrap();
        assert_ne!(pending.transmission().unwrap().id, id);
        assert_eq!(pending.deadline, 100);
    }
}
