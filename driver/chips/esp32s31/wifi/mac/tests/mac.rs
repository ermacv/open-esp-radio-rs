use std::collections::BTreeMap;

use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma};
use open_esp_radio_esp32s31_hal::types::{
    MacHtTxProgram, MacInterface, MacInterruptMask, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacTxCompletionRegisters, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi_dma::descriptor::{
    BIT_30, BIT_31, DESCRIPTOR_BYTES, Descriptor, LENGTH_SHIFT, descriptor_address_valid,
    dma_range_valid, length, rx_armed_word, rx_rearm_word, size, tx_owned_word,
};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{
        CcmpKeyHardware, CryptoKeyError, clear_sta_ccmp_slots, install_sta_group_ccmp,
        install_sta_pairwise_ccmp,
    },
    init::{
        MacClockControl, MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacColdAntennaHardware,
        MacColdCoexHardware, MacColdCoexPti, MacColdCryptoHardware, MacColdEnableHardware,
        MacColdHalTailHardware, MacColdHandshakeHardware, MacColdHeHardware,
        MacColdLastRxBufferHardware, MacColdRxBufferHardware, MacColdRxPolicyHardware,
        MacColdStartConfig, MacColdStartError, MacColdStartOutcome, MacColdTxRxHardware,
        MacDelayEntropy, MacDelaySlot, MacInterfaceAddressHardware, MacLowRateHardware,
        MacSlowClockCalibration, MacSlowClockCalibrationSource, MacSnifferHardware, MacTxPowerPair,
        MacTxPowerSource, MacTxPowerTable, StaLinkRxPolicyHardware, activate_promiscuous_receive,
        configure_sta_link_receive_policy, initialize_wifi_mac,
    },
    irq::{
        IrqDisposition, IrqState, IrqWork, MAC_INT_COLLISION, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
        MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT, MacInterrupt, handle_mac_irq,
    },
    rate_schedule::{RateScheduleKind, RateScheduleRef},
    rx::{
        HeBandwidth, HeGuardIntervalAndLtf, HeMuBandwidth, HeMuSignal, HeSuSignal,
        HeTriggerBasedSignal, INGRESS_STRICT_DUMP, INGRESS_STRICT_RXEND, RX_BUFFER_SENTINEL,
        RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT, RxBasebandFormat, RxDma, RxDmaBinding,
        RxDmaCursorObservation, RxDmaWalkerStopped, RxError, RxHe20MuSigBUsersError,
        RxIngressConfig, RxPhyInfo, RxReloadObservation, RxRingError, RxRingLive, RxRingStopped,
        RxSegment, build_cold_ring, decode_normalized_rx_metadata, decode_rx_he_mu_sig_b,
        decode_rx_phy_info, disable_receive, enable_receive, extract_ccmp_data, extract_control,
        extract_data, extract_management, first_segment_layout, prepare_recycled_buffer,
        publish_cold_ring, rearm_descriptor, view_normalized_rx_frame,
    },
    rx_pool::{RxStagePool, RxStageTransactionError},
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeBccDcmMcs, HeEdcaTxopLimit, HeFecCoding, HeLdpcDcmMcs,
        HeMcs, HeRate, HeResourceUnit, HeSmpduTxConfig, HeTriggerScheduledRate,
        HeTriggerScheduledRateError, HtAmpduDensity, HtAmpduTxConfig, HtChannelWidth,
        HtGuardInterval, HtMcs, HtPeerAmpduParameters, HtProtectionSpacing, HtRate, HtTxConfig,
        LegacyRate, LegacyTxConfig, LegacyTxQueue, TxCompletion, TxError, TxHardware,
        TxLifetimeClass, TxPhyRate, TxSlot, TxSlotState, he_ampdu_q0_image, he_smpdu_q0_image,
        ht_ampdu_q0_image, ht_q0_image, legacy_q0_image,
    },
};
use open_esp_radio_ieee80211::he::{HeMuSigBMimoUser, HeMuSigBNonMimoUser, HeMuSigBUser};
use open_esp_radio_ieee80211::trigger::{
    parse_trigger_common_info, parse_trigger_frame, parse_trigger_user_spatial_stream,
};
use open_esp_radio_wifi_softmac::{MacRxEvidence, MacRxMetadata};

/// Logical register identities used by the protocol-layer test double.
///
/// These values deliberately carry no address, width, reset value or access
/// authority. Register addresses and access policy are tested by the PAC and
/// HAL owners; this test observes only which logical hardware operation the
/// MAC requested.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TestRegister {
    Named(&'static str),
    Indexed(&'static str, u8),
    CryptoKeyWord(u8, u8),
}

const fn named(name: &'static str) -> TestRegister {
    TestRegister::Named(name)
}

const fn indexed(name: &'static str, index: u8) -> TestRegister {
    TestRegister::Indexed(name, index)
}

#[derive(Clone, Copy)]
struct TestField(u32);

impl TestField {
    const fn mask(self) -> u32 {
        self.0
    }
}

mod mac {
    use super::{TestField, TestRegister, indexed, named};

    pub const RX_CONTROL: TestRegister = named("RX_CONTROL");
    pub const RX_DESCRIPTOR_BASE: TestRegister = named("RX_DESCRIPTOR_BASE");
    pub const RX_NEXT_DESCRIPTOR: TestRegister = named("RX_NEXT_DESCRIPTOR");
    pub const RX_LAST_DESCRIPTOR: TestRegister = named("RX_LAST_DESCRIPTOR");
    pub const RX_LAST_DESCRIPTOR_HIGH: TestRegister = named("RX_LAST_DESCRIPTOR_HIGH");
    pub const CRYPTO_KEY_VALID_BITMAP: TestRegister = named("CRYPTO_KEY_VALID_BITMAP");
    pub const CRYPTO_POLICY_CONTROL: TestRegister = named("CRYPTO_POLICY_CONTROL");
    pub const CRYPTO_INTERFACE_CONTROL: [TestRegister; 3] = [
        indexed("CRYPTO_INTERFACE_CONTROL", 0),
        indexed("CRYPTO_INTERFACE_CONTROL", 1),
        indexed("CRYPTO_INTERFACE_CONTROL", 2),
    ];
    pub const CRYPTO_KEY_ENTRY_WORDS: u8 = 10;

    pub const TX_Q_CONFIG: [TestRegister; 4] = queue("TX_Q_CONFIG");
    pub const TX_Q_CONTROL: [TestRegister; 4] = queue("TX_Q_CONTROL");
    pub const TX_Q_PPDU_CONTROL: [TestRegister; 4] = queue("TX_Q_PPDU_CONTROL");
    pub const TX_Q_PROTECTION: [TestRegister; 4] = queue("TX_Q_PROTECTION");
    pub const TX_Q_PLCP1: [TestRegister; 4] = queue("TX_Q_PLCP1");
    pub const TX_Q_PTI: [TestRegister; 4] = queue("TX_Q_PTI");
    pub const TX_Q_HT_SIGNAL: [TestRegister; 4] = queue("TX_Q_HT_SIGNAL");
    pub const TX_Q_POWER: [TestRegister; 4] = queue("TX_Q_POWER");
    pub const TX_Q_HT_DESCRIPTOR_COUNTS: [TestRegister; 4] = queue("TX_Q_HT_DESCRIPTOR_COUNTS");
    pub const TX_Q_DATA_LENGTH: [TestRegister; 4] = queue("TX_Q_DATA_LENGTH");
    pub const TX_Q_LENGTH_CONTROL: [TestRegister; 4] = queue("TX_Q_LENGTH_CONTROL");
    pub const TX_Q0_CONTROL: TestRegister = TX_Q_CONTROL[0];
    pub const TX_STATE_CLEAR: TestRegister = named("TX_STATE_CLEAR");
    pub const TX_STATE: TestRegister = named("TX_STATE");
    pub const TX_CCA_CONTROL: TestRegister = named("TX_CCA_CONTROL");
    pub const TX_COMPLETE_CLEAR: TestRegister = named("TX_COMPLETE_CLEAR");
    pub const TX_COMPLETE_STATE: TestRegister = named("TX_COMPLETE_STATE");
    pub const TX_COMPLETE_PRIMARY: [TestRegister; 4] = queue("TX_COMPLETE_PRIMARY");
    pub const TX_COMPLETE_ALTERNATE: [TestRegister; 4] = queue("TX_COMPLETE_ALTERNATE");
    pub const TX_COMPLETE_AUX_A: [TestRegister; 4] = queue("TX_COMPLETE_AUX_A");
    pub const TX_COMPLETE_AUX_B: [TestRegister; 4] = queue("TX_COMPLETE_AUX_B");
    pub const TX_COMPLETE_AUX_C: [TestRegister; 4] = queue("TX_COMPLETE_AUX_C");
    pub const TX_COMPLETE_PRIMARY_Q0: TestRegister = TX_COMPLETE_PRIMARY[0];
    pub const TX_COMPLETE_ALTERNATE_Q0: TestRegister = TX_COMPLETE_ALTERNATE[0];
    pub const TX_COMPLETE_AUX_A_Q0: TestRegister = TX_COMPLETE_AUX_A[0];
    pub const TX_COMPLETE_AUX_B_Q0: TestRegister = TX_COMPLETE_AUX_B[0];
    pub const TX_COMPLETE_AUX_C_Q0: TestRegister = TX_COMPLETE_AUX_C[0];

    const fn queue(name: &'static str) -> [TestRegister; 4] {
        [
            indexed(name, 0),
            indexed(name, 1),
            indexed(name, 2),
            indexed(name, 3),
        ]
    }

    pub const fn crypto_key_entry_word(index: u8, word: u8) -> Option<TestRegister> {
        if index < 25 && word < CRYPTO_KEY_ENTRY_WORDS {
            Some(TestRegister::CryptoKeyWord(index, word))
        } else {
            None
        }
    }

    pub mod rx_control {
        use super::TestField;
        pub const APPEND_DESCRIPTOR_RELOAD: TestField = TestField(1);
        pub const WALKER_ENABLE: TestField = TestField(1 << 31);
    }

    pub mod tx_state {
        pub const TIMEOUT_SHIFT: u32 = 16;
    }

    pub mod tx_cca_control {
        use super::TestField;
        pub const FORCE: TestField = TestField(3 << 30);
    }
}

mod mac_init {
    use super::{TestRegister, indexed, named};

    pub const HANDSHAKE: TestRegister = named("HANDSHAKE");
    pub const R_4C00: TestRegister = named("R_4C00");
    pub const R_407C: TestRegister = named("R_407C");
    pub const R_4098: TestRegister = named("R_4098");
    pub const R_4114: TestRegister = named("R_4114");
    pub const R_4118: TestRegister = named("R_4118");
    pub const R_4120: TestRegister = named("R_4120");
    pub const R_4308: TestRegister = named("R_4308");
    pub const R_444C: TestRegister = named("R_444C");
    pub const R_4450: TestRegister = named("R_4450");
    pub const R_4458: TestRegister = named("R_4458");
    pub const R_445C: TestRegister = named("R_445C");
    pub const R_4C1C: TestRegister = named("R_4C1C");
    pub const R_4C20: TestRegister = named("R_4C20");
    pub const R_4C24: TestRegister = named("R_4C24");
    pub const R_4C54: TestRegister = named("R_4C54");
    pub const R_4C58: TestRegister = named("R_4C58");
    pub const R_4C60: TestRegister = named("R_4C60");
    pub const R_4C68: TestRegister = named("R_4C68");
    pub const R_4C6C: TestRegister = named("R_4C6C");
    pub const R_4C8C: TestRegister = named("R_4C8C");
    pub const R_4C98: TestRegister = named("R_4C98");
    pub const R_4CA0: TestRegister = named("R_4CA0");
    pub const R_4CA8: TestRegister = named("R_4CA8");
    pub const R_4E04: TestRegister = named("R_4E04");
    pub const R_8060: TestRegister = named("R_8060");
    pub const R_807C: TestRegister = named("R_807C");

    pub const INTERFACE_ADDRESS_LOW: [TestRegister; 4] = group("INTERFACE_ADDRESS_LOW");
    pub const INTERFACE_ADDRESS_HIGH: [TestRegister; 4] = group("INTERFACE_ADDRESS_HIGH");
    pub const RX_FILTER: [TestRegister; 4] = group("RX_FILTER");
    pub const BSSID_LOW: [TestRegister; 4] = group("BSSID_LOW");
    pub const BSSID_HIGH: [TestRegister; 4] = group("BSSID_HIGH");
    pub const RX_QUEUE_DEFAULT: [TestRegister; 4] = group("RX_QUEUE_DEFAULT");
    pub const CRYPTO_BYPASS: [TestRegister; 5] = [
        indexed("CRYPTO_BYPASS", 0),
        indexed("CRYPTO_BYPASS", 1),
        indexed("CRYPTO_BYPASS", 2),
        indexed("CRYPTO_BYPASS", 3),
        indexed("CRYPTO_BYPASS", 4),
    ];
    pub const LAST_RX_BUFFER: [TestRegister; 18] = [
        indexed("LAST_RX_BUFFER", 0),
        indexed("LAST_RX_BUFFER", 1),
        indexed("LAST_RX_BUFFER", 2),
        indexed("LAST_RX_BUFFER", 3),
        indexed("LAST_RX_BUFFER", 4),
        indexed("LAST_RX_BUFFER", 5),
        indexed("LAST_RX_BUFFER", 6),
        indexed("LAST_RX_BUFFER", 7),
        indexed("LAST_RX_BUFFER", 8),
        indexed("LAST_RX_BUFFER", 9),
        indexed("LAST_RX_BUFFER", 10),
        indexed("LAST_RX_BUFFER", 11),
        indexed("LAST_RX_BUFFER", 12),
        indexed("LAST_RX_BUFFER", 13),
        indexed("LAST_RX_BUFFER", 14),
        indexed("LAST_RX_BUFFER", 15),
        indexed("LAST_RX_BUFFER", 16),
        indexed("LAST_RX_BUFFER", 17),
    ];

    const fn group(name: &'static str) -> [TestRegister; 4] {
        [
            indexed(name, 0),
            indexed(name, 1),
            indexed(name, 2),
            indexed(name, 3),
        ]
    }
}

trait Mmio {
    fn read32(&mut self, register: TestRegister) -> u32;
    fn write32(&mut self, register: TestRegister, value: u32);
    fn fence(&mut self);
}

const RX_CONTROL: TestRegister = mac::RX_CONTROL;
const RX_DESCRIPTOR_BASE: TestRegister = mac::RX_DESCRIPTOR_BASE;
const RX_LAST_DESCRIPTOR: TestRegister = mac::RX_LAST_DESCRIPTOR;
const RX_LAST_DESCRIPTOR_HIGH: TestRegister = mac::RX_LAST_DESCRIPTOR_HIGH;
const RX_NEXT_DESCRIPTOR: TestRegister = mac::RX_NEXT_DESCRIPTOR;
const TX_CCA_CONTROL: TestRegister = mac::TX_CCA_CONTROL;
const TX_COMPLETE_ALTERNATE_Q0: TestRegister = mac::TX_COMPLETE_ALTERNATE_Q0;
const TX_COMPLETE_AUX_A_Q0: TestRegister = mac::TX_COMPLETE_AUX_A_Q0;
const TX_COMPLETE_AUX_B_Q0: TestRegister = mac::TX_COMPLETE_AUX_B_Q0;
const TX_COMPLETE_AUX_C_Q0: TestRegister = mac::TX_COMPLETE_AUX_C_Q0;
const TX_COMPLETE_CLEAR: TestRegister = mac::TX_COMPLETE_CLEAR;
const TX_COMPLETE_PRIMARY_Q0: TestRegister = mac::TX_COMPLETE_PRIMARY_Q0;
const TX_COMPLETE_STATE: TestRegister = mac::TX_COMPLETE_STATE;
const TX_Q0_CONTROL: TestRegister = mac::TX_Q0_CONTROL;
const TX_STATE: TestRegister = mac::TX_STATE;
const TX_STATE_CLEAR: TestRegister = mac::TX_STATE_CLEAR;

const RX_ENABLE: u32 = mac::rx_control::WALKER_ENABLE.mask();
const RX_RELOAD: u32 = mac::rx_control::APPEND_DESCRIPTOR_RELOAD.mask();
const TX_Q_ENABLE_VALID: u32 = 0xc000_0000;
const TX_CCA_FORCE_MASK: u32 = mac::tx_cca_control::FORCE.mask();
const TX_TIMEOUT_SHIFT: u32 = mac::tx_state::TIMEOUT_SHIFT;
const TX_COMPLETE_Q0: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Read(TestRegister),
    Write(TestRegister, u32),
    InitializeMacAntenna,
    InitializeHalTail(u32, MacSlowClockCalibration),
    InitializeColdCoex(MacColdCoexPti),
    InitializeHePrefix,
    InitializeTxPower(MacTxPowerTable),
    InitializeHeSuffix,
    ConfigureOpenPromiscuousReceive,
    ReadInterruptStatus,
    WriteInterruptEnable(u32),
    ClearInterrupt(u32),
    Fence,
}

