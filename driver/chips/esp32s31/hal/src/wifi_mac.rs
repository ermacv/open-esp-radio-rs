//! Wi-Fi-specific MAC operations reached from the vendor PHY ABI.
//!
//! These leaves are deliberately separate from the shared PHY modules: their
//! physical registers belong to the 802.11 MAC and are not reusable by
//! Bluetooth, BLE or IEEE 802.15.4 PHY paths.

use core::{
    cell::RefMut,
    ops::{Deref, DerefMut},
};

pub use crate::types::{
    MacApReceivePolicySnapshot, MacInterface, MacItwtClearIndex, MacKeyInstallOutcome, MacPti,
    MacRoleReceivePolicy, MacRxDmaSnapshot, MacRxPrimaryStatistics, MacRxStatisticsSnapshot,
    MacStaApReceivePlan, MacStaPolicyMode, MacStaReceivePolicySnapshot, MacTxPowerTable,
    MacTxPtiCount, MacTxPtiProgram, MacTxQueueIndex,
};
use crate::types::{
    MacHe20PeerConfig, MacHe20PeerError, MacHeBeamformingReportProfile, MacHeErSuAckRateProfile,
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHeTxVectorSnapshot,
    MacHtAmpduCompletionRegisters, MacHtTxProgram, MacKeyEntryIndex, MacLegacyTxProgram,
    MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    MacTxCompletionRegisters, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
use open_esp_radio_esp32s31_pac::{ColdRadioRegisters, RadioRegisters};

/// Complete identity of one hardware MAC interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacInterfaceIdentity {
    pub interface: MacInterface,
    pub address: [u8; 6],
    pub bssid: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdHandshakeOutcome {
    pub samples: u32,
    pub value: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdHandshakeTimeout {
    pub samples: u32,
    pub observed: u32,
}

/// Execute the finite cold-MAC handshake and interrupt cleanup sequence.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`, offsets
/// `0x00..0x3a`. The vendor waits forever; this HAL operation owns the finite
/// sample limit, polling, and multi-register cleanup order.
pub fn begin_cold_mac_handshake(
    registers: &mut ColdRadioRegisters,
    sample_limit: u32,
) -> Result<MacColdHandshakeOutcome, MacColdHandshakeTimeout> {
    registers.request_mac_cold_start();

    let mut samples = 0;
    let value = loop {
        let value = registers.sample_mac_cold_start();
        if value & 1 != 0 {
            break value;
        }
        samples += 1;
        if samples >= sample_limit {
            return Err(MacColdHandshakeTimeout {
                samples,
                observed: value,
            });
        }
    };

    registers.mask_all_mac_interrupts();
    registers.clear_all_mac_interrupts();
    Ok(MacColdHandshakeOutcome { samples, value })
}

/// Closed HAL authority for the one-way cold Wi-Fi MAC transition.
///
/// The capability is borrowed from `Radio<_, Powered>` and cannot be stored
/// beyond that lifecycle owner. It exposes reviewed cold transactions only;
/// neither the PAC owner nor arbitrary register access is available.
pub struct WifiMacColdHal<'registers> {
    registers: &'registers mut ColdRadioRegisters,
}

impl<'registers> WifiMacColdHal<'registers> {
    pub(crate) fn from_owned(registers: &'registers mut ColdRadioRegisters) -> Self {
        Self { registers }
    }

    pub fn initialize_antenna(&mut self) {
        self.registers.initialize_mac_antenna();
    }

    pub fn initialize_coex(
        &mut self,
        rx_ack: u8,
        wifi_default: u8,
        tb: [u8; 7],
        beamforming: [u8; 3],
        multi_target: [u8; 2],
    ) {
        self.registers
            .initialize_mac_coex(rx_ack, wifi_default, tb, beamforming, multi_target);
    }

    pub fn initialize_crypto_bypass(&mut self) {
        self.registers.initialize_mac_crypto_bypass();
    }

    pub fn enable_interrupts(&mut self, event_mask: super::MacInterruptMask) {
        self.registers.enable_mac_with_interrupt_mask(event_mask);
    }

    pub fn initialize_hal_tail(
        &mut self,
        event_mask: super::MacInterruptMask,
        slow_clock_calibration: u32,
    ) -> bool {
        self.registers
            .initialize_mac_hal_tail(event_mask, slow_clock_calibration)
    }

    pub fn begin_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdHandshakeOutcome, MacColdHandshakeTimeout> {
        begin_cold_mac_handshake(self.registers, sample_limit)
    }

    pub fn initialize_he_prefix(&mut self) {
        self.registers.initialize_mac_he_prefix();
    }

    pub fn initialize_tx_power(&mut self, table: &MacTxPowerTable) {
        self.registers.initialize_mac_tx_power(table);
    }

    pub fn initialize_he_suffix(&mut self) {
        self.registers.initialize_mac_he_suffix();
    }

    pub fn initialize_last_rx_buffer_table(&mut self) {
        self.registers.initialize_mac_last_rx_buffer();
    }

    pub fn initialize_rx_buffer_prefix(&mut self) {
        self.registers.initialize_mac_rx_buffer_prefix();
    }

    pub fn initialize_receive_policy(&mut self) {
        self.registers.initialize_cold_receive_policy();
    }

    pub fn initialize_txrx_prefix(&mut self) {
        self.registers.initialize_mac_txrx_prefix();
    }

    pub fn initialize_txrx_callbacks(&mut self, delay_slot: u8) -> bool {
        self.registers.initialize_mac_txrx_callbacks(delay_slot)
    }

    pub fn initialize_txrx_suffix(&mut self) {
        self.registers.initialize_mac_txrx_suffix();
    }

    pub fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.registers
            .program_receive_interface_address(interface, address);
    }

    pub fn disable_phy_low_rate(&mut self) {
        self.registers.configure_phy_low_rate(false);
    }

    pub fn configure_open_promiscuous_receive(&mut self) {
        self.registers.configure_open_mac_promiscuous_receive();
    }
}

