use super::*;
use crate::{DialogEvent, Identities, Sender, Transmission, TxOutcome, deadline};
use oer_ieee80211_mac::roaming as wire;
use oer_time::{Duration, Instant};
struct Request<const BYTES: usize> {
    id: OperationId,
    body: Body<BYTES>,
    until: Instant,
}
struct Replay<const BYTES: usize> {
    request: Body<BYTES>,
    response: Body<BYTES>,
    until: Instant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmsApEvent {
    Ignored,
    Requested { id: OperationId },
    ReplayQueued { id: OperationId },
    Transmitted { id: OperationId },
    TxFailed { id: OperationId },
    TimedOut { id: OperationId },
}

/// Peer dialog owner over an independently owned BSS registry. Prepared
/// transactions are checked against both BSS and request identities before
/// publication, and a replay never commits registry changes a second time.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-directed-multicast-service-802-11v
pub struct DmsAccessPoint<const BYTES: usize> {
    bss: BssIdentity,
    ids: Identities,
    timeout: Duration,
    sender: Sender<BYTES>,
    request: Option<Request<BYTES>>,
    replay: Option<Replay<BYTES>>,
    last_update: Instant,
}
impl<const B: usize> DmsAccessPoint<B> {
    pub fn new(bss: BssIdentity, peer: LinkIdentity, timeout: Duration) -> Result<Self, DmsError> {
        if !crate::valid_peer_address(bss.bssid) || !crate::valid_peer_address(peer.peer) {
            return Err(Error::InvalidTarget.into());
        }
        deadline(Instant::EPOCH, timeout)?;
        Ok(Self {
            bss,
            ids: Identities::new(peer),
            timeout,
            sender: Sender::new(peer, timeout)?,
            request: None,
            replay: None,
            last_update: Instant::EPOCH,
        })
    }
    fn time(&self, now: Instant) -> Result<(), DmsError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    pub fn receive(
        &mut self,
        peer: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<DmsApEvent, DmsError> {
        if peer != self.ids.link {
            return Ok(DmsApEvent::Ignored);
        }
        self.time(now)?;
        let request = DmsRequest::parse(bytes)?;
        if let Some(replay) = &self.replay
            && now < replay.until
            && replay.request.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token
        {
            if replay.request.bytes() != bytes {
                return Err(Error::ConflictingDialog.into());
            }
            if self.sender.pending.is_some() {
                return Ok(DmsApEvent::Ignored);
            }
            let id = self.sender.start_using_until(
                now,
                replay.response.clone(),
                &mut self.ids,
                replay.until,
            )?;
            self.last_update = now;
            return Ok(DmsApEvent::ReplayQueued { id });
        }
        if let Some(current) = &self.request
            && now < current.until
        {
            if current.body.bytes() == bytes {
                return Ok(DmsApEvent::Ignored);
            }
            return Err(
                if current.body.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token {
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
        self.request = Some(Request { id, body, until });
        self.last_update = now;
        Ok(DmsApEvent::Requested { id })
    }
    pub fn request(&self) -> Option<(OperationId, DmsRequest<'_>)> {
        self.request.as_ref().map(|request| {
            (
                request.id,
                DmsRequest::parse(request.body.bytes()).expect("validated request"),
            )
        })
    }
    pub fn respond<const M: usize, const A: usize, const O: usize, const R: usize>(
        &mut self,
        registry: &mut DmsRegistry<M, A>,
        transaction: DmsTransaction<A, O, R>,
        now: Instant,
    ) -> Result<OperationId, DmsError> {
        self.time(now)?;
        let request = self.request.as_ref().ok_or(Error::NoPendingOperation)?;
        if registry.bss() != self.bss
            || request.id != transaction.request
            || request.body.bytes()[wire::DIALOG_TOKEN_OFFSET]
                != transaction.response().dialog_token
        {
            return Err(Error::WrongOperation.into());
        }
        if now >= request.until {
            return Err(Error::NoPendingOperation.into());
        }
        registry.validate_transaction(&transaction)?;
        let response = Body::copy(transaction.bytes())?;
        let original = request.body.clone();
        let until = request.until;
        let tx = self
            .sender
            .start_using_until(now, response.clone(), &mut self.ids, until)?;
        registry.apply(transaction);
        self.replay = Some(Replay {
            request: original,
            response,
            until,
        });
        self.request = None;
        self.last_update = now;
        Ok(tx)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.sender.transmission()
    }
    /// Autonomous termination is queued before committing removal. The caller
    /// supplies the actual last group sequence or its explicit unavailable code.
    pub fn terminate<const M: usize, const A: usize>(
        &mut self,
        registry: &mut DmsRegistry<M, A>,
        dms_id: u8,
        last_sequence: LastSequenceControl,
        now: Instant,
    ) -> Result<OperationId, DmsError> {
        self.time(now)?;
        if registry.bss() != self.bss {
            return Err(DmsError::StaleTransaction);
        }
        if self.request.is_some() || self.sender.pending.is_some() {
            return Err(Error::Busy.into());
        }
        let request = self.ids.issue()?;
        let transaction = registry.prepare_termination::<B>(request, dms_id, last_sequence)?;
        let body = Body::copy(transaction.bytes())?;
        let tx = self.sender.start_using(now, body, &mut self.ids)?;
        registry.apply(transaction);
        self.last_update = now;
        Ok(tx)
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), DmsError> {
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
    ) -> Result<DmsApEvent, DmsError> {
        if self
            .sender
            .pending
            .as_ref()
            .is_none_or(|pending| pending.id != id)
        {
            return Ok(DmsApEvent::Ignored);
        }
        self.time(now)?;
        let event = self.sender.completed(id, outcome, now)?;
        self.last_update = now;
        Ok(match event {
            DialogEvent::Transmitted => DmsApEvent::Transmitted { id },
            DialogEvent::TxFailed => DmsApEvent::TxFailed { id },
            DialogEvent::TimedOut => DmsApEvent::TimedOut { id },
            _ => DmsApEvent::Ignored,
        })
    }
    pub fn poll(&mut self, now: Instant) -> Result<DmsApEvent, DmsError> {
        self.time(now)?;
        if let Some(request) = &self.request
            && now >= request.until
        {
            let id = request.id;
            self.request = None;
            self.last_update = now;
            return Ok(DmsApEvent::TimedOut { id });
        }
        if let Some(pending) = &self.sender.pending
            && now >= pending.deadline
        {
            let id = pending.id;
            self.sender.poll(now)?;
            self.last_update = now;
            return Ok(DmsApEvent::TimedOut { id });
        }
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| now >= replay.until)
        {
            self.replay = None;
        }
        self.last_update = now;
        Ok(DmsApEvent::Ignored)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.request
            .as_ref()
            .map(|request| request.until)
            .into_iter()
            .chain(self.sender.next_deadline())
            .chain(self.replay.as_ref().map(|replay| replay.until))
            .min()
    }
}
