//! Association transmission ownership. MLME owns the frame header, admission,
//! scheduling and IO; this owner bounds retries of one immutable IE transcript.
use super::*;

/// Caller-issued generation, unique across peer-slot/association reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExchangeId {
    pub addresses: Addresses,
    pub generation: u64,
}

/// Microsecond durations and a global retransmission budget, all explicit.
/// The original attempt is not counted in `max_retries`. Group changes also
/// consume this budget and retain the original absolute exchange deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    pub exchange_us: u64,
    pub retry_us: u64,
    pub max_retries: u8,
}

/// A single queue admission/completion; retries receive a different ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxTicket {
    exchange: ExchangeId,
    interface: crate::RsnInterface,
    serial: u16,
}
impl TxTicket {
    pub const fn exchange(self) -> ExchangeId {
        self.exchange
    }
}

/// OWE IE suffix for an Association Request/Response, not a complete frame.
/// Repeated transmissions retain the exact DH public key and security bytes.
pub struct Transmission<'a> {
    pub ticket: TxTicket,
    pub bytes: &'a [u8],
    pub retransmission: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TxPhase {
    Ready,
    Admitted,
    Waiting,
}
pub(super) struct Exchange {
    pub(super) id: ExchangeId,
    interface: crate::RsnInterface,
    policy: RetryPolicy,
    deadline: u64,
    now: u64,
    serial: u16,
    retries: u8,
    phase: TxPhase,
    admitted_serial: Option<u16>,
    transcript_start: u16,
    retry_at: Option<u64>,
}
impl Exchange {
    pub(super) fn new(
        id: ExchangeId,
        interface: crate::RsnInterface,
        policy: RetryPolicy,
        now: u64,
    ) -> Result<Self, Error> {
        id.addresses.validate()?;
        if id.generation == 0 || policy.exchange_us == 0 || policy.retry_us == 0 {
            return Err(Error::InvalidConfiguration);
        }
        let deadline = now
            .checked_add(policy.exchange_us)
            .ok_or(Error::DeadlineOverflow)?;
        Ok(Self {
            id,
            interface,
            policy,
            deadline,
            now,
            serial: 0,
            retries: 0,
            phase: TxPhase::Ready,
            admitted_serial: None,
            transcript_start: 0,
            retry_at: None,
        })
    }
    pub(super) fn observe(&mut self, now: u64) -> Result<(), Error> {
        if now < self.now {
            return Err(Error::TimeWentBackwards);
        }
        self.now = now;
        if now >= self.deadline {
            return Err(Error::TimedOut);
        }
        Ok(())
    }
    fn ticket(&self) -> TxTicket {
        TxTicket {
            exchange: self.id,
            interface: self.interface,
            serial: self.serial,
        }
    }
    pub(super) fn transmission<'a>(&self, bytes: &'a [u8]) -> Option<Transmission<'a>> {
        (self.phase == TxPhase::Ready).then(|| Transmission {
            ticket: self.ticket(),
            bytes,
            retransmission: self.serial != self.transcript_start,
        })
    }
    pub(super) fn admitted(&mut self, ticket: TxTicket, now: u64) -> Result<(), Error> {
        self.observe(now)?;
        if ticket != self.ticket() {
            return Err(Error::StaleCompletion);
        }
        if self.phase != TxPhase::Ready {
            return Err(Error::WrongPhase);
        }
        self.phase = TxPhase::Admitted;
        self.admitted_serial = Some(self.serial);
        Ok(())
    }
    pub(super) fn completed(&mut self, ticket: TxTicket, now: u64) -> Result<(), Error> {
        self.observe(now)?;
        if ticket != self.ticket() {
            return Err(Error::StaleCompletion);
        }
        if self.phase != TxPhase::Admitted {
            return Err(Error::WrongPhase);
        }
        // A retry beyond the exchange lifetime cannot run. Clamping its
        // wakeup also keeps an acknowledged completion valid near u64::MAX.
        let retry_at = Some(now.saturating_add(self.policy.retry_us).min(self.deadline));
        self.phase = TxPhase::Waiting;
        self.retry_at = retry_at;
        Ok(())
    }
    pub(super) fn accepts_response(&self, ticket: TxTicket) -> Result<(), Error> {
        if ticket.exchange != self.id || ticket.interface != self.interface {
            return Err(Error::WrongContext);
        }
        let admitted = self.admitted_serial.ok_or(Error::WrongPhase)?;
        if ticket.serial < self.transcript_start || ticket.serial > admitted {
            return Err(Error::StaleCompletion);
        }
        Ok(())
    }
    /// Reset admission even after a response supersedes an in-flight request.
    /// Its late completion now carries a stale ticket. No new entropy is drawn
    /// for a same-transcript retry; a group change is handled by the STA owner.
    pub(super) fn retry(&mut self, force: bool, now: u64) -> Result<bool, Error> {
        self.observe(now)?;
        if !force && !self.retry_at.is_some_and(|at| now >= at) {
            return Ok(false);
        }
        if self.retries == self.policy.max_retries {
            return if force {
                Err(Error::RetryExhausted)
            } else {
                Ok(false)
            };
        }
        self.retries += 1;
        self.serial += 1; // At most u8::MAX retries; no wrap is possible.
        self.phase = TxPhase::Ready;
        self.retry_at = None;
        Ok(true)
    }
    pub(super) fn new_transcript(&mut self) {
        self.admitted_serial = None;
        self.transcript_start = self.serial;
    }
    pub(super) fn next_deadline(&self, retry: bool) -> u64 {
        if retry && self.retries < self.policy.max_retries {
            self.retry_at
                .map_or(self.deadline, |at| at.min(self.deadline))
        } else {
            self.deadline
        }
    }
}
