//! Generated-PAC ownership for ordinary EDCA TX queue transactions.

#![forbid(unsafe_code)]

use core::marker::PhantomData;

use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma};

use super::{
    MacInterface, MacPti, MacTxPtiCount, MacTxQueueIndex, WifiRadioRegisters, device_fence,
    mac_tx_queue,
};

const ORDINARY_QUEUE_COUNT: u8 = 4;
const DESCRIPTOR_ADDRESS_LOW_MASK: u32 = 0x000f_ffff;

/// Closed argument projection of complete `hal_set_tx_pti`.
///
/// The four PTI fields are intentionally distinct: the vendor leaf accepts
/// independent values and writes them through five separate fresh-read RMW
/// edges after the scheduler priority edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPtiProgram {
    pub scheduler_priority: MacPti,
    pub pti_2: MacPti,
    pub pti_1: MacPti,
    pub pti_0: MacPti,
    pub pti_3: MacPti,
    pub count: MacTxPtiCount,
}

fn assert_tx_descriptor_head(authority_head: u32, descriptor_address_low: u32) {
    assert_eq!(
        descriptor_address_low,
        authority_head & DESCRIPTOR_ADDRESS_LOW_MASK,
        "TX control register does not reference the retained DMA chain",
    );
}

/// Reviewed ESP32-S31 selector for one non-HT transmit rate.
///
/// The numeric encoding is private to this PAC module. Protocol code selects
/// a named rate and cannot compose the corresponding PLCP field itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacLegacyRate {
    Dsss1MLong,
    Dsss2MLong,
    Cck5M5Long,
    Cck11MLong,
    Dsss2MShort,
    Cck5M5Short,
    Cck11MShort,
    Ofdm48M,
    Ofdm24M,
    Ofdm12M,
    Ofdm6M,
    Ofdm54M,
    Ofdm36M,
    Ofdm18M,
    Ofdm9M,
}

impl MacLegacyRate {
    const fn register_value(self) -> u8 {
        match self {
            Self::Dsss1MLong => 0x00,
            Self::Dsss2MLong => 0x01,
            Self::Cck5M5Long => 0x02,
            Self::Cck11MLong => 0x03,
            Self::Dsss2MShort => 0x05,
            Self::Cck5M5Short => 0x06,
            Self::Cck11MShort => 0x07,
            Self::Ofdm48M => 0x08,
            Self::Ofdm24M => 0x09,
            Self::Ofdm12M => 0x0a,
            Self::Ofdm6M => 0x0b,
            Self::Ofdm54M => 0x0c,
            Self::Ofdm36M => 0x0d,
            Self::Ofdm18M => 0x0e,
            Self::Ofdm9M => 0x0f,
        }
    }
}

/// Semantic inputs for one bounded legacy queue publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxParameters {
    pub rate: MacLegacyRate,
    pub rts_rate: MacLegacyRate,
    pub signal: u16,
    pub data_power: u8,
    pub rts_power_low: u8,
    pub rts_power_high: u8,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
    pub group_receiver: bool,
    pub hardware_key_selector: u8,
}

/// PAC-owned register program for one bounded legacy PPDU.
///
/// All register images are private. Higher layers can retain and inspect the
/// semantic values through accessors, but cannot publish or test raw PLCP
/// geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxProgram {
    descriptor_head: u32,
    parameters: MacLegacyTxParameters,
}

impl MacLegacyTxProgram {
    /// Bind one semantic legacy publication to its prepared DMA authority.
    pub fn new(dma: &dyn PreparedTxDma, parameters: MacLegacyTxParameters) -> Option<Self> {
        if parameters.signal > 0x0fff
            || parameters.timeout > 0x0fff
            || parameters.scheduler_priority > 0x0f
            || parameters.packet_priority > 0x0f
            || parameters.priority_count > 0x0fff
            || parameters.aifsn > 0x0f
            || parameters.contention_window > 0x03ff
            || parameters.hardware_key_selector > 0x3f
        {
            return None;
        }

        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
        })
    }

    pub const fn interface(self) -> MacInterface {
        self.parameters.interface
    }

    pub const fn scheduler_priority(self) -> u8 {
        self.parameters.scheduler_priority
    }

    pub const fn packet_priority(self) -> u8 {
        self.parameters.packet_priority
    }

    pub const fn signal(self) -> u16 {
        self.parameters.signal
    }
}

/// Reviewed MCS selector for one non-HE HT PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtMcs {
    Mcs0,
    Mcs1,
    Mcs2,
    Mcs3,
    Mcs4,
    Mcs5,
    Mcs6,
    Mcs7,
}

