use std::{cell::RefCell, rc::Rc};

use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma};
use open_esp_radio_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHtTxProgram, MacInterface, MacInterruptEvents, MacInterruptMask,
    MacInterruptObservation, MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionObservation,
    MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi_dma::descriptor::{
    BIT_30, BIT_31, DESCRIPTOR_BYTES, DMA_LOW, Descriptor, LENGTH_SHIFT, descriptor_address_valid,
    dma_range_valid, length, rx_armed_word, rx_rearm_word, size, tx_owned_word,
};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{
        CcmpKeyHardware, CryptoKeyError, clear_sta_ccmp_slots, install_sta_group_ccmp,
        install_sta_pairwise_ccmp,
    },
    init::{
        MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacColdAntennaHardware, MacColdCoexHardware,
        MacColdCoexPti, MacColdCryptoHardware, MacColdEnableHardware, MacColdHalTailHardware,
        MacColdHandshakeHardware, MacColdHeHardware, MacColdLastRxBufferHardware,
        MacColdRxBufferHardware, MacColdRxPolicyHardware, MacColdStartConfig, MacColdStartError,
        MacColdStartOutcome, MacColdTxRxHardware, MacDelayEntropy, MacDelaySlot,
        MacInterfaceAddressHardware, MacLowRateHardware, MacSharedClockHardware,
        MacSlowClockCalibration, MacSlowClockCalibrationSource, MacSnifferHardware, MacTxPowerPair,
        MacTxPowerSource, MacTxPowerTable, StaLinkRxPolicyHardware, activate_promiscuous_receive,
        configure_sta_link_receive_policy, initialize_wifi_mac,
    },
    irq::{
        EVENT_RX_SUCCESS, EVENT_TX_COMPLETE, IrqDisposition, IrqState, IrqWork, MacInterrupt,
        handle_mac_irq,
    },
    rate_schedule::{RateScheduleKind, RateScheduleRef},
    rx::{
        HeBandwidth, HeGuardIntervalAndLtf, HeMuBandwidth, HeMuSignal, HeSuSignal,
        HeTriggerBasedSignal, HtDuplicateRxClassification, INGRESS_STRICT_DUMP,
        INGRESS_STRICT_RXEND, RX_BUFFER_SENTINEL, RxBasebandFormat, RxDma, RxDmaBinding,
        RxDmaCursorObservation, RxDmaWalkerStopped, RxError, RxHe20MuSigBUsersError,
        RxIngressConfig, RxPhyInfo, RxReloadObservation, RxRingError, RxRingLive, RxRingStopped,
        RxSegment, build_cold_ring, decode_normalized_rx_metadata, decode_rx_he_mu_sig_b,
        decode_rx_phy_info, disable_receive, enable_receive, extract_ccmp_data, extract_control,
        extract_data, extract_management, first_segment_layout, prepare_recycled_buffer,
        publish_cold_ring, rearm_descriptor, view_normalized_rx_frame,
    },
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeBccDcmMcs, HeEdcaTxopLimit, HeFecCoding, HeLdpcDcmMcs,
        HeMcs, HeRate, HeResourceUnit, HeTriggerScheduledRate, HeTriggerScheduledRateError,
        HtAmpduDensity, HtAmpduTxConfig, HtChannelWidth, HtDuplicateCertificationRequest,
        HtDuplicateRate, HtDuplicateTxEvidenceGaps, HtDuplicateTxLinkCapabilities,
        HtDuplicateTxOracleField, HtDuplicateTxOracleGaps, HtDuplicateTxQualificationField,
        HtDuplicateTxQualificationGaps, HtDuplicateTxRejection, HtDuplicateTxSelection,
        HtDuplicateTxUnavailable, HtGuardInterval, HtMcs, HtPeerAmpduParameters,
        HtProtectionSpacing, HtRate, LegacyRate, LegacyTxConfig, LegacyTxQueue, TxCompletion,
        TxError, TxHardware, TxPhyRate, TxSlot, TxSlotState, select_esp32s31_ht_duplicate_tx,
    },
};
use open_esp_radio_ieee80211::he::{HeMuSigBMimoUser, HeMuSigBNonMimoUser, HeMuSigBUser};
use open_esp_radio_ieee80211::trigger::{
    parse_trigger_common_info, parse_trigger_frame, parse_trigger_user_spatial_stream,
};
use open_esp_radio_wifi_softmac::{MacRxEvidence, MacRxMetadata};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    BeginColdHandshake(u32),
    InitializeMacAntenna,
    InitializeHalTail(MacInterruptMask, MacSlowClockCalibration),
    InitializeColdCoex(MacColdCoexPti),
    InitializeCryptoBypass,
    InitializeLastRxBufferTable,
    InitializeTxRxPrefix,
    InitializeTxRxCallbacks(MacDelaySlot),
    InitializeTxRxSuffix,
    InitializeColdReceivePolicy,
    DisablePhyLowRate,
    EnableMacInterrupts(MacInterruptMask),
    ProgramInterfaceAddress(MacInterface, [u8; 6]),
    ApplyStaLinkPolicy([u8; 6]),
    InstallCcmp(MacInterface, u8, MacCcmpKeyIdentity),
    ClearCcmp(u8),
    InitializeHePrefix,
    InitializeTxPower(MacTxPowerTable),
    InitializeHeSuffix,
    RetainCoexistenceClock,
    ConfigureModemSourceClocks,
    EnableWifiMacClocks,
    SetWifiMacReset(bool),
    InitializeRxBufferPrefix,
    ConfigureRxDescriptorWindow,
    ObserveRxLastDescriptor,
    ObserveRxNextDescriptor,
    ObserveRxWalkerEnabled,
    ObserveRxReloadPending,
    PublishRxDescriptorBase(u32),
    PublishRxWalkerEnable,
    RequestRxReload,
    StopRxWalker,
    ConfigureOpenPromiscuousReceive,
    ReadInterruptStatus,
    AcknowledgeInterrupt,
    ForceTxCca,
    DisableTxQueue(u8),
    ReleaseTxCca,
    AcknowledgeTxEvent(u8, MacTxDetachReason),
    Fence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColdStartClockEdge {
    EnableWifiMacClocks,
    RetainCoexistenceClock,
    ConfigureModemSourceClocks,
    SetWifiMacReset(bool),
}

type ColdStartClockTrace = Rc<RefCell<Vec<ColdStartClockEdge>>>;

#[derive(Default)]
struct MockMmio {
    operations: Vec<Operation>,
    interrupt_status: MacInterruptObservation,
    cold_handshake_result: Option<Result<MacColdStartOutcome, MacColdStartError>>,
    cold_start_clock_trace: Option<ColdStartClockTrace>,
    tx_completions: [Option<MacTxCompletionObservation>; 4],
    tx_timeout_pending: [bool; 4],
    tx_collision_pending: [bool; 4],
    tx_queue_attached: [bool; 4],
    tx_detach_fails: [bool; 4],
    rx_last_descriptor_low: u32,
    rx_next_descriptor_low: u32,
    rx_walker_enabled: bool,
    rx_reload_pending: bool,
    rx_descriptor_base: u32,
    ccmp_valid: [bool; 25],
}

impl MockMmio {
    fn operations(&self) -> &[Operation] {
        &self.operations
    }

    fn record_fence(&mut self) {
        self.operations.push(Operation::Fence);
    }

    fn set_tx_timeout_pending(&mut self, queue: u8, pending: bool) {
        self.tx_timeout_pending[usize::from(queue)] = pending;
    }

    fn set_tx_collision_pending(&mut self, queue: u8, pending: bool) {
        self.tx_collision_pending[usize::from(queue)] = pending;
    }

    fn set_tx_queue_attached(&mut self, queue: u8, attached: bool) {
        self.tx_queue_attached[usize::from(queue)] = attached;
    }

    fn set_rx_last_descriptor_address(&mut self, address: u32) {
        self.rx_last_descriptor_low = rx_descriptor_low(address);
    }

    fn set_rx_last_descriptor_low(&mut self, address_low: u32) {
        self.rx_last_descriptor_low = address_low;
    }

    fn set_rx_next_descriptor_address(&mut self, address: u32) {
        self.rx_next_descriptor_low = rx_descriptor_low(address);
    }

    fn set_rx_next_descriptor_low(&mut self, address_low: u32) {
        self.rx_next_descriptor_low = address_low;
    }

    fn set_rx_walker_enabled(&mut self, enabled: bool) {
        self.rx_walker_enabled = enabled;
    }

    fn set_rx_reload_pending(&mut self, pending: bool) {
        self.rx_reload_pending = pending;
    }

    fn rx_descriptor_base(&self) -> u32 {
        self.rx_descriptor_base
    }

    fn set_tx_completion(&mut self, queue: u8, completion: MacTxCompletionObservation) {
        self.tx_completions[usize::from(queue)] = Some(completion);
    }

    fn record_clock_edge(&self, edge: ColdStartClockEdge) {
        if let Some(trace) = &self.cold_start_clock_trace {
            trace.borrow_mut().push(edge);
        }
    }
}

impl MacSharedClockHardware for MockMmio {
    fn retain_coexistence_clock(&mut self) {
        self.record_clock_edge(ColdStartClockEdge::RetainCoexistenceClock);
        self.operations.push(Operation::RetainCoexistenceClock);
    }

    fn configure_modem_source_clocks(&mut self) {
        self.record_clock_edge(ColdStartClockEdge::ConfigureModemSourceClocks);
        self.operations.push(Operation::ConfigureModemSourceClocks);
    }

    fn enable_wifi_mac_clocks(&mut self) {
        self.record_clock_edge(ColdStartClockEdge::EnableWifiMacClocks);
        self.operations.push(Operation::EnableWifiMacClocks);
    }

    fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.record_clock_edge(ColdStartClockEdge::SetWifiMacReset(asserted));
        self.operations.push(Operation::SetWifiMacReset(asserted));
    }
}

impl RxDma for MockMmio {
    fn last_descriptor_low(&mut self) -> u32 {
        self.operations.push(Operation::ObserveRxLastDescriptor);
        self.rx_last_descriptor_low
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.operations.push(Operation::ObserveRxNextDescriptor);
        self.rx_next_descriptor_low
    }

