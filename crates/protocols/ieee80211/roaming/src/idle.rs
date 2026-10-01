//! BSS Max Idle leases and acknowledged keep-alive decisions.
use crate::{Error, Identities, LinkIdentity, OperationId, TxOutcome};
use oer_ieee80211_mac::management::IEEE_TIME_UNIT_MICROS;
use oer_ieee80211_mac::roaming::BssMaxIdle;
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleError {
    Protocol(Error),
    InvalidMargin,
    ProtectionUnavailable,
    UnsupportedOptions(u8),
}
impl From<Error> for IdleError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleEvent {
    Ignored,
    ActivityAccepted,
    KeepAlive {
        id: OperationId,
        protected: bool,
        until: Instant,
    },
    KeepAliveFailed {
        id: OperationId,
    },
    Expired {
        link: LinkIdentity,
    },
}
struct IdleLease {
    link: LinkIdentity,
    period: Duration,
    options: BssMaxIdle,
    until: Instant,
    last: Instant,
    expired: bool,
}
impl IdleLease {
    fn new(link: LinkIdentity, options: BssMaxIdle, now: Instant) -> Result<Self, IdleError> {
        options.validate().map_err(Error::from)?;
        if options.options & !BssMaxIdle::PROTECTED_KEEP_ALIVE != 0 {
            return Err(IdleError::UnsupportedOptions(options.options));
        }
        let period = Duration::from_micros(
            u64::from(options.period) * BssMaxIdle::PERIOD_UNIT_TU * IEEE_TIME_UNIT_MICROS,
        );
        let until = now.checked_add(period).ok_or(Error::TimeOverflow)?;
        Ok(Self {
            link,
            period,
            options,
            until,
            last: now,
            expired: false,
        })
    }
    fn time(&self, now: Instant) -> Result<(), IdleError> {
        if now < self.last {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    fn poll(&mut self, now: Instant) -> Result<IdleEvent, IdleError> {
        self.time(now)?;
        self.last = now;
        if now >= self.until && !self.expired {
            self.expired = true;
            return Ok(IdleEvent::Expired { link: self.link });
        }
        Ok(IdleEvent::Ignored)
    }
    fn activity(
        &mut self,
        link: LinkIdentity,
        protected: bool,
        now: Instant,
    ) -> Result<IdleEvent, IdleError> {
        if link != self.link {
            return Ok(IdleEvent::Ignored);
        }
        self.time(now)?;
        if self.expired || (self.options.protected_keep_alive() && !protected) {
            return Ok(IdleEvent::Ignored);
        }
        if now >= self.until {
            return self.poll(now);
        }
        let until = now.checked_add(self.period).ok_or(Error::TimeOverflow)?;
        self.until = until;
        self.last = now;
        Ok(IdleEvent::ActivityAccepted)
    }
}
#[derive(Clone, Copy)]
struct KeepAlive {
    id: OperationId,
    admitted: bool,
}
/// STA lease. Only acknowledged, STA-initiated data/management traffic resets
/// it; a queued or failed transmission cannot prove that the AP received it.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-bss-max-idle-period-802-11v
pub struct BssIdleStation {
    ids: Identities,
    lease: IdleLease,
    margin: Duration,
    pending: Option<KeepAlive>,
}
impl BssIdleStation {
    pub fn new(
        link: LinkIdentity,
        options: BssMaxIdle,
        protected_traffic_available: bool,
        margin: Duration,
        now: Instant,
    ) -> Result<Self, IdleError> {
        let lease = IdleLease::new(link, options, now)?;
        if margin == Duration::ZERO || margin >= lease.period {
            return Err(IdleError::InvalidMargin);
        }
        if options.protected_keep_alive() && !protected_traffic_available {
            return Err(IdleError::ProtectionUnavailable);
        }
        Ok(Self {
            ids: Identities::new(link),
            lease,
            margin,
            pending: None,
        })
    }
    /// An ordinary successful data/management exchange initiated by this STA.
    pub fn activity(
        &mut self,
        link: LinkIdentity,
        protected: bool,
        now: Instant,
    ) -> Result<IdleEvent, IdleError> {
        let event = self.lease.activity(link, protected, now)?;
        if event == IdleEvent::ActivityAccepted {
            self.pending = None;
        }
        Ok(event)
    }
    pub fn poll(&mut self, now: Instant) -> Result<IdleEvent, IdleError> {
        let event = self.lease.poll(now)?;
        if event != IdleEvent::Ignored {
            self.pending = None;
            return Ok(event);
        }
        if self.lease.expired
            || self.pending.is_some()
            || now.as_micros() < self.lease.until.as_micros() - self.margin.as_micros()
        {
            return Ok(IdleEvent::Ignored);
        }
        let id = self.ids.issue()?;
        self.pending = Some(KeepAlive {
            id,
            admitted: false,
        });
        Ok(IdleEvent::KeepAlive {
            id,
            protected: self.lease.options.protected_keep_alive(),
            until: self.lease.until,
        })
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), IdleError> {
        self.lease.time(now)?;
        let pending = self.pending.as_mut().ok_or(Error::NoPendingOperation)?;
        if pending.id != id {
            return Err(Error::WrongOperation.into());
        }
        if now >= self.lease.until {
            return Err(Error::NoPendingOperation.into());
        }
        if pending.admitted {
            return Err(Error::AlreadyAdmitted.into());
        }
        pending.admitted = true;
        self.lease.last = now;
        Ok(())
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<IdleEvent, IdleError> {
        let Some(pending) = self.pending.filter(|pending| pending.id == id) else {
            return Ok(IdleEvent::Ignored);
        };
        self.lease.time(now)?;
        if !pending.admitted {
            return Err(Error::NotAdmitted.into());
        }
        if now >= self.lease.until {
            return self.poll(now);
        }
        if outcome == TxOutcome::Acknowledged {
            self.activity(id.link, self.lease.options.protected_keep_alive(), now)
        } else {
            self.pending = None;
            self.lease.last = now;
            Ok(IdleEvent::KeepAliveFailed { id })
        }
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.lease.expired {
            None
        } else if self.pending.is_some() {
            Some(self.lease.until)
        } else {
            Some(Instant::from_micros(
                self.lease.until.as_micros() - self.margin.as_micros(),
            ))
        }
    }
}

/// AP lease for admitted data/management traffic in an exchange initiated by
/// the bound STA. ACK/RTS/CTS and AP-initiated exchanges are not idle activity.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-bss-max-idle-period-802-11v
pub struct BssIdleAccessPoint(IdleLease);
impl BssIdleAccessPoint {
    pub fn new(link: LinkIdentity, options: BssMaxIdle, now: Instant) -> Result<Self, IdleError> {
        Ok(Self(IdleLease::new(link, options, now)?))
    }
    pub fn activity(
        &mut self,
        link: LinkIdentity,
        protected: bool,
        now: Instant,
    ) -> Result<IdleEvent, IdleError> {
        self.0.activity(link, protected, now)
    }
    pub fn poll(&mut self, now: Instant) -> Result<IdleEvent, IdleError> {
        self.0.poll(now)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        (!self.0.expired).then_some(self.0.until)
    }
}

#[cfg(test)]
mod tests;
