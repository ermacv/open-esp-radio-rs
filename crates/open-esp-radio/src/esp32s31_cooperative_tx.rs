//! Cooperative TX register access for one task-owned S31 radio.
//!
//! A TX future must not retain a mutable PAC borrow while it waits for a
//! completion interrupt. The vendor FIQ services RX-success before
//! TX-complete, so a single-task Rust runtime needs to let its RX bottom half
//! borrow the same register owner between finite TX hardware transactions.

use core::cell::RefCell;

use open_esp_radio_hal_esp32s31::RadioRegisters;
use open_esp_radio_mac_esp32s31::{registers::Mmio, tx::TxHardware, tx_ampdu::HtAmpduHardware};
use open_esp_radio_pac_esp32s31::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHeTxVectorSnapshot,
    MacHtAmpduCompletionRegisters, MacHtTxProgram, MacLegacyTxProgram, MacTxCompletionRegisters,
};

/// Short-lived TX facade over the task's sole [`RadioRegisters`] owner.
///
/// Every trait method borrows the register owner only for one synchronous
/// hardware transaction. Consequently an async TX operation may keep this
/// facade across `.await` without keeping the PAC mutably borrowed; the same
/// task can service a pending RX-success edge through the original
/// [`RefCell`].
///
/// A dynamic borrow failure identifies an accidental overlap of two
/// synchronous register transactions. This type neither steals peripherals
/// nor creates another `RadioRegisters` value.
///
/// SOURCE: complete `_oracles/libpp.a[wdev.o]::wDev_ProcessFiq` handles
/// RX-success bit `0x4000` before TX-complete bit `0x80`.
pub struct CooperativeTxHardware<'cell, 'registers> {
    registers: &'cell RefCell<&'registers mut RadioRegisters>,
}

impl<'cell, 'registers> CooperativeTxHardware<'cell, 'registers> {
    /// Borrow the one task-owned register cell for cooperative TX.
    pub const fn new(registers: &'cell RefCell<&'registers mut RadioRegisters>) -> Self {
        Self { registers }
    }
}

impl Mmio for CooperativeTxHardware<'_, '_> {
    fn read32(&mut self, register: open_esp_radio_pac_esp32s31::Register32) -> u32 {
        Mmio::read32(&mut **self.registers.borrow_mut(), register)
    }

    fn write32(&mut self, register: open_esp_radio_pac_esp32s31::Register32, value: u32) {
        Mmio::write32(&mut **self.registers.borrow_mut(), register, value);
    }

    fn fence(&mut self) {
        Mmio::fence(&mut **self.registers.borrow_mut());
    }
}

impl TxHardware for CooperativeTxHardware<'_, '_> {
    fn prepare_legacy_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool {
        TxHardware::prepare_legacy_tx(&mut **self.registers.borrow_mut(), queue, program)
    }

    fn start_legacy_tx(&mut self, queue: u8, plcp0: u32) {
        TxHardware::start_legacy_tx(&mut **self.registers.borrow_mut(), queue, plcp0);
    }

    fn prepare_ht_tx(&mut self, queue: u8, program: MacHtTxProgram) -> bool {
        TxHardware::prepare_ht_tx(&mut **self.registers.borrow_mut(), queue, program)
    }

    fn start_ht_tx(&mut self, queue: u8, plcp0: u32) {
        TxHardware::start_ht_tx(&mut **self.registers.borrow_mut(), queue, plcp0);
    }

    fn prepare_he_tx(&mut self, queue: u8, program: MacHeTxProgram) -> bool {
        TxHardware::prepare_he_tx(&mut **self.registers.borrow_mut(), queue, program)
    }

    fn start_he_tx(&mut self, queue: u8, plcp0: u32) {
        TxHardware::start_he_tx(&mut **self.registers.borrow_mut(), queue, plcp0);
    }

    fn he_tx_vector_snapshot(&self, queue: u8) -> Option<MacHeTxVectorSnapshot> {
        TxHardware::he_tx_vector_snapshot(&**self.registers.borrow(), queue)
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        TxHardware::take_tx_completion(&mut **self.registers.borrow_mut(), queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        TxHardware::begin_tx_timeout_abort(&mut **self.registers.borrow_mut(), queue)
    }

    fn finish_tx_timeout_abort(&mut self, queue: u8) -> Option<bool> {
        TxHardware::finish_tx_timeout_abort(&mut **self.registers.borrow_mut(), queue)
    }

    fn detach_completed_tx(&mut self, queue: u8) -> bool {
        TxHardware::detach_completed_tx(&mut **self.registers.borrow_mut(), queue)
    }
}

impl HtAmpduHardware for CooperativeTxHardware<'_, '_> {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        HtAmpduHardware::take_ht_ampdu_completion(&mut **self.registers.borrow_mut(), queue)
    }

    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        HtAmpduHardware::prepare_he_trigger_based_queue(
            &mut **self.registers.borrow_mut(),
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        HtAmpduHardware::clear_he_trigger_based_queue(
            &mut **self.registers.borrow_mut(),
            reservation,
        );
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        HtAmpduHardware::he_trigger_based_queue_snapshot(&**self.registers.borrow(), reservation)
    }
}