    fn next_descriptor(&mut self) -> open_esp_radio_esp32s31_wifi_dma::rx_dma::RxDmaNextDescriptor {
        open_esp_radio_esp32s31_wifi_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low(),
            false,
        )
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.record_fence();
        let next = self.next_descriptor_low();
        self.record_fence();
        observed(RxDmaCursorObservation::validation(last, next))
    }

    fn walker_enabled(&mut self) -> bool {
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        self.rx_walker_enabled
    }

    fn reload_pending(&mut self) -> bool {
        self.operations.push(Operation::ObserveRxReloadPending);
        self.rx_reload_pending
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending()).then(|| {
            settled(open_esp_radio_esp32s31_wifi_mac::rx::RxDmaReloadSettled::validation())
        })
    }

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding) {
        self.operations.push(Operation::ConfigureRxDescriptorWindow);
    }

    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.rx_descriptor_base = address;
        self.operations
            .push(Operation::PublishRxDescriptorBase(address));
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.rx_walker_enabled = true;
        self.operations.push(Operation::PublishRxWalkerEnable);
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.rx_reload_pending = true;
        self.operations.push(Operation::RequestRxReload);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        if self.rx_walker_enabled {
            return None;
        }
        self.rx_walker_enabled = true;
        self.operations.push(Operation::PublishRxWalkerEnable);
        self.record_fence();
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        self.rx_walker_enabled.then(|| {
            enabled(open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled::validation())
        })
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.rx_walker_enabled = false;
        self.operations.push(Operation::StopRxWalker);
        self.record_fence();
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        (!self.rx_walker_enabled).then(|| stopped(RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {
        self.record_fence();
    }
}

const fn rx_descriptor_low(address: u32) -> u32 {
    if address == 0 { 0 } else { address - DMA_LOW }
}

fn confirm_completed_unit_link_release<const COUNT: usize>(
    live: &mut RxRingLive<'_, COUNT>,
    mmio: &mut MockMmio,
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    last_descriptor_low: u32,
    descriptor_count: usize,
) {
    let last_descriptor_low = rx_descriptor_low(last_descriptor_low);
    assert!(
        !live.observe_completed_unit_link_release(mmio, last_descriptor_low, descriptor_count,),
        "the current LAST descriptor still owns its nonterminal link",
    );
    let tail_index = usize::try_from(
        (last_descriptor_low.wrapping_sub(rx_descriptor_low(descriptor_base))) / DESCRIPTOR_BYTES,
    )
    .unwrap();
    let later_index = (tail_index + 1) % COUNT;
    descriptors[later_index].write_word0(descriptors[later_index].word0() | BIT_30);
    let later_low = rx_descriptor_low(descriptor_base + later_index as u32 * DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_low(later_low);
    assert!(live.observe_completed_unit_link_release(mmio, later_low, descriptor_count,));
}

impl MacInterrupt for MockMmio {
    type Snapshot = MacInterruptObservation;

    fn status(&mut self) -> Self::Snapshot {
        self.operations.push(Operation::ReadInterruptStatus);
        self.interrupt_status
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        let _snapshot = snapshot;
        self.operations.push(Operation::AcknowledgeInterrupt);
        self.interrupt_status = MacInterruptObservation::default();
        self.record_fence();
    }
}

impl CcmpKeyHardware for MockMmio {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let Some(valid) = self.ccmp_valid.get_mut(usize::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        if *valid {
            return MacKeyInstallOutcome::Occupied;
        }
        *valid = true;
        self.operations.push(Operation::InstallCcmp(
            MacInterface::Station,
            index,
            identity,
        ));
        MacKeyInstallOutcome::Installed
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let Some(valid) = self.ccmp_valid.get_mut(usize::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        if *valid {
            return MacKeyInstallOutcome::Occupied;
        }
        *valid = true;
        self.operations.push(Operation::InstallCcmp(
            MacInterface::AccessPoint,
            index,
            identity,
        ));
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        if let Some(valid) = self.ccmp_valid.get_mut(usize::from(index)) {
            *valid = false;
            self.operations.push(Operation::ClearCcmp(index));
        }
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        self.ccmp_valid.get(usize::from(index)).copied()
    }
}

impl StaLinkRxPolicyHardware for MockMmio {
    fn apply_sta_link_policy(&mut self, bssid_address: [u8; 6]) {
        self.operations
            .push(Operation::ApplyStaLinkPolicy(bssid_address));
    }
}

impl MacInterfaceAddressHardware for MockMmio {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.operations
            .push(Operation::ProgramInterfaceAddress(interface, address));
    }
}

impl MacColdHandshakeHardware for MockMmio {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError> {
        self.operations
            .push(Operation::BeginColdHandshake(sample_limit));
        self.cold_handshake_result
            .unwrap_or(Err(MacColdStartError::HandshakeTimedOut {
                samples: sample_limit,
                sample_limit,
            }))
    }
}

impl MacSnifferHardware for MockMmio {
    fn configure_open_promiscuous_receive(&mut self) {
        self.operations
            .push(Operation::ConfigureOpenPromiscuousReceive);
    }

    fn disable_open_promiscuous_receive(&mut self) {}
}

impl MacColdCryptoHardware for MockMmio {
    fn initialize_crypto_bypass(&mut self) {
        self.operations.push(Operation::InitializeCryptoBypass);
    }
}

impl MacColdAntennaHardware for MockMmio {
    fn initialize_mac_antenna(&mut self) {
        self.operations.push(Operation::InitializeMacAntenna);
    }
}

impl MacColdHalTailHardware for MockMmio {
    fn initialize_hal_tail(
        &mut self,
        event_mask: MacInterruptMask,
        slow_clock_calibration: MacSlowClockCalibration,
    ) {
        self.operations.push(Operation::InitializeHalTail(
            event_mask,
            slow_clock_calibration,
        ));
    }
}

impl MacColdCoexHardware for MockMmio {
    fn initialize_cold_coex(&mut self, pti: MacColdCoexPti) {
        self.operations.push(Operation::InitializeColdCoex(pti));
    }
}

impl MacColdHeHardware for MockMmio {
    fn initialize_he_prefix(&mut self) {
        self.operations.push(Operation::InitializeHePrefix);
    }

    fn initialize_tx_power(&mut self, table: &MacTxPowerTable) {
        self.operations.push(Operation::InitializeTxPower(*table));
    }

    fn initialize_he_suffix(&mut self) {
        self.operations.push(Operation::InitializeHeSuffix);
    }
}

impl MacColdEnableHardware for MockMmio {
    fn enable_mac_interrupts(&mut self, event_mask: MacInterruptMask) {
        self.operations
            .push(Operation::EnableMacInterrupts(event_mask));
    }
}

impl MacColdLastRxBufferHardware for MockMmio {
    fn initialize_last_rx_buffer_table(&mut self) {
        self.operations.push(Operation::InitializeLastRxBufferTable);
    }
}

impl MacColdTxRxHardware for MockMmio {
    fn initialize_txrx_prefix(&mut self) {
        self.operations.push(Operation::InitializeTxRxPrefix);
    }

    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        self.operations
            .push(Operation::InitializeTxRxCallbacks(delay_slot));
    }

    fn initialize_txrx_suffix(&mut self) {
        self.operations.push(Operation::InitializeTxRxSuffix);
    }
}

impl MacColdRxPolicyHardware for MockMmio {
    fn initialize_cold_receive_policy(&mut self) {
        self.operations.push(Operation::InitializeColdReceivePolicy);
    }
}

impl MacLowRateHardware for MockMmio {
    fn disable_phy_low_rate(&mut self) {
        self.operations.push(Operation::DisablePhyLowRate);
    }
}

impl MacColdRxBufferHardware for MockMmio {
    fn initialize_rx_buffer_prefix(&mut self) {
        self.operations.push(Operation::InitializeRxBufferPrefix);
    }
}

impl TxHardware for MockMmio {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        true
    }

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacHtTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.record_fence();
        self.record_fence();
    }

    fn start_bound_ht_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.record_fence();
        self.record_fence();
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        self.tx_completions[usize::from(queue)].take()
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        if !self.tx_timeout_pending[usize::from(queue)] {
            return false;
        }
        self.operations.push(Operation::ForceTxCca);
        self.record_fence();
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let index = usize::from(queue);
        match reason {
            MacTxDetachReason::Collision => {
                if !self.tx_collision_pending[index] {
                    return MacTxDetachOutcome::NoEvent;
                }
                self.operations.push(Operation::DisableTxQueue(queue));
                self.tx_queue_attached[index] = false;
                self.record_fence();
                self.operations
                    .push(Operation::AcknowledgeTxEvent(queue, reason));
                self.tx_collision_pending[index] = false;
            }
            MacTxDetachReason::Timeout => {
                if !self.tx_timeout_pending[index] {
                    return MacTxDetachOutcome::NoEvent;
                }
                self.operations.push(Operation::DisableTxQueue(queue));
                self.tx_queue_attached[index] = false;
                self.operations.push(Operation::ReleaseTxCca);
                self.operations
                    .push(Operation::AcknowledgeTxEvent(queue, reason));
                self.tx_timeout_pending[index] = false;
            }
            MacTxDetachReason::Completed => {
                self.operations.push(Operation::DisableTxQueue(queue));
                self.tx_queue_attached[index] = false;
            }
        }
        self.record_fence();
        if self.tx_detach_fails[index] {
            self.tx_queue_attached[index] = true;
            MacTxDetachOutcome::Failed
        } else {
            MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                expected_descriptor_head,
            )))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlatformOperation {
    MacDelayRandom,
    SlowClockCalibration,
    TxPower(u8),
    CoexPti(MacCoexEvent),
}

#[derive(Default)]
struct MockPlatform {
    operations: Vec<PlatformOperation>,
}

impl MacDelayEntropy for MockPlatform {
    fn mac_delay_random(&mut self) -> u32 {
        self.operations.push(PlatformOperation::MacDelayRandom);
        7
    }
}

impl MacSlowClockCalibrationSource for MockPlatform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        self.operations
            .push(PlatformOperation::SlowClockCalibration);
        MacSlowClockCalibration::Unavailable
    }
}

impl MacTxPowerSource for MockPlatform {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        self.operations.push(PlatformOperation::TxPower(rate));
        MacTxPowerPair {
            primary: rate as i8,
            alternate: -(rate as i8),
        }
    }
}

impl MacCoexPtiSource for MockPlatform {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        self.operations.push(PlatformOperation::CoexPti(event));
        MacCoexPti::from_osi_value(match event {
            MacCoexEvent::Event1 => 5,
            MacCoexEvent::Event3 => 7,
            MacCoexEvent::Event10 => 3,
            MacCoexEvent::Event15 => 1,
        })
    }
}

#[test]
fn descriptor_words_preserve_the_recovered_geometry() {
    assert_eq!(
        core::mem::size_of::<Descriptor>(),
        DESCRIPTOR_BYTES as usize
    );
    assert!(descriptor_address_valid(0x2f00_0000));
    assert!(!descriptor_address_valid(0x2f00_0002));
    assert!(dma_range_valid(0x2f00_0100, 0x100));
    assert!(!dma_range_valid(0x2f07_fff0, 0x20));

    let rx = rx_armed_word(1700).unwrap();
    assert_eq!(size(rx), 1700);
    assert_eq!(length(rx), 1700);
    assert_ne!(rx & BIT_31, 0);
    assert_eq!(rx & BIT_30, 0);

    let completed = 1700 | (96 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    let recycled = rx_rearm_word(completed).unwrap();
    assert_eq!(size(recycled), 1700);
    assert_eq!(length(recycled), 1700);
    assert_ne!(recycled & BIT_31, 0);
    assert_eq!(recycled & BIT_30, 0);

    let tx = tx_owned_word(512, 123).unwrap();
    assert_eq!(size(tx), 512);
    assert_eq!(length(tx), 123);
    assert_eq!(tx & (BIT_30 | BIT_31), BIT_30 | BIT_31);
    assert_eq!(tx_owned_word(64, 65), None);
}

#[test]
fn mac_delay_slot_reproduces_vendor_modulo_eleven() {
    assert_eq!(MacDelaySlot::from_random(0).value(), 0);
    assert_eq!(MacDelaySlot::from_random(10).value(), 10);
    assert_eq!(MacDelaySlot::from_random(11).value(), 0);
    assert_eq!(MacDelaySlot::from_random(u32::MAX).value(), 3);
}

#[test]
fn cold_mac_init_orders_semantic_hardware_transactions() {
    let clock_trace = Rc::new(RefCell::new(Vec::new()));
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio {
        cold_handshake_result: Some(Ok(MacColdStartOutcome {
            handshake_samples: 0,
            handshake_observations: 1,
        })),
        cold_start_clock_trace: Some(clock_trace.clone()),
        ..MockMmio::default()
    };

    let station = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    let access_point = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
    let outcome = initialize_wifi_mac(
        &mut platform,
        &mut mmio,
        MacColdStartConfig {
            handshake_sample_limit: 4,
            station_address: station,
            access_point_address: access_point,
        },
    )
    .unwrap();
    assert_ne!(
        mmio.operations().last(),
        Some(&Operation::ConfigureOpenPromiscuousReceive)
    );
    activate_promiscuous_receive(&mut mmio);

    assert_eq!(outcome.handshake_samples, 0);
    assert_eq!(outcome.handshake_observations, 1);
    assert_eq!(
        *clock_trace.borrow(),
        [
            ColdStartClockEdge::EnableWifiMacClocks,
            ColdStartClockEdge::RetainCoexistenceClock,
            ColdStartClockEdge::ConfigureModemSourceClocks,
            ColdStartClockEdge::SetWifiMacReset(true),
            ColdStartClockEdge::SetWifiMacReset(false),
        ]
    );
    assert_eq!(
        &mmio.operations()[..6],
        [
            Operation::EnableWifiMacClocks,
            Operation::RetainCoexistenceClock,
            Operation::ConfigureModemSourceClocks,
            Operation::SetWifiMacReset(true),
            Operation::SetWifiMacReset(false),
            Operation::BeginColdHandshake(4),
        ]
    );
    assert_eq!(
        &mmio.operations()[6..12],
        [
            Operation::InitializeTxRxPrefix,
            Operation::InitializeTxRxCallbacks(MacDelaySlot::from_random(7)),
            Operation::InitializeTxRxSuffix,
            Operation::InitializeColdReceivePolicy,
            Operation::InitializeRxBufferPrefix,
            Operation::InitializeHePrefix,
        ]
    );
    assert!(matches!(
        mmio.operations()[12],
        Operation::InitializeTxPower(_)
    ));
    assert_eq!(
        &mmio.operations()[13..19],
        [
            Operation::InitializeHeSuffix,
            Operation::InitializeLastRxBufferTable,
            Operation::DisablePhyLowRate,
            Operation::InitializeCryptoBypass,
            Operation::InitializeMacAntenna,
            Operation::InitializeHalTail(
                MacInterruptMask::COLD_RX,
                MacSlowClockCalibration::Unavailable,
            ),
        ]
    );
    assert!(matches!(
        mmio.operations()[19],
        Operation::InitializeColdCoex(_)
    ));
    assert_eq!(
        &mmio.operations()[20..],
        [
            Operation::EnableMacInterrupts(MacInterruptMask::COLD_RX),
            Operation::ProgramInterfaceAddress(MacInterface::Station, station),
            Operation::ProgramInterfaceAddress(MacInterface::AccessPoint, access_point),
            Operation::ConfigureOpenPromiscuousReceive,
        ]
    );
    let mut expected_platform = vec![PlatformOperation::MacDelayRandom];
    expected_platform.extend((0..43).map(PlatformOperation::TxPower));
    expected_platform.extend(
        (0..26)
            .filter(|rate| *rate != 4)
            .map(PlatformOperation::TxPower),
    );
    expected_platform.extend([
        PlatformOperation::SlowClockCalibration,
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event15),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event10),
        PlatformOperation::CoexPti(MacCoexEvent::Event10),
    ]);
    assert_eq!(platform.operations, expected_platform);
}

#[test]
fn cold_mac_handshake_timeout_stops_mac_initialization() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio {
        cold_handshake_result: Some(Err(MacColdStartError::HandshakeTimedOut {
            samples: 2,
            sample_limit: 2,
        })),
        ..MockMmio::default()
    };

    assert_eq!(
        initialize_wifi_mac(
            &mut platform,
            &mut mmio,
            MacColdStartConfig {
                handshake_sample_limit: 2,
                station_address: [0; 6],
                access_point_address: [0; 6],
            },
        ),
        Err(MacColdStartError::HandshakeTimedOut {
            samples: 2,
            sample_limit: 2,
        })
    );
    assert_eq!(
        mmio.operations(),
        [
            Operation::EnableWifiMacClocks,
            Operation::RetainCoexistenceClock,
            Operation::ConfigureModemSourceClocks,
            Operation::SetWifiMacReset(true),
            Operation::SetWifiMacReset(false),
            Operation::BeginColdHandshake(2),
        ]
    );
}

#[test]
fn sta_link_rx_policy_forwards_one_bssid_transaction() {
    let bssid = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let mut mmio = MockMmio::default();

    configure_sta_link_receive_policy(&mut mmio, bssid);

    assert_eq!(mmio.operations(), &[Operation::ApplyStaLinkPolicy(bssid)]);
}

#[test]
fn cold_rx_ring_publishes_links_and_hardware_in_order() {
    let descriptors = [Descriptor::new(), Descriptor::new()];
    build_cold_ring(&descriptors, 0x2f00_1000, &[0x2f00_2000, 0x2f00_2800], 1700).unwrap();
    assert_eq!(
        descriptors[0].next_address(),
        0x2f00_1000 + DESCRIPTOR_BYTES
    );
    assert_eq!(descriptors[1].next_address(), 0);

    let mut mmio = MockMmio::default();
    publish_cold_ring(&mut mmio, 0x2f00_1000, true).unwrap();

    assert_eq!(
        mmio.operations(),
        &[
            Operation::Fence,
            Operation::ConfigureRxDescriptorWindow,
            Operation::PublishRxDescriptorBase(0x2f00_1000),
            Operation::PublishRxWalkerEnable,
            Operation::Fence,
        ]
    );
}

#[test]
fn completed_rx_descriptor_rearms_only_for_the_expected_storage() {
    let descriptor = Descriptor::new();
    let completed = 256 | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptor.publish(completed, 0x2f00_3000, 0);
    rearm_descriptor(&descriptor, 0x2f00_3000, 0).unwrap();
    assert_eq!(length(descriptor.word0()), 256);
    assert_ne!(descriptor.word0() & BIT_31, 0);

    descriptor.publish(completed, 0x2f00_3000, 0);
    assert!(rearm_descriptor(&descriptor, 0x2f00_3400, 0).is_err());
}