impl MacHtMcs {
    const fn index(self) -> u8 {
        match self {
            Self::Mcs0 => 0,
            Self::Mcs1 => 1,
            Self::Mcs2 => 2,
            Self::Mcs3 => 3,
            Self::Mcs4 => 4,
            Self::Mcs5 => 5,
            Self::Mcs6 => 6,
            Self::Mcs7 => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtGuardInterval {
    Long800Ns,
    Short400Ns,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtChannelWidth {
    Mhz20,
    Mhz40,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtProtectionSpacing {
    Density0To4,
    Density5,
    Density6,
    Density7,
}

impl MacHtProtectionSpacing {
    const fn register_value(self) -> u16 {
        match self {
            Self::Density0To4 => 20,
            Self::Density5 => 40,
            Self::Density6 => 76,
            Self::Density7 => 148,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHtTxFormat {
    SingleMpdu,
    Ampdu,
}

/// Protocol-semantic rate selected for one non-HE HT PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtRate {
    pub mcs: MacHtMcs,
    pub guard_interval: MacHtGuardInterval,
    pub channel_width: MacHtChannelWidth,
}

impl MacHtRate {
    const fn register_value(self) -> u8 {
        match (self.guard_interval, self.mcs) {
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs0) => 16,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs1) => 17,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs2) => 18,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs3) => 19,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs4) => 20,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs5) => 21,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs6) => 22,
            (MacHtGuardInterval::Long800Ns, MacHtMcs::Mcs7) => 23,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs0) => 26,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs1) => 27,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs2) => 28,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs3) => 29,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs4) => 30,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs5) => 31,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs6) => 0,
            (MacHtGuardInterval::Short400Ns, MacHtMcs::Mcs7) => 1,
        }
    }

    const fn rts_register_value(self) -> u8 {
        match self.mcs {
            MacHtMcs::Mcs0 => 0x0b,
            MacHtMcs::Mcs1 | MacHtMcs::Mcs2 => 0x0a,
            MacHtMcs::Mcs3 | MacHtMcs::Mcs4 | MacHtMcs::Mcs5 | MacHtMcs::Mcs6 | MacHtMcs::Mcs7 => {
                0x09
            }
        }
    }
}

/// Semantic inputs for one bounded HT queue publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtTxParameters {
    pub rate: MacHtRate,
    pub format: MacHtTxFormat,
    pub length: u16,
    pub descriptor_count: u8,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub rts_power_primary: u8,
    pub rts_power_alternate: u8,
    pub protection_spacing: MacHtProtectionSpacing,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
    pub txop: bool,
}

/// PAC-owned register program for one bounded HT PPDU.
///
/// Whole queue-vector words and their field geometry remain private. Higher
/// layers select only protocol-semantic values and bind them to a prepared DMA
/// authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtTxProgram {
    descriptor_head: u32,
    parameters: MacHtTxParameters,
}

impl MacHtTxProgram {
    /// Bind one semantic HT publication to its prepared DMA authority.
    pub fn new(dma: &dyn PreparedTxDma, parameters: MacHtTxParameters) -> Option<Self> {
        if parameters.length == 0
            || parameters.descriptor_count == 0
            || parameters.descriptor_count > 0x7f
            || (matches!(parameters.format, MacHtTxFormat::SingleMpdu)
                && parameters.descriptor_count != 1)
            || parameters.timeout > 0x0fff
            || parameters.scheduler_priority > 0x0f
            || parameters.packet_priority > 0x0f
            || parameters.priority_count > 0x0fff
            || parameters.aifsn > 0x0f
            || parameters.contention_window > 0x03ff
            || parameters.hardware_key_selector > 0x3f
        {
            return None;
        }

        Some(Self {
            descriptor_head: dma.descriptor_head(),
            parameters,
        })
    }
}

/// Complete bounded queue-vector image for one HE SU A-MPDU.
///
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::{
/// hal_mac_tx_set_ppdu,mac_tx_set_hesig,mac_tx_set_len}` and
/// `HIL_VENDOR_HE20_MCS9_SU_2026_07_29`. The MAC layer constructs the
/// standard-semantic fields; this PAC layer owns only their ordered MMIO
/// publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTxProgram {
    pub plcp0: u32,
    pub plcp1: u32,
    pub he_signal_a1: u32,
    pub he_signal_a2_length: u32,
    /// Explicit software HE-Control image, when one is present.
    ///
    /// `None` preserves the vendor hardware-generated path used for BSR.
    /// `Some` writes the complete four-byte OMC/other A-Control image and
    /// selects it through the independent per-queue override bit.
    pub software_he_control: Option<u32>,
    pub power: u32,
    pub length_control: u32,
    pub descriptor_count_a: u8,
    pub descriptor_count_b: u8,
    pub protection_spacing: u16,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
}

/// HE queue-vector words sampled from one physical queue bank.
///
/// This read-only snapshot exists for formatter HIL: it lets the caller copy
/// the vector immediately after the final submit edge and defer logging until
/// hardware ownership has ended. In particular, a later non-HE retry may
/// legitimately replace PLCP1 while leaving the HE-SIG words unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTxVectorSnapshot {
    pub plcp0: u32,
    pub plcp1: u32,
    pub he_signal_a1: u32,
    pub he_signal_a2_length: u32,
    pub he_control: u32,
    pub software_he_control_enabled: bool,
    pub power: u32,
    pub length_control: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxCompletionObservation {
    status: u8,
    detail: u8,
    /// This queue completed as part of a hardware Trigger-based transmit flow.
    ///
    /// SOURCE: complete `libpp.a[hal_mac_tx.o]::
    /// hal_mac_get_txq_in_trig_flow_state` and
    /// `libpp.a[lmac.o]::lmacProcessTxComplete`. The HAL returns
    /// `QUEUE_STATE[31:24]` as one bitmap; the completion dispatcher selects
    /// the completed queue's bit.
    trigger_flow: bool,
    trigger_based_packet_count: u8,
    last_tx_was_trigger_based: bool,
    secondary_trigger_based_packet_count: u8,
    ack_snr_encoded: u8,
}