/// Closed HAL authority for reviewed runtime Wi-Fi MAC transactions.
///
/// This type intentionally exposes neither the contained [`RadioRegisters`]
/// nor `Deref`. LMAC code can request only the finite operations defined here.
enum WifiMacRegisters<'registers> {
    Owned(&'registers mut RadioRegisters),
    Published(RefMut<'registers, RadioRegisters>),
}

impl Deref for WifiMacRegisters<'_> {
    type Target = RadioRegisters;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(registers) => registers,
            Self::Published(registers) => registers,
        }
    }
}

impl DerefMut for WifiMacRegisters<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Owned(registers) => registers,
            Self::Published(registers) => registers,
        }
    }
}

pub struct WifiMacHal<'registers> {
    registers: WifiMacRegisters<'registers>,
}

impl<'registers> WifiMacHal<'registers> {
    pub(crate) fn from_owned(registers: &'registers mut RadioRegisters) -> Self {
        Self {
            registers: WifiMacRegisters::Owned(registers),
        }
    }

    pub(crate) fn from_published(registers: RefMut<'registers, RadioRegisters>) -> Self {
        Self {
            registers: WifiMacRegisters::Published(registers),
        }
    }

    pub fn configure_open_promiscuous_receive(&mut self) {
        self.registers.configure_open_mac_promiscuous_receive();
    }