#[test]
fn recycled_rx_buffer_restores_both_migration_sentinels() {
    let mut storage = [0x5a; 20];
    prepare_recycled_buffer(&mut storage, 16).unwrap();
    assert_eq!(&storage[..4], &RX_BUFFER_SENTINEL.to_le_bytes());
    assert_eq!(&storage[4..16], &[0x5a; 12]);
    assert_eq!(&storage[16..20], &RX_BUFFER_SENTINEL.to_le_bytes());
    assert_eq!(
        prepare_recycled_buffer(&mut storage[..16], 16),
        Err(RxRingError::Size)
    );
}

#[test]
fn live_rx_ring_owns_physical_cold_order_reload_and_rom_base_repair() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut prepared = Vec::new();
    let mut mmio = MockMmio::default();
    // A previous last pointer remains diagnostic only. A stopped/rebuilt rev0
    // list must begin at physical zero so it never depends on a cold 31->0
    // wrap link.
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_walker_enabled(true);

    let stopped = RxRingStopped::prepare(
        &mut mmio,
        &descriptors,
        BASE,
        &buffers,
        BUFFER_SIZE,
        |index| {
            prepared.push(index);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(prepared, [0, 1, 2, 3]);
    assert_eq!(stopped.initial_start(), 0);
    assert_eq!(stopped.accepted_tail(), 3);
    assert_eq!(descriptors[2].next_address(), BASE + 3 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), 0);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(mmio.rx_descriptor_base(), BASE);
    let disable = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::StopRxWalker)
        .unwrap();
    let retained_last = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::ObserveRxLastDescriptor)
        .unwrap();
    assert!(disable < retained_last);
    assert!(mmio.operations()[disable + 1..retained_last].contains(&Operation::Fence));
    let topology = stopped.topology_snapshot();
    assert!(topology.valid);
    assert_eq!(topology.start_index, 0);
    assert_eq!(topology.tail_index, 3);
    assert_eq!(topology.visited_descriptors, COUNT);
    assert_eq!(topology.terminal_descriptors, 1);

    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(live.take_completed(0).unwrap().index(), 0);
    assert_eq!(live.take_completed(0), None);
    assert_eq!(live.take_completed(1).unwrap().index(), 1);
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        2,
    );

    let mut recycled = Vec::new();
    let first = live
        .recycle_completed_half(&mut mmio, |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [0, 1]);
    assert_eq!(first.head_index, 0);
    assert_eq!(first.tail_index, 1);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert!(mmio.rx_reload_pending);
    assert!(live.reload_pending());
    assert_eq!(live.accepted_tail(), 3);

    descriptors[2].write_word0(completed);
    descriptors[3].write_word0(completed);
    assert!(live.take_completed(2).is_some());
    assert!(live.take_completed(3).is_some());

    // Model bit-0 self-clear at a terminal frontier. ROM repairs BASE from the
    // last accepted descriptor's now-published next link before accepting the
    // pending tail and appending the following group.
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(0);
    mmio.set_rx_last_descriptor_address(BASE + 3 * DESCRIPTOR_BYTES);
    mmio.operations.clear();
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(
        &mmio.operations()[..6],
        &[
            Operation::ObserveRxReloadPending,
            Operation::ObserveRxNextDescriptor,
            Operation::Fence,
            Operation::ObserveRxLastDescriptor,
            Operation::Fence,
            Operation::PublishRxDescriptorBase(BASE),
        ],
        "reload repair must preserve vendor NEXT -> conditional LAST -> BASE order",
    );
    assert_eq!(live.accepted_tail(), 1);
    assert!(!live.reload_pending());
    assert!(live.exhausted_republication_probe_pending());
    recycled.clear();
    // LAST reached descriptor three while NEXT was zero, so the base-repair
    // write has been issued but hardware has not yet proved that it fetched
    // descriptor three's newly appended link to descriptor zero.
    assert!(
        live.recycle_completed_half(&mut mmio, |_| Ok(()))
            .unwrap()
            .is_none()
    );
    assert!(live.completion_release_probe_pending());
    mmio.set_rx_next_descriptor_address(BASE);
    assert!(
        live.recycle_completed_half(&mut mmio, |_| Ok(()))
            .unwrap()
            .is_none()
    );
    // Repeated NEXT observations still do not release descriptor three's
    // link. A later completed LAST does.
    descriptors[0].write_word0(descriptors[0].word0() | BIT_30);
    mmio.set_rx_last_descriptor_address(BASE);
    let second = live
        .recycle_completed_half(&mut mmio, |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [2, 3]);
    assert_eq!(second.head_index, 2);
    assert_eq!(second.tail_index, 3);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert!(
        mmio.operations()
            .contains(&Operation::PublishRxDescriptorBase(BASE))
    );
    assert_eq!(live.accepted_tail(), 1);
    assert!(live.reload_pending());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn reload_repair_observation_reads_last_only_after_zero_next() {
    const BASE: u32 = 0x2f00_1000;
    let mut mmio = MockMmio::default();
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_address(BASE);

    let active = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(active, (rx_descriptor_low(BASE + DESCRIPTOR_BYTES), None));
    assert_eq!(
        mmio.operations(),
        &[Operation::ObserveRxNextDescriptor, Operation::Fence],
    );

    mmio.operations.clear();
    mmio.set_rx_next_descriptor_address(0);
    let exhausted = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(exhausted, (0, Some(rx_descriptor_low(BASE))));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::ObserveRxNextDescriptor,
            Operation::Fence,
            Operation::ObserveRxLastDescriptor,
            Operation::Fence,
        ],
    );
}

#[test]
fn stopped_rx_ring_ignores_every_retained_last_for_cold_publication() {
    const COUNT: usize = 32;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;

    for retained_index in 0..COUNT {
        let descriptors = [const { Descriptor::new() }; COUNT];
        let buffers = core::array::from_fn(|index| 0x2f01_0000 + index as u32 * 0x400);
        let mut mmio = MockMmio::default();
        mmio.set_rx_last_descriptor_address(BASE + retained_index as u32 * DESCRIPTOR_BYTES);

        let stopped =
            RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
                Ok(())
            })
            .unwrap();
        let topology = stopped.topology_snapshot();
        assert_eq!(stopped.initial_start(), 0);
        assert_eq!(stopped.accepted_tail(), COUNT - 1);
        assert!(topology.valid, "retained descriptor {retained_index}");
        assert_eq!(topology.start_index, 0);
        assert_eq!(topology.tail_index, COUNT - 1);
        assert_eq!(topology.visited_descriptors, COUNT);
        assert_eq!(topology.terminal_descriptors, 1);
        assert_eq!(descriptors[COUNT - 1].next_address(), 0);
    }
}

#[test]
fn stopped_rx_ring_rebuilds_from_the_retained_hardware_next_cursor() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_address(BASE);

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();

    assert_eq!(
        stopped.retained_next_low(),
        rx_descriptor_low(BASE + DESCRIPTOR_BYTES)
    );
    assert_eq!(stopped.retained_last_low(), rx_descriptor_low(BASE));
    assert_eq!(stopped.initial_start(), 1);
    assert_eq!(stopped.accepted_tail(), 0);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), BASE + DESCRIPTOR_BYTES);
    assert!(stopped.topology_snapshot().valid);
}

#[test]
fn stopped_rx_ring_rejects_a_nonzero_cursor_outside_its_owned_arena() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_next_descriptor_address(BASE + COUNT as u32 * DESCRIPTOR_BYTES);

    assert!(matches!(
        RxRingStopped::prepare(
            &mut mmio,
            &descriptors,
            BASE,
            &buffers,
            BUFFER_SIZE,
            |_| Ok(())
        ),
        Err(RxRingError::Corrupt)
    ));
}

#[test]
fn stopped_rx_ring_avoids_a_cold_head_on_the_final_descriptor() {
    const COUNT: usize = 32;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;

    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = core::array::from_fn(|index| 0x2f01_0000 + index as u32 * 0x400);
    let mut mmio = MockMmio::default();
    mmio.set_rx_last_descriptor_address(BASE + (COUNT as u32 - 2) * DESCRIPTOR_BYTES);

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();

    assert_eq!(stopped.initial_start(), 0);
    assert_eq!(stopped.accepted_tail(), COUNT - 1);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[COUNT - 1].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), BASE);
    assert!(stopped.topology_snapshot().valid);
}

#[test]
fn stopped_rx_ring_uses_zero_for_an_invalid_retained_last_pointer() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];

    for retained_last in [0, BASE + 1, BASE + COUNT as u32 * DESCRIPTOR_BYTES] {
        let descriptors = [const { Descriptor::new() }; COUNT];
        let mut mmio = MockMmio::default();
        mmio.set_rx_last_descriptor_address(retained_last);
        let stopped =
            RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
                Ok(())
            })
            .unwrap();
        assert_eq!(stopped.initial_start(), 0);
        assert!(stopped.topology_snapshot().valid);
    }
}

#[test]
fn stopped_rx_ring_rejects_corrupt_topology_before_walker_enable() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(false);
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    assert!(stopped.topology_snapshot().valid);

    descriptors[0].publish(descriptors[0].word0(), buffers[0], 0);
    assert!(!stopped.topology_snapshot().valid);
    let (stopped, error) = match stopped.try_start(&mut mmio) {
        Ok(_) => panic!("corrupt RX topology started"),
        Err(failure) => failure,
    };
    assert_eq!(error, RxRingError::Corrupt);
    assert!(!mmio.walker_enabled());
    assert!(!stopped.topology_snapshot().valid);
}

#[test]
fn live_rx_ring_can_replenish_one_descriptor_per_rom_append() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let first = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.head_index, 0);
    assert_eq!(first.tail_index, 0);
    assert_eq!(descriptors[3].next_address(), BASE);

    // Model the doorbell self-clear while the walker still has a live next
    // pointer. No BASE repair is required for this ordinary append.
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail(), 0);

    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert!(live.take_completed(1).is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        1,
    );
    let second = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(second.head_index, 1);
    assert_eq!(second.tail_index, 1);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    assert_eq!(
        live.recycle_completed_batch::<0, _, _>(&mut mmio, |_| Ok(())),
        Err(RxRingError::Count)
    );
    assert_eq!(
        live.recycle_completed_batch::<3, _, _>(&mut mmio, |_| Ok(())),
        Err(RxRingError::Count)
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_republishes_an_exhausted_software_list_without_a_self_link() {
    const COUNT: usize = 2;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    // First append descriptor zero normally, making it the accepted tail of
    // the software list 1 -> 0.
    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail(), 0);

    // Hardware then exhausts that whole list before software returns either
    // node. Discarding 1 -> 0 leaves the vendor software head null, so the
    // returned chain must become the new BASE directly. Linking old tail zero
    // to head one would create the invalid cycle 1 -> 0 -> 1.
    descriptors[1].write_word0(completed);
    descriptors[0].write_word0(completed);
    mmio.set_rx_next_descriptor_address(0);
    mmio.set_rx_last_descriptor_address(BASE);
    assert!(live.take_completed(1).is_some());
    assert!(live.take_completed(0).is_some());
    mmio.operations.clear();
    let append = live
        .recycle_completed_prefix::<COUNT, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();

    assert_eq!(append.head_index, 1);
    assert_eq!(append.tail_index, 0);
    assert_eq!(descriptors[1].next_address(), BASE);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(live.recycle_start(), 1);
    assert!(!live.reload_pending());
    assert!(live.exhausted_republication_probe_pending());
    assert!(!mmio.operations().contains(&Operation::RequestRxReload));

    // A timer is not evidence that hardware accepted BASE. Keep polling while
    // NEXT is still exhausted. Even an exact cursor match retains one final
    // cooperative probe: the returned head may complete while this task is
    // still consuming the IRQ which exhausted the preceding list.
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    mmio.set_rx_next_descriptor_address(BASE);
    live.observe_exhausted_republication(&mut mmio);
    assert!(
        live.exhausted_republication_probe_pending(),
        "a nonzero cursor outside the republished head is stale evidence"
    );
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    live.observe_exhausted_republication(&mut mmio);
    assert!(!live.exhausted_republication_probe_pending());

    // Hardware resumes at the newly published head. The next RX edge must
    // inspect that same descriptor rather than the physical slot after the
    // returned chain's tail.
    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    let frontier = live.completed_unit_frontier_through_with(mmio.last_descriptor_low(), |_| true);
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 1);
    assert!(live.take_completed_unit(1).unwrap().is_some());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_does_not_rewrite_a_nonterminal_link_before_next_accepts_it() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptors[0].write_word0(completed);

    // LAST/RX_DONE can precede the walker's fetch of descriptor zero's link.
    // Rewriting that nonzero link to the recycle-chain terminal here would
    // strand descriptors one through three.
    let head_low = rx_descriptor_low(BASE);
    let successor_low = rx_descriptor_low(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_low(0);
    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    // Even repeated exact old-successor observations are not ownership
    // evidence. HIL reproduced the stale link fetch after two such samples.
    mmio.set_rx_next_descriptor_low(successor_low);
    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    descriptors[1].write_word0(completed);
    let later_low = rx_descriptor_low(BASE + DESCRIPTOR_BYTES);
    assert!(live.observe_completed_unit_link_release(&mut mmio, later_low, 1));
    assert!(!live.completion_release_probe_pending());
    assert!(live.take_completed_unit(1).unwrap().is_some());
    assert!(live.try_stop(&mut mmio).is_ok());
}