impl MacTxCompletionObservation {
    pub const fn status(self) -> u8 {
        self.status
    }

    pub const fn detail(self) -> u8 {
        self.detail
    }

    pub const fn trigger_flow(self) -> bool {
        self.trigger_flow
    }

    pub const fn used_alternate(self) -> bool {
        self.last_tx_was_trigger_based
    }

    pub const fn trigger_based_packet_count(self) -> u8 {
        self.trigger_based_packet_count
    }

    pub const fn last_tx_was_trigger_based(self) -> bool {
        self.last_tx_was_trigger_based
    }

    pub const fn secondary_trigger_based_packet_count(self) -> u8 {
        self.secondary_trigger_based_packet_count
    }

    pub const fn ack_snr_encoded(self) -> u8 {
        self.ack_snr_encoded
    }

    /// Construct a semantic completion supplied by a native hardware model.
    #[cfg(not(target_pointer_width = "32"))]
    pub const fn new_model(status: u8, detail: u8) -> Self {
        Self {
            status,
            detail,
            trigger_flow: false,
            trigger_based_packet_count: 0,
            last_tx_was_trigger_based: false,
            secondary_trigger_based_packet_count: 0,
            ack_snr_encoded: 0,
        }
    }

    #[cfg(not(target_pointer_width = "32"))]
    pub const fn with_trigger_flow_model(mut self, trigger_flow: bool) -> Self {
        self.trigger_flow = trigger_flow;
        self
    }

    #[cfg(not(target_pointer_width = "32"))]
    pub const fn with_trigger_packet_counts_model(
        mut self,
        primary: u8,
        last_tx_was_trigger_based: bool,
        secondary: u8,
    ) -> Self {
        self.trigger_based_packet_count = primary;
        self.last_tx_was_trigger_based = last_tx_was_trigger_based;
        self.secondary_trigger_based_packet_count = secondary;
        self
    }

    #[cfg(not(target_pointer_width = "32"))]
    pub const fn with_ack_snr_encoded_model(mut self, encoded: u8) -> Self {
        self.ack_snr_encoded = encoded;
        self
    }

    /// Construct a semantic completion in a compiled validation image.
    #[cfg(feature = "validation-probes")]
    pub const fn new_validation(status: u8, detail: u8) -> Self {
        Self {
            status,
            detail,
            trigger_flow: false,
            trigger_based_packet_count: 0,
            last_tx_was_trigger_based: false,
            secondary_trigger_based_packet_count: 0,
            ack_snr_encoded: 0,
        }
    }
}

/// TX completion and BlockAck sampled before acknowledging the completion
/// edge for one A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtAmpduCompletionObservation {
    tx: MacTxCompletionObservation,
    block_ack_control: u8,
    block_ack_starting_sequence: u16,
    block_ack_bitmap: u64,
    /// Hardware says the completed PPDU received a BlockAck response.
    ///
    /// This is independent of the ordinary TX status. The bitmap registers
    /// are not cleared at every completion and therefore must not be trusted
    /// when this result bit is clear.
    block_ack_received: bool,
}

impl MacHtAmpduCompletionObservation {
    pub const fn tx(self) -> MacTxCompletionObservation {
        self.tx
    }

    pub const fn block_ack_control(self) -> u8 {
        self.block_ack_control
    }

    pub const fn block_ack_starting_sequence(self) -> u16 {
        self.block_ack_starting_sequence
    }

    pub const fn block_ack_bitmap(self) -> u64 {
        self.block_ack_bitmap
    }

    pub const fn block_ack_received(self) -> bool {
        self.block_ack_received
    }

    #[cfg(not(target_pointer_width = "32"))]
    pub const fn new_model(
        tx: MacTxCompletionObservation,
        block_ack_control: u8,
        block_ack_starting_sequence: u16,
        block_ack_bitmap: u64,
        block_ack_received: bool,
    ) -> Self {
        Self {
            tx,
            block_ack_control,
            block_ack_starting_sequence,
            block_ack_bitmap,
            block_ack_received,
        }
    }
}

/// Hardware edge which must precede reuse of one TX descriptor chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTxDetachReason {
    Collision,
    Timeout,
    Completed,
}

/// Result of one finite queue-detach transaction.
#[derive(Debug, Eq, PartialEq)]
pub enum MacTxDetachOutcome<T> {
    /// The requested collision/timeout edge was not pending.
    NoEvent,
    /// The edge existed, but queue disable/invalid readback did not converge.
    Failed,
    /// Queue ownership was returned and the callback consumed its proof.
    Detached(T),
}

/// Non-forgeable proof that one hardware queue no longer owns its TX chain.
///
/// The value borrows the register owner, so it cannot be retained while that
/// same owner starts another queue transaction. Its fields and target
/// constructor remain private to this crate; safe production code can obtain
/// it only from [`WifiRadioRegisters::with_detached_mac_tx`]. Validation builds
/// additionally expose an isolated model constructor for compiled probes.
pub struct MacTxQueueDetached<'registers> {
    descriptor_address_low: u32,
    _registers: PhantomData<&'registers mut WifiRadioRegisters>,
}