    /// Read the calibrated baseband noise-floor observation used by link
    /// policy. This exposes the measured value, not the underlying register
    /// owner or encoding.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        self.registers.read_noise_floor_dbm()
    }

    /// Begin the reviewed no-power-save MAC quiesce sequence before PHY
    /// retuning. Sequencing belongs to the HAL; the PAC method is only the
    /// register-local RMW transaction.
    pub fn request_channel_stop(&mut self) {
        self.registers.request_mac_channel_stop_without_power_save();
    }

    /// Sample the hardware activity field used by the bounded HAL poll.
    pub fn channel_active_state(&self) -> u8 {
        self.registers.mac_channel_active_state()
    }

    /// Finish a channel switch and return the selected REGDMA link for
    /// diagnostics. Delays and polling remain outside the PAC.
    pub fn restart_after_channel_switch(&mut self) -> u8 {
        // SOURCE: complete `ic_mac_init -> hal_mac_init ->
        // pwr_hal_select_wifimac_regdma_link` in
        // `BLOB_LIBPP_MAC_CHANNEL_SWITCH`. This is intentionally a HAL
        // sequence: the PAC exposes only the two register-local operations.
        self.registers.resume_mac_channel_without_power_save();
        self.registers.select_wifi_no_power_save_regdma_link();
        self.registers.wifi_mac_regdma_link()
    }

    /// Publish the receive address and BSSID using two complete vendor leaves.
    pub fn program_interface_identity(&mut self, identity: MacInterfaceIdentity) {
        self.registers
            .program_receive_interface_address(identity.interface, identity.address);
        self.registers
            .program_interface_bssid(identity.interface, identity.bssid);
    }

    /// Apply one exact reviewed role-policy transaction.
    pub fn configure_role_receive_policy(&mut self, policy: MacRoleReceivePolicy) {
        self.registers.apply_role_receive_policy(policy);
    }

    pub fn configure_station_receive_policy(&mut self, bssid: [u8; 6]) {
        self.registers.apply_sta_link_receive_policy(bssid);
    }

    /// Apply only the exact vendor policy-six register transaction.
    /// Connected-STA entry normally uses [`Self::configure_station_receive_policy`]
    /// so the open scan/sniffer frontier is closed first.
    pub fn configure_station_policy_six(&mut self, bssid: [u8; 6], mode: MacStaPolicyMode) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::Station { bssid, mode });
    }

    pub fn configure_access_point_receive_policy(&mut self, address: [u8; 6]) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::AccessPoint { address });
    }

    /// Program both reviewed receive contexts as one register composition.
    ///
    /// This does not claim simultaneous runtime ownership or select a channel.
    pub fn configure_sta_ap_receive_plan(&mut self, plan: MacStaApReceivePlan) {
        self.registers.apply_sta_ap_receive_plan(plan);
    }

    pub fn station_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        self.registers.sta_receive_policy_snapshot()
    }

    pub fn access_point_receive_policy_snapshot(&self) -> MacApReceivePolicySnapshot {
        self.registers.ap_receive_policy_snapshot()
    }

    pub fn receive_statistics_snapshot(&self) -> MacRxStatisticsSnapshot {
        self.registers.rx_statistics_snapshot()
    }

    /// Read the bounded hardware-owned RX walker projection without exposing
    /// the PAC owner to diagnostics or runtime integration code.
    pub fn receive_dma_snapshot(&self) -> MacRxDmaSnapshot {
        self.registers.mac_rx_dma_snapshot()
    }

    /// Install one reviewed station key-table image. Slot selection and key
    /// lifetime remain driver policy; this method owns only the finite MMIO
    /// transaction and rejects indices outside the generated table.
    pub fn install_station_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; 6],
    ) -> MacKeyInstallOutcome {
        let Some(index) = MacKeyEntryIndex::new(u32::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        self.registers.install_sta_ccmp_key_entry(index, words)
    }

    /// Install one reviewed access-point key-table image. Association/key-ID
    /// mapping remains driver policy above this finite transaction.
    pub fn install_access_point_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; 6],
    ) -> MacKeyInstallOutcome {
        let Some(index) = MacKeyEntryIndex::new(u32::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        self.registers.install_ap_ccmp_key_entry(index, words)
    }

    /// Clear one key-table entry when its generated index is valid.
    pub fn clear_ccmp_entry(&mut self, index: u8) {
        if let Some(index) = MacKeyEntryIndex::new(u32::from(index)) {
            self.registers.clear_mac_key_entry(index);
        }
    }

    /// Observe key-table validity without exposing the table owner.
    pub fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        MacKeyEntryIndex::new(u32::from(index))
            .map(|index| self.registers.mac_key_entry_is_valid(index))
    }

    /// Stop the access-point TSF using the complete reviewed vendor leaf.
    pub fn stop_access_point_tsf(&mut self) {
        self.registers.stop_softap_tsf();
    }

    /// Start a new access-point TSF epoch through the reviewed selector-zero
    /// transaction. No raw timestamp word or register image crosses the HAL.
    pub fn reset_and_start_access_point_tsf(&mut self) {
        self.registers.reset_and_start_softap_tsf();
    }

    /// Publish the complete two-edge receive-beacon PTI transaction.
    pub fn set_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        self.registers.set_rx_beacon_pti(beacon, shared);
    }

    /// Publish the complete receive-beacon PTI clear edge.
    pub fn clear_rx_beacon_pti(&mut self) {
        self.registers.clear_rx_beacon_pti();
    }

    /// Publish the complete two-edge individual-TWT PTI transaction.
    pub fn set_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        self.registers.set_itwt_pti(argument_is_zero, shared);
    }

    /// Publish one bounded individual-TWT clear request.
    pub fn clear_itwt_pti(&mut self, index: MacItwtClearIndex) {
        self.registers.clear_itwt_pti(index);
    }

    /// Publish the complete scheduler and queue-vector PTI transaction.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        self.registers.set_tx_pti(queue, program);
    }

    /// Program the reviewed HE20 peer register image.
    pub fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        self.registers.program_he20_peer(config, rts_threshold)
    }

    /// Program the reviewed HE20 association fields.
    pub fn program_he20_association(
        &mut self,
        association_id: u16,
        minimum_mpdu_start_spacing: u8,
        bssid_index: u8,
    ) -> Result<(), MacHe20PeerError> {
        self.registers.program_he20_association(
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
    }

    /// Initialize the reviewed HE buffer-status-report register state.
    pub fn initialize_he_buffer_status_report(&mut self) {
        self.registers.initialize_he_buffer_status_report();
    }

    /// Publish the reviewed HE beamforming report-rate register image.
    pub fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        self.registers.set_he_beamforming_report_profile(profile);
    }

    /// Publish the matching reviewed ER-SU ACK-rate register image.
    pub fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        self.registers.set_he_ersu_ack_rate_profile(profile);
    }

    pub fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.registers
            .prepare_bound_legacy_mac_tx(dma, queue, program)
    }

    pub fn start_bound_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        self.registers.start_bound_mac_tx(dma, queue);
    }

    pub fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        self.registers.prepare_bound_ht_mac_tx(dma, queue, program)
    }

    pub fn prepare_bound_he_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        self.registers.prepare_bound_he_mac_tx(dma, queue, program)
    }

    pub fn he_tx_vector_snapshot(&self, queue: u8) -> MacHeTxVectorSnapshot {
        self.registers.he_mac_tx_vector_snapshot(queue)
    }

    pub fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        self.registers.take_mac_tx_completion(queue)
    }

    pub fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        self.registers.begin_mac_tx_timeout_abort(queue)
    }

    pub fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        self.registers.with_detached_mac_tx(queue, reason, detached)
    }

    pub fn take_ht_ampdu_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        self.registers.take_mac_ht_ampdu_completion(queue)
    }

    pub fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        self.registers.prepare_he_trigger_based_queue(
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    pub fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        self.registers.clear_he_trigger_based_queue(reservation);
    }

    pub fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> MacHeTriggerTxQueueSnapshot {
        self.registers.he_trigger_based_queue_snapshot(reservation)
    }

    pub fn rx_buffer_full_count(&mut self) -> u16 {
        self.registers.mac_rx_buffer_full_count()
    }

    pub fn rx_last_descriptor_low(&mut self) -> u32 {
        self.registers.mac_rx_last_descriptor_low()
    }

    pub fn rx_next_descriptor_low(&mut self) -> u32 {
        self.registers.mac_rx_next_descriptor_low()
    }

    pub fn rx_walker_enabled(&mut self) -> bool {
        self.registers.mac_rx_walker_enabled()
    }

    pub fn rx_reload_pending(&mut self) -> bool {
        self.registers.mac_rx_reload_pending()
    }

    pub fn set_rx_descriptor_high_window(&mut self, range: &StableDmaRange<'_>, address_high: u16) {
        self.registers
            .set_mac_rx_descriptor_high_window(range, address_high);
    }

    pub fn write_rx_descriptor_base(&mut self, range: &StableDmaRange<'_>, address: u32) {
        self.registers.write_mac_rx_descriptor_base(range, address);
    }

    pub fn publish_rx_walker_enable(&mut self, range: &StableDmaRange<'_>) {
        self.registers.publish_mac_rx_walker_enable(range);
    }

    pub fn request_rx_descriptor_reload(&mut self, range: &StableDmaRange<'_>) {
        self.registers.request_mac_rx_descriptor_reload(range);
    }

    pub fn try_enable_rx_walker(&mut self, range: &StableDmaRange<'_>) -> bool {
        self.registers.try_enable_mac_rx_walker(range)
    }

    pub fn try_disable_rx_walker(&mut self) -> bool {
        self.registers.try_disable_mac_rx_walker()
    }

    pub fn order_device_accesses(&mut self) {
        self.registers.order_device_accesses();
    }

    pub fn station_tsf(&mut self) -> u64 {
        self.registers.station_tsf()
    }

    pub fn program_rx_block_ack_entry(
        &mut self,
        index: MacRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        self.registers.program_rx_block_ack_entry(
            index,
            interface,
            peer,
            tid,
            starting_sequence,
            window,
        );
    }

    pub fn delete_rx_block_ack_entry(&mut self, index: MacRxBlockAckEntryIndex) {
        self.registers.delete_rx_block_ack_entry(index);
    }

    pub fn set_he_trigger_based_tid_enabled(&mut self, tid: MacHeTid, enabled: bool) {
        self.registers
            .set_he_trigger_based_tid_enabled(tid, enabled);
    }
}

/// Execute the exact role-policy HAL transaction in an isolated validation
/// image without exposing the PAC owner to the probe crate.
///
/// Only singleton acquisition is replaced. Register ownership and the named
/// production HAL operation are unchanged.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_configure_role_receive_policy(policy: MacRoleReceivePolicy) {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    WifiMacHal::from_owned(&mut registers).configure_role_receive_policy(policy);
}

/// Apply complete rev0 ROM `phy_enable_cca` or `phy_disable_cca`.
#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
fn set_cca_enabled(registers: &mut RadioRegisters, enabled: bool) {
    registers.set_phy_wifi_cca_enabled(enabled);
}

/// Apply complete rev0 ROM `phy_sifs_reg_init`.
#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
fn initialize_sifs(registers: &mut RadioRegisters) {
    registers.initialize_phy_wifi_sifs();
}

#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
#[doc(hidden)]
pub fn validation_set_cca_enabled(enabled: bool) {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    set_cca_enabled(&mut registers, enabled);
}

#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
#[doc(hidden)]
pub fn validation_initialize_sifs() {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    initialize_sifs(&mut registers);
}