fn exercise_single_descriptor_rx_interleavings<const COUNT: usize>() {
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = core::array::from_fn(|index| 0x2f01_0000 + index as u32 * 0x400);
    let mut mmio = MockMmio::default();
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    // Two complete rotations cover both the cold physical topology and the
    // live topology assembled entirely through append/reload transactions.
    for epoch in 0..2 {
        for (cursor, descriptor) in descriptors.iter().enumerate() {
            assert_eq!(
                live.recycle_start(),
                cursor,
                "epoch {epoch}, cursor {cursor}"
            );
            let old_next = descriptor.next_address();
            assert_ne!(
                old_next, 0,
                "the live head must not also be the accepted terminal"
            );
            descriptor.write_word0(completed);
            mmio.set_rx_last_descriptor_address(BASE + cursor as u32 * DESCRIPTOR_BYTES);
            assert!(live.take_completed(cursor).is_some());

            // LAST/RX_DONE without the old successor in NEXT does not release
            // the link word. A failed probe must be a read-only transaction.
            mmio.set_rx_next_descriptor_low(0);
            let before_word0 = descriptors[cursor].word0();
            assert!(
                live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].word0(), before_word0);
            assert_eq!(descriptors[cursor].next_address(), old_next);

            // Even a stable exact successor is not a link-ownership proof.
            mmio.set_rx_next_descriptor_address(old_next);
            assert!(
                live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].next_address(), old_next);
            let later = (cursor + 1) % COUNT;
            descriptors[later].write_word0(descriptors[later].word0() | BIT_30);
            mmio.set_rx_last_descriptor_address(BASE + later as u32 * DESCRIPTOR_BYTES);
            let append = live
                .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                .unwrap()
                .unwrap();
            assert_eq!(append.head_index, cursor);
            assert_eq!(append.tail_index, cursor);
            assert_eq!(descriptors[cursor].next_address(), 0);
            assert_eq!(descriptors[cursor].word0() & BIT_30, 0);
            assert!(live.topology_snapshot().valid);

            mmio.set_rx_walker_enabled(true);
            mmio.set_rx_reload_pending(false);
            mmio.set_rx_next_descriptor_address(old_next);
            assert_eq!(
                live.poll_pending_reload(&mut mmio).unwrap(),
                RxReloadObservation::Settled
            );
            assert_eq!(live.accepted_tail(), cursor);
            let topology = live.topology_snapshot();
            assert!(topology.valid, "epoch {epoch}, cursor {cursor}");
            assert_eq!(topology.visited_descriptors, COUNT);
            assert_eq!(topology.terminal_descriptors, 1);
            assert_eq!(topology.tail_index, cursor);
        }
    }
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_preserves_topology_across_every_two_and_four_slot_interleaving() {
    exercise_single_descriptor_rx_interleavings::<2>();
    exercise_single_descriptor_rx_interleavings::<4>();
}

#[test]
fn live_rx_frontier_rejects_last_beyond_the_accepted_tail_during_reload() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert!(live.reload_pending());
    assert_eq!(live.recycle_start(), 1);
    assert_eq!(live.accepted_tail(), 3);

    // Hardware-visible pending tail zero is outside the still accepted list
    // 1 -> 2 -> 3. Even if descriptor one is complete, that impossible LAST
    // snapshot must not manufacture ownership before reload settles.
    descriptors[1].write_word0(completed);
    let pending_tail_low = rx_descriptor_low(BASE);
    let frontier = live.completed_unit_frontier_through_with(pending_tail_low, |_| true);
    assert_eq!(frontier, Default::default());

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    let frontier = live.completed_unit_frontier_through_with(pending_tail_low, |_| true);
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 1);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_recycle_rejects_a_corrupt_append_tail_before_any_mutation() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());

    // The accepted tail must be zero-terminated until the sole ring owner
    // publishes an append. Model foreign/corrupt mutation of that link.
    descriptors[3].publish(
        descriptors[3].word0(),
        descriptors[3].buffer_address(),
        BASE + 2 * DESCRIPTOR_BYTES,
    );
    let before = core::array::from_fn::<_, COUNT, _>(|index| {
        (
            descriptors[index].word0(),
            descriptors[index].buffer_address(),
            descriptors[index].next_address(),
        )
    });
    let mut prepare_calls = 0;
    assert_eq!(
        live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| {
            prepare_calls += 1;
            Ok(())
        }),
        Err(RxRingError::Corrupt)
    );
    assert_eq!(prepare_calls, 0);
    for (index, expected) in before.into_iter().enumerate() {
        assert_eq!(
            (
                descriptors[index].word0(),
                descriptors[index].buffer_address(),
                descriptors[index].next_address(),
            ),
            expected
        );
    }

    // Restore the deliberately corrupted host model so teardown can prove a
    // conventional halted list.
    descriptors[3].publish(descriptors[3].word0(), descriptors[3].buffer_address(), 0);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_snapshots_only_the_current_contiguous_frontier() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    assert_eq!(live.completed_frontier_len(), 0);
    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    descriptors[3].write_word0(completed);
    assert_eq!(live.completed_frontier_len(), 2);

    assert!(live.take_completed(0).is_some());
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(live.completed_frontier_len(), 0);
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let first = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.descriptor_count, 1);

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.completed_frontier_len(), 1);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_transfers_and_recycles_one_chained_unit_atomically() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    descriptors[0].write_word0(BUFFER_SIZE | (BUFFER_SIZE << LENGTH_SHIFT) | BIT_31);
    descriptors[1].write_word0(BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);

    assert_eq!(live.completed_frontier_len(), 0);
    let frontier = live.completed_unit_frontier();
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 2);
    let unit = live
        .take_completed_unit(frontier.descriptor_count)
        .unwrap()
        .unwrap();
    assert_eq!(unit.head_index(), 0);
    assert_eq!(unit.descriptor_count(), 2);
    assert_eq!(unit.segment_length(0), Some(256));
    assert_eq!(unit.segment_length(1), Some(80));
    assert_eq!(unit.total_length(), 336);
    assert_ne!(unit.staged_word0() & BIT_30, 0);
    assert_eq!(length(unit.staged_word0()), 336);
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        2,
    );

    let mut recycled = Vec::new();
    let append = live
        .recycle_completed_unit(&mut mmio, unit.descriptor_count(), |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [0, 1]);
    assert_eq!(append.descriptor_count, 2);
    assert_eq!(live.recycle_start(), 2);
    assert_eq!(descriptors[0].word0() & BIT_30, 0);
    assert_eq!(descriptors[1].word0() & BIT_30, 0);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_replenishes_the_available_variable_prefix() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    assert!(live.take_completed(1).is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        2,
    );
    let first = live
        .recycle_completed_prefix::<4, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.head_index, 0);
    assert_eq!(first.tail_index, 1);
    assert_eq!(first.descriptor_count, 2);

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );

    descriptors[2].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 3 * DESCRIPTOR_BYTES);
    assert!(live.take_completed(2).is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + 2 * DESCRIPTOR_BYTES,
        1,
    );
    let second = live
        .recycle_completed_prefix::<4, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(second.head_index, 2);
    assert_eq!(second.tail_index, 2);
    assert_eq!(second.descriptor_count, 1);

    assert_eq!(
        live.recycle_completed_prefix::<0, _, _>(&mut mmio, |_| Ok(())),
        Err(RxRingError::Count)
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn receive_disable_confirms_the_ring_ownership_edge() {
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    disable_receive(&mut mmio).unwrap();
    assert!(!mmio.walker_enabled());
    assert_eq!(
        mmio.operations(),
        &[
            Operation::StopRxWalker,
            Operation::Fence,
            Operation::ObserveRxWalkerEnabled,
            Operation::ObserveRxWalkerEnabled,
        ]
    );
}

#[test]
fn receive_enable_is_a_separate_confirmed_hardware_edge() {
    let mut mmio = MockMmio::default();
    enable_receive(&mut mmio).unwrap();
    assert!(mmio.walker_enabled());
    assert_eq!(
        mmio.operations(),
        &[
            Operation::ObserveRxWalkerEnabled,
            Operation::PublishRxWalkerEnable,
            Operation::Fence,
            Operation::ObserveRxWalkerEnabled,
            Operation::ObserveRxWalkerEnabled,
        ]
    );

    let mut already_enabled = MockMmio::default();
    already_enabled.set_rx_walker_enabled(true);
    assert_eq!(enable_receive(&mut already_enabled), Err(RxRingError::Busy));
    assert_eq!(
        already_enabled.operations(),
        &[Operation::ObserveRxWalkerEnabled]
    );
}

#[test]
fn sta_pairwise_ccmp_install_owns_one_bounded_hardware_slot() {
    let mut mmio = MockMmio::default();
    let peer = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let temporal_key = core::array::from_fn(|index| index as u8);

    let mut slot = install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 4);
    assert_eq!(slot.peer(), &peer);
    assert_eq!(
        mmio.operations(),
        &[Operation::InstallCcmp(
            MacInterface::Station,
            4,
            MacCcmpKeyIdentity::Pairwise { peer },
        )]
    );
    assert_eq!(slot.next_tx_ccmp_header(), Ok([3, 0, 0, 0x20, 0, 0, 0, 0]));
    assert_eq!(slot.next_tx_ccmp_header(), Ok([6, 0, 0, 0x20, 0, 0, 0, 0]));

    slot.clear(&mut mmio);
    assert!(!mmio.ccmp_valid[4]);
    assert_eq!(mmio.operations().last(), Some(&Operation::ClearCcmp(4)));

    mmio.ccmp_valid[4] = true;
    assert_eq!(
        install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).err(),
        Some(CryptoKeyError::Occupied)
    );
}

#[test]
fn sta_group_ccmp_install_uses_the_owned_semantic_slot() {
    let mut mmio = MockMmio::default();
    let temporal_key = core::array::from_fn(|index| 0xf0 | index as u8);

    let slot = install_sta_group_ccmp(&mut mmio, 1, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 1);
    assert_eq!(slot.key_id(), 1);
    assert_eq!(
        mmio.operations(),
        &[Operation::InstallCcmp(
            MacInterface::Station,
            1,
            MacCcmpKeyIdentity::Group { key_id: 1 },
        )]
    );

    slot.clear(&mut mmio);
    assert!(!mmio.ccmp_valid[1]);
    assert_eq!(
        install_sta_group_ccmp(&mut mmio, 4, &temporal_key).err(),
        Some(CryptoKeyError::InvalidGroupKeyId)
    );
}

#[test]
fn station_key_teardown_consumes_and_clears_both_hardware_slots() {
    let mut mmio = MockMmio::default();
    let pairwise = install_sta_pairwise_ccmp(&mut mmio, [1, 2, 3, 4, 5, 6], &[0x55; 16]).unwrap();
    let group = install_sta_group_ccmp(&mut mmio, 2, &[0xaa; 16]).unwrap();
    assert!(mmio.ccmp_valid[4]);
    assert!(mmio.ccmp_valid[1]);

    let report = clear_sta_ccmp_slots(&mut mmio, pairwise, group);
    assert_eq!(report.pairwise_hardware_index, 4);
    assert_eq!(report.group_hardware_index, 1);
    assert!(!mmio.ccmp_valid[4]);
    assert!(!mmio.ccmp_valid[1]);
    assert!(
        mmio.operations()
            .ends_with(&[Operation::ClearCcmp(1), Operation::ClearCcmp(4)])
    );
}

fn single_frame_segment<'a>(storage: &'a mut [u8; 128], frame_control_low: u8) -> RxSegment<'a> {
    const SIGNAL_LENGTH: usize = 34;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = frame_control_low;
    storage[FRAME_OFFSET + 1] = 0;
    storage[FRAME_OFFSET + 22] = 0;

    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: storage,
        next_descriptor_address: 0,
    }
}

#[test]
fn management_rx_extracts_one_bounded_mpdu_and_strips_fcs() {
    let mut storage = [0_u8; 128];
    let segment = single_frame_segment(&mut storage, 0xb0);
    let mut output = [0_u8; 64];
    let frame = extract_management(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 4,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.length, 30);
    assert_eq!(frame.signal_length, 34);
    assert_eq!(frame.dump_length, 38);
    assert!(frame.dump_length_matches);
    assert_eq!(output[0], 0xb0);
}

#[test]
fn control_rx_extracts_trigger_mpdu_without_interpreting_its_payload() {
    let mut storage = [0_u8; 128];
    let segment = single_frame_segment(&mut storage, 0x24);
    let mut output = [0_u8; 64];
    let frame = extract_control(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.length, 30);
    assert_eq!(output[0], 0x24);
    assert_eq!(output[1], 0);
}

#[test]
fn management_rx_rejects_failed_hardware_status() {
    let mut storage = [0_u8; 128];
    let mut segment = single_frame_segment(&mut storage, 0xb0);
    let mut failed = [0_u8; 128];
    failed.copy_from_slice(segment.buffer);
    failed[0x38 + 4] = 0xf5;
    segment.buffer = &failed;
    let mut output = [0_u8; 64];
    assert_eq!(
        extract_management(
            &[segment],
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            &mut output,
        ),
        Err(RxError::MicFailure)
    );
}

#[test]
fn data_rx_reports_qos_llc_payload_offset() {
    const SIGNAL_LENGTH: usize = 26 + 8 + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x02;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + 26..FRAME_OFFSET + 34]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 64];
    let frame = extract_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, SIGNAL_LENGTH - 4);
    assert_eq!(frame.payload_offset, 26);
    assert_eq!(
        &output[frame.payload_offset..frame.payload_offset + 8],
        &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]
    );
}

#[test]
fn ccmp_data_rx_reproduces_the_oracle_header_and_mic_adjustment() {
    const HEADER_LENGTH: usize = 26;
    const LLC_LENGTH: usize = 8;
    const PAYLOAD_LENGTH: usize = 4;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + PAYLOAD_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 16..FRAME_OFFSET + HEADER_LENGTH + 20]
        .copy_from_slice(&[1, 2, 3, 4]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 80];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, MPDU_LENGTH);
    assert_eq!(frame.ccmp_header.packet_number().value(), 3);
    assert_eq!(frame.ccmp_header.key_id().value(), 0);
    assert_eq!(frame.ccmp_header_offset, HEADER_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + PAYLOAD_LENGTH);
    assert_eq!(frame.mic_offset, MPDU_LENGTH - 8);
    assert_eq!(frame.mic_bytes_in_dma, 8);
    assert!(frame.mic_present_in_dma);
    assert_eq!(
        &output[frame.payload_offset..frame.payload_offset + LLC_LENGTH],
        &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]
    );
}

#[test]
fn ccmp_data_rx_rejects_reserved_header_encodings() {
    const HEADER_LENGTH: usize = 24;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + 8 + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    for header in [
        [1, 0, 1, 0x20, 0, 0, 0, 0],
        [1, 0, 0, 0, 0, 0, 0, 0],
        [1, 0, 0, 0x21, 0, 0, 0, 0],
    ] {
        let mut storage = [0_u8; 128];
        storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
        );
        storage[FRAME_OFFSET..FRAME_OFFSET + 2].copy_from_slice(&0x4008_u16.to_le_bytes());
        storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
            .copy_from_slice(&header);
        let segment = RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            buffer: &storage,
            next_descriptor_address: 0,
        };
        let mut output = [0_u8; 80];
        assert_eq!(
            extract_ccmp_data(
                &[segment],
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
                },
                &mut output,
            ),
            Err(RxError::Unsupported)
        );
    }
}