#[derive(Default)]
struct MockMmio {
    words: BTreeMap<TestRegister, u32>,
    operations: Vec<Operation>,
    interrupt_status: u32,
    interrupt_enable: u32,
}

impl MockMmio {
    fn set(&mut self, register: TestRegister, value: u32) {
        self.words.insert(register, value);
    }

    fn operations(&self) -> &[Operation] {
        &self.operations
    }
}

impl Mmio for MockMmio {
    fn read32(&mut self, register: TestRegister) -> u32 {
        self.operations.push(Operation::Read(register));
        self.words.get(&register).copied().unwrap_or(0)
    }

    fn write32(&mut self, register: TestRegister, value: u32) {
        self.operations.push(Operation::Write(register, value));
        self.words.insert(register, value);
    }

    fn fence(&mut self) {
        self.operations.push(Operation::Fence);
    }
}

impl RxDma for MockMmio {
    fn last_descriptor_low(&mut self) -> u32 {
        self.read32(RX_LAST_DESCRIPTOR) & 0x000f_ffff
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.read32(RX_NEXT_DESCRIPTOR) & 0x000f_ffff
    }

    fn next_descriptor_word(&mut self) -> u32 {
        self.read32(RX_NEXT_DESCRIPTOR)
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        Mmio::fence(self);
        let next = self.next_descriptor_low();
        Mmio::fence(self);
        observed(RxDmaCursorObservation::validation(last, next))
    }

    fn walker_enabled(&mut self) -> bool {
        self.read32(RX_CONTROL) & RX_ENABLE != 0
    }

