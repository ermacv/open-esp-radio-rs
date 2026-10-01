//! Transport for a bounded, replaceable incoming WNM request. Protocol-specific
//! admission and effects stay in the public owners. No retry extends a lease.
use crate::{
    Body, DialogEvent, Error, Identities, LinkIdentity, OperationId, Sender, Transmission,
    TxOutcome, deadline,
};
use oer_ieee80211_mac::roaming as wire;
use oer_time::{Duration, Instant};

pub(crate) struct Incoming<const B: usize> {
    pub(crate) id: OperationId,
    pub(crate) body: Body<B>,
    pub(crate) received: Instant,
    pub(crate) until: Instant,
}
struct Replay<const B: usize> {
    request: Body<B>,
    response: Body<B>,
    until: Instant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Reception {
    Ignored,
    Requested {
        id: OperationId,
        superseded: Option<OperationId>,
    },
    ReplayQueued {
        id: OperationId,
    },
}
pub(crate) struct PeerDialog<const B: usize> {
    pub(crate) ids: Identities,
    pub(crate) incoming: Option<Incoming<B>>,
    sender: Sender<B>,
    replay: Option<Replay<B>>,
    timeout: Duration,
    pub(crate) last_update: Instant,
}
impl<const B: usize> PeerDialog<B> {
    pub(crate) fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        if !crate::valid_peer_address(link.peer) {
            return Err(Error::InvalidTarget);
        }
        Ok(Self {
            ids: Identities::new(link),
            incoming: None,
            sender: Sender::new(link, timeout)?,
            replay: None,
            timeout,
            last_update: Instant::EPOCH,
        })
    }
    pub(crate) fn time(&self, now: Instant) -> Result<(), Error> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation)
        } else {
            Ok(())
        }
    }
    /// Call after the complete action has been parsed by its protocol owner.
    pub(crate) fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Reception, Error> {
        if link != self.ids.link {
            return Ok(Reception::Ignored);
        }
        self.time(now)?;
        let until = deadline(now, self.timeout)?;
        self.receive_until(link, bytes, now, until)
    }
    pub(crate) fn receive_until(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
        until: Instant,
    ) -> Result<Reception, Error> {
        if link != self.ids.link {
            return Ok(Reception::Ignored);
        }
        self.time(now)?;
        if until <= now {
            return Err(Error::NoPendingOperation);
        }
        if let Some(current) = &self.incoming
            && now < current.until
            && current.body.bytes()[wire::DIALOG_TOKEN_OFFSET] == bytes[wire::DIALOG_TOKEN_OFFSET]
        {
            if current.body.bytes() != bytes {
                return Err(Error::ConflictingDialog);
            }
            self.last_update = now;
            return Ok(Reception::Ignored);
        }
        if self.incoming.is_none()
            && let Some(replay) = &self.replay
            && now < replay.until
            && replay.request.bytes()[wire::DIALOG_TOKEN_OFFSET] == bytes[wire::DIALOG_TOKEN_OFFSET]
        {
            if replay.request.bytes() != bytes {
                return Err(Error::ConflictingDialog);
            }
            if self.sender.pending.is_some() {
                self.last_update = now;
                return Ok(Reception::Ignored);
            }
            let id = self.sender.start_using_until(
                now,
                replay.response.clone(),
                &mut self.ids,
                replay.until,
            )?;
            self.last_update = now;
            return Ok(Reception::ReplayQueued { id });
        }
        let body = Body::copy(bytes)?;
        let id = self.ids.issue()?;
        let superseded = self.incoming.as_ref().map(|old| old.id);
        self.sender.cancel();
        self.replay = None;
        self.incoming = Some(Incoming {
            id,
            body,
            received: now,
            until,
        });
        self.last_update = now;
        Ok(Reception::Requested { id, superseded })
    }
    pub(crate) fn reply(
        &mut self,
        id: OperationId,
        body: Body<B>,
        now: Instant,
        finish: bool,
    ) -> Result<OperationId, Error> {
        self.time(now)?;
        let current = self.incoming.as_ref().ok_or(Error::NoPendingOperation)?;
        if current.id != id {
            return Err(Error::WrongOperation);
        }
        if now >= current.until {
            return Err(Error::NoPendingOperation);
        }
        let tx = self
            .sender
            .start_using_until(now, body.clone(), &mut self.ids, current.until)?;
        if finish {
            let current = self.incoming.take().expect("checked request");
            self.replay = Some(Replay {
                request: current.body,
                response: body,
                until: current.until,
            });
        }
        self.last_update = now;
        Ok(tx)
    }
    /// A diagnostic-caused excursion preserves work, then returns to the same AP.
    pub(crate) fn rebind(&mut self, link: LinkIdentity, now: Instant) -> Result<(), Error> {
        self.time(now)?;
        if link.peer != self.ids.link.peer || link.generation == self.ids.link.generation {
            return Err(Error::WrongOperation);
        }
        if self.sender.pending.is_some() {
            return Err(Error::Busy);
        }
        self.sender = Sender::new(link, self.timeout)?;
        self.ids.link = link;
        self.last_update = now;
        Ok(())
    }
    pub(crate) fn send(&mut self, body: Body<B>, now: Instant) -> Result<OperationId, Error> {
        self.time(now)?;
        let id = self.sender.start_using(now, body, &mut self.ids)?;
        self.last_update = now;
        Ok(id)
    }
    pub(crate) fn reply_until(
        &mut self,
        id: OperationId,
        body: Body<B>,
        now: Instant,
        until: Instant,
    ) -> Result<OperationId, Error> {
        self.time(now)?;
        let current = self.incoming.as_ref().ok_or(Error::NoPendingOperation)?;
        if current.id != id {
            return Err(Error::WrongOperation);
        }
        let until = until.min(current.until);
        let tx = self
            .sender
            .start_using_until(now, body, &mut self.ids, until)?;
        self.last_update = now;
        Ok(tx)
    }
    pub(crate) fn transmission(&self) -> Option<Transmission<'_>> {
        self.sender.transmission()
    }
    pub(crate) fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        self.time(now)?;
        self.sender.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    pub(crate) fn completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DialogEvent, Error> {
        if self.sender.pending.as_ref().is_none_or(|p| p.id != id) {
            return Ok(DialogEvent::Ignored);
        }
        self.time(now)?;
        let event = self.sender.completed(id, outcome, now)?;
        self.last_update = now;
        Ok(event)
    }
    pub(crate) fn poll(
        &mut self,
        now: Instant,
    ) -> Result<(Option<OperationId>, Option<OperationId>), Error> {
        self.time(now)?;
        let expired_tx = self
            .sender
            .pending
            .as_ref()
            .filter(|p| now >= p.deadline)
            .map(|p| p.id);
        let expired = self
            .incoming
            .as_ref()
            .filter(|p| now >= p.until)
            .map(|p| p.id);
        if expired.is_some() {
            self.incoming = None;
        }
        self.sender.poll(now)?;
        if self.replay.as_ref().is_some_and(|p| now >= p.until) {
            self.replay = None;
        }
        self.last_update = now;
        Ok((expired, expired_tx))
    }
    pub(crate) fn cancel(&mut self) -> Option<OperationId> {
        self.sender.cancel();
        self.replay = None;
        self.incoming.take().map(|p| p.id)
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        [
            self.incoming.as_ref().map(|p| p.until),
            self.sender.next_deadline(),
            self.replay.as_ref().map(|p| p.until),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}