#[test]
fn first_segment_layout_exposes_a_consumed_ccmp_mic_shortfall() {
    const MPDU_LENGTH: usize = 26 + 8 + 8 + 4 + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 8;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let layout = first_segment_layout(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
    )
    .unwrap();

    assert_eq!(layout.received_length, RECEIVED);
    assert_eq!(layout.expected_frame_length, MPDU_LENGTH);
    assert_eq!(layout.available_frame_bytes, DMA_FRAME_LENGTH);
    assert_eq!(layout.frame_shortfall, 8);
}

#[test]
fn ccmp_data_rx_accepts_a_hardware_consumed_mic() {
    const HEADER_LENGTH: usize = 26;
    const LLC_LENGTH: usize = 8;
    const PAYLOAD_LENGTH: usize = 4;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + PAYLOAD_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 8;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 16..FRAME_OFFSET + HEADER_LENGTH + 20]
        .copy_from_slice(&[1, 2, 3, 4]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 80];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, DMA_FRAME_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + PAYLOAD_LENGTH);
    assert_eq!(frame.mic_offset, DMA_FRAME_LENGTH);
    assert_eq!(frame.mic_bytes_in_dma, 0);
    assert!(!frame.mic_present_in_dma);
}

#[test]
fn ccmp_data_rx_accepts_a_dma_view_ending_inside_the_verified_mic() {
    const HEADER_LENGTH: usize = 24;
    const LLC_LENGTH: usize = 8;
    const ARP_AND_PADDING_LENGTH: usize = 46;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + ARP_AND_PADDING_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    // The external-LAN ARP HIL frame retained the first two MIC bytes.
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 6;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 192];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x08;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 192 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 128];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, DMA_FRAME_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + ARP_AND_PADDING_LENGTH);
    assert_eq!(frame.mic_offset, MPDU_LENGTH - 8);
    assert_eq!(frame.mic_bytes_in_dma, 2);
    assert!(!frame.mic_present_in_dma);
}

#[test]
fn ccmp_data_rx_rejects_missing_extiv_and_hardware_mic_failure() {
    const SIGNAL_LENGTH: usize = 26 + 8 + 8 + 8 + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    let config = RxIngressConfig {
        ring_entry_limit: 1,
        csi_config: 0,
        flags: 0,
    };
    let mut output = [0_u8; 80];
    {
        let segment = RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            buffer: &storage,
            next_descriptor_address: 0,
        };
        assert_eq!(
            extract_ccmp_data(&[segment], config, &mut output),
            Err(RxError::Unsupported)
        );
    }

    storage[FRAME_OFFSET + 26 + 3] = 0x20;
    storage[TAIL_OFFSET + 4] = 0xf5;
    let failed = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };
    assert_eq!(
        extract_ccmp_data(&[failed], config, &mut output),
        Err(RxError::MicFailure)
    );
}

#[test]
fn irq_state_coalesces_named_work_and_records_unhandled_causes() {
    let mut mmio = MockMmio {
        interrupt_status: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::TX_COMPLETE.union(MacInterruptEvents::RX_SUCCESS),
            true,
            true,
        ),
        ..MockMmio::default()
    };
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert!(snapshot.had_auxiliary_event);
    assert!(snapshot.had_unhandled_event);
    assert!(state.observed_unhandled());
    let event = state.try_take().unwrap();
    assert_eq!(event.events, EVENT_TX_COMPLETE | EVENT_RX_SUCCESS);
    assert_eq!(mmio.operations().last(), Some(&Operation::Fence));
    assert!(mmio.operations().contains(&Operation::AcknowledgeInterrupt));
}

#[test]
fn irq_acknowledges_auxiliary_status_without_posting_independent_work() {
    let mut mmio = MockMmio {
        interrupt_status: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::empty(),
            true,
            false,
        ),
        ..MockMmio::default()
    };
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

    assert_eq!(disposition, IrqDisposition::AcknowledgedOnly);
    assert_eq!(snapshot.posted_events, 0);
    assert!(snapshot.had_auxiliary_event);
    assert!(!snapshot.had_unhandled_event);
    assert_eq!(state.try_take(), None);
    assert!(!state.observed_unhandled());
    assert!(mmio.operations().contains(&Operation::AcknowledgeInterrupt));
}

#[test]
fn irq_state_exposes_vendor_run_to_completion_order() {
    let mut mmio = MockMmio {
        interrupt_status: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::COLLISION
                .union(MacInterruptEvents::TX_TIMEOUT)
                .union(MacInterruptEvents::TX_COMPLETE)
                .union(MacInterruptEvents::RX_SUCCESS),
            false,
            false,
        ),
        ..MockMmio::default()
    };
    let state = IrqState::new();
    assert_eq!(handle_mac_irq(&mut mmio, &state).0, IrqDisposition::Posted);

    assert_eq!(state.try_take_next(), Some(IrqWork::RxSuccess));
    assert_eq!(state.try_take_next(), Some(IrqWork::TxComplete));
    assert_eq!(state.try_take_next(), Some(IrqWork::TxTimeout));
    assert_eq!(state.try_take_next(), Some(IrqWork::Collision));
    assert_eq!(state.try_take_next(), None);
}

#[test]
fn tx_slot_rejects_stale_cookie_and_completes_one_generation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    slot.as_mut().buffer_mut().unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    assert_eq!(size(slot.descriptor_word0()), 512);
    assert_eq!(length(slot.descriptor_word0()), 100);
    assert_eq!(slot.state(), TxSlotState::Reserved);
    assert_eq!(slot.as_mut().mark_hardware_owned(cookie), Ok(()));
    assert_eq!(
        slot.as_mut().mark_hardware_owned(cookie),
        Err(TxError::Stale)
    );

    let mut mmio = MockMmio::default();
    mmio.set_tx_completion(
        0,
        MacTxCompletionObservation::new_model(3, 0).with_trigger_flow_model(true),
    );

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(completion.cookie(), cookie);
    assert_eq!(completion.status(), 3);
    assert!(completion.is_trigger_flow());
    assert!(!completion.used_alternate_record());
    assert_eq!(slot.state(), TxSlotState::Completed);

    mmio.set_tx_queue_attached(0, true);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
    assert_eq!(slot.state(), TxSlotState::Free);
}

#[test]
fn tx_slot_cancels_only_an_unpublished_reservation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();

    assert_eq!(slot.as_mut().cancel_reservation(cookie), Ok(()));
    assert_eq!(slot.state(), TxSlotState::Free);
    assert_eq!(slot.descriptor_word0(), 0);
    assert!(slot.as_mut().buffer_mut().is_ok());
    assert_eq!(
        slot.as_mut().cancel_reservation(cookie),
        Err(TxError::Stale)
    );
}

#[test]
fn executor_deadline_quarantines_hardware_owned_tx_storage_without_drop_panic() {
    let mut slot = std::boxed::Box::pin(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    assert_eq!(slot.as_mut().require_reset(cookie), Ok(()));
    assert_eq!(slot.state(), TxSlotState::ResetRequired);
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    assert_eq!(slot.as_mut().require_reset(cookie), Err(TxError::Stale));
    drop(slot);
}

#[test]
fn tx_completion_decodes_the_blob_ack_snr_byte() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    // Encoded 0x8b plus the pinned 0x60 offset narrows to signed -21.
    mmio.set_tx_completion(
        0,
        MacTxCompletionObservation::new_model(0, 0).with_ack_snr_encoded_model(0x8b),
    );

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(completion.status(), 0);
    assert_eq!(completion.ack_snr_sample(), Some(-21));

    let failed = TxCompletion::new_model(cookie, 5, 0).with_ack_snr_encoded_model(0x8b);
    assert_eq!(failed.ack_snr_sample(), None);

    mmio.set_tx_queue_attached(0, true);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
}

#[test]
fn tx_slot_preserves_the_semantic_timeout_abort_order() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    mmio.set_tx_timeout_pending(0, true);
    mmio.set_tx_queue_attached(0, true);

    assert_eq!(
        slot.as_mut().begin_timeout_abort(&mut mmio, cookie),
        Ok(true)
    );
    slot.as_mut()
        .finish_timeout_abort(&mut mmio, cookie)
        .unwrap();

    assert_eq!(slot.state(), TxSlotState::Free);
    assert!(!mmio.tx_queue_attached[0]);
    assert!(!mmio.tx_timeout_pending[0]);

    let invalidation = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::DisableTxQueue(0))
        .unwrap();
    let cca_release = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::ReleaseTxCca)
        .unwrap();
    let timeout_clear = mmio
        .operations()
        .iter()
        .position(|operation| {
            *operation == Operation::AcknowledgeTxEvent(0, MacTxDetachReason::Timeout)
        })
        .unwrap();
    assert!(invalidation < cca_release);
    assert!(cca_release < timeout_clear);
}

#[test]
fn tx_slot_disables_before_acknowledging_one_collision_queue() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    mmio.set_tx_collision_pending(0, true);
    mmio.set_tx_queue_attached(0, true);

    assert_eq!(slot.as_mut().abort_collision(&mut mmio, cookie), Ok(true));
    assert_eq!(slot.state(), TxSlotState::Free);
    assert!(!mmio.tx_queue_attached[0]);
    assert!(!mmio.tx_collision_pending[0]);

    let disable = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::DisableTxQueue(0))
        .unwrap();
    let acknowledge = mmio
        .operations()
        .iter()
        .position(|operation| {
            *operation == Operation::AcknowledgeTxEvent(0, MacTxDetachReason::Collision)
        })
        .unwrap();
    assert!(disable < acknowledge);
}

#[test]
fn legacy_rate_codes_preserve_the_non_monotonic_hardware_encoding() {
    assert_eq!(LegacyRate::Dsss1MLong.code(), 0x00);
    assert_eq!(LegacyRate::Ofdm48M.code(), 0x08);
    assert_eq!(LegacyRate::Ofdm6M.code(), 0x0b);
    assert_eq!(LegacyRate::Ofdm54M.code(), 0x0c);
    assert_eq!(LegacyRate::Ofdm9M.code(), 0x0f);
    assert_eq!(LegacyRate::Ofdm54M.nominal_kbps(), 54_000);
}

#[test]
fn ht_rate_codes_keep_gi_separate_from_power_lookup_and_width() {
    let lgi = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz40,
    );
    let sgi = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    assert_eq!(lgi.code(), 23);
    assert_eq!(sgi.code(), 33);
    assert_eq!(lgi.power_lookup_code(), 23);
    assert_eq!(sgi.power_lookup_code(), 23);
    assert_eq!(lgi.nominal_kbps(), 135_000);
    assert_eq!(sgi.nominal_kbps(), 150_000);
    assert_eq!(lgi.vendor_ampdu_byte_limit(), Some(65_535));
    assert_eq!(sgi.vendor_ampdu_byte_limit(), None);
    assert_eq!(sgi.vendor_rts_rate(), LegacyRate::Ofdm24M);
    assert_eq!(sgi.vendor_retry_rate(0), Some(TxPhyRate::Ht(sgi)));
    assert_eq!(
        sgi.vendor_retry_rate(2),
        Some(TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs6,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        ))),
    );
    assert_eq!(
        sgi.vendor_retry_rate(4),
        Some(TxPhyRate::Legacy(LegacyRate::Ofdm6M)),
    );

    assert_eq!(
        HtRate::new(
            HtMcs::Mcs0,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_rts_rate(),
        LegacyRate::Ofdm6M,
    );
    assert_eq!(
        HtRate::new(
            HtMcs::Mcs0,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_ampdu_byte_limit(),
        Some(9_600),
    );
    assert_eq!(
        HtRate::new(
            HtMcs::Mcs2,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_rts_rate(),
        LegacyRate::Ofdm12M,
    );
}

#[test]
fn ht_duplicate_rate_stays_outside_the_ordinary_phy_and_formatter_domains() {
    let duplicate = HtDuplicateRate::new(HtGuardInterval::Short400Ns);
    assert_eq!(duplicate.mcs_index(), 32);
    assert_eq!(duplicate.channel_width(), HtChannelWidth::Mhz40);
    assert_eq!(duplicate.nominal_kbps(), 6_700);

    // The finite ordinary decoder has no raw byte which can manufacture the
    // separate duplicate-mode type.
    for code in u8::MIN..=u8::MAX {
        if let Some(TxPhyRate::Ht(rate)) = TxPhyRate::from_code(code, HtChannelWidth::Mhz40) {
            assert!(rate.mcs.index() <= HtMcs::Mcs7.index());
        }
    }
}

#[test]
fn ht_duplicate_selector_validates_protocol_before_reporting_exact_oracle_gaps() {
    let request = HtDuplicateCertificationRequest::new(
        HtChannelWidth::Mhz40,
        HtGuardInterval::Short400Ns,
        5_484,
    );
    let capable = HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), true, true);
    assert_eq!(capable.channel_width(), Some(HtChannelWidth::Mhz40));
    assert!(capable.peer_supports_mcs32());
    assert!(capable.peer_supports_short_guard_interval());

    let selection = select_esp32s31_ht_duplicate_tx(Some(request), capable);
    assert_eq!(selection.request(), Some(request));
    assert_eq!(selection.plan(), None);
    let Some(HtDuplicateTxRejection::Hardware(
        HtDuplicateTxUnavailable::Esp32s31EvidenceIncomplete(evidence),
    )) = selection.rejection()
    else {
        panic!("a protocol-valid request must stop at the reviewed hardware frontier");
    };
    assert_eq!(evidence, HtDuplicateTxEvidenceGaps::ESP32S31);
    let formatter = evidence.formatter();
    assert_eq!(formatter, HtDuplicateTxOracleGaps::ESP32S31);
    assert!(!formatter.is_empty());
    for field in [
        HtDuplicateTxOracleField::DescriptorSelector,
        HtDuplicateTxOracleField::PlcpAndHtSig,
        HtDuplicateTxOracleField::Length,
        HtDuplicateTxOracleField::Protection,
        HtDuplicateTxOracleField::Power,
        HtDuplicateTxOracleField::Retry,
    ] {
        assert!(formatter.contains(field));
    }
    let qualification = evidence.qualification();
    assert_eq!(qualification, HtDuplicateTxQualificationGaps::ESP32S31);
    assert!(!qualification.is_empty());
    assert!(qualification.contains(HtDuplicateTxQualificationField::OnAirAck));
}

