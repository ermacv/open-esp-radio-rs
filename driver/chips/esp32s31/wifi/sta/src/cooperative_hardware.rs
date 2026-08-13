//! Cooperative register access for one task-owned ESP32-S31 radio.
//!
//! A TX future must not retain a mutable PAC borrow while it waits for a
//! completion interrupt. The vendor FIQ services RX-success before
//! TX-complete, so a single-task Rust runtime needs to let its RX bottom half
//! borrow the same register owner between finite TX hardware transactions.

use open_esp_radio_esp32s31_hal::{RadioRegisters, wifi_mac::WifiMacHal};
use open_esp_radio_esp32s31_pac::{
    MacHe20PeerConfig, MacHe20PeerError, MacHeBeamformingReportProfile, MacHeErSuAckRateProfile,
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHeTxVectorSnapshot,
    MacHtAmpduCompletionRegisters, MacHtTxProgram, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacRxDmaSnapshot, MacStaReceivePolicySnapshot, MacTxCompletionRegisters, MacTxDetachOutcome,
    MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi::register_arena::{
    Esp32s31PublishedRadioRegisters, Esp32s31RadioRegistersAccess,
    Esp32s31RadioRegistersArenaError, Esp32s31ReclaimedRadioRegisters,
};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{CcmpKeyHardware, CryptoKeyError, StaGroupCcmpSlot, replace_sta_group_ccmp},
    he::He20PeerHardware,
    init::{StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate_control::BeamformingReportHardware,
    rx::{
        RxDma, RxDmaBinding, RxDmaCursorObservation, RxDmaReloadSettled, RxDmaWalkerEnabled,
        RxDmaWalkerStopped,
    },
    rx_ampdu_hw::{self, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxHardware},
    tx_ampdu::HtAmpduHardware,
};

use crate::connected_control_hardware::ConnectedControlHardware;

/// Short-lived radio facade over the task's sole [`RadioRegisters`] owner.
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
/// SOURCE: complete `libpp.a[wdev.o]::wDev_ProcessFiq` handles
/// RX-success bit `0x4000` before TX-complete bit `0x80`.
pub struct CooperativeRadioHardware<'arena> {
    registers: Esp32s31PublishedRadioRegisters<'arena>,
}

impl<'arena> CooperativeRadioHardware<'arena> {
    /// Borrow the one task-owned register cell for cooperative radio access.
    pub const fn new(registers: Esp32s31PublishedRadioRegisters<'arena>) -> Self {
        Self { registers }
    }

    /// Shared cell used by executor tasks which perform bounded cooperative
    /// register transactions.
    ///
    /// Returning the cell does not expose a second PAC owner: every access is
    /// still dynamically serialized by the same `RefCell`, and lifecycle code
    /// must stop those tasks before consuming this hardware facade.
    pub const fn register_access(&self) -> Esp32s31RadioRegistersAccess<'arena> {
        self.registers.access()
    }

    /// Read the associated-STA RX policy through the same serialized PAC
    /// owner used by the live service.
    pub fn sta_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        self.registers.borrow().sta_receive_policy_snapshot()
    }

    /// Read the live MAC RX walker frontier through the serialized PAC owner.
    pub fn mac_rx_dma_snapshot(&self) -> MacRxDmaSnapshot {
        self.registers.borrow().mac_rx_dma_snapshot()
    }

    /// Recover the exact PAC owner after every child task and synchronous
    /// register transaction has returned.
    pub fn try_into_registers(
        self,
    ) -> Result<RadioRegisters, (Self, Esp32s31RadioRegistersArenaError)> {
        match self.registers.try_reclaim() {
            Ok(registers) => Ok(registers),
            Err((registers, error)) => Err((Self { registers }, error)),
        }
    }

    /// Recover the PAC owner and its exact task-stable arena binding.
    pub fn try_into_reclaimed_registers(
        self,
    ) -> Result<Esp32s31ReclaimedRadioRegisters<'arena>, (Self, Esp32s31RadioRegistersArenaError)>
    {
        match self.registers.try_reclaim_with_republish() {
            Ok(reclaimed) => Ok(reclaimed),
            Err((registers, error)) => Err((Self { registers }, error)),
        }
    }
}

impl CcmpKeyHardware for CooperativeRadioHardware<'_> {
    fn install_sta_ccmp_entry(&mut self, index: u8, words: [u32; 6]) -> MacKeyInstallOutcome {
        CcmpKeyHardware::install_sta_ccmp_entry(&mut *self.registers.borrow_mut(), index, words)
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        CcmpKeyHardware::clear_ccmp_entry(&mut *self.registers.borrow_mut(), index);
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        CcmpKeyHardware::ccmp_entry_is_valid(&*self.registers.borrow(), index)
    }
}