    fn reload_pending(&mut self) -> bool {
        self.read32(RX_CONTROL) & RX_RELOAD != 0
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

    fn set_descriptor_high_window(&mut self, _: &RxDmaBinding, address_high: u16) {
        let previous = self.read32(RX_LAST_DESCRIPTOR_HIGH);
        self.write32(
            RX_LAST_DESCRIPTOR_HIGH,
            (previous & 0x000f_ffff) | (u32::from(address_high) << 20),
        );
    }

    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.write32(RX_DESCRIPTOR_BASE, address);
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        let control = self.read32(RX_CONTROL);
        self.write32(RX_CONTROL, control | RX_ENABLE);
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        let control = self.read32(RX_CONTROL);
        self.write32(RX_CONTROL, control | RX_RELOAD);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        let control = self.read32(RX_CONTROL);
        if control & RX_ENABLE != 0 {
            return None;
        }
        self.write32(RX_CONTROL, control | RX_ENABLE);
        Mmio::fence(self);
        (self.read32(RX_CONTROL) & RX_ENABLE != 0).then(|| {
            enabled(open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled::validation())
        })
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        let control = self.read32(RX_CONTROL);
        self.write32(RX_CONTROL, control & !RX_ENABLE);
        Mmio::fence(self);
        (self.read32(RX_CONTROL) & RX_ENABLE == 0)
            .then(|| stopped(RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {
        Mmio::fence(self);
    }
}

fn confirm_completed_unit_link_release<const COUNT: usize>(
    live: &mut RxRingLive<'_, COUNT>,
    mmio: &mut MockMmio,
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    last_descriptor_low: u32,
    descriptor_count: usize,
) {
    let last_descriptor_low = last_descriptor_low & 0x000f_ffff;
    assert!(
        !live.observe_completed_unit_link_release(mmio, last_descriptor_low, descriptor_count,),
        "the current LAST descriptor still owns its nonterminal link",
    );
    let tail_index = usize::try_from(
        (last_descriptor_low.wrapping_sub(descriptor_base & 0x000f_ffff)) / DESCRIPTOR_BYTES,
    )
    .unwrap();
    let later_index = (tail_index + 1) % COUNT;
    descriptors[later_index].write_word0(descriptors[later_index].word0() | BIT_30);
    let later_low = (descriptor_base + later_index as u32 * DESCRIPTOR_BYTES) & 0x000f_ffff;
    mmio.set(RX_LAST_DESCRIPTOR, later_low);
    assert!(live.observe_completed_unit_link_release(mmio, later_low, descriptor_count,));
}

impl MacInterrupt for MockMmio {
    type Snapshot = u32;

    fn status(&mut self) -> Self::Snapshot {
        self.operations.push(Operation::ReadInterruptStatus);
        self.interrupt_status
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.operations.push(Operation::ClearInterrupt(snapshot));
        self.interrupt_status &= !snapshot;
        Mmio::fence(self);
    }
}

impl CcmpKeyHardware for MockMmio {
    fn install_sta_ccmp_entry(&mut self, index: u8, words: &[u32; 6]) -> MacKeyInstallOutcome {
        let validity = self.read32(mac::CRYPTO_KEY_VALID_BITMAP);
        let valid_bit = 1_u32 << index;
        if validity & valid_bit != 0 {
            return MacKeyInstallOutcome::Occupied;
        }
        for word in 0..mac::CRYPTO_KEY_ENTRY_WORDS {
            self.write32(mac::crypto_key_entry_word(index, word).unwrap(), 0);
        }
        for (word, value) in words.iter().copied().enumerate() {
            self.write32(
                mac::crypto_key_entry_word(index, word as u8).unwrap(),
                value,
            );
        }
        self.write32(mac::CRYPTO_KEY_VALID_BITMAP, validity | valid_bit);
        self.write32(mac::CRYPTO_INTERFACE_CONTROL[0], 0x0003_0103);
        let policy = self.read32(mac::CRYPTO_POLICY_CONTROL);
        self.write32(mac::CRYPTO_POLICY_CONTROL, policy & 0xffc0_003f);
        let control = self.read32(mac::CRYPTO_INTERFACE_CONTROL[0]);
        self.write32(mac::CRYPTO_INTERFACE_CONTROL[0], control & 0x3fff_ffff);
        Mmio::fence(self);
        if self.read32(mac::CRYPTO_KEY_VALID_BITMAP) & valid_bit == 0 {
            MacKeyInstallOutcome::Rejected
        } else {
            MacKeyInstallOutcome::Installed
        }
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        let validity = self.read32(mac::CRYPTO_KEY_VALID_BITMAP);
        self.write32(mac::CRYPTO_KEY_VALID_BITMAP, validity & !(1_u32 << index));
        for word in 0..mac::CRYPTO_KEY_ENTRY_WORDS {
            self.write32(mac::crypto_key_entry_word(index, word).unwrap(), 0);
        }
        Mmio::fence(self);
    }
}

impl StaLinkRxPolicyHardware for MockMmio {
    fn apply_sta_link_policy(&mut self, bssid_address: [u8; 6]) {
        let filter = mac_init::RX_FILTER[0];
        let bssid_low = mac_init::BSSID_LOW[0];
        let bssid = mac_init::BSSID_HIGH[0];
        let interface = mac_init::INTERFACE_ADDRESS_HIGH[0];
        let current = self.read32(bssid);
        self.write32(bssid, current & !(1 << 31));
        self.write32(
            bssid_low,
            u32::from_le_bytes([
                bssid_address[0],
                bssid_address[1],
                bssid_address[2],
                bssid_address[3],
            ]),
        );
        let current = self.read32(bssid);
        self.write32(
            bssid,
            (current & !0xffff)
                | u32::from(u16::from_le_bytes([bssid_address[4], bssid_address[5]])),
        );
        let current = self.read32(bssid);
        self.write32(bssid, current | (1 << 31));
        let mut modify = |register: TestRegister, mask: u32, value: u32| {
            let current = self.read32(register);
            self.write32(register, (current & !mask) | (value & mask));
        };
        modify(filter, (1 << 10) | (1 << 4), 0);
        modify(bssid, 1 << 30, 0);
        modify(filter, 1 << 6, 0);
        modify(bssid, 1 << 31, 1 << 31);
        modify(interface, 1 << 16, 1 << 16);
        modify(filter, 1 << 8, 1 << 8);
        modify(filter, 1 << 1, 1 << 1);
        Mmio::fence(self);
    }
}

impl MacInterfaceAddressHardware for MockMmio {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        let interface = interface.bits() as usize;
        let low = mac_init::INTERFACE_ADDRESS_LOW[interface];
        let high = mac_init::INTERFACE_ADDRESS_HIGH[interface];
        self.write32(
            low,
            u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
        );
        self.write32(high, u32::from(address[4]) | (u32::from(address[5]) << 8));
        let value = self.read32(high);
        self.write32(high, value | (1 << 16));
    }
}

impl MacColdHandshakeHardware for MockMmio {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError> {
        let current = self.read32(mac_init::HANDSHAKE);
        self.write32(mac_init::HANDSHAKE, current | (1 << 1));
        let mut samples = 0;
        let value = loop {
            let value = self.read32(mac_init::HANDSHAKE);
            if value & 1 != 0 {
                break value;
            }
            samples += 1;
            if samples >= sample_limit {
                return Err(MacColdStartError::HandshakeTimedOut {
                    samples,
                    observed: value,
                });
            }
        };
        self.interrupt_enable = 0;
        self.interrupt_status = 0;
        self.operations.push(Operation::WriteInterruptEnable(0));
        self.operations.push(Operation::ClearInterrupt(u32::MAX));
        Ok(MacColdStartOutcome {
            handshake_samples: samples,
            handshake_value: value,
        })
    }
}

impl MacSnifferHardware for MockMmio {
    fn configure_open_promiscuous_receive(&mut self) {
        self.operations
            .push(Operation::ConfigureOpenPromiscuousReceive);
    }
}

impl MacColdCryptoHardware for MockMmio {
    fn initialize_crypto_bypass(&mut self) {
        for (register, value) in
            mac_init::CRYPTO_BYPASS
                .into_iter()
                .zip([0x0003_0000, 0x0003_0000, 0, 0, 0])
        {
            self.write32(register, value);
        }
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
            event_mask.bits(),
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
        self.words.insert(mac_init::R_4E04, 0);
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
        let current = self.read32(mac_init::R_4C00);
        self.write32(mac_init::R_4C00, current & !0x0000_00f0);
        self.interrupt_enable = event_mask.bits();
        self.operations
            .push(Operation::WriteInterruptEnable(event_mask.bits()));
    }
}

impl MacColdLastRxBufferHardware for MockMmio {
    fn initialize_last_rx_buffer_table(&mut self) {
        for (register, value) in mac_init::LAST_RX_BUFFER.into_iter().zip([
            0x0002_3006,
            0x0000_0608,
            0x0000_ffff,
            0x0002_3006,
            0x0000_0808,
            0x0000_ffff,
            0x0002_3006,
            0x0000_8e88,
            0x0000_ffff,
            0x0002_301c,
            0x4400_4300,
            0xffff_ffff,
            0x0002_301c,
            0x4300_4400,
            0xffff_ffff,
            0x0002_3011,
            0x0000_0001,
            0x0000_00ff,
        ]) {
            self.write32(register, value);
        }
        for (register, mask) in [
            (mac_init::R_4120, 0x0000_3f00),
            (mac_init::R_4120, 0x0000_007e),
            (mac_init::R_4098, 0x0800_0000),
        ] {
            let current = self.read32(register);
            self.write32(register, current | mask);
        }
    }
}

impl MacColdTxRxHardware for MockMmio {
    fn initialize_txrx_prefix(&mut self) {
        let mut modify = |register: TestRegister, mask: u32, value: u32| {
            let current = self.read32(register);
            self.write32(register, (current & !mask) | (value & mask));
        };
        modify(mac_init::R_4C8C, 0x8080_a000, 0x8080_a000);
        modify(mac_init::R_4C8C, 1 << 12, 1 << 12);
        modify(mac_init::R_4C8C, 1 << 28, 1 << 28);
        modify(mac_init::R_4C98, 1 << 3, 0);
        for register in mac_init::RX_QUEUE_DEFAULT {
            modify(register, 0xffff_0000, 0);
        }
        modify(mac_init::RX_QUEUE_DEFAULT[0], 1 << 24, 1 << 24);
        modify(mac_init::RX_QUEUE_DEFAULT[1], 1 << 24, 1 << 24);
        modify(mac_init::RX_QUEUE_DEFAULT[0], 1 << 26, 1 << 26);
        modify(mac_init::RX_QUEUE_DEFAULT[1], 1 << 26, 1 << 26);
        modify(mac_init::R_4C8C, 1 << 9, 1 << 9);
        modify(mac_init::R_4114, 1 << 0, 1 << 0);
        modify(mac_init::R_4114, 1 << 4, 1 << 4);
        modify(mac_init::R_4118, 1 << 31, 1 << 31);
        modify(mac_init::R_4118, 0x0ff0_0000, 0x01b0_0000);
        modify(mac_init::R_4CA0, 0x3, 0x3);
    }

    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        let slot = u32::from(delay_slot.value());
        {
            let mut modify = |register: TestRegister, mask: u32, value: u32| {
                let current = self.read32(register);
                self.write32(register, (current & !mask) | (value & mask));
            };
            modify(mac_init::R_4C58, 0x001f_fc00, 0x000e_e400);
            modify(mac_init::R_4C58, 0x0000_03ff, 0x0000_00f5 + slot);
            modify(mac_init::R_4C58, 0x7fe0_0000, 0x0bc0_0000);
            modify(mac_init::R_4C54, 0x7fe0_0000, (0x0000_00fa + slot) << 21);
            modify(mac_init::R_4C54, 0x001f_fc00, 0x0009_d800);
        }
        self.write32(mac_init::R_444C, 0x0009_0a0b);
        self.write32(mac_init::R_4458, 0x0009_0a0b);
        self.write32(mac_init::R_4450, 0x0005_0100);
        self.write32(mac_init::R_445C, 0x0005_0100);
        let current = self.read32(mac_init::R_4C1C);
        self.write32(mac_init::R_4C1C, (current & !0x0000_0fff) | 0x0000_000f);
    }

    fn initialize_txrx_suffix(&mut self) {
        let mut modify = |register: TestRegister, mask: u32, value: u32| {
            let current = self.read32(register);
            self.write32(register, (current & !mask) | (value & mask));
        };
        modify(mac_init::R_4C1C, 1 << 31, 1 << 31);
        modify(mac_init::R_4C1C, 1 << 30, 1 << 30);
        modify(mac_init::R_4C20, 0x0000_0fff, 0x0000_00f0);
        modify(mac_init::R_4C24, 0x0000_0fff, 0x0000_00f0);
        modify(mac_init::R_4CA8, 0x0000_00f0, 0x0000_0040);
        modify(mac_init::R_4C60, 0x7fff_0000, 0x7fff_0000);
        modify(mac_init::R_4C60, 1 << 31, 1 << 31);
        modify(mac_init::R_4308, 1 << 1, 1 << 1);
        modify(mac::RX_CONTROL, 1 << 31, 0);
    }
}

impl MacColdRxPolicyHardware for MockMmio {
    fn initialize_cold_receive_policy(&mut self) {
        fn modify(mmio: &mut MockMmio, register: TestRegister, mask: u32, value: u32) {
            let current = mmio.read32(register);
            mmio.write32(register, (current & !mask) | (value & mask));
        }

        for queue in 0..4 {
            let filter = mac_init::RX_FILTER[queue];
            modify(self, filter, 0x0000_0280, 0x0000_0280);
            modify(self, filter, 0x0000_0400, 0);
            modify(self, filter, 0x0000_0005, 0x0000_0005);
            modify(self, filter, 0x0000_2040, 0);

            if queue < 3 {
                let bssid = mac_init::BSSID_HIGH[queue];
                modify(self, filter, 0x0000_0410, 0);
                modify(
                    self,
                    bssid,
                    if queue == 1 { 0x4000_0000 } else { 0xc000_0000 },
                    if queue == 1 { 0x4000_0000 } else { 0 },
                );
                modify(self, filter, 0x0000_0040, 0);
                modify(self, bssid, 0x8000_0000, 0);
                modify(
                    self,
                    mac_init::INTERFACE_ADDRESS_HIGH[queue],
                    0x0000_ffff,
                    0,
                );
            }
        }
    }
}

impl MacLowRateHardware for MockMmio {
    fn disable_phy_low_rate(&mut self) {
        for (register, mask) in [
            (mac_init::R_8060, 1 << 10),
            (mac_init::R_8060, 1 << 11),
            (mac_init::R_807C, 1 << 11),
        ] {
            let current = self.read32(register);
            self.write32(register, current & !mask);
        }
    }
}

impl MacColdRxBufferHardware for MockMmio {
    fn initialize_rx_buffer_prefix(&mut self) {
        let mut modify = |register: TestRegister, mask: u32, value: u32| {
            let current = self.read32(register);
            self.write32(register, (current & !mask) | (value & mask));
        };
        modify(mac_init::R_4C68, 0x000f_ffff, 0x000f_ffff);
        modify(mac_init::R_4C6C, 0x000f_ffff, 4);
        modify(mac::RX_LAST_DESCRIPTOR_HIGH, 0xfff0_0000, 0x2f00_0000);
        modify(mac_init::R_407C, 0x0000_00ff, 0);
    }
}

impl TxHardware for MockMmio {
    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        assert_eq!(
            dma.descriptor_head() & 0x000f_ffff,
            program.plcp0 & 0x000f_ffff
        );
        let index = usize::from(queue);
        if self.read32(mac::TX_Q_CONTROL[index]) & TX_Q_ENABLE_VALID != 0 {
            return false;
        }
        let mut config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xffff_f000) | u32::from(program.timeout),
        );
        self.write32(mac::TX_Q_CONTROL[index], program.plcp0);
        self.write32(mac::TX_Q_PLCP1[index], program.plcp1);
        let ppdu = self.read32(mac::TX_Q_PPDU_CONTROL[index]);
        self.write32(mac::TX_Q_PPDU_CONTROL[index], ppdu & !0x08);
        let protection = self.read32(mac::TX_Q_PROTECTION[index]);
        self.write32(mac::TX_Q_PROTECTION[index], protection & 0x7fff_ffff);
        self.write32(mac::TX_Q_LENGTH_CONTROL[index], program.length_control);
        self.write32(mac::TX_Q_POWER[index], program.power);

        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0x0fff_ffff) | (u32::from(program.scheduler_priority) << 28),
        );
        for (mask, shift) in [
            (0xffff_0fff, 12),
            (0xffff_f0ff, 8),
            (0xffff_ff0f, 4),
            (0xfff0_ffff, 16),
        ] {
            let pti = self.read32(mac::TX_Q_PTI[index]);
            self.write32(
                mac::TX_Q_PTI[index],
                (pti & mask) | (u32::from(program.packet_priority) << shift),
            );
        }
        let pti = self.read32(mac::TX_Q_PTI[index]);
        self.write32(
            mac::TX_Q_PTI[index],
            (pti & 0x000f_ffff) | (u32::from(program.priority_count) << 20),
        );

        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xf0ff_ffff) | (u32::from(program.aifsn) << 24),
        );
        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xffc0_0fff) | (u32::from(program.contention_window) << 12),
        );
        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xff3f_ffff) | (program.interface.bits() << 22),
        );
        true
    }

    fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        assert_eq!(
            dma.descriptor_head() & 0x000f_ffff,
            program.plcp0 & 0x000f_ffff
        );
        let index = usize::from(queue);
        if self.read32(mac::TX_Q_CONTROL[index]) & TX_Q_ENABLE_VALID != 0 {
            return false;
        }
        let mut config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xffff_f000) | u32::from(program.timeout),
        );
        self.write32(mac::TX_Q_CONTROL[index], program.plcp0);
        self.write32(mac::TX_Q_PLCP1[index], program.plcp1);
        let ppdu = self.read32(mac::TX_Q_PPDU_CONTROL[index]);
        self.write32(mac::TX_Q_PPDU_CONTROL[index], ppdu & !0x08);
        let protection = self.read32(mac::TX_Q_PROTECTION[index]);
        self.write32(mac::TX_Q_PROTECTION[index], protection & 0x7fff_ffff);
        self.write32(mac::TX_Q_HT_SIGNAL[index], program.ht_signal);
        let descriptor_counts = self.read32(mac::TX_Q_HT_DESCRIPTOR_COUNTS[index]);
        self.write32(
            mac::TX_Q_HT_DESCRIPTOR_COUNTS[index],
            (descriptor_counts & !0x7f) | u32::from(program.descriptor_count_a),
        );
        let descriptor_counts = self.read32(mac::TX_Q_HT_DESCRIPTOR_COUNTS[index]);
        self.write32(
            mac::TX_Q_HT_DESCRIPTOR_COUNTS[index],
            (descriptor_counts & !(0x7f << 7)) | (u32::from(program.descriptor_count_b) << 7),
        );
        let descriptor_counts = self.read32(mac::TX_Q_HT_DESCRIPTOR_COUNTS[index]);
        self.write32(
            mac::TX_Q_HT_DESCRIPTOR_COUNTS[index],
            (descriptor_counts & !(0x7f << 14)) | (u32::from(program.descriptor_count_a) << 14),
        );
        for shift in [0, 10, 20] {
            let protection = self.read32(mac::TX_Q_PROTECTION[index]);
            self.write32(
                mac::TX_Q_PROTECTION[index],
                (protection & !(0x3ff << shift)) | (u32::from(program.protection_spacing) << shift),
            );
        }
        self.write32(mac::TX_Q_LENGTH_CONTROL[index], program.length_control);
        self.write32(mac::TX_Q_DATA_LENGTH[index], program.data_length);
        self.write32(mac::TX_Q_POWER[index], program.power);

        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0x0fff_ffff) | (u32::from(program.scheduler_priority) << 28),
        );
        for (mask, shift) in [
            (0xffff_0fff, 12),
            (0xffff_f0ff, 8),
            (0xffff_ff0f, 4),
            (0xfff0_ffff, 16),
        ] {
            let pti = self.read32(mac::TX_Q_PTI[index]);
            self.write32(
                mac::TX_Q_PTI[index],
                (pti & mask) | (u32::from(program.packet_priority) << shift),
            );
        }
        let pti = self.read32(mac::TX_Q_PTI[index]);
        self.write32(
            mac::TX_Q_PTI[index],
            (pti & 0x000f_ffff) | (u32::from(program.priority_count) << 20),
        );

        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xf0ff_ffff) | (u32::from(program.aifsn) << 24),
        );
        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xffc0_0fff) | (u32::from(program.contention_window) << 12),
        );
        config = self.read32(mac::TX_Q_CONFIG[index]);
        self.write32(
            mac::TX_Q_CONFIG[index],
            (config & 0xff3f_ffff) | (program.interface.bits() << 22),
        );
        true
    }

    fn start_bound_legacy_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8, plcp0: u32) {
        assert_eq!(dma.descriptor_head() & 0x000f_ffff, plcp0 & 0x000f_ffff);
        Mmio::fence(self);
        self.write32(
            mac::TX_Q_CONTROL[usize::from(queue)],
            plcp0 | TX_Q_ENABLE_VALID,
        );
        Mmio::fence(self);
    }

    fn start_bound_ht_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8, plcp0: u32) {
        assert_eq!(dma.descriptor_head() & 0x000f_ffff, plcp0 & 0x000f_ffff);
        Mmio::fence(self);
        self.write32(
            mac::TX_Q_CONTROL[usize::from(queue)],
            plcp0 | TX_Q_ENABLE_VALID,
        );
        Mmio::fence(self);
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        let index = usize::from(queue);
        let mask = 1_u32 << queue;
        if self.read32(TX_COMPLETE_STATE) & mask == 0 {
            return None;
        }
        let aux_a = self.read32(mac::TX_COMPLETE_AUX_A[index]);
        let aux_b = self.read32(mac::TX_COMPLETE_AUX_B[index]);
        let aux_c = self.read32(mac::TX_COMPLETE_AUX_C[index]);
        let primary = self.read32(mac::TX_COMPLETE_PRIMARY[index]);
        let alternate = self.read32(mac::TX_COMPLETE_ALTERNATE[index]);
        let trigger_flow = self.read32(TX_STATE) & (1_u32 << (24 + queue)) != 0;
        let clear = self.read32(TX_COMPLETE_CLEAR);
        self.write32(TX_COMPLETE_CLEAR, clear | mask);
        Mmio::fence(self);
        Some(MacTxCompletionRegisters {
            aux_a,
            aux_b,
            aux_c,
            primary,
            alternate,
            trigger_flow,
        })
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        let timeout_mask = 1_u32 << (TX_TIMEOUT_SHIFT + u32::from(queue));
        if self.read32(TX_STATE) & timeout_mask == 0 {
            return false;
        }
        let cca = self.read32(TX_CCA_CONTROL);
        self.write32(
            TX_CCA_CONTROL,
            (cca & !TX_CCA_FORCE_MASK) | TX_CCA_FORCE_MASK,
        );
        Mmio::fence(self);
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
        let control = self.read32(mac::TX_Q_CONTROL[index]);
        match reason {
            MacTxDetachReason::Collision => {
                let collision_mask = 1_u32 << queue;
                if self.read32(TX_STATE) & collision_mask == 0 {
                    return MacTxDetachOutcome::NoEvent;
                }
                self.write32(mac::TX_Q_CONTROL[index], control & !TX_Q_ENABLE_VALID);
                Mmio::fence(self);
                self.write32(TX_STATE_CLEAR, collision_mask);
            }
            MacTxDetachReason::Timeout => {
                let timeout_mask = 1_u32 << (TX_TIMEOUT_SHIFT + u32::from(queue));
                if self.read32(TX_STATE) & timeout_mask == 0 {
                    return MacTxDetachOutcome::NoEvent;
                }
                let was_valid = control & (1 << 30) != 0;
                self.write32(mac::TX_Q_CONTROL[index], control & !(1 << 30));
                let cca = self.read32(TX_CCA_CONTROL);
                self.write32(TX_CCA_CONTROL, cca & !TX_CCA_FORCE_MASK);
                if was_valid {
                    let invalid = self.read32(mac::TX_Q_CONTROL[index]);
                    self.write32(mac::TX_Q_CONTROL[index], invalid & !(1 << 31));
                }
                self.write32(TX_STATE_CLEAR, timeout_mask);
            }
            MacTxDetachReason::Completed => {
                self.write32(mac::TX_Q_CONTROL[index], control & !TX_Q_ENABLE_VALID);
            }
        }
        Mmio::fence(self);
        if self.read32(mac::TX_Q_CONTROL[index]) & TX_Q_ENABLE_VALID != 0 {
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
    EnableWifiMacClocks,
    EnableCoexistenceClock,
    ConfigureModemSourceClocks,
    SetWifiMacReset(bool),
    RequestMacDelayRandom,
    RequestSlowClockCalibration,
    RequestTxPower(u8),
    RequestCoexPti(MacCoexEvent),
}

#[derive(Default)]
struct MockPlatform {
    operations: Vec<PlatformOperation>,
}

impl MacClockControl for MockPlatform {
    fn enable_wifi_mac_clocks(&mut self) {
        self.operations.push(PlatformOperation::EnableWifiMacClocks);
    }

    fn enable_coexistence_clock(&mut self) {
        self.operations
            .push(PlatformOperation::EnableCoexistenceClock);
    }

    fn configure_modem_source_clocks(&mut self) {
        self.operations
            .push(PlatformOperation::ConfigureModemSourceClocks);
    }

    fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.operations
            .push(PlatformOperation::SetWifiMacReset(asserted));
    }
}