#[test]
fn ht_duplicate_selector_reports_each_pre_hardware_rejection_without_a_plan() {
    let request = |width, guard_interval, duration| {
        Some(HtDuplicateCertificationRequest::new(
            width,
            guard_interval,
            duration,
        ))
    };
    let capable = HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), true, true);
    assert_eq!(
        select_esp32s31_ht_duplicate_tx(None, capable),
        HtDuplicateTxSelection::NotRequested
    );

    let cases = [
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns, 0),
            capable,
            HtDuplicateTxRejection::ZeroMaximumPpduDuration,
        ),
        (
            request(HtChannelWidth::Mhz20, HtGuardInterval::Long800Ns, 1),
            capable,
            HtDuplicateTxRejection::RequestedWidthMustBe40Mhz,
        ),
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns, 1),
            HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz20), true, true),
            HtDuplicateTxRejection::LinkIsNot40Mhz,
        ),
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns, 1),
            HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), false, true),
            HtDuplicateTxRejection::PeerDoesNotSupportMcs32,
        ),
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Short400Ns, 1),
            HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), true, false),
            HtDuplicateTxRejection::PeerDoesNotSupportShortGuardInterval,
        ),
    ];
    for (request, link, expected) in cases {
        let selection = select_esp32s31_ht_duplicate_tx(request, link);
        assert_eq!(selection.rejection(), Some(expected));
        assert_eq!(selection.plan(), None);
    }
}

#[test]
fn he_retry_rates_follow_the_owned_dot11ax_schedule_and_preserve_ldpc() {
    let mcs9 = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    assert_eq!(mcs9.vendor_retry_rate(0), Some(TxPhyRate::He(mcs9)));
    assert_eq!(mcs9.vendor_retry_rate(1), Some(TxPhyRate::He(mcs9)));
    assert_eq!(
        mcs9.vendor_retry_rate(2),
        Some(TxPhyRate::He(HeRate::ldpc(
            HeMcs::Mcs7,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
        )))
    );
    assert_eq!(
        mcs9.vendor_retry_rate(4),
        Some(TxPhyRate::Legacy(LegacyRate::Ofdm6M))
    );

    let mcs9_800 = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::OneLtf800Ns);
    assert_eq!(
        mcs9_800.vendor_retry_rate(2),
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs8,
            HeGuardIntervalAndLtf::OneLtf800Ns,
        )))
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs8, HeGuardIntervalAndLtf::OneLtf800Ns).vendor_retry_rate(0),
        None
    );
}

#[test]
fn rate_control_code_is_decoded_in_its_ht_or_he_arena() {
    let ht = TxPhyRate::from_rate_control_code(
        RateScheduleKind::Dot11N,
        0x17,
        HtChannelWidth::Mhz40,
        HeGuardIntervalAndLtf::OneLtf800Ns,
    );
    assert_eq!(
        ht,
        Some(TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        )))
    );

    let he_long = TxPhyRate::from_rate_control_schedule(
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 1).unwrap(),
        HtChannelWidth::Mhz20,
        HeGuardIntervalAndLtf::OneLtf800Ns,
    );
    assert_eq!(
        he_long,
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
        )))
    );

    let he_short = TxPhyRate::from_rate_control_schedule(
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap(),
        HtChannelWidth::Mhz20,
        HeGuardIntervalAndLtf::OneLtf800Ns,
    );
    assert_eq!(
        he_short,
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::OneLtf800Ns,
        )))
    );
    assert_eq!(
        TxPhyRate::from_rate_control_code(
            RateScheduleKind::Dot11Ax,
            0x23,
            HtChannelWidth::Mhz20,
            HeGuardIntervalAndLtf::FourLtf3200Ns,
        ),
        None,
    );
}

#[test]
fn he_bcc_dcm_rates_publish_the_recovered_a1_bit_and_ru242_rates() {
    for (mcs, expected_index, expected_kbps) in [
        (HeBccDcmMcs::Mcs0, 0, 4_300),
        (HeBccDcmMcs::Mcs1, 1, 8_600),
        (HeBccDcmMcs::Mcs3, 3, 17_200),
    ] {
        let rate = HeRate::bcc_dcm(mcs, HeGuardIntervalAndLtf::TwoLtf800Ns);
        assert!(rate.is_dcm());
        assert_eq!(rate.mcs().index(), expected_index);
        assert_eq!(rate.code(), 0x1a + expected_index);
        assert_eq!(
            rate.rate_control_dcm_fallback_code(),
            Some(0x10 + expected_index)
        );
        assert_eq!(rate.power_lookup_code(), 0x10 + expected_index);
        assert_eq!(rate.nominal_kbps(), expected_kbps);
        assert_eq!(
            rate.minimum_ampdu_subframe_bytes(HtAmpduDensity::EightMicroseconds),
            expected_kbps.div_ceil(1_000) as u16
        );
    }

    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::TwoLtf1600Ns).nominal_kbps(),
        16_300
    );
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::FourLtf3200Ns).nominal_kbps(),
        14_600
    );
    // Preserve the blob's two-stage integer truncation instead of replacing
    // it with a superficially equivalent ceil(rate*density/80).
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs1, HeGuardIntervalAndLtf::TwoLtf1600Ns)
            .minimum_ampdu_subframe_bytes(HtAmpduDensity::QuarterMicrosecond),
        1
    );
}

#[test]
fn he_ldpc_profile_owns_coding_control_and_the_dcm_mcs4_rom_column() {
    let ordinary = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(ordinary.fec_coding(), HeFecCoding::Ldpc);
    assert!(ordinary.is_ldpc());
    assert!(!ordinary.is_dcm());
    for (gi_ltf, expected_kbps) in [
        (HeGuardIntervalAndLtf::TwoLtf800Ns, 25_800),
        (HeGuardIntervalAndLtf::TwoLtf1600Ns, 24_400),
        (HeGuardIntervalAndLtf::FourLtf3200Ns, 21_900),
    ] {
        let rate = HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, gi_ltf);
        assert_eq!(rate.fec_coding(), HeFecCoding::Ldpc);
        assert!(rate.is_dcm());
        assert_eq!(rate.mcs(), HeMcs::Mcs4);
        assert_eq!(rate.code(), 0x1e);
        // rcGetDCMMaxRate publishes only its separate MCS0/1/3 fallback
        // domain. Direct LDPC+DCM MCS4 retains the canonical HE rate code.
        assert_eq!(rate.rate_control_dcm_fallback_code(), None);
        assert_eq!(rate.nominal_kbps(), expected_kbps);
    }
}

#[test]
fn he_resource_unit_rates_match_all_complete_blob_table_endpoints() {
    let mcs0 = HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let mcs9 = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    for (ru, mcs0_kbps, mcs9_kbps) in [
        (HeResourceUnit::Ru26, 900, 11_800),
        (HeResourceUnit::Ru52, 1_800, 23_500),
        (HeResourceUnit::Ru106, 3_800, 50_000),
        (HeResourceUnit::Ru242, 8_600, 114_700),
    ] {
        assert_eq!(mcs0.nominal_kbps_for_resource_unit(ru), mcs0_kbps);
        assert_eq!(mcs9.nominal_kbps_for_resource_unit(ru), mcs9_kbps);
    }
    assert_eq!(mcs9.nominal_kbps(), 114_700);

    let dcm_mcs3 = HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::FourLtf3200Ns);
    let dcm_mcs4 = HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    for (ru, mcs3_kbps, mcs4_kbps) in [
        (HeResourceUnit::Ru26, 1_500, 2_500),
        (HeResourceUnit::Ru52, 3_000, 5_000),
        (HeResourceUnit::Ru106, 6_400, 10_600),
        (HeResourceUnit::Ru242, 14_600, 24_400),
    ] {
        assert_eq!(dcm_mcs3.nominal_kbps_for_resource_unit(ru), mcs3_kbps);
        assert_eq!(dcm_mcs4.nominal_kbps_for_resource_unit(ru), mcs4_kbps);
    }
}

fn scheduled_trigger_user(
    aid12: u16,
    ru_allocation: u8,
    coding_type: bool,
    mcs: u8,
    dcm: bool,
    starting_spatial_stream_encoding: u8,
    spatial_stream_count_encoding: u8,
) -> [u8; 5] {
    [
        aid12 as u8,
        ((aid12 >> 8) as u8 & 0x0f) | ((ru_allocation & 0x07) << 5),
        ((ru_allocation >> 3) & 0x0f) | ((coding_type as u8) << 4) | ((mcs & 0x07) << 5),
        ((mcs >> 3) & 0x01)
            | ((dcm as u8) << 1)
            | ((starting_spatial_stream_encoding & 0x07) << 2)
            | ((spatial_stream_count_encoding & 0x07) << 5),
        0x7f,
    ]
}

fn basic_trigger_with_users(users: &[[u8; 5]]) -> Vec<u8> {
    let mut frame = vec![0_u8; 24];
    frame[..2].copy_from_slice(&0x0024_u16.to_le_bytes());
    // Trigger Common Info selector one is 2x HE-LTF + 1.6-us GI. This is a
    // different wire table from HE-SU HE-SIG-A GI/LTF.
    frame[16..24].copy_from_slice(&(1_u64 << 20).to_le_bytes());
    for user in users {
        frame.extend_from_slice(user);
        frame.push(0);
    }
    frame
}

#[test]
fn scheduled_he20_trigger_rate_selects_our_user_from_the_complete_iterator() {
    let other = scheduled_trigger_user(0x123, 0, false, 0, false, 0, 0);
    let assigned = scheduled_trigger_user(0x234, 53, true, 4, true, 0, 0);
    let bytes = basic_trigger_with_users(&[other, assigned]);
    let frame = parse_trigger_frame(&bytes).unwrap();
    let scheduled = HeTriggerScheduledRate::from_trigger_frame(&frame, 0x234).unwrap();
    assert_eq!(scheduled.resource_unit, HeResourceUnit::Ru106);
    assert_eq!(scheduled.resource_unit_index, 1);
    assert_eq!(scheduled.rate.mcs(), HeMcs::Mcs4);
    assert!(scheduled.rate.is_ldpc());
    assert!(scheduled.rate.is_dcm());

    assert_eq!(
        HeTriggerScheduledRate::from_trigger_frame(&frame, 0x345),
        Err(HeTriggerScheduledRateError::AssociationIdNotScheduled)
    );

    let duplicate_bytes = basic_trigger_with_users(&[assigned, assigned]);
    let duplicate = parse_trigger_frame(&duplicate_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::from_trigger_frame(&duplicate, 0x234),
        Err(HeTriggerScheduledRateError::DuplicateAssociationId)
    );

    let mut malformed_bytes = basic_trigger_with_users(&[assigned]);
    malformed_bytes.push(0);
    let malformed = parse_trigger_frame(&malformed_bytes).unwrap();
    assert!(matches!(
        HeTriggerScheduledRate::from_trigger_frame(&malformed, 0x234),
        Err(HeTriggerScheduledRateError::MalformedUserInfo(_))
    ));

    let mut padding_hidden_bytes = basic_trigger_with_users(&[]);
    padding_hidden_bytes.extend_from_slice(&[0xff, 0xef, 0x0f, 0, 0]);
    padding_hidden_bytes.extend_from_slice(&assigned);
    padding_hidden_bytes.push(0);
    let padding_hidden = parse_trigger_frame(&padding_hidden_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::from_trigger_frame(&padding_hidden, 0x234),
        Err(HeTriggerScheduledRateError::AssociationIdNotScheduled)
    );
}

#[test]
fn scheduled_he20_trigger_rate_fails_closed_at_every_owned_boundary() {
    let common =
        parse_trigger_common_info(&(1_u64 << 20).to_le_bytes()).expect("complete common info");
    let user_bytes = scheduled_trigger_user(0x234, 53, true, 4, true, 0, 0);
    let user = parse_trigger_user_spatial_stream(&user_bytes).unwrap();
    let scheduled = HeTriggerScheduledRate::new(common, user, 0x234).unwrap();
    assert_eq!(scheduled.resource_unit, HeResourceUnit::Ru106);
    assert_eq!(scheduled.resource_unit_index, 1);
    assert_eq!(scheduled.partial_ru_power_selector.trigger_encoding(), 53);
    assert_eq!(scheduled.rate.mcs(), HeMcs::Mcs4);
    assert_eq!(
        scheduled.trigger_gi_ltf,
        open_esp_radio_ieee80211::trigger::TriggerGiLtf::TwoLtf1600Ns
    );
    assert!(scheduled.rate.is_ldpc());
    assert!(scheduled.rate.is_dcm());
    assert_eq!(scheduled.nominal_kbps(), 10_600);

    let bsrp_common = parse_trigger_common_info(&((2_u64 << 20) | 4).to_le_bytes()).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(bsrp_common, user, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedTriggerType)
    );
    let wide_common =
        parse_trigger_common_info(&((2_u64 << 20) | (1 << 18)).to_le_bytes()).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(wide_common, user, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedBandwidth)
    );
    assert_eq!(
        HeTriggerScheduledRate::new(common, user, 0x235),
        Err(HeTriggerScheduledRateError::AssociationIdMismatch)
    );

    let two_stream_bytes = scheduled_trigger_user(0x234, 53, true, 4, true, 0, 1);
    let two_streams = parse_trigger_user_spatial_stream(&two_stream_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(common, two_streams, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedSpatialStreams)
    );

    for ru_allocation in [9, 62, 69] {
        let unsupported_ru_bytes =
            scheduled_trigger_user(0x234, ru_allocation, false, 0, false, 0, 0);
        let unsupported_ru = parse_trigger_user_spatial_stream(&unsupported_ru_bytes).unwrap();
        assert_eq!(
            HeTriggerScheduledRate::new(common, unsupported_ru, 0x234),
            Err(HeTriggerScheduledRateError::UnsupportedResourceUnit)
        );
    }

    let mcs10_bytes = scheduled_trigger_user(0x234, 53, true, 10, false, 0, 0);
    let mcs10 = parse_trigger_user_spatial_stream(&mcs10_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(common, mcs10, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedMcs)
    );

    let dcm_mcs2_bytes = scheduled_trigger_user(0x234, 53, true, 2, true, 0, 0);
    let dcm_mcs2 = parse_trigger_user_spatial_stream(&dcm_mcs2_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(common, dcm_mcs2, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedDcmCombination)
    );

    let reserved_gi =
        parse_trigger_common_info(&(3_u64 << 20).to_le_bytes()).expect("complete common info");
    assert_eq!(
        HeTriggerScheduledRate::new(reserved_gi, user, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedGiLtf)
    );
}

