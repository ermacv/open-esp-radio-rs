//! Cooperative TX register access for one task-owned S31 radio.
//!
//! A TX future must not retain a mutable PAC borrow while it waits for a
//! completion interrupt. The vendor FIQ services RX-success before
//! TX-complete, so a single-task Rust runtime needs to let its RX bottom half
//! borrow the same register owner between finite TX hardware transactions.

use core::cell::RefCell;

use open_esp_radio_esp32s31_hal::RadioRegisters;
use open_esp_radio_esp32s31_pac::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHeTxVectorSnapshot,
    MacHtAmpduCompletionRegisters, MacHtTxProgram, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacTxCompletionRegisters,
};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::CcmpKeyHardware,
    registers::Mmio,
    rx::RxDma,
    rx_ampdu_hw::{self, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::TxHardware,
    tx_ampdu::HtAmpduHardware,
};

use crate::connected_control::ConnectedControlHardware;

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

    /// Shared cell used by executor tasks which perform bounded cooperative
    /// register transactions.
    ///
    /// Returning the cell does not expose a second PAC owner: every access is
    /// still dynamically serialized by the same `RefCell`, and lifecycle code
    /// must stop those tasks before consuming this hardware facade.
    pub const fn register_cell(&self) -> &'cell RefCell<&'registers mut RadioRegisters> {
        self.registers
    }
}

impl Mmio for CooperativeTxHardware<'_, '_> {
    fn read32(&mut self, register: open_esp_radio_esp32s31_pac::Register32) -> u32 {
        Mmio::read32(&mut **self.registers.borrow_mut(), register)
    }

    fn write32(&mut self, register: open_esp_radio_esp32s31_pac::Register32, value: u32) {
        Mmio::write32(&mut **self.registers.borrow_mut(), register, value);
    }

    fn fence(&mut self) {
        Mmio::fence(&mut **self.registers.borrow_mut());
    }
}

impl CcmpKeyHardware for CooperativeTxHardware<'_, '_> {
    fn install_sta_ccmp_entry(&mut self, index: u8, words: [u32; 6]) -> MacKeyInstallOutcome {
        CcmpKeyHardware::install_sta_ccmp_entry(&mut **self.registers.borrow_mut(), index, words)
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        CcmpKeyHardware::clear_ccmp_entry(&mut **self.registers.borrow_mut(), index);
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

    fn abort_tx_collision(&mut self, queue: u8) -> bool {
        TxHardware::abort_tx_collision(&mut **self.registers.borrow_mut(), queue)
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

impl RxDma for CooperativeTxHardware<'_, '_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut **self.registers.borrow_mut())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut **self.registers.borrow_mut())
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut **self.registers.borrow_mut())
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut **self.registers.borrow_mut())
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut **self.registers.borrow_mut())
    }

    fn set_descriptor_high_window(&mut self, address_high: u16) {
        RxDma::set_descriptor_high_window(&mut **self.registers.borrow_mut(), address_high);
    }

    fn write_descriptor_base(&mut self, address: u32) {
        RxDma::write_descriptor_base(&mut **self.registers.borrow_mut(), address);
    }

    fn publish_walker_enable(&mut self) {
        RxDma::publish_walker_enable(&mut **self.registers.borrow_mut());
    }

    fn request_reload(&mut self) {
        RxDma::request_reload(&mut **self.registers.borrow_mut());
    }

    fn try_enable_walker(&mut self) -> bool {
        RxDma::try_enable_walker(&mut **self.registers.borrow_mut())
    }

    fn try_disable_walker(&mut self) -> bool {
        RxDma::try_disable_walker(&mut **self.registers.borrow_mut())
    }

    fn fence(&mut self) {
        RxDma::fence(&mut **self.registers.borrow_mut());
    }
}

impl ConnectedControlHardware for CooperativeTxHardware<'_, '_> {
    fn station_tsf(&mut self) -> u64 {
        self.registers.borrow_mut().station_tsf()
    }

    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::program(&mut self.registers.borrow_mut(), agreement)
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::clear(&mut self.registers.borrow_mut(), hardware_index)
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        let tid = MacHeTid::new(tid).ok_or(S31RxBlockAckAgreementError::Tid(tid))?;
        self.registers
            .borrow_mut()
            .set_he_trigger_based_tid_enabled(tid, enabled);
        Ok(())
    }
}