impl MacDelayEntropy for MockPlatform {
    fn mac_delay_random(&mut self) -> u32 {
        self.operations
            .push(PlatformOperation::RequestMacDelayRandom);
        7
    }
}

impl MacSlowClockCalibrationSource for MockPlatform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        self.operations
            .push(PlatformOperation::RequestSlowClockCalibration);
        MacSlowClockCalibration::Unavailable
    }
}

impl MacTxPowerSource for MockPlatform {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        self.operations
            .push(PlatformOperation::RequestTxPower(rate));
        MacTxPowerPair {
            primary: rate as i8,
            alternate: -(rate as i8),
        }
    }
}

impl MacCoexPtiSource for MockPlatform {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        self.operations
            .push(PlatformOperation::RequestCoexPti(event));
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
fn slow_clock_calibration_reproduces_vendor_eighteen_bit_truncation() {
    assert_eq!(MacSlowClockCalibration::Unavailable.register_value(), 0);
    assert_eq!(
        MacSlowClockCalibration::from_osi_value(0).register_value(),
        0
    );
    assert_eq!(
        MacSlowClockCalibration::from_osi_value(0x0003_ffff).register_value(),
        0x0003_ffff
    );
    assert_eq!(
        MacSlowClockCalibration::from_osi_value(0xabcd_0001).register_value(),
        0x0001_0001
    );
}

#[test]
fn cold_mac_init_uses_only_pac_registers_and_publishes_both_interfaces() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio::default();
    mmio.set(mac_init::HANDSHAKE, 1);

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
    assert_eq!(outcome.handshake_value, 3);
    assert_eq!(
        &mmio.operations()[..5],
        [
            Operation::Read(mac_init::HANDSHAKE),
            Operation::Write(mac_init::HANDSHAKE, 3),
            Operation::Read(mac_init::HANDSHAKE),
            Operation::WriteInterruptEnable(0),
            Operation::ClearInterrupt(u32::MAX),
        ]
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_LOW[0]),
        Some(&0x3322_1102)
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_HIGH[0]),
        Some(&0x0001_5544)
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_LOW[1]),
        Some(&0xccbb_aa02)
    );
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_HIGH[1]),
        Some(&0x0001_eedd)
    );
    assert!(mmio.operations().windows(8).any(|operations| {
        operations
            == [
                Operation::Write(mac_init::INTERFACE_ADDRESS_LOW[0], 0x3322_1102),
                Operation::Write(mac_init::INTERFACE_ADDRESS_HIGH[0], 0x0000_5544),
                Operation::Read(mac_init::INTERFACE_ADDRESS_HIGH[0]),
                Operation::Write(mac_init::INTERFACE_ADDRESS_HIGH[0], 0x0001_5544),
                Operation::Write(mac_init::INTERFACE_ADDRESS_LOW[1], 0xccbb_aa02),
                Operation::Write(mac_init::INTERFACE_ADDRESS_HIGH[1], 0x0000_eedd),
                Operation::Read(mac_init::INTERFACE_ADDRESS_HIGH[1]),
                Operation::Write(mac_init::INTERFACE_ADDRESS_HIGH[1], 0x0001_eedd),
            ]
    }));
    let last_rx_values = [
        0x0002_3006,
        0x0000_0608,
        0x0000_ffff,
        0x0002_3006,
        0x0000_0808,
        0x0000_ffff,
        0x0002_3006,
        0x0000_8e88,
        0x0000_ffff,
        0x0002_301c,
        0x4400_4300,
        0xffff_ffff,
        0x0002_301c,
        0x4300_4400,
        0xffff_ffff,
        0x0002_3011,
        0x0000_0001,
        0x0000_00ff,
    ];
    assert!(mmio.operations().windows(24).any(|operations| {
        operations[..18].iter().copied().eq(mac_init::LAST_RX_BUFFER
            .into_iter()
            .zip(last_rx_values)
            .map(|(register, value)| Operation::Write(register, value)))
            && operations[18..]
                == [
                    Operation::Read(mac_init::R_4120),
                    Operation::Write(mac_init::R_4120, 0x0000_3f00),
                    Operation::Read(mac_init::R_4120),
                    Operation::Write(mac_init::R_4120, 0x0000_3f7e),
                    Operation::Read(mac_init::R_4098),
                    Operation::Write(mac_init::R_4098, 0x0800_0000),
                ]
    }));
    assert!(
        mmio.operations()
            .contains(&Operation::ConfigureOpenPromiscuousReceive)
    );
    assert_eq!(mmio.interrupt_enable, 0x19a8_79e0);
    assert!(mmio.operations().windows(3).any(|operations| {
        operations
            == [
                Operation::Read(mac_init::R_4C00),
                Operation::Write(mac_init::R_4C00, 0),
                Operation::WriteInterruptEnable(0x19a8_79e0),
            ]
    }));
    assert_eq!(mmio.words.get(&mac_init::R_4C60), Some(&0xffff_0000));
    let expected_txrx_prefix = [
        (mac_init::R_4C8C, 0x8080_a000),
        (mac_init::R_4C8C, 0x8080_b000),
        (mac_init::R_4C8C, 0x9080_b000),
        (mac_init::R_4C98, 0),
        (mac_init::RX_QUEUE_DEFAULT[0], 0),
        (mac_init::RX_QUEUE_DEFAULT[1], 0),
        (mac_init::RX_QUEUE_DEFAULT[2], 0),
        (mac_init::RX_QUEUE_DEFAULT[3], 0),
        (mac_init::RX_QUEUE_DEFAULT[0], 0x0100_0000),
        (mac_init::RX_QUEUE_DEFAULT[1], 0x0100_0000),
        (mac_init::RX_QUEUE_DEFAULT[0], 0x0500_0000),
        (mac_init::RX_QUEUE_DEFAULT[1], 0x0500_0000),
        (mac_init::R_4C8C, 0x9080_b200),
        (mac_init::R_4114, 0x0000_0001),
        (mac_init::R_4114, 0x0000_0011),
        (mac_init::R_4118, 0x8000_0000),
        (mac_init::R_4118, 0x81b0_0000),
        (mac_init::R_4CA0, 0x0000_0003),
    ];
    assert!(mmio.operations().windows(36).any(|operations| {
        operations.chunks_exact(2).zip(expected_txrx_prefix).all(
            |(operation, (register, value))| {
                operation == [Operation::Read(register), Operation::Write(register, value)]
            },
        )
    }));
    assert!(mmio.operations().windows(16).any(|operations| {
        operations
            == [
                Operation::Read(mac_init::R_4C58),
                Operation::Write(mac_init::R_4C58, 0x000e_e400),
                Operation::Read(mac_init::R_4C58),
                Operation::Write(mac_init::R_4C58, 0x000e_e4fc),
                Operation::Read(mac_init::R_4C58),
                Operation::Write(mac_init::R_4C58, 0x0bce_e4fc),
                Operation::Read(mac_init::R_4C54),
                Operation::Write(mac_init::R_4C54, 0x2020_0000),
                Operation::Read(mac_init::R_4C54),
                Operation::Write(mac_init::R_4C54, 0x2029_d800),
                Operation::Write(mac_init::R_444C, 0x0009_0a0b),
                Operation::Write(mac_init::R_4458, 0x0009_0a0b),
                Operation::Write(mac_init::R_4450, 0x0005_0100),
                Operation::Write(mac_init::R_445C, 0x0005_0100),
                Operation::Read(mac_init::R_4C1C),
                Operation::Write(mac_init::R_4C1C, 0x0000_000f),
            ]
    }));
    let expected_txrx_suffix = [
        (mac_init::R_4C1C, 0x8000_000f),
        (mac_init::R_4C1C, 0xc000_000f),
        (mac_init::R_4C20, 0x0000_00f0),
        (mac_init::R_4C24, 0x0000_00f0),
        (mac_init::R_4CA8, 0x0000_0040),
        (mac_init::R_4C60, 0x7fff_0000),
        (mac_init::R_4C60, 0xffff_0000),
        (mac_init::R_4308, 0x0000_0002),
        (mac::RX_CONTROL, 0),
    ];
    assert!(mmio.operations().windows(18).any(|operations| {
        operations.chunks_exact(2).zip(expected_txrx_suffix).all(
            |(operation, (register, value))| {
                operation == [Operation::Read(register), Operation::Write(register, value)]
            },
        )
    }));
    let mut expected_cold_rx_policy = Vec::new();
    for queue in 0..4 {
        let filter = mac_init::RX_FILTER[queue];
        expected_cold_rx_policy.extend([
            Operation::Read(filter),
            Operation::Write(filter, 0x0000_0280),
            Operation::Read(filter),
            Operation::Write(filter, 0x0000_0280),
            Operation::Read(filter),
            Operation::Write(filter, 0x0000_0285),
            Operation::Read(filter),
            Operation::Write(filter, 0x0000_0285),
        ]);
        if queue < 3 {
            let bssid = mac_init::BSSID_HIGH[queue];
            let bssid_after_first_edge = if queue == 1 { 0x4000_0000 } else { 0 };
            expected_cold_rx_policy.extend([
                Operation::Read(filter),
                Operation::Write(filter, 0x0000_0285),
                Operation::Read(bssid),
                Operation::Write(bssid, bssid_after_first_edge),
                Operation::Read(filter),
                Operation::Write(filter, 0x0000_0285),
                Operation::Read(bssid),
                Operation::Write(bssid, bssid_after_first_edge),
                Operation::Read(mac_init::INTERFACE_ADDRESS_HIGH[queue]),
                Operation::Write(mac_init::INTERFACE_ADDRESS_HIGH[queue], 0),
            ]);
        }
    }
    assert_eq!(expected_cold_rx_policy.len(), 62);
    assert!(
        mmio.operations()
            .windows(expected_cold_rx_policy.len())
            .any(|operations| operations == expected_cold_rx_policy)
    );
    assert_eq!(mmio.words.get(&mac_init::R_4E04), Some(&0));
    assert!(mmio.operations().windows(8).any(|operations| {
        operations
            == [
                Operation::Read(mac_init::R_4C68),
                Operation::Write(mac_init::R_4C68, 0x000f_ffff),
                Operation::Read(mac_init::R_4C6C),
                Operation::Write(mac_init::R_4C6C, 4),
                Operation::Read(mac::RX_LAST_DESCRIPTOR_HIGH),
                Operation::Write(mac::RX_LAST_DESCRIPTOR_HIGH, 0x2f00_0000),
                Operation::Read(mac_init::R_407C),
                Operation::Write(mac_init::R_407C, 0),
            ]
    }));
    assert!(mmio.operations().windows(5).any(|operations| {
        operations
            == [
                Operation::Write(mac_init::CRYPTO_BYPASS[0], 0x0003_0000),
                Operation::Write(mac_init::CRYPTO_BYPASS[1], 0x0003_0000),
                Operation::Write(mac_init::CRYPTO_BYPASS[2], 0),
                Operation::Write(mac_init::CRYPTO_BYPASS[3], 0),
                Operation::Write(mac_init::CRYPTO_BYPASS[4], 0),
            ]
    }));
    assert!(mmio.operations().windows(6).any(|operations| {
        operations
            == [
                Operation::Read(mac_init::R_8060),
                Operation::Write(mac_init::R_8060, 0),
                Operation::Read(mac_init::R_8060),
                Operation::Write(mac_init::R_8060, 0),
                Operation::Read(mac_init::R_807C),
                Operation::Write(mac_init::R_807C, 0),
            ]
    }));
    assert!(mmio.operations().contains(&Operation::InitializeMacAntenna));
    assert!(mmio.operations().contains(&Operation::InitializeHalTail(
        0x19a8_79e0,
        MacSlowClockCalibration::Unavailable,
    )));
    assert!(
        mmio.operations()
            .iter()
            .any(|operation| matches!(operation, Operation::InitializeColdCoex(_)))
    );
    assert!(mmio.operations().contains(&Operation::InitializeHePrefix));
    assert!(
        mmio.operations()
            .iter()
            .any(|operation| matches!(operation, Operation::InitializeTxPower(_)))
    );
    assert!(mmio.operations().contains(&Operation::InitializeHeSuffix));
    let mut expected_platform = vec![
        PlatformOperation::EnableWifiMacClocks,
        PlatformOperation::EnableCoexistenceClock,
        PlatformOperation::ConfigureModemSourceClocks,
        PlatformOperation::SetWifiMacReset(true),
        PlatformOperation::SetWifiMacReset(false),
        PlatformOperation::RequestMacDelayRandom,
    ];
    expected_platform.extend((0..43).map(PlatformOperation::RequestTxPower));
    expected_platform.extend(
        (0..26)
            .filter(|rate| *rate != 4)
            .map(PlatformOperation::RequestTxPower),
    );
    expected_platform.extend([
        PlatformOperation::RequestSlowClockCalibration,
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event3),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event15),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event1),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event3),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event3),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event3),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event1),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event1),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event1),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event1),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event3),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event3),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event10),
        PlatformOperation::RequestCoexPti(MacCoexEvent::Event10),
    ]);
    assert_eq!(platform.operations, expected_platform);
    assert_eq!(
        mmio.operations().last(),
        Some(&Operation::ConfigureOpenPromiscuousReceive)
    );
}