#[test]
fn he_ampdu_density_and_empty_delimiters_match_complete_blob_integer_policy() {
    let expected_microseconds = [0, 1, 1, 1, 2, 4, 8, 16];
    for (encoding, expected) in expected_microseconds.into_iter().enumerate() {
        let density = HtAmpduDensity::from_ampdu_parameters((encoding as u8) << 2);
        assert_eq!(density.encoding(), encoding as u8);
        assert_eq!(density.vendor_integer_microseconds(), expected);
    }

    let ordinary = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(
        ordinary.minimum_ampdu_subframe_bytes(HtAmpduDensity::SixteenMicroseconds),
        230
    );
    assert_eq!(
        ordinary.ampdu_empty_delimiters(28, HtAmpduDensity::SixteenMicroseconds),
        Some(50)
    );
    assert_eq!(
        ordinary.ampdu_empty_delimiters(28, HtAmpduDensity::NoRestriction),
        Some(0)
    );

    let dcm = HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(
        dcm.minimum_ampdu_subframe_bytes(HtAmpduDensity::SixteenMicroseconds),
        35
    );
    assert_eq!(
        dcm.ampdu_empty_delimiters(28, HtAmpduDensity::SixteenMicroseconds),
        Some(1)
    );
    assert_eq!(
        dcm.ampdu_empty_delimiters(0, HtAmpduDensity::SixteenMicroseconds),
        None
    );
}

#[test]
fn he_default_apep_limit_matches_rom_and_the_blob_dcm_branch() {
    assert_eq!(
        HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::OneLtf800Ns).maximum_default_apep_bytes(),
        3_700
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns).maximum_default_apep_bytes(),
        50_000
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs6, HeGuardIntervalAndLtf::TwoLtf1600Ns).maximum_default_apep_bytes(),
        31_500
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::FourLtf3200Ns).maximum_default_apep_bytes(),
        42_000
    );

    // Complete ppCheckTxHEAMPDUlength halves the selected rate/GI limit when
    // descriptor-state bit 15 requests DCM.
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns)
            .maximum_default_apep_bytes(),
        1_850
    );
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::FourLtf3200Ns)
            .maximum_default_apep_bytes(),
        6_400
    );
}

#[test]
fn he_ampdu_config_rejects_an_apep_above_the_selected_rate_limit() {
    let gi_1600 = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    assert!(
        HeAmpduTxConfig::new(gi_1600, 27, 47_000, 31, HtAmpduDensity::NoRestriction,).is_some()
    );
    assert!(
        HeAmpduTxConfig::new(gi_1600, 27, 47_001, 32, HtAmpduDensity::NoRestriction,).is_none()
    );

    let gi_800 = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert!(HeAmpduTxConfig::new(gi_800, 27, 50_000, 32, HtAmpduDensity::NoRestriction,).is_some());
    assert!(HeAmpduTxConfig::new(gi_800, 27, 50_001, 32, HtAmpduDensity::NoRestriction,).is_none());
}

#[test]
fn he_nonzero_edca_txop_apep_limits_match_the_complete_blob_producer() {
    // Complete rx11AXRate2AMPDULimit_update output for the standard WMM
    // voice TXOP of 47 * 32 us. Rows are 0.8/1.6/3.2-us GI.
    const VOICE_47: [[u32; 10]; 3] = [
        [
            1_469, 2_992, 4_490, 6_039, 9_061, 12_082, 13_593, 15_104, 18_125, 20_139,
        ],
        [
            1_386, 2_824, 4_238, 5_701, 8_552, 11_404, 12_830, 14_256, 17_108, 19_009,
        ],
        [
            1_240, 2_527, 3_792, 5_101, 7_653, 10_205, 11_481, 12_757, 15_309, 17_011,
        ],
    ];
    // Complete producer output for the standard WMM video TXOP of 94 * 32 us.
    const VIDEO_94: [[u32; 10]; 3] = [
        [
            3_086, 6_227, 9_342, 12_509, 18_765, 25_021, 28_149, 31_277, 37_533, 41_704,
        ],
        [
            2_914, 5_879, 8_821, 11_811, 17_717, 23_624, 26_578, 29_531, 35_438, 39_376,
        ],
        [
            2_615, 5_276, 7_916, 10_600, 15_901, 21_203, 23_854, 26_505, 31_806, 35_341,
        ],
    ];
    let profiles = [
        HeGuardIntervalAndLtf::TwoLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
        HeGuardIntervalAndLtf::FourLtf3200Ns,
    ];

    for (row, guard_interval_and_ltf) in profiles.into_iter().enumerate() {
        for mcs_index in 0..10 {
            let rate = HeRate::new(
                HeMcs::from_index(mcs_index as u8).unwrap(),
                guard_interval_and_ltf,
            );
            assert_eq!(
                rate.maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
                VOICE_47[row][mcs_index]
            );
            assert_eq!(
                rate.maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(94).unwrap()),
                VIDEO_94[row][mcs_index]
            );
        }
    }

    // Both 0.8-us encodings select the first producer row.
    assert_eq!(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::OneLtf800Ns)
            .maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
        VOICE_47[0][9]
    );
    // Complete ppCheckTxHEAMPDUlength halves either generated table for DCM.
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::TwoLtf1600Ns)
            .maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(94).unwrap()),
        VIDEO_94[1][3] / 2
    );
}

#[test]
fn he_checked_apep_producer_matches_positive_blob_domain_and_rejects_wrap() {
    let profiles = [
        (HeGuardIntervalAndLtf::TwoLtf800Ns, 31.2_f32, 13.6_f32),
        (HeGuardIntervalAndLtf::TwoLtf1600Ns, 32.0_f32, 14.4_f32),
        (HeGuardIntervalAndLtf::FourLtf3200Ns, 40.0_f32, 16.0_f32),
    ];
    let data_bits_per_symbol = [117_i32, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
    let estimated_block_ack_us = [68_i32, 44, 44, 32, 32, 32, 32, 32, 32, 32];

    let mut rejected = 0_u16;
    let mut rejected_short_limits = [0_u16; 4];
    for units_32_us in 1_u16..=u16::from(u8::MAX) {
        let txop = HeEdcaTxopLimit::from_units_32_us(units_32_us).unwrap();
        for (guard_interval_and_ltf, preamble_us, symbol_us) in profiles {
            for mcs_index in 0..10 {
                let data_symbols = (((i32::from(units_32_us) * 32 - 36)
                    - estimated_block_ack_us[mcs_index])
                    as f32
                    - preamble_us)
                    / symbol_us;
                // This is the complete blob's fsub/fdiv/fmadd/fcvt/div
                // instruction sequence used as the independent test oracle.
                let signed_expected = (data_bits_per_symbol[mcs_index] as f32)
                    .mul_add(data_symbols, -22.0_f32) as i32
                    / 8;
                let rate = HeRate::new(
                    HeMcs::from_index(mcs_index as u8).unwrap(),
                    guard_interval_and_ltf,
                );
                if signed_expected <= 0 {
                    rejected = rejected.saturating_add(1);
                    if let Some(count) =
                        rejected_short_limits.get_mut(usize::from(units_32_us.saturating_sub(1)))
                    {
                        *count = count.saturating_add(1);
                    }
                    assert_eq!(rate.checked_maximum_apep_bytes(txop), None);
                    assert_eq!(rate.maximum_apep_bytes(txop), 0);
                    assert!(
                        HeAmpduTxConfig::new_with_txop(
                            rate,
                            1,
                            1,
                            1,
                            HtAmpduDensity::NoRestriction,
                            txop,
                        )
                        .is_none()
                    );
                } else {
                    let expected = signed_expected as u32;
                    assert_eq!(rate.checked_maximum_apep_bytes(txop), Some(expected));
                    assert_eq!(rate.maximum_apep_bytes(txop), expected);
                }
            }
        }
    }
    assert_ne!(rejected, 0, "the short-TXOP wrap domain remains covered");
    assert!(
        rejected_short_limits.into_iter().all(|count| count != 0),
        "every AP-controlled 1..=4-unit limit covers a non-positive rate/GI budget"
    );
}

#[test]
fn zero_edca_txop_selects_the_rom_apep_table_for_every_he_rate() {
    let profiles = [
        HeGuardIntervalAndLtf::OneLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
        HeGuardIntervalAndLtf::FourLtf3200Ns,
    ];
    for guard_interval_and_ltf in profiles {
        for mcs_index in 0..10 {
            let rate = HeRate::new(
                HeMcs::from_index(mcs_index).unwrap(),
                guard_interval_and_ltf,
            );
            assert_eq!(
                rate.maximum_apep_bytes(HeEdcaTxopLimit::DEFAULT),
                u32::from(rate.maximum_default_apep_bytes())
            );
        }
    }
}

#[test]
fn ht_peer_ampdu_parameters_keep_length_density_and_queue_spacing_together() {
    let expected_maximum = [0x1fff, 0x3fff, 0x7fff, 0xffff];
    for exponent in 0_u8..=3 {
        let parameters = HtPeerAmpduParameters::from_capability_byte(exponent | (6 << 2));
        assert_eq!(
            parameters.maximum_aggregate_bytes(),
            expected_maximum[usize::from(exponent)]
        );
        assert_eq!(parameters.density(), HtAmpduDensity::EightMicroseconds);
        assert_eq!(
            parameters.protection_spacing(),
            HtProtectionSpacing::Density6
        );
    }
}

#[test]
fn aggregate_config_updates_the_same_retry_geometry_for_ht_and_he() {
    let ht_rate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    let mut ht = AmpduTxConfig::Ht(HtAmpduTxConfig::new(ht_rate, 1_000, 2).unwrap());
    ht.update_retained_retry(512, 1, 31);
    assert_eq!(ht.rate(), TxPhyRate::Ht(ht_rate));
    assert_eq!(ht.hardware_key_selector(), 0);
    assert!(matches!(
        ht,
        AmpduTxConfig::Ht(HtAmpduTxConfig {
            aggregate_length: 512,
            subframes: 1,
            contention_window: 31,
            ..
        })
    ));

    let he_rate = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let mut he = AmpduTxConfig::He(
        HeAmpduTxConfig::new(he_rate, 7, 1_000, 2, HtAmpduDensity::NoRestriction).unwrap(),
    );
    he.update_retained_retry(640, 1, 63);
    assert_eq!(he.rate(), TxPhyRate::He(he_rate));
    assert_eq!(he.hardware_key_selector(), 0);
    assert!(matches!(
        he,
        AmpduTxConfig::He(HeAmpduTxConfig {
            aggregate_length: 640,
            subframes: 1,
            contention_window: 63,
            ..
        })
    ));
}

#[test]
fn legacy_rts_rates_match_the_complete_vendor_selector() {
    let cases = [
        (LegacyRate::Dsss1MLong, LegacyRate::Dsss1MLong),
        (LegacyRate::Dsss2MLong, LegacyRate::Dsss2MLong),
        (LegacyRate::Cck5M5Long, LegacyRate::Dsss2MLong),
        (LegacyRate::Cck11MLong, LegacyRate::Dsss2MLong),
        (LegacyRate::Dsss2MShort, LegacyRate::Dsss2MShort),
        (LegacyRate::Cck5M5Short, LegacyRate::Dsss2MShort),
        (LegacyRate::Cck11MShort, LegacyRate::Dsss2MShort),
        (LegacyRate::Ofdm48M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm24M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm12M, LegacyRate::Ofdm12M),
        (LegacyRate::Ofdm6M, LegacyRate::Ofdm6M),
        (LegacyRate::Ofdm54M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm36M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm18M, LegacyRate::Ofdm12M),
        (LegacyRate::Ofdm9M, LegacyRate::Ofdm6M),
    ];
    for (data, expected) in cases {
        assert_eq!(data.vendor_rts_rate(), expected);
    }
}

#[test]
fn data_queue_priorities_match_the_complete_blob_event_mapping() {
    for (queue, expected) in [
        (LegacyTxQueue::Voice, 3),
        (LegacyTxQueue::Video, 2),
        (LegacyTxQueue::BestEffort, 1),
        (LegacyTxQueue::Background, 1),
    ] {
        assert_eq!(queue.vendor_data_packet_priority(), expected);
        assert_eq!(queue.vendor_data_scheduler_priority(), expected);
    }
}

#[test]
fn management_profile_derives_plcp1_from_mpdu_plus_fcs() {
    let config = LegacyTxConfig::management_1m_from_mpdu_length(30).unwrap();
    assert_eq!(config.signal, 0x22);
    assert!(LegacyTxConfig::management_1m_from_mpdu_length(0x0ffc).is_none());
}

#[test]
fn rx_phy_info_matches_the_pinned_s31_public_metadata_layout() {
    let mut metadata = [0_u8; 0x40];
    metadata[1] = 0xe9;
    metadata[4..8].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
    metadata[9..11].copy_from_slice(&0x9abc_u16.to_le_bytes());
    metadata[0x25] = 0x4f;
    assert_eq!(
        decode_rx_phy_info(&metadata),
        Some(RxPhyInfo {
            rate: 9,
            bb_format: 4,
            he_siga1: 0x1234_5678,
            he_siga2: 0x9abc,
        })
    );
    assert_eq!(decode_rx_phy_info(&metadata[..0x25]), None);
}

#[test]
fn staged_rx_metadata_decodes_only_instruction_proved_s31_fields() {
    let mut metadata = [0_u8; 0x40];
    metadata[0] = (-47_i8) as u8;
    metadata[1] = 0xeb;
    metadata[4..8].copy_from_slice(&0x0040_5b4b_u32.to_le_bytes());
    metadata[9..11].copy_from_slice(&0x1234_u16.to_le_bytes());
    metadata[0x1c] = 6;
    metadata[0x1f] = 0;
    metadata[0x25] = 0x4f;

    assert_eq!(
        decode_normalized_rx_metadata(&metadata),
        Some(MacRxMetadata {
            channel: MacRxEvidence::Unavailable,
            rate: MacRxEvidence::HardwareObserved(RxPhyInfo {
                rate: 11,
                bb_format: 4,
                he_siga1: 0x0040_5b4b,
                he_siga2: 0x1234,
            }),
            rssi_dbm: MacRxEvidence::HardwareObserved(-47),
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::HardwareObserved(false),
            ampdu: MacRxEvidence::ProtocolValidated(true),
            amsdu: MacRxEvidence::Unavailable,
        })
    );
    assert_eq!(decode_normalized_rx_metadata(&metadata[..0x1c]), None);

    // A plausible callback-ABI value still is not raw-DMA evidence.
    metadata[0x1c] = 11;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata)
            .expect("complete metadata")
            .channel,
        MacRxEvidence::Unavailable,
    );
}

