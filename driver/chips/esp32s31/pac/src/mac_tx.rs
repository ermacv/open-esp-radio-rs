//! Generated-PAC ownership for ordinary EDCA TX queue transactions.

#![forbid(unsafe_code)]

use core::marker::PhantomData;

use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma};

use super::{
    MacInterface, MacPti, MacTxPtiCount, MacTxQueueIndex, RadioRegisters, device_fence,
    mac_tx_queue,
};

const ORDINARY_QUEUE_COUNT: u8 = 4;
const ENABLE_VALID_MASK: u32 = 0xc000_0000;
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

fn assert_tx_descriptor_head(authority_head: u32, control_word: u32) {
    assert_eq!(
        control_word & DESCRIPTOR_ADDRESS_LOW_MASK,
        authority_head & DESCRIPTOR_ADDRESS_LOW_MASK,
        "TX control word does not reference the retained DMA chain",
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLegacyTxProgram {
    pub plcp0: u32,
    pub plcp1: u32,
    pub power: u32,
    pub length_control: u32,
    pub timeout: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    pub priority_count: u16,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: MacInterface,
}

/// Complete queue-vector image for one HT PPDU.
///
/// The MAC layer owns the meaning and construction of these whole words. This
/// PAC layer only publishes them in the instruction-exact order recovered
/// from `libpp.a[hal_mac_tx.o]::{hal_mac_tx_set_ppdu,
/// mac_tx_set_htsig,mac_tx_set_len}`. Single MPDUs and A-MPDUs use distinct
/// MAC-layer formatters, but publish the same finite register set here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtTxProgram {
    pub plcp0: u32,
    pub plcp1: u32,
    pub ht_signal: u32,
    pub data_length: u32,
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
pub struct MacTxCompletionRegisters {
    pub aux_a: u32,
    pub aux_b: u32,
    pub aux_c: u32,
    pub primary: u32,
    pub alternate: u32,
    /// This queue completed as part of a hardware Trigger-based transmit flow.
    ///
    /// SOURCE: complete `libpp.a[hal_mac_tx.o]::
    /// hal_mac_get_txq_in_trig_flow_state` and
    /// `libpp.a[lmac.o]::lmacProcessTxComplete`. The HAL returns
    /// `QUEUE_STATE[31:24]` as one bitmap; the completion dispatcher selects
    /// the completed queue's bit.
    pub trigger_flow: bool,
}

/// TX completion and BlockAck sampled before acknowledging the completion
/// edge for one A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHtAmpduCompletionRegisters {
    pub tx: MacTxCompletionRegisters,
    pub block_ack_control_and_sequence: u32,
    pub block_ack_bitmap_low: u32,
    pub block_ack_bitmap_high: u32,
    /// Hardware says the completed PPDU received a BlockAck response.
    ///
    /// This is independent of the ordinary TX status. The bitmap registers
    /// are not cleared at every completion and therefore must not be trusted
    /// when this result bit is clear.
    pub block_ack_received: bool,
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
/// constructor remain private to this crate; safe target code can obtain it
/// only from [`RadioRegisters::with_detached_mac_tx`].
pub struct MacTxQueueDetached<'registers> {
    descriptor_address_low: u32,
    _registers: PhantomData<&'registers mut RadioRegisters>,
}

impl MacTxQueueDetached<'_> {
    fn from_control_word(control_word: u32) -> Self {
        Self {
            descriptor_address_low: control_word & DESCRIPTOR_ADDRESS_LOW_MASK,
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
}

const fn physical_bank(queue: u8) -> usize {
    (ORDINARY_QUEUE_COUNT - 1 - queue) as usize
}

impl RadioRegisters {
    /// Execute complete `hal_set_tx_pti` over one bounded logical queue.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        let bank = physical_bank(queue.get() as u8);
        let control = &self.peripherals.wifi_mac_tx_queue_control;
        control.config(bank).modify(|_, writer| {
            writer
                .scheduler_priority()
                .set(program.scheduler_priority.get() as u8)
        });
        let pti = self.peripherals.wifi_mac_tx_queue_vector.pti(bank);
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
        assert_tx_descriptor_head(dma.descriptor_head(), program.plcp0);
        self.prepare_legacy_mac_tx(queue, program)
    }

    /// Prepare one HT queue whose descriptor chain is retained by `dma`.
    pub fn prepare_bound_ht_mac_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        assert_tx_descriptor_head(dma.descriptor_head(), program.plcp0);
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
        let control_word = self
            .peripherals
            .wifi_mac_tx_queue_control
            .control(physical_bank(queue))
            .read()
            .bits();
        assert_tx_descriptor_head(dma.descriptor_head(), control_word);
        self.start_prepared_mac_tx(queue, control_word);
    }

    /// Apply complete rev0 ROM `phy_enable_cca` or `phy_disable_cca` to the
    /// two Wi-Fi MAC CCA fields through separate fresh-read updates.
    pub fn set_phy_wifi_cca_enabled(&mut self, enabled: bool) {
        let image = if enabled { 0 } else { 2 };
        let control = self.peripherals.wifi_mac_tx_common.cca_control();
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
                .wifi_mac_tx_queue_control
                .control(bank)
                .read()
                .bits(),
            plcp1: self
                .peripherals
                .wifi_mac_tx_queue_vector
                .plcp1(bank)
                .read()
                .bits(),
            he_signal_a1: self
                .peripherals
                .wifi_mac_tx_queue_vector
                .he_su_signal_a1(bank)
                .read()
                .bits(),
            he_signal_a2_length: self
                .peripherals
                .wifi_mac_tx_queue_vector
                .he_su_signal_a2_length(bank)
                .read()
                .bits(),
            he_control: self
                .peripherals
                .wifi_mac_tx_queue_vector
                .he_control(bank)
                .read()
                .bits(),
            software_he_control_enabled: self
                .peripherals
                .wifi_mac_tx_queue_vector
                .he_control_config(bank)
                .read()
                .software_he_control_enable()
                .bit_is_set(),
            power: self
                .peripherals
                .wifi_mac_tx_queue_vector
                .power(bank)
                .read()
                .bits(),
            length_control: self
                .peripherals
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
        assert!(queue < ORDINARY_QUEUE_COUNT);
        assert!(program.timeout <= 0x0fff);
        assert!(program.scheduler_priority <= 0x0f);
        assert!(program.packet_priority <= 0x0f);
        assert!(program.priority_count <= 0x0fff);
        assert!(program.aifsn <= 0x0f);
        assert!(program.contention_window <= 0x03ff);

        let bank = physical_bank(queue);
        let control_bank = &self.peripherals.wifi_mac_tx_queue_control;
        let control = control_bank.control(bank);
        if control.read().bits() & ENABLE_VALID_MASK != 0 {
            return false;
        }

        // SOURCE: complete hal_mac_tx_config_timeout. This precedes every
        // vector write in the recovered lmacSetTxFrame parent.
        control_bank
            .config(bank)
            .modify(|_, w| w.timeout().set(program.timeout));

        super::generated::publish_mac_tx_control(
            control_bank,
            bank,
            super::generated::MacTxControlImage::new(program.plcp0),
        );
        super::generated::publish_mac_tx_plcp1(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPlcp1Image::new(program.plcp1),
        );
        self.peripherals
            .wifi_mac_he_init_suffix
            .queue_control(4 + bank)
            .modify(|_, w| w.trigger_based_enable().clear_bit());
        control_bank
            .protection(bank)
            .modify(|_, w| w.software_cts().clear_bit());
        super::generated::publish_mac_tx_length_control(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxLengthControlImage::new(program.length_control),
        );
        super::generated::publish_mac_tx_power(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPowerImage::new(program.power),
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
            .modify(|_, w| w.scheduler_priority().set(program.scheduler_priority));
        let pti = self.peripherals.wifi_mac_tx_queue_vector.pti(bank);
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

    /// Program one non-aggregate HT queue up to its final ENABLE|VALID edge.
    ///
    /// This is deliberately separate from the legacy routine: an HT PPDU has
    /// two additional vector words and three descriptor-count RMW edges which
    /// must not be silently omitted by a shared "mostly legacy" formatter.
    fn prepare_ht_mac_tx(&mut self, queue: u8, program: MacHtTxProgram) -> bool {
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
        let control_bank = &self.peripherals.wifi_mac_tx_queue_control;
        let control = control_bank.control(bank);
        if control.read().bits() & ENABLE_VALID_MASK != 0 {
            return false;
        }

        // SOURCE: complete hal_mac_tx_config_timeout, followed by the
        // hal_mac_tx_set_ppdu non-HE HT branch.
        control_bank
            .config(bank)
            .modify(|_, w| w.timeout().set(program.timeout));
        super::generated::publish_mac_tx_control(
            control_bank,
            bank,
            super::generated::MacTxControlImage::new(program.plcp0),
        );
        super::generated::publish_mac_tx_plcp1(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPlcp1Image::new(program.plcp1),
        );
        self.peripherals
            .wifi_mac_he_init_suffix
            .queue_control(4 + bank)
            .modify(|_, w| w.trigger_based_enable().clear_bit());
        control_bank
            .protection(bank)
            .modify(|_, w| w.software_cts().clear_bit());

        // SOURCE: complete mac_tx_set_htsig writes HT-SIG first, then uses the
        // separate vector word at 0x20105504-q*0x7c to copy descriptor byte
        // 0x2a into count A and its second lane, and byte 0x2e into count B.
        // Keep the three fresh-read hardware edges distinct. In particular,
        // these fields do not belong to the 0x20104d64 protection word above.
        super::generated::publish_mac_tx_ht_signal(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHtSignalImage::new(program.ht_signal),
        );
        let descriptor_counts = self
            .peripherals
            .wifi_mac_tx_queue_vector
            .ht_descriptor_counts(bank);
        descriptor_counts.modify(|_, w| w.descriptor_count_a().set(program.descriptor_count_a));
        descriptor_counts.modify(|_, w| w.descriptor_count_b().set(program.descriptor_count_b));
        descriptor_counts
            .modify(|_, w| w.descriptor_count_a_copy().set(program.descriptor_count_a));

        // SOURCE: complete mac_tx_set_htsig offsets 0x1da..0x21a. The peer's
        // finite spacing value from rcUpdateAMPDUParam is copied into the
        // CBW20/CBW40/CBW80 minimum-MPDU lanes through three fresh-read RMW
        // edges. Complete dbg_read_txq_conf2 supplies those lane names.
        // HIL_VENDOR_ACTIVE_HT_VECTOR_2026_07_29 observed value 40 in all
        // three fields (whole word 0x0280_a028) on a hardware-owned HT queue.
        let protection = control_bank.protection(bank);
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

        // SOURCE: complete mac_tx_set_len followed by the HT power branch in
        // hal_mac_tx_set_ppdu.
        super::generated::publish_mac_tx_length_control(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxLengthControlImage::new(program.length_control),
        );
        super::generated::publish_mac_tx_data_length(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxDataLengthImage::new(program.data_length),
        );
        super::generated::publish_mac_tx_power(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPowerImage::new(program.power),
        );
        control_bank
            .config(bank)
            .modify(|_, w| w.scheduler_priority().set(program.scheduler_priority));
        let pti = self.peripherals.wifi_mac_tx_queue_vector.pti(bank);
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
        let control_bank = &self.peripherals.wifi_mac_tx_queue_control;
        let control = control_bank.control(bank);
        if control.read().bits() & ENABLE_VALID_MASK != 0 {
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
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPlcp1Image::new(program.plcp1),
        );
        self.peripherals
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
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHeSignalA1Image::new(program.he_signal_a1),
        );
        super::generated::publish_mac_tx_he_signal_a2_length(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHeSignalA2LengthImage::new(program.he_signal_a2_length),
        );
        let descriptor_counts = self
            .peripherals
            .wifi_mac_tx_queue_vector
            .ht_descriptor_counts(bank);
        descriptor_counts.modify(|_, w| w.descriptor_count_a().set(program.descriptor_count_a));
        descriptor_counts.modify(|_, w| w.descriptor_count_b().set(program.descriptor_count_b));
        descriptor_counts
            .modify(|_, w| w.descriptor_count_a_copy().set(program.descriptor_count_a));

        // HE reaches mac_tx_set_len for LENGTH_CONTROL, but its flag-bit-31
        // branch intentionally skips the non-HE DATA_LENGTH register.
        super::generated::publish_mac_tx_length_control(
            &self.peripherals.wifi_mac_tx_queue_vector,
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
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxHeControlImage::new(he_control),
        );
        self.peripherals
            .wifi_mac_tx_queue_vector
            .he_control_config(bank)
            .modify(|_, w| {
                w.software_he_control_enable()
                    .bit(software_he_control_enabled)
            });
        // The complete parent selects and publishes the data/RTS power pair
        // only after the HE-Control leaf returns.
        super::generated::publish_mac_tx_power(
            &self.peripherals.wifi_mac_tx_queue_vector,
            bank,
            super::generated::MacTxPowerImage::new(program.power),
        );

        control_bank
            .config(bank)
            .modify(|_, w| w.scheduler_priority().set(program.scheduler_priority));
        let pti = self.peripherals.wifi_mac_tx_queue_vector.pti(bank);
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

    fn start_prepared_mac_tx(&mut self, queue: u8, _plcp0: u32) {
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
            &self.peripherals.wifi_mac_tx_queue_control,
            u32::from(queue),
        );
        device_fence();
    }

    pub fn take_mac_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let completion_mask = 1_u32 << queue;
        let common = &self.peripherals.wifi_mac_tx_common;
        if common.complete_state().read().bits() & completion_mask == 0 {
            return None;
        }

        let bank = physical_bank(queue);
        let completion = &self.peripherals.wifi_mac_tx_completion;
        let tx_results = &self.peripherals.wifi_mac_rx_dma;
        let (aux_a, aux_b) = match queue {
            0 => (
                tx_results
                    .tx_block_ack_transmitter_address_high_q0()
                    .read()
                    .bits(),
                tx_results.tx_queue_information_q0().read().bits(),
            ),
            1 => (
                tx_results
                    .tx_block_ack_transmitter_address_high_q1()
                    .read()
                    .bits(),
                tx_results.tx_queue_information_q1().read().bits(),
            ),
            2 => (
                tx_results
                    .tx_block_ack_transmitter_address_high_q2()
                    .read()
                    .bits(),
                tx_results.tx_queue_information_q2().read().bits(),
            ),
            3 => (
                tx_results
                    .tx_block_ack_transmitter_address_high_q3()
                    .read()
                    .bits(),
                tx_results.tx_queue_information_q3().read().bits(),
            ),
            _ => unreachable!(),
        };
        let aux_c = completion.aux_c(bank).read().bits();
        let primary = completion.primary(bank).read().bits();
        let alternate = completion.alternate(bank).read().bits();
        let trigger_flow = mac_tx_queue::trigger_flow_state(common) & (1_u32 << queue) != 0;
        let clear = common.complete_clear().read().bits();
        super::generated::mac_tx_complete_clear_image(
            common,
            super::generated::MacTxCompleteClearImage::new(clear | completion_mask),
        );
        device_fence();
        Some(MacTxCompletionRegisters {
            aux_a,
            aux_b,
            aux_c,
            primary,
            alternate,
            trigger_flow,
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
    ) -> Option<MacHtAmpduCompletionRegisters> {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let completion_mask = 1_u32 << queue;
        if self
            .peripherals
            .wifi_mac_tx_common
            .complete_state()
            .read()
            .bits()
            & completion_mask
            == 0
        {
            return None;
        }
        let block_ack = self.read_tx_block_ack_registers(queue)?;
        let tx = self.take_mac_tx_completion(queue)?;
        Some(MacHtAmpduCompletionRegisters {
            tx,
            block_ack_control_and_sequence: block_ack.control_and_sequence,
            block_ack_bitmap_low: block_ack.bitmap_low,
            block_ack_bitmap_high: block_ack.bitmap_high,
            block_ack_received: block_ack.block_ack_received,
        })
    }

    pub fn begin_mac_tx_timeout_abort(&mut self, queue: u8) -> bool {
        assert!(queue < ORDINARY_QUEUE_COUNT);
        let timeout_mask = 1_u32 << (16 + queue);
        let common = &self.peripherals.wifi_mac_tx_common;
        if common.queue_state().read().bits() & timeout_mask == 0 {
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
        let common = &self.peripherals.wifi_mac_tx_common;
        let queue_control = &self.peripherals.wifi_mac_tx_queue_control;
        let bank = physical_bank(queue);
        let control_word = queue_control.control(bank).read().bits();
        let queue_index = u32::from(queue);
        match reason {
            MacTxDetachReason::Collision => {
                let collision_mask = 1_u32 << queue;
                if common.queue_state().read().bits() & collision_mask == 0 {
                    return MacTxDetachOutcome::NoEvent;
                }
                // SOURCE: complete `libpp.a[lmac.o]::lmacProcessCollisions`
                // reaches disable before clearing the collision edge.
                let _ = mac_tx_queue::disable_queue(queue_control, queue_index);
                device_fence();
                super::generated::mac_tx_queue_state_clear(
                    common,
                    super::generated::MacTxQueueStateClearMask::new(collision_mask),
                );
            }
            MacTxDetachReason::Timeout => {
                let timeout_mask = 1_u32 << (16 + queue);
                if common.queue_state().read().bits() & timeout_mask == 0 {
                    return MacTxDetachOutcome::NoEvent;
                }
                let was_valid = mac_tx_queue::queue_valid(queue_control, queue_index);
                let _ = mac_tx_queue::invalidate_queue(queue_control, queue_index);
                let _ = mac_tx_queue::set_cca_force(common, 0);
                if was_valid {
                    let _ = mac_tx_queue::disable_queue(queue_control, queue_index);
                }
                super::generated::mac_tx_queue_state_clear(
                    common,
                    super::generated::MacTxQueueStateClearMask::new(timeout_mask),
                );
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
        MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::from_control_word(
            control_word,
        )))
    }
}