#[test]
fn cold_mac_handshake_timeout_does_not_touch_interrupt_state() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio::default();

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
            observed: 2,
        })
    );
    assert_eq!(
        mmio.operations(),
        [
            Operation::Read(mac_init::HANDSHAKE),
            Operation::Write(mac_init::HANDSHAKE, 2),
            Operation::Read(mac_init::HANDSHAKE),
            Operation::Read(mac_init::HANDSHAKE),
        ]
    );
    assert!(!mmio.operations().iter().any(|operation| matches!(
        operation,
        Operation::WriteInterruptEnable(_) | Operation::ClearInterrupt(_)
    )));
}

#[test]
fn sta_link_rx_policy_matches_live_vendor_policy_six() {
    let bssid = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let mut mmio = MockMmio::default();
    mmio.set(mac_init::RX_FILTER[0], u32::MAX);
    mmio.set(mac_init::BSSID_HIGH[0], u32::MAX);
    mmio.set(mac_init::INTERFACE_ADDRESS_HIGH[0], 0x0000_5544);

    configure_sta_link_receive_policy(&mut mmio, bssid);

    assert_eq!(
        mmio.words.get(&mac_init::RX_FILTER[0]),
        Some(&!((1 << 10) | (1 << 6) | (1 << 4)))
    );
    assert_eq!(mmio.words.get(&mac_init::BSSID_LOW[0]), Some(&0x54c8_15dc));
    assert_eq!(mmio.words.get(&mac_init::BSSID_HIGH[0]), Some(&0xbfff_1ebc));
    assert_eq!(
        mmio.words.get(&mac_init::INTERFACE_ADDRESS_HIGH[0]),
        Some(&0x0001_5544)
    );
    assert_eq!(
        mmio.operations()
            .iter()
            .filter(|operation| **operation == Operation::Fence)
            .count(),
        1
    );
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
    mmio.set(RX_LAST_DESCRIPTOR_HIGH, 0x0005_4321);
    mmio.set(RX_CONTROL, 0x1234);
    publish_cold_ring(&mut mmio, 0x2f00_1000, true).unwrap();

    assert_eq!(
        mmio.operations(),
        &[
            Operation::Fence,
            Operation::Read(RX_LAST_DESCRIPTOR_HIGH),
            Operation::Write(RX_LAST_DESCRIPTOR_HIGH, 0x2f05_4321),
            Operation::Write(RX_DESCRIPTOR_BASE, 0x2f00_1000),
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, 0x1234 | RX_ENABLE),
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_CONTROL, RX_ENABLE);

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
    assert_eq!(mmio.words.get(&RX_DESCRIPTOR_BASE), Some(&BASE));
    let disable = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::Write(RX_CONTROL, 0))
        .unwrap();
    let retained_last = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::Read(RX_LAST_DESCRIPTOR))
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + 2 * DESCRIPTOR_BYTES);
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
    assert_ne!(mmio.words[&RX_CONTROL] & RX_RELOAD, 0);
    assert!(live.reload_pending());
    assert_eq!(live.accepted_tail(), 3);

    descriptors[2].write_word0(completed);
    descriptors[3].write_word0(completed);
    assert!(live.take_completed(2).is_some());
    assert!(live.take_completed(3).is_some());

    // Model bit-0 self-clear at a terminal frontier. ROM repairs BASE from the
    // last accepted descriptor's now-published next link before accepting the
    // pending tail and appending the following group.
    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, 0);
    mmio.set(RX_LAST_DESCRIPTOR, BASE + 3 * DESCRIPTOR_BYTES);
    mmio.operations.clear();
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(
        &mmio.operations()[..6],
        &[
            Operation::Read(RX_CONTROL),
            Operation::Read(RX_NEXT_DESCRIPTOR),
            Operation::Fence,
            Operation::Read(RX_LAST_DESCRIPTOR),
            Operation::Fence,
            Operation::Write(RX_DESCRIPTOR_BASE, BASE),
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
    mmio.set(RX_NEXT_DESCRIPTOR, BASE);
    assert!(
        live.recycle_completed_half(&mut mmio, |_| Ok(()))
            .unwrap()
            .is_none()
    );
    // Repeated NEXT observations still do not release descriptor three's
    // link. A later completed LAST does.
    descriptors[0].write_word0(descriptors[0].word0() | BIT_30);
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
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
            .contains(&Operation::Write(RX_DESCRIPTOR_BASE, BASE,))
    );
    assert_eq!(live.accepted_tail(), 1);
    assert!(live.reload_pending());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn reload_repair_observation_reads_last_only_after_zero_next() {
    const BASE: u32 = 0x2f00_1000;
    let mut mmio = MockMmio::default();
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_LAST_DESCRIPTOR, BASE);

    let active = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(active, ((BASE + DESCRIPTOR_BYTES) & 0x000f_ffff, None));
    assert_eq!(
        mmio.operations(),
        &[Operation::Read(RX_NEXT_DESCRIPTOR), Operation::Fence],
    );

    // The complete vendor leaf compares the whole register word with zero.
    // A zero address projection with nonzero upper status bits is not the
    // terminal branch and must not authorize a stale BASE repair.
    mmio.operations.clear();
    mmio.set(RX_NEXT_DESCRIPTOR, 0xa5a0_0000);
    let upper_status = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_word(),
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(upper_status, (0xa5a0_0000, 0, None));
    assert_eq!(
        mmio.operations(),
        &[Operation::Read(RX_NEXT_DESCRIPTOR), Operation::Fence],
    );

    mmio.operations.clear();
    mmio.set(RX_NEXT_DESCRIPTOR, 0);
    let exhausted = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(exhausted, (0, Some(BASE & 0x000f_ffff)));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::Read(RX_NEXT_DESCRIPTOR),
            Operation::Fence,
            Operation::Read(RX_LAST_DESCRIPTOR),
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
        mmio.set(
            RX_LAST_DESCRIPTOR,
            BASE + retained_index as u32 * DESCRIPTOR_BYTES,
        );

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
    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_LAST_DESCRIPTOR, BASE);

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();

    assert_eq!(
        stopped.retained_next_low(),
        (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff
    );
    assert_eq!(stopped.retained_last_low(), BASE & 0x000f_ffff);
    assert_eq!(stopped.initial_start(), 1);
    assert_eq!(stopped.accepted_tail(), 0);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.words[&RX_DESCRIPTOR_BASE], BASE + DESCRIPTOR_BYTES);
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
    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + COUNT as u32 * DESCRIPTOR_BYTES);

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
    mmio.set(
        RX_LAST_DESCRIPTOR,
        BASE + (COUNT as u32 - 2) * DESCRIPTOR_BYTES,
    );

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();

    assert_eq!(stopped.initial_start(), 0);
    assert_eq!(stopped.accepted_tail(), COUNT - 1);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[COUNT - 1].next_address(), 0);
    assert_eq!(mmio.words[&RX_DESCRIPTOR_BASE], BASE);
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
        mmio.set(RX_LAST_DESCRIPTOR, retained_last);
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
    mmio.set(RX_CONTROL, 0);
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail(), 0);

    descriptors[1].write_word0(completed);
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + 2 * DESCRIPTOR_BYTES);
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    mmio.set(RX_NEXT_DESCRIPTOR, 0);
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
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
    assert_eq!(mmio.words[&RX_DESCRIPTOR_BASE], BASE + DESCRIPTOR_BYTES);
    assert_eq!(live.recycle_start(), 1);
    assert!(!live.reload_pending());
    assert!(live.exhausted_republication_probe_pending());
    assert!(!mmio.operations().iter().any(|operation| {
        matches!(operation, Operation::Write(register, value) if *register == RX_CONTROL && value & RX_RELOAD != 0)
    }));

    // A timer is not evidence that hardware accepted BASE. Keep polling while
    // NEXT is still exhausted. Even an exact cursor match retains one final
    // cooperative probe: the returned head may complete while this task is
    // still consuming the IRQ which exhausted the preceding list.
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    mmio.set(RX_NEXT_DESCRIPTOR, BASE);
    live.observe_exhausted_republication(&mut mmio);
    assert!(
        live.exhausted_republication_probe_pending(),
        "a nonzero cursor outside the republished head is stale evidence"
    );
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    live.observe_exhausted_republication(&mut mmio);
    assert!(!live.exhausted_republication_probe_pending());

    // Hardware resumes at the newly published head. The next RX edge must
    // inspect that same descriptor rather than the physical slot after the
    // returned chain's tail.
    descriptors[1].write_word0(completed);
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    let head_low = BASE & 0x000f_ffff;
    let successor_low = (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff;
    mmio.set(RX_NEXT_DESCRIPTOR, 0);
    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    // Even repeated exact old-successor observations are not ownership
    // evidence. HIL reproduced the stale link fetch after two such samples.
    mmio.set(RX_NEXT_DESCRIPTOR, successor_low);
    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    descriptors[1].write_word0(completed);
    let later_low = (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff;
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
            mmio.set(RX_LAST_DESCRIPTOR, BASE + cursor as u32 * DESCRIPTOR_BYTES);
            assert!(live.take_completed(cursor).is_some());

            // LAST/RX_DONE without the old successor in NEXT does not release
            // the link word. A failed probe must be a read-only transaction.
            mmio.set(RX_NEXT_DESCRIPTOR, 0);
            let before_word0 = descriptors[cursor].word0();
            assert!(
                live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].word0(), before_word0);
            assert_eq!(descriptors[cursor].next_address(), old_next);

            // Even a stable exact successor is not a link-ownership proof.
            mmio.set(RX_NEXT_DESCRIPTOR, old_next);
            assert!(
                live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].next_address(), old_next);
            let later = (cursor + 1) % COUNT;
            descriptors[later].write_word0(descriptors[later].word0() | BIT_30);
            mmio.set(RX_LAST_DESCRIPTOR, BASE + later as u32 * DESCRIPTOR_BYTES);
            let append = live
                .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                .unwrap()
                .unwrap();
            assert_eq!(append.head_index, cursor);
            assert_eq!(append.tail_index, cursor);
            assert_eq!(descriptors[cursor].next_address(), 0);
            assert_eq!(descriptors[cursor].word0() & BIT_30, 0);
            assert!(live.topology_snapshot().valid);

            mmio.set(RX_CONTROL, RX_ENABLE);
            mmio.set(RX_NEXT_DESCRIPTOR, old_next);
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    let pending_tail_low = BASE & 0x000f_ffff;
    let frontier = live.completed_unit_frontier_through_with(pending_tail_low, |_| true);
    assert_eq!(frontier, Default::default());

    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    assert_eq!(live.completed_frontier_len(), 0);
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let first = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.descriptor_count, 1);

    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
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
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + 2 * DESCRIPTOR_BYTES);

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
    mmio.set(RX_LAST_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + 2 * DESCRIPTOR_BYTES);
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

    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );

    descriptors[2].write_word0(completed);
    mmio.set(RX_LAST_DESCRIPTOR, BASE + 2 * DESCRIPTOR_BYTES);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + 3 * DESCRIPTOR_BYTES);
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
fn staged_rx_frame_remains_private_until_reload_settles() {
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
    descriptors[0].write_word0(BUFFER_SIZE | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    let completed = live.take_completed(0).unwrap();
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let pool = RxStagePool::<1, 16>::new();
    mmio.operations.clear();
    let mut pending = pool
        .stage_recycle(completed, &[1, 2, 3, 4], &mut mmio, &mut live, |_| Ok(()))
        .unwrap();

    assert_eq!(
        mmio.operations(),
        &[
            Operation::Read(RX_CONTROL),
            Operation::Read(RX_CONTROL),
            Operation::Read(RX_LAST_DESCRIPTOR),
            Operation::Fence,
            Operation::Read(RX_NEXT_DESCRIPTOR),
            Operation::Fence,
            Operation::Fence,
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, RX_ENABLE | RX_RELOAD),
            Operation::Fence,
        ],
        "a confirmed release, descriptor publication and reload retain their exact device-ordering boundaries",
    );

    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(pool.network_slots(), 0);
    assert_eq!(live.accepted_tail(), 1);

    mmio.set(RX_CONTROL, RX_ENABLE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    let network = pending.complete_reload(&mut mmio, &mut live).unwrap();
    assert_eq!(network.segment().buffer, &[1, 2, 3, 4]);
    assert_eq!(pool.network_slots(), 1);
    assert_eq!(live.accepted_tail(), 0);
    drop(network);
    assert_eq!(pool.claimed_slots(), 0);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn staged_rx_reload_timeout_is_exact_and_releases_the_private_copy() {
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
    descriptors[0].write_word0(BUFFER_SIZE | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    mmio.set(RX_LAST_DESCRIPTOR, BASE);
    mmio.set(RX_NEXT_DESCRIPTOR, BASE + DESCRIPTOR_BYTES);
    let completed = live.take_completed(0).unwrap();
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let pool = RxStagePool::<1, 16>::new();
    let mut pending = pool
        .stage_recycle(completed, &[1, 2, 3, 4], &mut mmio, &mut live, |_| Ok(()))
        .unwrap();

    let reload_reads_before = mmio
        .operations()
        .iter()
        .filter(|operation| matches!(operation, Operation::Read(RX_CONTROL)))
        .count();
    assert!(matches!(
        pending.complete_reload(&mut mmio, &mut live),
        Err(RxStageTransactionError::Ring(RxRingError::Busy))
    ));
    let reload_reads_after = mmio
        .operations()
        .iter()
        .filter(|operation| matches!(operation, Operation::Read(RX_CONTROL)))
        .count();
    assert_eq!(
        reload_reads_after - reload_reads_before,
        RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT as usize
    );
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(pool.network_slots(), 0);
    assert!(live.reload_pending());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn receive_disable_confirms_the_ring_ownership_edge() {
    let mut mmio = MockMmio::default();
    mmio.set(RX_CONTROL, RX_ENABLE | 0x1234);
    disable_receive(&mut mmio).unwrap();
    assert_eq!(mmio.words.get(&RX_CONTROL), Some(&0x1234));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, 0x1234),
            Operation::Fence,
            Operation::Read(RX_CONTROL),
        ]
    );
}

#[test]
fn receive_enable_is_a_separate_confirmed_hardware_edge() {
    let mut mmio = MockMmio::default();
    mmio.set(RX_CONTROL, 0x1234);
    enable_receive(&mut mmio).unwrap();
    assert_eq!(mmio.words.get(&RX_CONTROL), Some(&(RX_ENABLE | 0x1234)));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::Read(RX_CONTROL),
            Operation::Write(RX_CONTROL, RX_ENABLE | 0x1234),
            Operation::Fence,
            Operation::Read(RX_CONTROL),
        ]
    );

    let mut already_enabled = MockMmio::default();
    already_enabled.set(RX_CONTROL, RX_ENABLE | 0x1234);
    assert_eq!(enable_receive(&mut already_enabled), Err(RxRingError::Busy));
    assert_eq!(already_enabled.operations(), &[Operation::Read(RX_CONTROL)]);
}