#[test]
fn normalized_ht_rx_metadata_uses_the_direct_ht_sig_aggregation_bit() {
    let mut metadata = [0_u8; 0x40];
    metadata[4..8].copy_from_slice(&(7_u32 | (1 << 7) | (1 << 27) | (1 << 31)).to_le_bytes());
    metadata[0x1c] = 11;
    metadata[0x1f] = 0;
    metadata[0x25] = 2 << 4;

    let decoded = decode_normalized_rx_metadata(&metadata).unwrap();
    assert_eq!(decoded.s_mpdu, MacRxEvidence::HardwareObserved(false));
    assert_eq!(decoded.ampdu, MacRxEvidence::HardwareObserved(true));
    let MacRxEvidence::HardwareObserved(phy) = decoded.rate else {
        panic!("HT PHY metadata must remain hardware-observed");
    };
    let signal = phy.ht_signal().unwrap();
    assert_eq!(signal.mcs, 7);
    assert_eq!(signal.channel_width_mhz, 40);
    assert!(signal.aggregation);
    assert!(signal.short_guard_interval);

    metadata[4..8].fill(0);
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::HardwareObserved(false)
    );
}

#[test]
fn normalized_ht_rx_metadata_separates_mcs32_from_the_five_bit_rate_summary() {
    let mut metadata = [0_u8; 0x40];
    // The public `rate` summary at byte one is only five bits and therefore
    // wraps MCS32 to zero. The format-specific HT-SIG word remains the owner
    // of the complete seven-bit MCS selector and CBW geometry.
    metadata[1] = 0;
    metadata[4..8].copy_from_slice(&(32_u32 | (1 << 7)).to_le_bytes());
    metadata[0x25] = 2 << 4;

    let decoded = decode_normalized_rx_metadata(&metadata).unwrap();
    let MacRxEvidence::HardwareObserved(phy) = decoded.rate else {
        panic!("HT PHY metadata must remain hardware-observed");
    };
    assert_eq!(phy.rate, 0);
    let signal = phy.ht_signal().unwrap();
    assert_eq!(
        signal.ht_duplicate_mcs32_classification(),
        HtDuplicateRxClassification::Ht40(open_esp_radio_ieee80211::ht::HtDuplicateMcs32::new())
    );
    assert!(signal.ht_duplicate_mcs32().is_some());

    metadata[4..8].copy_from_slice(&32_u32.to_le_bytes());
    let MacRxEvidence::HardwareObserved(phy) =
        decode_normalized_rx_metadata(&metadata).unwrap().rate
    else {
        panic!("HT PHY metadata must remain hardware-observed");
    };
    let signal = phy.ht_signal().unwrap();
    assert_eq!(
        signal.ht_duplicate_mcs32_classification(),
        HtDuplicateRxClassification::Mismatch {
            channel_width_mhz: 20,
        }
    );
    assert_eq!(signal.ht_duplicate_mcs32(), None);
}

#[test]
fn normalized_rx_metadata_separates_format_validated_ampdu_from_ht_hardware_status() {
    let mut metadata = [0_u8; 0x40];
    metadata[0x25] = 4 << 4;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::ProtocolValidated(true)
    );

    metadata[0x25] = 1 << 4;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::ProtocolValidated(false)
    );

    metadata[0x25] = 9 << 4;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::Unavailable
    );
}

#[test]
fn normalized_monitor_view_excludes_the_vendor_prefix_and_stripped_fcs() {
    const MPDU_LENGTH: usize = 24;
    const RECEIVED: usize = 0x40 + MPDU_LENGTH;
    let mut storage = [0_u8; RECEIVED];
    storage[0] = (-42_i8) as u8;
    storage[1] = 3;
    storage[0x1c] = 11;
    storage[0x25] = 1 << 4;
    let signal_length = (MPDU_LENGTH + 4) as u32;
    storage[0x38..0x3c].copy_from_slice(&signal_length.to_le_bytes());
    for (index, byte) in storage[0x40..].iter_mut().enumerate() {
        *byte = index as u8;
    }
    let segment = RxSegment {
        descriptor_address: 0x2f00_1000,
        descriptor_word0: (RECEIVED as u32) | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };

    let frame = view_normalized_rx_frame(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    )
    .unwrap();
    assert_eq!(frame.mpdu, &storage[0x40..]);
    assert_eq!(frame.logical_length, MPDU_LENGTH);
    assert_eq!(
        frame.metadata.rssi_dbm,
        MacRxEvidence::HardwareObserved(-42)
    );
}

#[test]
fn rx_phy_info_decodes_the_qualified_he20_mcs9_signal() {
    let phy = RxPhyInfo {
        rate: 11,
        bb_format: 4,
        he_siga1: 0x0040_5b4b,
        he_siga2: 0,
    };
    assert_eq!(phy.baseband_format(), RxBasebandFormat::HeSu);
    assert_eq!(
        phy.he_su_signal(),
        Some(HeSuSignal {
            format: true,
            beam_change: true,
            uplink: false,
            mcs: 9,
            dcm: false,
            bss_color: 27,
            spatial_reuse: 0,
            bandwidth: HeBandwidth::Mhz20,
            guard_interval_and_ltf: HeGuardIntervalAndLtf::TwoLtf1600Ns,
            nsts_and_midamble_periodicity: 0,
            txop: 0,
            ldpc: false,
            ldpc_extra_symbol: false,
            stbc: false,
            beamformed: false,
            pre_fec_padding_factor: 0,
            packet_extension_disambiguity: false,
            doppler: false,
        })
    );
    let signal = phy.he_su_signal().unwrap();
    assert_eq!(signal.bandwidth.mhz(), 20);
    assert_eq!(signal.guard_interval_and_ltf.guard_interval_ns(), 1_600);
    assert_eq!(signal.guard_interval_and_ltf.ltf_count(), 2);
    assert_eq!(signal.space_time_stream_count(), Some(1));
    assert_eq!(signal.spatial_stream_count(), Some(1));
}

#[test]
fn he_su_stbc_distinguishes_space_time_and_spatial_stream_counts() {
    let signal = HeSuSignal::decode(0x00e0_591b, 0x4a0c);
    assert!(signal.stbc);
    assert!(!signal.doppler);
    assert_eq!(signal.nsts_and_midamble_periodicity, 1);
    assert_eq!(signal.space_time_stream_count(), Some(2));
    assert_eq!(signal.spatial_stream_count(), Some(1));

    let doppler = HeSuSignal::decode(0x00e0_591b, 0xca0c);
    assert!(doppler.doppler);
    assert_eq!(doppler.space_time_stream_count(), None);
    assert_eq!(doppler.spatial_stream_count(), None);
}

#[test]
fn rx_phy_info_uses_the_blob_su_layout_for_extended_range_su() {
    let phy = RxPhyInfo {
        rate: 11,
        bb_format: 6,
        he_siga1: 0x0040_5b4b,
        he_siga2: 0,
    };
    assert_eq!(phy.he_su_signal().map(|signal| signal.mcs), Some(9));
}

#[test]
fn rx_phy_info_decodes_complete_he_mu_common_signal_fields() {
    let phy = RxPhyInfo {
        rate: 0,
        bb_format: 5,
        he_siga1: 0x03de_4d5b,
        he_siga2: 0xdbb5,
    };
    assert_eq!(
        phy.he_mu_signal(),
        Some(HeMuSignal {
            uplink: true,
            sig_b_mcs: 5,
            sig_b_dcm: true,
            bss_color: 42,
            spatial_reuse: 9,
            bandwidth: HeMuBandwidth::Unknown(4),
            sig_b_symbols_or_mu_mimo_users_minus_one: 7,
            sig_b_compression: true,
            guard_interval_and_ltf: HeGuardIntervalAndLtf::FourLtf3200Ns,
            doppler: true,
            txop: 0x35,
            nltf_and_midamble_periodicity: 3,
            ldpc_extra_symbol_segment: true,
            stbc: true,
            pre_fec_padding_factor: 2,
            packet_extension_disambiguity: true,
        })
    );
    let signal = phy.he_mu_signal().unwrap();
    assert_eq!(signal.bandwidth.mhz(), None);
    assert_eq!(signal.bandwidth.raw(), 4);
    assert_eq!(signal.sig_b_symbols_or_mu_mimo_users(), 8);
    assert_eq!(signal.he_ltf_symbols(), 6);
}

#[test]
fn rx_phy_info_decodes_complete_he_trigger_based_common_signal_fields() {
    let siga1 = 1 | (17 << 1) | (1 << 7) | (2 << 11) | (3 << 15) | (4 << 19) | (1 << 24);
    let phy = RxPhyInfo {
        rate: 0,
        bb_format: 7,
        he_siga1: siga1,
        he_siga2: 0x01d5,
    };
    assert_eq!(
        phy.he_trigger_based_signal(),
        Some(HeTriggerBasedSignal {
            format: true,
            bss_color: 17,
            spatial_reuse: [1, 2, 3, 4],
            bandwidth: HeBandwidth::Mhz40,
            txop: 0x55,
        })
    );
}

#[test]
fn rx_he_mu_sig_b_borrows_only_the_blob_advertised_complete_bytes() {
    let mut metadata = [0_u8; 0x40];
    metadata[0x25] = 5 << 4;
    metadata[4..8].copy_from_slice(&(1_u32 << 22).to_le_bytes());
    metadata[0x1a] = 0xfe;
    metadata[0x1e] = 0xb7;

    let selected_user = (1 << 20) | (7 << 15) | (12 << 11) | 0x345;
    metadata[0x28] = selected_user as u8;
    metadata[0x29] = (selected_user >> 8) as u8;
    metadata[0x2a] = ((selected_user >> 16) as u8 & 0x1f) | (5 << 5);
    metadata[0x2b] = 0x80 | 2;

    let common = 0x1a_bcde_u32;
    metadata[0x2d] = (common << 2) as u8;
    metadata[0x2e] = (common >> 6) as u8;
    metadata[0x2f] = (common >> 14) as u8 & 0x7f;
    metadata[0x38..0x3b].copy_from_slice(&[0xaa, 0xbb, 0x1c]);
    metadata[0x3b] = 0xee;

    let sig_b = decode_rx_he_mu_sig_b(&metadata).unwrap();
    assert_eq!(sig_b.bit_length, 21);
    assert_eq!(sig_b.common_info_raw, common);
    assert_eq!(sig_b.selected_user_info_raw, selected_user);
    assert_eq!(
        sig_b.selected_user,
        HeMuSigBUser::Mimo(HeMuSigBMimoUser {
            station_id: 0x345,
            spatial_configuration: 12,
            mcs: 7,
            ldpc: true,
        })
    );
    assert_eq!(sig_b.ru_size, 2);
    assert_eq!(sig_b.ru_position, 11);
    assert_eq!(sig_b.complete_bytes, &[0xaa, 0xbb, 0x1c]);
    let compressed_users: Vec<_> = sig_b.he20_mimo_users().unwrap().collect();
    assert_eq!(compressed_users.len(), 1);
    assert_eq!(compressed_users[0].bit_offset, 0);
    assert_eq!(compressed_users[0].raw, 0x1c_bbaa & 0x1f_ffff);

    assert_eq!(decode_rx_he_mu_sig_b(&metadata[..0x3a]), None);
    metadata[0x2b] &= 0x7f;
    assert_eq!(
        decode_rx_he_mu_sig_b(&metadata).unwrap().complete_bytes,
        &[]
    );
    assert_eq!(decode_rx_he_mu_sig_b(&metadata[..0x30]), None);
    metadata[0x25] = 4 << 4;
    assert_eq!(decode_rx_he_mu_sig_b(&metadata), None);
}

#[test]
fn rx_he20_non_mimo_sig_b_iterates_complete_users_and_rejects_other_layouts() {
    fn write_user(bytes: &mut [u8], bit_offset: usize, word: u32) {
        for output_bit in 0..21 {
            let destination_bit = bit_offset + output_bit;
            if word & (1 << output_bit) != 0 {
                bytes[destination_bit / 8] |= 1 << (destination_bit % 8);
            }
        }
    }

    let mut metadata = [0_u8; 0x48];
    metadata[0x25] = 5 << 4;
    let bit_length = 101_u16;
    metadata[0x2a] = ((bit_length % 8) as u8) << 5;
    metadata[0x2b] = 0x80 | (bit_length / 8) as u8;

    let users = [
        (1 << 20) | (3 << 15) | 0x123,
        (1 << 19) | (5 << 15) | 0x456,
        (1 << 14) | (7 << 15) | 0x321,
    ];
    write_user(&mut metadata[0x38..], 18, users[0]);
    write_user(&mut metadata[0x38..], 39, users[1]);
    write_user(&mut metadata[0x38..], 70, users[2]);

    let sig_b = decode_rx_he_mu_sig_b(&metadata).unwrap();
    assert_eq!(sig_b.signal.bandwidth, HeMuBandwidth::Mhz20);
    assert!(!sig_b.signal.sig_b_compression);
    let entries: Vec<_> = sig_b.he20_non_mimo_users().unwrap().collect();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].bit_offset, 18);
    assert_eq!(entries[1].bit_offset, 39);
    assert_eq!(entries[2].bit_offset, 70);
    assert_eq!(entries[2].user, HeMuSigBNonMimoUser::decode(users[2]));

    metadata[4..8].copy_from_slice(&(1_u32 << 22).to_le_bytes());
    assert_eq!(
        decode_rx_he_mu_sig_b(&metadata)
            .unwrap()
            .he20_non_mimo_users(),
        Err(RxHe20MuSigBUsersError::MuMimoCompressed)
    );
    metadata[4..8].copy_from_slice(&(1_u32 << 15).to_le_bytes());
    assert_eq!(
        decode_rx_he_mu_sig_b(&metadata)
            .unwrap()
            .he20_non_mimo_users(),
        Err(RxHe20MuSigBUsersError::WiderOrUnknownBandwidth)
    );
}

#[test]
fn rx_baseband_format_preserves_unknown_hardware_values() {
    assert_eq!(RxBasebandFormat::decode(9), RxBasebandFormat::Unknown(9));
    assert_eq!(RxBasebandFormat::Unknown(9).raw(), 9);
}