impl He20PeerHardware for CooperativeRadioHardware<'_> {
    fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        He20PeerHardware::program_he20_peer(
            &mut *self.registers.borrow_mut(),
            config,
            rts_threshold,
        )
    }

    fn program_he20_association(
        &mut self,
        association_id: u16,
        minimum_mpdu_start_spacing: u8,
        bssid_index: u8,
    ) -> Result<(), MacHe20PeerError> {
        He20PeerHardware::program_he20_association(
            &mut *self.registers.borrow_mut(),
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
    }

    fn initialize_he_buffer_status_report(&mut self) {
        He20PeerHardware::initialize_he_buffer_status_report(&mut *self.registers.borrow_mut());
    }
}

impl BeamformingReportHardware for CooperativeRadioHardware<'_> {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        BeamformingReportHardware::set_he_beamforming_report_profile(
            &mut *self.registers.borrow_mut(),
            profile,
        );
    }

    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        BeamformingReportHardware::set_he_ersu_ack_rate_profile(
            &mut *self.registers.borrow_mut(),
            profile,
        );
    }
}

impl StaLinkRxPolicyHardware for CooperativeRadioHardware<'_> {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        let mut registers = self.registers.borrow_mut();
        let mut hal = WifiMacHal::new(&mut registers);
        StaLinkRxPolicyHardware::apply_sta_link_policy(&mut hal, bssid);
    }
}

impl StaNoiseFloorHardware for CooperativeRadioHardware<'_> {
    fn read_noise_floor_dbm(&self) -> i8 {
        StaNoiseFloorHardware::read_noise_floor_dbm(&*self.registers.borrow())
    }
}

impl TxHardware for CooperativeRadioHardware<'_> {
    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_legacy_tx(&mut *self.registers.borrow_mut(), dma, queue, program)
    }

    fn start_bound_legacy_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8, plcp0: u32) {
        TxHardware::start_bound_legacy_tx(&mut *self.registers.borrow_mut(), dma, queue, plcp0);
    }

    fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_ht_tx(&mut *self.registers.borrow_mut(), dma, queue, program)
    }

    fn start_bound_ht_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8, plcp0: u32) {
        TxHardware::start_bound_ht_tx(&mut *self.registers.borrow_mut(), dma, queue, plcp0);
    }

    fn prepare_bound_he_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_he_tx(&mut *self.registers.borrow_mut(), dma, queue, program)
    }

    fn start_bound_he_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8, plcp0: u32) {
        TxHardware::start_bound_he_tx(&mut *self.registers.borrow_mut(), dma, queue, plcp0);
    }

    fn he_tx_vector_snapshot(&self, queue: u8) -> Option<MacHeTxVectorSnapshot> {
        TxHardware::he_tx_vector_snapshot(&*self.registers.borrow(), queue)
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        TxHardware::take_tx_completion(&mut *self.registers.borrow_mut(), queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        TxHardware::begin_tx_timeout_abort(&mut *self.registers.borrow_mut(), queue)
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        TxHardware::with_tx_queue_detached(
            &mut *self.registers.borrow_mut(),
            queue,
            expected_descriptor_head,
            reason,
            detached,
        )
    }
}

impl HtAmpduHardware for CooperativeRadioHardware<'_> {
    fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        HtAmpduHardware::take_ht_ampdu_completion(&mut *self.registers.borrow_mut(), queue)
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
            &mut *self.registers.borrow_mut(),
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        HtAmpduHardware::clear_he_trigger_based_queue(
            &mut *self.registers.borrow_mut(),
            reservation,
        );
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        HtAmpduHardware::he_trigger_based_queue_snapshot(&*self.registers.borrow(), reservation)
    }
}

impl RxDma for CooperativeRadioHardware<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut *self.registers.borrow_mut())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut *self.registers.borrow_mut())
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut *self.registers.borrow_mut())
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        RxDma::with_ordered_cursor(&mut *self.registers.borrow_mut(), observed)
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut *self.registers.borrow_mut())
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut *self.registers.borrow_mut())
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_reload_settled(&mut *self.registers.borrow_mut(), settled)
    }

    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding, address_high: u16) {
        RxDma::set_descriptor_high_window(&mut *self.registers.borrow_mut(), binding, address_high);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding, address: u32) {
        RxDma::write_descriptor_base(&mut *self.registers.borrow_mut(), binding, address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding) {
        RxDma::publish_walker_enable(&mut *self.registers.borrow_mut(), binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding) {
        RxDma::request_reload(&mut *self.registers.borrow_mut(), binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_enabled(&mut *self.registers.borrow_mut(), binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_stopped(&mut *self.registers.borrow_mut(), stopped)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut *self.registers.borrow_mut());
    }
}

impl ConnectedControlHardware for CooperativeRadioHardware<'_> {
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

    fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        key_id: u8,
        temporal_key: &[u8; 16],
    ) -> Result<(), CryptoKeyError> {
        replace_sta_group_ccmp(
            &mut *self.registers.borrow_mut(),
            slot,
            key_id,
            temporal_key,
        )
    }
}