#[test]
fn sta_pairwise_ccmp_install_owns_one_bounded_hardware_slot() {
    let mut mmio = MockMmio::default();
    mmio.set(mac::CRYPTO_POLICY_CONTROL, u32::MAX);
    let peer = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let temporal_key = core::array::from_fn(|index| index as u8);

    let mut slot = install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 4);
    assert_eq!(slot.peer(), &peer);
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP),
        Some(&(1 << 4))
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 0).unwrap()),
        Some(&0x54c8_15dc)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 1).unwrap()),
        Some(&0x086c_1ebc)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 2).unwrap()),
        Some(&0x0302_0100)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 5).unwrap()),
        Some(&0x0f0e_0d0c)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(4, 6).unwrap()),
        Some(&0)
    );
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_INTERFACE_CONTROL[0]),
        Some(&0x0003_0103)
    );
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_POLICY_CONTROL),
        Some(&0xffc0_003f)
    );
    assert_eq!(slot.next_tx_ccmp_header(), Ok([3, 0, 0, 0x20, 0, 0, 0, 0]));
    assert_eq!(slot.next_tx_ccmp_header(), Ok([6, 0, 0, 0x20, 0, 0, 0, 0]));

    slot.clear(&mut mmio);
    assert_eq!(mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP), Some(&0));
    for word in 0..mac::CRYPTO_KEY_ENTRY_WORDS {
        assert_eq!(
            mmio.words
                .get(&mac::crypto_key_entry_word(4, word).unwrap()),
            Some(&0)
        );
    }

    mmio.set(mac::CRYPTO_KEY_VALID_BITMAP, 1 << 4);
    assert_eq!(
        install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).err(),
        Some(CryptoKeyError::Occupied)
    );
}

