//! Safe register-side authority required by the A-MPDU lifecycle.

use open_esp_radio_esp32s31_registers::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHtAmpduCompletionRegisters, RadioRegisters,
};

use crate::tx::TxHardware;

/// Hardware authority needed specifically by an aggregate completion.
///
/// A normal [`TxHardware`] completion may acknowledge the edge immediately.
/// A-MPDU must first sample the queue's three BlockAck words, so the ordering
/// is a separate trait operation and cannot accidentally be replaced with
/// the single-MPDU completion method.
pub trait HtAmpduHardware: TxHardware {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters>;

    /// Prepare one queue for a future AP Trigger before publishing TX enable.
    ///
    /// Implementations must validate every fallible input before the first
    /// hardware write so an error cannot leave a partially published queue.
    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError>;

    /// Remove Trigger eligibility only after DMA ownership has returned.
    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation);

    /// Read back a live Trigger queue while the aggregate owner still retains
    /// the reservation. Test doubles may return `None`.
    fn he_trigger_based_queue_snapshot(
        &self,
        _reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        None
    }
}

impl HtAmpduHardware for RadioRegisters {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        self.take_mac_ht_ampdu_completion(queue)
    }

    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        RadioRegisters::prepare_he_trigger_based_queue(
            self,
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        RadioRegisters::clear_he_trigger_based_queue(self, reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        Some(RadioRegisters::he_trigger_based_queue_snapshot(
            self,
            reservation,
        ))
    }
}