impl MacTxQueueDetached<'_> {
    fn from_descriptor_address_low(descriptor_address_low: u32) -> Self {
        Self {
            descriptor_address_low,
            _registers: PhantomData,
        }
    }

    /// Check that the detached queue referenced this descriptor chain.
    pub const fn confirms_descriptor_head(&self, descriptor_head: u32) -> bool {
        self.descriptor_address_low == descriptor_head & DESCRIPTOR_ADDRESS_LOW_MASK
    }

    /// Construct a detach edge in a native model with no asynchronous DMA.
    #[cfg(not(target_pointer_width = "32"))]
    pub const fn new_model(descriptor_head: u32) -> Self {
        Self {
            descriptor_address_low: descriptor_head & DESCRIPTOR_ADDRESS_LOW_MASK,
            _registers: PhantomData,
        }
    }

    /// Construct a detach edge in an isolated compiled-validation image.
    ///
    /// This is unavailable in production feature sets. It lets a Blobray
    /// hardware double drive the exact upper DMA/LMAC state machine without
    /// claiming that a real target queue was detached.
    #[cfg(feature = "validation-probes")]
    pub const fn new_validation(descriptor_head: u32) -> Self {
        Self {
            descriptor_address_low: descriptor_head & DESCRIPTOR_ADDRESS_LOW_MASK,
            _registers: PhantomData,
        }
    }
}

const fn physical_bank(queue: u8) -> usize {
    (ORDINARY_QUEUE_COUNT - 1 - queue) as usize
}