#[test]
fn sta_group_ccmp_install_matches_the_migration_slot_and_control_word() {
    let mut mmio = MockMmio::default();
    mmio.set(mac::CRYPTO_POLICY_CONTROL, u32::MAX);
    let temporal_key = core::array::from_fn(|index| 0xf0 | index as u8);

    let slot = install_sta_group_ccmp(&mut mmio, 1, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 1);
    assert_eq!(slot.key_id(), 1);
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP),
        Some(&(1 << 1))
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 0).unwrap()),
        Some(&u32::MAX)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 1).unwrap()),
        Some(&0x48cc_ffff)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 2).unwrap()),
        Some(&0xf3f2_f1f0)
    );
    assert_eq!(
        mmio.words.get(&mac::crypto_key_entry_word(1, 5).unwrap()),
        Some(&0xfffe_fdfc)
    );

    slot.clear(&mut mmio);
    assert_eq!(mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP), Some(&0));
    assert_eq!(
        install_sta_group_ccmp(&mut mmio, 4, &temporal_key).err(),
        Some(CryptoKeyError::InvalidGroupKeyId)
    );
}

#[test]
fn station_key_teardown_consumes_and_clears_both_hardware_slots() {
    let mut mmio = MockMmio::default();
    mmio.set(mac::CRYPTO_POLICY_CONTROL, u32::MAX);
    let pairwise = install_sta_pairwise_ccmp(&mut mmio, [1, 2, 3, 4, 5, 6], &[0x55; 16]).unwrap();
    let group = install_sta_group_ccmp(&mut mmio, 2, &[0xaa; 16]).unwrap();
    assert_eq!(
        mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP),
        Some(&((1 << 4) | (1 << 1)))
    );

    let report = clear_sta_ccmp_slots(&mut mmio, pairwise, group);
    assert_eq!(report.pairwise_hardware_index, 4);
    assert_eq!(report.group_hardware_index, 1);
    assert_eq!(mmio.words.get(&mac::CRYPTO_KEY_VALID_BITMAP), Some(&0));
    let clears: std::vec::Vec<_> = mmio
        .operations()
        .iter()
        .filter_map(|operation| match operation {
            Operation::Write(address, value) if *address == mac::CRYPTO_KEY_VALID_BITMAP => {
                Some(*value)
            }
            _ => None,
        })
        .collect();
    assert!(clears.ends_with(&[1 << 4, 0]));
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
fn irq_state_coalesces_known_bits_and_records_unknown_bits() {
    let mut mmio = MockMmio {
        interrupt_enable: u32::MAX,
        interrupt_status: MAC_INT_TX_COMPLETE
            | MAC_INT_RX_SUCCESS
            | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK
            | 0x8000_0000,
        ..MockMmio::default()
    };
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert_eq!(snapshot.auxiliary, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK);
    assert_eq!(snapshot.unhandled, 0x8000_0000);
    assert_eq!(state.observed_unhandled(), 0x8000_0000);
    let event = state.try_take().unwrap();
    assert_eq!(event.mac_pending, MAC_INT_TX_COMPLETE | MAC_INT_RX_SUCCESS);
    assert_eq!(mmio.operations().last(), Some(&Operation::Fence));
    assert!(
        mmio.operations()
            .contains(&Operation::ClearInterrupt(snapshot.status))
    );
}

#[test]
fn irq_acknowledges_auxiliary_status_without_posting_independent_work() {
    let mut mmio = MockMmio {
        interrupt_enable: u32::MAX,
        interrupt_status: MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
        ..MockMmio::default()
    };
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

    assert_eq!(disposition, IrqDisposition::AcknowledgedOnly);
    assert_eq!(snapshot.handled, 0);
    assert_eq!(snapshot.auxiliary, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK);
    assert_eq!(snapshot.unhandled, 0);
    assert_eq!(state.try_take(), None);
    assert_eq!(state.observed_unhandled(), 0);
    assert!(
        mmio.operations()
            .contains(&Operation::ClearInterrupt(snapshot.status))
    );
}

#[test]
fn irq_state_exposes_vendor_run_to_completion_order() {
    let mut mmio = MockMmio {
        interrupt_enable: u32::MAX,
        interrupt_status: MAC_INT_COLLISION
            | MAC_INT_TX_TIMEOUT
            | MAC_INT_TX_COMPLETE
            | MAC_INT_RX_SUCCESS,
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
    mmio.set(TX_COMPLETE_STATE, TX_COMPLETE_Q0);
    mmio.set(TX_COMPLETE_AUX_A_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_B_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_C_Q0, 0);
    mmio.set(TX_COMPLETE_PRIMARY_Q0, 3 << 12);
    mmio.set(TX_COMPLETE_ALTERNATE_Q0, 7 << 12);
    mmio.set(TX_STATE, 1 << 24);
    mmio.set(TX_COMPLETE_CLEAR, 0x100);

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(completion.cookie, cookie);
    assert_eq!(completion.status, 3);
    assert!(completion.trigger_flow);
    assert!(!completion.used_alternate);
    assert_eq!(slot.state(), TxSlotState::Completed);

    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID | 0x100);
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
    mmio.set(TX_COMPLETE_STATE, TX_COMPLETE_Q0);
    mmio.set(TX_COMPLETE_AUX_A_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_B_Q0, 0);
    mmio.set(TX_COMPLETE_AUX_C_Q0, 0);
    // Encoded 0x8b plus the pinned 0x60 offset narrows to signed -21.
    mmio.set(TX_COMPLETE_PRIMARY_Q0, 0x8b << 16);
    mmio.set(TX_COMPLETE_ALTERNATE_Q0, 0);
    mmio.set(TX_COMPLETE_CLEAR, 0);

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(completion.status, 0);
    assert_eq!(completion.ack_snr_sample(), Some(-21));

    let failed = TxCompletion {
        status: 5,
        ..completion
    };
    assert_eq!(failed.ack_snr_sample(), None);

    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
}

#[test]
fn tx_slot_reproduces_the_migration_timeout_abort_order() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let timeout_mask = 1 << TX_TIMEOUT_SHIFT;
    let mut mmio = MockMmio::default();
    mmio.set(TX_STATE, timeout_mask);
    mmio.set(TX_CCA_CONTROL, 0x1234_5678);
    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID | 0x100);

    assert_eq!(
        slot.as_mut().begin_timeout_abort(&mut mmio, cookie),
        Ok(true)
    );
    assert_eq!(
        mmio.words.get(&TX_CCA_CONTROL).copied().unwrap() & TX_CCA_FORCE_MASK,
        TX_CCA_FORCE_MASK,
    );
    slot.as_mut()
        .finish_timeout_abort(&mut mmio, cookie)
        .unwrap();

    assert_eq!(slot.state(), TxSlotState::Free);
    assert_eq!(
        mmio.words.get(&TX_Q0_CONTROL).copied().unwrap() & TX_Q_ENABLE_VALID,
        0,
    );
    assert_eq!(
        mmio.words.get(&TX_CCA_CONTROL).copied().unwrap() & TX_CCA_FORCE_MASK,
        0,
    );
    assert!(
        mmio.operations()
            .contains(&Operation::Write(TX_STATE_CLEAR, timeout_mask))
    );

    let invalidation = mmio
        .operations()
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Write(register, value)
                if *register == TX_Q0_CONTROL && value & (1 << 30) == 0)
        })
        .unwrap();
    let cca_release = mmio
        .operations()
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Write(register, value)
                if *register == TX_CCA_CONTROL && value & TX_CCA_FORCE_MASK == 0)
        })
        .unwrap();
    let timeout_clear = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::Write(TX_STATE_CLEAR, timeout_mask))
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
    mmio.set(TX_STATE, 1);
    mmio.set(TX_Q0_CONTROL, TX_Q_ENABLE_VALID | 0x100);

    assert_eq!(slot.as_mut().abort_collision(&mut mmio, cookie), Ok(true));
    assert_eq!(slot.state(), TxSlotState::Free);

    let disable = mmio
        .operations()
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Write(register, value)
                if *register == TX_Q0_CONTROL && value & TX_Q_ENABLE_VALID == 0)
        })
        .unwrap();
    let acknowledge = mmio
        .operations()
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Write(register, value)
                if *register == TX_STATE_CLEAR && *value == 1)
        })
        .unwrap();
    assert!(disable < acknowledge);
}

