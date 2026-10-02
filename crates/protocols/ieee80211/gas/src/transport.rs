use super::*;

pub(crate) struct Identities {
    next: u64,
}
impl Identities {
    pub(crate) const fn new() -> Self {
        Self { next: 0 }
    }
    fn serial(&mut self) -> Result<u64, Error> {
        let next = self.next.checked_add(1).ok_or(Error::IdentityExhausted)?;
        self.next = next;
        Ok(next)
    }
    pub(crate) fn dialog(&mut self, peer: PeerIdentity) -> Result<DialogId, Error> {
        Ok(DialogId {
            peer,
            serial: self.serial()?,
        })
    }
    pub(crate) fn tx(&mut self, dialog: DialogId) -> Result<TxId, Error> {
        Ok(TxId {
            dialog,
            serial: self.serial()?,
        })
    }
}

pub(crate) struct PendingTx<const N: usize> {
    pub(crate) id: TxId,
    pub(crate) body: Bytes<N>,
    pub(crate) admitted: bool,
}
impl<const N: usize> PendingTx<N> {
    pub(crate) fn new(
        ids: &mut Identities,
        dialog: DialogId,
        body: Bytes<N>,
    ) -> Result<Self, Error> {
        Ok(Self {
            id: ids.tx(dialog)?,
            body,
            admitted: false,
        })
    }
    pub(crate) fn transmission(&self, category: Category) -> Option<Transmission<'_>> {
        (!self.admitted).then_some(Transmission {
            id: self.id,
            category,
            body: self.body.bytes(),
        })
    }
    pub(crate) fn admit(&mut self, id: TxId) -> Result<(), Error> {
        if self.id != id {
            return Err(Error::WrongOperation);
        }
        if self.admitted {
            return Err(Error::AlreadyAdmitted);
        }
        self.admitted = true;
        Ok(())
    }
    pub(crate) fn matches_completion(&self, id: TxId) -> Result<bool, Error> {
        if self.id != id {
            return Ok(false);
        }
        if !self.admitted {
            return Err(Error::NotAdmitted);
        }
        Ok(true)
    }
}
