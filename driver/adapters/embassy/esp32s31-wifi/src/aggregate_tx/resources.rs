use super::*;

impl<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub(super) fn cancel_prepared(&mut self) {
        if let Some(cookie) = self.cookie.take() {
            let _ = self.ampdu.cancel(cookie);
        }
        self.release_frames();
        self.active = ConnectedTxActive::Idle;
    }

    pub(super) fn release_completed(&mut self) -> Result<(), AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        self.ampdu.release_completed(cookie)?;
        self.cookie = None;
        self.release_frames();
        Ok(())
    }

    pub(super) fn release_frames(&mut self) {
        if self.ampdu.release_free_backings().is_err() {
            self.ampdu.forget_backings();
        }
    }

    pub(super) fn forget_frames(&mut self) {
        self.ampdu.forget_backings();
    }

    pub(super) fn reset_required(
        &mut self,
        reason: AggregateTxResetReason,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        self.ampdu.require_reset(cookie)?;
        self.forget_frames();
        Err(AggregateTxError::RadioResetRequired(reason))
    }
}

impl<
    M: RawMutex,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> Drop
    for Esp32s31ConnectedTx<
        '_,
        '_,
        '_,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn drop(&mut self) {
        if !self.ordinary.is_present() || !self.ampdu.is_present() {
            return;
        }
        match self.ampdu.state() {
            TxSlotState::Free => self.release_frames(),
            TxSlotState::Reserved => {
                if self
                    .cookie
                    .is_some_and(|cookie| self.ampdu.cancel(cookie).is_ok())
                {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::Completed => {
                if self
                    .cookie
                    .is_some_and(|cookie| self.ampdu.release_completed(cookie).is_ok())
                {
                    self.release_frames();
                } else {
                    self.forget_frames();
                }
            }
            TxSlotState::HardwareOwned | TxSlotState::ResetRequired => self.forget_frames(),
        }
    }
}