#[test]
fn legacy_q0_image_reproduces_the_recovered_management_profile() {
    let image = legacy_q0_image(0x2f00_5000, LegacyTxConfig::management_1m(0x40)).unwrap();
    assert_eq!(image.plcp0, 0x0160_5000);
    assert_eq!(image.plcp1, 0x0000_0040);
    assert_eq!(image.power, 0x0808_0008);
    assert_eq!(image.length_control, 0x0040_0004);
    assert_eq!(LegacyTxConfig::management_1m(0x40).timeout, 0x03ff);
    assert_eq!(LegacyTxConfig::management_1m(0x40).scheduler_priority, 1);
    assert_eq!(LegacyTxConfig::management_1m(0x40).pti, 1);
    assert_eq!(LegacyTxConfig::management_1m(0x40).pti_count, 1);
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
fn ht_single_mpdu_image_matches_complete_blob_word_formulas() {
    let rate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    let mut config = HtTxConfig::single_mpdu(rate, 0x0117, 8).unwrap();
    assert_eq!(config.length, 0x0123);
    assert_eq!(
        config.timeout,
        TxLifetimeClass::DirectMpdu.fresh_queue_timeout()
    );
    config.data_power_primary = 1;
    config.data_power_alternate = 2;
    config.rts_power_primary = 3;
    config.rts_power_alternate = 4;
    config.hardware_key_selector = 4;
    config.protection_spacing = HtProtectionSpacing::Density5;

    let image = ht_q0_image(0x2f00_5000, config).unwrap();
    assert_eq!(image.plcp0, 0x0160_5000);
    assert_eq!(image.plcp1, 0x0208_1000);
    assert_eq!(image.ht_signal, 0x8701_2387);
    assert_eq!(image.data_length, 0x7000_0123);
    assert_eq!(image.power, 0x0403_0201);
    assert_eq!(image.length_control, 0x0000_0244);
    assert_eq!(image.descriptor_count_a, 1);
    assert_eq!(image.descriptor_count_b, 1);
    assert_eq!(image.protection_spacing, 40);
}

#[test]
fn ht_ampdu_image_matches_the_two_mpdu_vendor_oracle() {
    let rate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz20,
    );
    let mut config = HtAmpduTxConfig::new(rate, 0x0c2e, 2).unwrap();
    assert_eq!(
        config.timeout,
        TxLifetimeClass::AmpduContainer.fresh_queue_timeout()
    );
    config.data_power_primary = 1;
    config.data_power_alternate = 2;
    config.rts_power_primary = 3;
    config.rts_power_alternate = 4;
    config.hardware_key_selector = 4;
    config.protection_spacing = HtProtectionSpacing::Density5;

    let image = ht_ampdu_q0_image(0x2f00_5000, config).unwrap();
    assert_eq!(image.plcp0, 0x0260_5000);
    assert_eq!(image.plcp1, 0x0208_1000);
    assert_eq!(image.ht_signal, 0x8f0c_2e07);
    assert_eq!(image.data_length, 0x7040_0c2e);
    assert_eq!(image.power, 0x0403_0201);
    assert_eq!(image.length_control, 0x0040_0244);
    assert_eq!(image.descriptor_count_a, 2);
    assert_eq!(image.descriptor_count_b, 2);
    assert_eq!(image.protection_spacing, 40);
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
fn he20_mcs9_image_matches_the_vendor_vector_and_blob_derived_spacing() {
    let rate = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let density = HtAmpduDensity::SixteenMicroseconds;
    let mut config = HeAmpduTxConfig::new(rate, 27, 0x76, 1, density).unwrap();
    assert_eq!(
        config.timeout,
        TxLifetimeClass::AmpduContainer.fresh_queue_timeout()
    );
    config.data_power_primary = 5;
    config.data_power_alternate = 5;
    config.rts_power_primary = 5;
    config.rts_power_alternate = 5;
    config.hardware_key_selector = 4;

    let image = he_ampdu_q0_image(0x2f03_1638, config).unwrap();
    assert_eq!(rate.code(), 35);
    assert_eq!(rate.power_lookup_code(), 25);
    assert_eq!(rate.nominal_kbps(), 114_700);
    assert_eq!(image.plcp0, 0x0563_1638);
    assert_eq!(image.plcp1, 0x0408_3000);
    assert_eq!(image.he_signal_a1, 0xfc20_5b4f);
    assert_eq!(image.he_signal_a2_length, 0x1003_b105);
    assert_eq!(image.power, 0x0505_0505);
    assert_eq!(image.length_control, 0x0040_0244);
    assert_eq!(image.descriptor_count_a, 1);
    assert_eq!(image.descriptor_count_b, 1);
    assert_eq!(image.protection_spacing, 230);
    assert_eq!(config.rate(), rate);
    assert_eq!(config.ampdu_density(), density);
    assert_eq!(config.protection_spacing(), 230);

    config = HeAmpduTxConfig::new(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::FourLtf3200Ns),
        27,
        0x76,
        1,
        density,
    )
    .unwrap();
    assert_eq!(
        he_ampdu_q0_image(0x2f03_1638, config).unwrap().he_signal_a1,
        0xfc60_5b4f,
    );
}

#[test]
fn he20_dcm_smpdu_image_matches_the_synchronous_vendor_oracle() {
    let rate = HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let mut config = HeSmpduTxConfig::new(rate, 0, 24).unwrap();
    assert_eq!(
        config.timeout,
        TxLifetimeClass::AmpduContainer.fresh_queue_timeout()
    );
    config.data_power_primary = 5;
    config.data_power_alternate = 5;
    config.rts_power_primary = 5;
    config.rts_power_alternate = 5;

    let image = he_smpdu_q0_image(0x2f03_1638, config).unwrap();
    assert_eq!(config.apep_length(), 32);
    assert_eq!(image.plcp0, 0x0163_1638);
    assert_eq!(image.plcp1, 0x0401_a000);
    assert_eq!(image.he_signal_a1, 0xfc20_4087);
    assert_eq!(image.he_signal_a2_length, 0x0001_0105);
    assert_eq!(image.power, 0x0505_0505);
    assert_eq!(image.length_control, 0x0040_02c4);
    assert_eq!(image.descriptor_count_a, 1);
    assert_eq!(image.descriptor_count_b, 1);
    assert_eq!(image.protection_spacing, 0x31);
}

#[test]
fn he20_formatter_covers_mcs0_through_mcs9_and_every_gi_ltf() {
    for mcs in 0..=9 {
        let mcs = HeMcs::from_index(mcs).unwrap();
        for gi_ltf in [
            HeGuardIntervalAndLtf::OneLtf800Ns,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
            HeGuardIntervalAndLtf::FourLtf3200Ns,
        ] {
            let rate = HeRate::new(mcs, gi_ltf);
            let image = he_ampdu_q0_image(
                0x2f00_5000,
                HeAmpduTxConfig::new(rate, 27, 312, 2, HtAmpduDensity::FourMicroseconds).unwrap(),
            )
            .unwrap();
            assert_eq!((image.plcp0 >> 24) & 7, 5);
            assert_eq!((image.he_signal_a1 >> 3) & 0x0f, u32::from(mcs.index()));
            assert_eq!(
                (image.he_signal_a1 >> 21) & 0x03,
                u32::from(gi_ltf.encoding()),
            );
            assert_eq!((image.he_signal_a2_length >> 11) & 0xffff, 312);
        }
    }
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
        let config =
            HeAmpduTxConfig::new(rate, 27, 312, 2, HtAmpduDensity::FourMicroseconds).unwrap();
        let image = he_ampdu_q0_image(0x2f00_5000, config).unwrap();
        assert_eq!((image.plcp1 >> 12) & 0x1f, u32::from(0x1a + expected_index));
        assert_eq!((image.he_signal_a1 >> 3) & 0x0f, u32::from(expected_index));
        assert_ne!(image.he_signal_a1 & (1 << 7), 0);
        // DCM does not change the bounded BCC coding/STBC A2 control image.
        assert_eq!(image.he_signal_a2_length & 0x7ff, 0x105);
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
    let ordinary_image = he_ampdu_q0_image(
        0x2f00_5000,
        HeAmpduTxConfig::new(ordinary, 27, 312, 2, HtAmpduDensity::FourMicroseconds).unwrap(),
    )
    .unwrap();
    // Complete blob and ROM mac_tx_set_hesig transform certification BCC=0
    // from intermediate 0x01ff to this queue-control low eleven-bit image.
    assert_eq!(ordinary_image.he_signal_a2_length & 0x7ff, 0x107);

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

        let image = he_ampdu_q0_image(
            0x2f00_5000,
            HeAmpduTxConfig::new(rate, 27, 312, 2, HtAmpduDensity::FourMicroseconds).unwrap(),
        )
        .unwrap();
        assert_eq!((image.plcp1 >> 12) & 0x1f, 0x1e);
        assert_eq!((image.he_signal_a1 >> 3) & 0x0f, 4);
        assert_ne!(image.he_signal_a1 & (1 << 7), 0);
        assert_eq!(image.he_signal_a2_length & 0x7ff, 0x107);
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
fn ht_single_and_ampdu_formatters_cover_every_mcs_width_and_gi() {
    for mcs in 0..=7 {
        let mcs = HtMcs::from_index(mcs).unwrap();
        for width in [HtChannelWidth::Mhz20, HtChannelWidth::Mhz40] {
            for gi in [HtGuardInterval::Long800Ns, HtGuardInterval::Short400Ns] {
                let rate = HtRate::new(mcs, gi, width);
                let single =
                    ht_q0_image(0x2f00_5000, HtTxConfig::single_mpdu(rate, 100, 8).unwrap())
                        .unwrap();
                let aggregate =
                    ht_ampdu_q0_image(0x2f00_5000, HtAmpduTxConfig::new(rate, 312, 2).unwrap())
                        .unwrap();

                assert_eq!((single.plcp0 >> 24) & 7, 1);
                assert_eq!((aggregate.plcp0 >> 24) & 7, 2);
                assert_eq!((single.ht_signal >> 27) & 1, 0);
                assert_eq!((aggregate.ht_signal >> 27) & 1, 1);
                assert_eq!(single.data_length & 0x00c0_0000, 0);
                assert_eq!(aggregate.data_length & 0x00c0_0000, 0x0040_0000);
                assert_eq!(single.length_control & 0x00c0_0000, 0);
                assert_eq!(aggregate.length_control & 0x00c0_0000, 0x0040_0000);
                assert_eq!(
                    (single.ht_signal >> 7) & 1,
                    (width == HtChannelWidth::Mhz40) as u32
                );
                assert_eq!(
                    (aggregate.ht_signal >> 7) & 1,
                    (width == HtChannelWidth::Mhz40) as u32
                );
                assert!(!(single.plcp1 & 0x2000_0000 != 0));
                assert!(!(aggregate.plcp1 & 0x2000_0000 != 0));
            }
        }
    }
}

#[test]
fn ht_peer_ampdu_density_maps_to_the_complete_blob_spacing_values() {
    let expected = [20, 20, 20, 20, 20, 40, 76, 148];
    for (density, expected) in expected.into_iter().enumerate() {
        assert_eq!(
            HtProtectionSpacing::from_ampdu_parameters((density as u8) << 2).hardware_value(),
            expected,
        );
    }
    assert_eq!(
        HtProtectionSpacing::from_ampdu_parameters(0xf7),
        HtProtectionSpacing::Density5,
    );
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
fn legacy_54m_image_publishes_the_vendor_24m_basic_rate() {
    let mut config = LegacyTxConfig::management_1m(0x0064);
    config.rate = LegacyRate::Ofdm54M;
    config.rts_rate = config.rate.vendor_rts_rate();
    let image = legacy_q0_image(0x2f00_0100, config).unwrap();

    assert_eq!(image.plcp1, 0x0000_c064);
    assert_eq!(image.length_control, 0x0040_0244);
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
fn protected_legacy_profile_publishes_sta_pairwise_slot_in_plcp1() {
    let mut config = LegacyTxConfig::management_1m(0x99);
    config.hardware_key_selector = 4;
    let image = legacy_q0_image(0x2f00_0100, config).unwrap();
    assert_eq!(image.plcp0, 0x0160_0100);
    assert_eq!(image.plcp1, 0x0008_0099);
    assert_eq!(image.length_control, 0x0040_0004);
}

#[test]
fn protected_legacy_profile_composes_ap_interface_and_pairwise_slot_in_plcp1() {
    let mut config = LegacyTxConfig::management_1m(0x99);
    config.interface = MacInterface::AccessPoint;
    config.hardware_key_selector = 8;
    let image = legacy_q0_image(0x2f00_0100, config).unwrap();
    // Descriptor control byte 0x48 occupies PLCP1 bits 24:17.
    assert_eq!(image.plcp1, 0x0090_0099);
    // LENGTH_CONTROL owns the key index, not the BSSID selector.
    assert_eq!(image.length_control, 0x0040_0004);
}

#[test]
fn legacy_q0_image_derives_the_recovered_format_from_receiver_class() {
    let mut config = LegacyTxConfig::management_1m(0x22);
    let image = legacy_q0_image(0x2f00_0100, config).unwrap();
    assert_eq!(image.plcp0, 0x0160_0100);

    config.group_receiver = true;
    let image = legacy_q0_image(0x2f00_0100, config).unwrap();
    assert_eq!(image.plcp0, 0x0060_0100);
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