impl WifiRadioRegisters {
    /// Execute complete `hal_set_tx_pti` over one bounded logical queue.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        let bank = physical_bank(queue.get() as u8);
        let control = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        control.config(bank).modify(|_, writer| {
            writer
                .scheduler_priority()
                .set(program.scheduler_priority.get() as u8)
        });
        let pti = self.peripherals.wifi_mac.wifi_mac_tx_queue_vector.pti(bank);
        pti.modify(|_, writer| writer.pti_2().set(program.pti_2.get() as u8));
        pti.modify(|_, writer| writer.pti_1().set(program.pti_1.get() as u8));
        pti.modify(|_, writer| writer.pti_0().set(program.pti_0.get() as u8));
        pti.modify(|_, writer| writer.pti_3().set(program.pti_3.get() as u8));
        pti.modify(|_, writer| writer.count().set(program.count.get() as u16));
    }

    /// Prepare one legacy queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_legacy_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        assert_eq!(dma.descriptor_head(), program.descriptor_head);
        self.prepare_legacy_mac_tx(queue, program)
    }

    /// Prepare one HT queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_ht_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        assert_eq!(dma.descriptor_head(), program.descriptor_head);
        self.prepare_ht_mac_tx(queue, program)
    }

    /// Prepare one HE queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_he_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        assert_tx_descriptor_head(dma.descriptor_head(), program.plcp0);
        self.prepare_he_mac_tx(queue, program)
    }

    /// Publish the final ENABLE|VALID edge for a hardware-owned TX chain.
    pub fn start_bound_mac_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_tx_queue_control
            .control(physical_bank(queue))
            .read();
        assert_tx_descriptor_head(
            dma.descriptor_head(),
            control.descriptor_address_low().bits(),
        );
        self.start_prepared_mac_tx(queue);
    }

    /// Apply complete rev0 ROM `phy_enable_cca` or `phy_disable_cca` to the
    /// two Wi-Fi MAC CCA fields through separate fresh-read updates.
    pub fn set_phy_wifi_cca_enabled(&mut self, enabled: bool) {
        let image = if enabled { 0 } else { 2 };
        let control = self.peripherals.wifi_mac.wifi_mac_tx_common.cca_control();
        control.modify(|_, w| w.force().set(image));
        control.modify(|_, w| w.phy_aux_force().set(image));
    }

    /// Sample the complete HE vector for one ordinary logical queue.
    ///
    /// SOURCE: the same generated-PAC identities and logical-to-physical
    /// queue mapping used by [`Self::prepare_he_mac_tx`]. This method performs
    /// no ownership transition and does not acknowledge a completion.
    pub fn he_mac_tx_vector_snapshot(&self, queue: u8) -> MacHeTxVectorSnapshot {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let bank = physical_bank(queue);
        MacHeTxVectorSnapshot {
            plcp0: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_control
                .control(bank)
                .read()
                .bits(),
            plcp1: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .plcp1(bank)
                .read()
                .bits(),
            he_signal_a1: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .he_su_signal_a1(bank)
                .read()
                .bits(),
            he_signal_a2_length: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .he_su_signal_a2_length(bank)
                .read()
                .bits(),
            he_control: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .he_control(bank)
                .read()
                .bits(),
            software_he_control_enabled: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .he_control_config(bank)
                .read()
                .software_he_control_enable()
                .bit_is_set(),
            power: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .power(bank)
                .read()
                .bits(),
            length_control: self
                .peripherals
                .wifi_mac
                .wifi_mac_tx_queue_vector
                .length_control(bank)
                .read()
                .bits(),
        }
    }

    /// Program one ordinary queue up to, but excluding, its ENABLE|VALID edge.
    ///
    /// Keeping the final edge separate lets the MAC publish its software
    /// ownership state before hardware can complete the queue.
    fn prepare_legacy_mac_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool {
        let parameters = program.parameters;
        assert!(queue < ORDINARY_QUEUE_COUNT);
        assert!(parameters.timeout <= 0x0fff);
        assert!(parameters.scheduler_priority <= 0x0f);
        assert!(parameters.packet_priority <= 0x0f);
        assert!(parameters.priority_count <= 0x0fff);
        assert!(parameters.aifsn <= 0x0f);
        assert!(parameters.contention_window <= 0x03ff);

        let bank = physical_bank(queue);
        let control_bank = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        let control = control_bank.control(bank);
        let control_state = control.read();
        if control_state.enable().bit_is_set() || control_state.valid().bit_is_set() {
            return false;
        }

        // SOURCE: complete hal_mac_tx_config_timeout. This precedes every
        // vector write in the recovered lmacSetTxFrame parent.
        control_bank
            .config(bank)
            .modify(|_, w| w.timeout().set(parameters.timeout));

        super::svd::zero_based_field_write::publish_mac_tx_prepared_control(
            control_bank,
            bank,
            program.descriptor_head,
            2,
            true,
            u8::from(!parameters.group_receiver),
        );
        let vectors = &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector;
        super::svd::zero_based_field_write::publish_mac_tx_plcp1_fields(
            vectors,
            bank,
            parameters.signal,
            parameters.rate.register_value(),
            parameters.hardware_key_selector,
            parameters.interface.bits() as u8,
            0,
            false,
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .queue_control(4 + bank)
            .modify(|_, w| w.trigger_based_enable().clear_bit());
        control_bank
            .protection(bank)
            .modify(|_, w| w.software_cts().clear_bit());
        super::svd::zero_based_field_write::publish_mac_tx_length_control_fields(
            vectors,
            bank,
            true,
            parameters.rts_rate.register_value(),
            1,
        );
        super::svd::zero_based_field_write::publish_mac_tx_power_fields(
            vectors,
            bank,
            parameters.data_power,
            0,
            parameters.rts_power_low,
            parameters.rts_power_high,
        );

        // SOURCE: complete
        // `libpp.a[hal_mac.o]::mac_tx_set_pti` and
        // `libpp.a[hal_coex.o]::hal_set_tx_pti`. The scheduler
        // priority in CONFIG is the unsigned minimum of the packet PTI and
        // coexistence event-one PTI. The four packet lanes retain the original
        // packet PTI, so these values must not be collapsed into one field.
        // Each field is intentionally a separate fresh-read RMW and must not
        // be coalesced.
        control_bank
            .config(bank)
            .modify(|_, w| w.scheduler_priority().set(parameters.scheduler_priority));
        let pti = vectors.pti(bank);
        pti.modify(|_, w| w.pti_2().set(parameters.packet_priority));
        pti.modify(|_, w| w.pti_1().set(parameters.packet_priority));
        pti.modify(|_, w| w.pti_0().set(parameters.packet_priority));
        pti.modify(|_, w| w.pti_3().set(parameters.packet_priority));
        pti.modify(|_, w| w.count().set(parameters.priority_count));

        mac_tx_queue::configure_edca(
            control_bank,
            u32::from(queue),
            parameters.aifsn,
            parameters.contention_window,
            parameters.interface,
        );
        true
    }

    /// Program one non-aggregate HT queue up to its final ENABLE|VALID edge.
    ///
    /// This is deliberately separate from the legacy routine: an HT PPDU has
    /// two additional vector words and three descriptor-count RMW edges which
    /// must not be silently omitted by a shared "mostly legacy" formatter.
    pub(crate) fn prepare_ht_mac_tx(&mut self, queue: u8, program: MacHtTxProgram) -> bool {
        let parameters = program.parameters;
        assert!(parameters.timeout <= 0x0fff);
        assert!(parameters.aifsn <= 0x0f);
        assert!(parameters.contention_window <= 0x03ff);

        assert!(queue < ORDINARY_QUEUE_COUNT);
        let bank = physical_bank(queue);
        {
            let control_bank = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
            let control = control_bank.control(bank);
            let control_state = control.read();
            if control_state.enable().bit_is_set() || control_state.valid().bit_is_set() {
                return false;
            }

            // SOURCE: complete hal_mac_tx_config_timeout, followed by the
            // hal_mac_tx_set_ppdu non-HE HT branch.
            control_bank
                .config(bank)
                .modify(|_, w| w.timeout().set(parameters.timeout));
        }

        self.program_ht_mac_tx_ppdu(queue, program);

        mac_tx_queue::configure_edca(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            u32::from(queue),
            parameters.aifsn,
            parameters.contention_window,
            parameters.interface,
        );
        true
    }

    /// Publish the complete non-HE HT responsibility of vendor
    /// `hal_mac_tx_set_ppdu` for one already-idle ordinary queue.
    ///
    /// Queue readiness, timeout, EDCA and the final ENABLE|VALID ownership
    /// edge belong to the surrounding production transaction. Keeping this
    /// slice explicit lets the compiled production implementation be compared
    /// to the same vendor responsibility without a shadow register model.
    pub(crate) fn program_ht_mac_tx_ppdu(&mut self, queue: u8, program: MacHtTxProgram) {
        let parameters = program.parameters;
        assert!(queue < ORDINARY_QUEUE_COUNT);
        assert!(parameters.descriptor_count <= 0x7f);
        assert!(parameters.protection_spacing.register_value() <= 0x03ff);
        assert!(parameters.scheduler_priority <= 0x0f);
        assert!(parameters.packet_priority <= 0x0f);
        assert!(parameters.priority_count <= 0x0fff);

        let bank = physical_bank(queue);
        let control_bank = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        super::svd::zero_based_field_write::publish_mac_tx_prepared_control(
            control_bank,
            bank,
            program.descriptor_head,
            2,
            true,
            match parameters.format {
                MacHtTxFormat::SingleMpdu => 1,
                MacHtTxFormat::Ampdu => 2,
            },
        );
        // `mac_tx_set_plcp0` publishes the control image and immediately
        // clears software CTS through one fresh-read protection update.
        control_bank
            .protection(bank)
            .modify(|_, w| w.software_cts().clear_bit());
        let vectors = &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector;
        super::svd::zero_based_field_write::publish_mac_tx_plcp1_fields(
            vectors,
            bank,
            0,
            parameters.rate.register_value(),
            parameters.hardware_key_selector,
            parameters.interface.bits() as u8,
            1,
            matches!(parameters.rate.channel_width, MacHtChannelWidth::Mhz40),
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .queue_control(4 + bank)
            .modify(|_, w| w.trigger_based_enable().clear_bit());

        // The parent owns this independent PTI-low edge. `mac_tx_set_pti`
        // follows later and intentionally preserves it while updating the
        // four PTI lanes and the count.
        let pti = vectors.pti(bank);
        pti.modify(|_, writer| writer.txop().bit(parameters.txop));

        // The bounded HT branch enters `mac_tx_set_htsig` here. It publishes
        // HT-SIG, the three descriptor-count edges, the three negotiated
        // minimum-MPDU spacing edges and the two length words.
        super::svd::zero_based_field_write::publish_mac_tx_ht_signal_fields(
            vectors,
            bank,
            parameters.rate.mcs.index(),
            matches!(parameters.rate.channel_width, MacHtChannelWidth::Mhz40),
            parameters.length,
            true,
            true,
            true,
            matches!(parameters.format, MacHtTxFormat::Ampdu),
            matches!(
                parameters.rate.guard_interval,
                MacHtGuardInterval::Short400Ns
            ),
        );
        let descriptor_counts = vectors.ht_descriptor_counts(bank);
        descriptor_counts.modify(|_, w| w.descriptor_count_a().set(parameters.descriptor_count));
        descriptor_counts.modify(|_, w| w.descriptor_count_b().set(parameters.descriptor_count));
        descriptor_counts
            .modify(|_, w| w.descriptor_count_a_copy().set(parameters.descriptor_count));

        let protection = control_bank.protection(bank);
        protection.modify(|_, w| {
            w.minimum_mpdu_length_cbw20()
                .set(parameters.protection_spacing.register_value())
        });
        protection.modify(|_, w| {
            w.minimum_mpdu_length_cbw40()
                .set(parameters.protection_spacing.register_value())
        });
        protection.modify(|_, w| {
            w.minimum_mpdu_length_cbw80()
                .set(parameters.protection_spacing.register_value())
        });

        let entry_class = match parameters.format {
            MacHtTxFormat::SingleMpdu => 0,
            MacHtTxFormat::Ampdu => 1,
        };
        super::svd::zero_based_field_write::publish_mac_tx_length_control_fields(
            vectors,
            bank,
            true,
            parameters.rate.rts_register_value(),
            entry_class,
        );
        super::svd::zero_based_field_write::publish_mac_tx_data_length_fields(
            vectors,
            bank,
            u32::from(parameters.length),
            entry_class,
            parameters.rate.mcs.index(),
        );
        super::svd::zero_based_field_write::publish_mac_tx_power_fields(
            vectors,
            bank,
            parameters.data_power_primary,
            parameters.data_power_alternate,
            parameters.rts_power_primary,
            parameters.rts_power_alternate,
        );
        control_bank
            .config(bank)
            .modify(|_, w| w.scheduler_priority().set(parameters.scheduler_priority));
        pti.modify(|_, w| w.pti_2().set(parameters.packet_priority));
        pti.modify(|_, w| w.pti_1().set(parameters.packet_priority));
        pti.modify(|_, w| w.pti_0().set(parameters.packet_priority));
        pti.modify(|_, w| w.pti_3().set(parameters.packet_priority));
        pti.modify(|_, w| w.count().set(parameters.priority_count));
    }

    /// Program one HE SU A-MPDU up to its final ENABLE|VALID edge.
    ///
    /// This is separate from HT because HE publishes two different vector
    /// words and deliberately does not write the non-HE DATA_LENGTH word.
    fn prepare_he_mac_tx(&mut self, queue: u8, program: MacHeTxProgram) -> bool {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        assert!(program.descriptor_count_a <= 0x7f);
        assert!(program.descriptor_count_b <= 0x7f);
        assert!(program.protection_spacing <= 0x03ff);
        assert!(program.timeout <= 0x0fff);
        assert!(program.scheduler_priority <= 0x0f);
        assert!(program.packet_priority <= 0x0f);
        assert!(program.priority_count <= 0x0fff);
        assert!(program.aifsn <= 0x0f);
        assert!(program.contention_window <= 0x03ff);

        let bank = physical_bank(queue);
        let control_bank = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        let control = control_bank.control(bank);
        let control_state = control.read();
        if control_state.enable().bit_is_set() || control_state.valid().bit_is_set() {
            return false;
        }

        control_bank
            .config(bank)
            .modify(|_, w| w.timeout().set(program.timeout));
        super::generated::publish_mac_tx_control(
            control_bank,
            bank,
            super::generated::MacTxControlImage::new(program.plcp0),
        );
        super::generated::publish_mac_tx_plcp1(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPlcp1Image::new(program.plcp1),
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .queue_control(4 + bank)
            .modify(|_, w| w.trigger_based_enable().clear_bit());

        // SOURCE: complete mac_tx_set_plcp0/hal_he_set_tx_protection followed
        // by mac_tx_set_hesig. The bounded SU profile clears software CTS,
        // then replaces the three finite channel-width minimum-MPDU lanes.
        let protection = control_bank.protection(bank);
        protection.modify(|_, w| w.software_cts().clear_bit());
        protection.modify(|_, w| {
            w.minimum_mpdu_length_cbw20()
                .set(program.protection_spacing)
        });
        protection.modify(|_, w| {
            w.minimum_mpdu_length_cbw40()
                .set(program.protection_spacing)
        });
        protection.modify(|_, w| {
            w.minimum_mpdu_length_cbw80()
                .set(program.protection_spacing)
        });

        // SOURCE: complete mac_tx_set_hesig stores A1 then A2/length before
        // publishing the same three descriptor-count edges used by HT.
        super::generated::publish_mac_tx_he_signal_a1(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHeSignalA1Image::new(program.he_signal_a1),
        );
        super::generated::publish_mac_tx_he_signal_a2_length(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHeSignalA2LengthImage::new(program.he_signal_a2_length),
        );
        let descriptor_counts = self
            .peripherals
            .wifi_mac
            .wifi_mac_tx_queue_vector
            .ht_descriptor_counts(bank);
        descriptor_counts.modify(|_, w| w.descriptor_count_a().set(program.descriptor_count_a));
        descriptor_counts.modify(|_, w| w.descriptor_count_b().set(program.descriptor_count_b));
        descriptor_counts
            .modify(|_, w| w.descriptor_count_a_copy().set(program.descriptor_count_a));

        // HE reaches mac_tx_set_len for LENGTH_CONTROL, but its flag-bit-31
        // branch intentionally skips the non-HE DATA_LENGTH register.
        super::generated::publish_mac_tx_length_control(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxLengthControlImage::new(program.length_control),
        );
        // SOURCE: complete `libpp.a[hal_mac_tx.o]::
        // hal_mac_tx_set_ppdu` calls complete
        // `libpp.a[hal_mac_ctl.o]::hal_he_set_htc` immediately after
        // `mac_tx_set_hesig`. That leaf always publishes the complete word,
        // then sets or clears bit 28 through a fresh-read RMW. A missing
        // software image is not "no HE-Control": it deliberately leaves the
        // hardware-generated BSR path selected.
        let (he_control, software_he_control_enabled) = program
            .software_he_control
            .map_or((0, false), |image| (image, true));
        super::generated::publish_mac_tx_he_control(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHeControlImage::new(he_control),
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_tx_queue_vector
            .he_control_config(bank)
            .modify(|_, w| {
                w.software_he_control_enable()
                    .bit(software_he_control_enabled)
            });
        // The complete parent selects and publishes the data/RTS power pair
        // only after the HE-Control leaf returns.
        super::generated::publish_mac_tx_power(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPowerImage::new(program.power),
        );

        control_bank
            .config(bank)
            .modify(|_, w| w.scheduler_priority().set(program.scheduler_priority));
        let pti = self.peripherals.wifi_mac.wifi_mac_tx_queue_vector.pti(bank);
        pti.modify(|_, w| w.pti_2().set(program.packet_priority));
        pti.modify(|_, w| w.pti_1().set(program.packet_priority));
        pti.modify(|_, w| w.pti_0().set(program.packet_priority));
        pti.modify(|_, w| w.pti_3().set(program.packet_priority));
        pti.modify(|_, w| w.count().set(program.priority_count));

        mac_tx_queue::configure_edca(
            control_bank,
            u32::from(queue),
            program.aifsn,
            program.contention_window,
            program.interface,
        );
        true
    }

    fn start_prepared_mac_tx(&mut self, queue: u8) {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        device_fence();
        // SOURCE: complete `libpp.a[hal_mac_tx.o]::
        // hal_mac_txq_enable` offsets 0x00..0x1e. The vendor leaf reads the
        // already prepared CONTROL word and ORs only ENABLE|VALID before
        // writing it back. In particular, it does not reconstruct PLCP0 from
        // the caller's earlier software image: formatter leaves may have
        // changed control fields after the initial PLCP0 publication.
        //
        mac_tx_queue::publish_queue(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            u32::from(queue),
        );
        device_fence();
    }

    pub fn take_mac_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let common = &self.peripherals.wifi_mac.wifi_mac_tx_common;
        if !mac_tx_queue::completion_pending(common, queue) {
            return None;
        }

        let bank = physical_bank(queue);
        let completion = &self.peripherals.wifi_mac.wifi_mac_tx_completion;
        let tx_results = &self.peripherals.wifi_mac.wifi_mac_rx_dma;
        let (trigger_based_packet_count, last_tx_was_trigger_based) = match queue {
            0 => {
                let information = tx_results.tx_queue_information_q0().read();
                (
                    information.trigger_based_packet_count().bits(),
                    information.last_tx_was_trigger_based().bit(),
                )
            }
            1 => {
                let information = tx_results.tx_queue_information_q1().read();
                (
                    information.trigger_based_packet_count().bits(),
                    information.last_tx_was_trigger_based().bit(),
                )
            }
            2 => {
                let information = tx_results.tx_queue_information_q2().read();
                (
                    information.trigger_based_packet_count().bits(),
                    information.last_tx_was_trigger_based().bit(),
                )
            }
            3 => {
                let information = tx_results.tx_queue_information_q3().read();
                (
                    information.trigger_based_packet_count().bits(),
                    information.last_tx_was_trigger_based().bit(),
                )
            }
            _ => unreachable!(),
        };
        let secondary_trigger_based_packet_count = completion
            .aux_c(bank)
            .read()
            .secondary_trigger_based_packet_count()
            .bits();
        let primary = completion.primary(bank).read();
        let alternate = completion.alternate(bank).read();
        let (status, detail) = if last_tx_was_trigger_based {
            (alternate.status().bits(), alternate.detail().bits())
        } else {
            (primary.status().bits(), primary.detail().bits())
        };
        let trigger_flow = mac_tx_queue::queue_in_trigger_flow(common, queue);
        mac_tx_queue::acknowledge_completion(common, queue);
        device_fence();
        Some(MacTxCompletionObservation {
            status,
            detail,
            trigger_flow,
            trigger_based_packet_count,
            last_tx_was_trigger_based,
            secondary_trigger_based_packet_count,
            ack_snr_encoded: primary.ack_snr_encoded().bits(),
        })
    }

    /// Sample an A-MPDU's BlockAck before acknowledging its TX-complete edge.
    ///
    /// SOURCE: complete `libpp.a[hal_mac_tx.o]::
    /// hal_mac_tx_get_blockack` and the recovered event-23 completion order.
    /// The three BlockAck words belong to the completed queue and must be read
    /// while that completion edge still owns the result registers.
    pub fn take_mac_ht_ampdu_completion(
        &mut self,
        queue: u8,
    ) -> Option<MacHtAmpduCompletionObservation> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        if !mac_tx_queue::completion_pending(&self.peripherals.wifi_mac.wifi_mac_tx_common, queue) {
            return None;
        }
        let block_ack = self.read_tx_block_ack_observation(queue)?;
        let tx = self.take_mac_tx_completion(queue)?;
        Some(MacHtAmpduCompletionObservation {
            tx,
            block_ack_control: block_ack.control,
            block_ack_starting_sequence: block_ack.starting_sequence,
            block_ack_bitmap: block_ack.bitmap,
            block_ack_received: block_ack.block_ack_received,
        })
    }

    pub fn begin_mac_tx_timeout_abort(&mut self, queue: u8) -> bool {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let common = &self.peripherals.wifi_mac.wifi_mac_tx_common;
        if !mac_tx_queue::timeout_pending(common, queue) {
            return false;
        }
        let _ = mac_tx_queue::set_cca_force(common, 3);
        device_fence();
        true
    }

    /// Execute one detach transaction and consume its proof before allowing
    /// this register owner to be borrowed for another queue operation.
    pub fn with_detached_mac_tx<'registers, R>(
        &'registers mut self,
        queue: u8,
        reason: MacTxDetachReason,
        detached: impl FnOnce(MacTxQueueDetached<'registers>) -> R,
    ) -> MacTxDetachOutcome<R> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let common = &self.peripherals.wifi_mac.wifi_mac_tx_common;
        let queue_control = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        let bank = physical_bank(queue);
        let descriptor_address_low = queue_control
            .control(bank)
            .read()
            .descriptor_address_low()
            .bits();
        let queue_index = u32::from(queue);
        match reason {
            MacTxDetachReason::Collision => {
                if !mac_tx_queue::collision_pending(common, queue) {
                    return MacTxDetachOutcome::NoEvent;
                }
                // SOURCE: complete `libpp.a[lmac.o]::lmacProcessCollisions`
                // reaches disable before clearing the collision edge.
                let _ = mac_tx_queue::disable_queue(queue_control, queue_index);
                device_fence();
                mac_tx_queue::acknowledge_collision(common, queue);
            }
            MacTxDetachReason::Timeout => {
                if !mac_tx_queue::timeout_pending(common, queue) {
                    return MacTxDetachOutcome::NoEvent;
                }
                let was_valid = mac_tx_queue::queue_valid(queue_control, queue_index);
                let _ = mac_tx_queue::invalidate_queue(queue_control, queue_index);
                let _ = mac_tx_queue::set_cca_force(common, 0);
                if was_valid {
                    let _ = mac_tx_queue::disable_queue(queue_control, queue_index);
                }
                mac_tx_queue::acknowledge_timeout(common, queue);
            }
            MacTxDetachReason::Completed => {
                // A completion edge was consumed separately before this
                // transaction. Disable is still required even if hardware
                // already cleared ENABLE|VALID while completing the PPDU.
                let _ = mac_tx_queue::disable_queue(queue_control, queue_index);
            }
        }
        device_fence();
        if mac_tx_queue::queue_enabled(queue_control, queue_index)
            || mac_tx_queue::queue_valid(queue_control, queue_index)
        {
            return MacTxDetachOutcome::Failed;
        }
        MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::from_descriptor_address_low(
            descriptor_address_low,
        )))
    }
}
