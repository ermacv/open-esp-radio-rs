//! Affine task/interrupt ownership split for the shared IEEE 802.15.4 MAC
//! register block.
//!
//! The hardware places task-side command, policy and DMA registers beside the
//! interrupt event/status registers in one SVD peripheral. A single raw PAC
//! owner therefore cannot model the public driver's simultaneous task and ISR
//! roles. This module consumes that owner, creates exactly two role handles,
//! and reunites them before the complete peripheral can be recovered.
//!
//! The interrupt handle deliberately has no raw-register accessor. Its surface
//! is limited to the source-confirmed ISR snapshot fields and the generated
//! affine W1C acknowledge transaction.

trait TransmitSecurityProgrammingPort {
    fn write_address_low(&mut self, word: u32);
    fn write_address_high(&mut self, word: u32);
    fn write_key_word(&mut self, index: usize, word: u32);
    fn write_payload_offset(&mut self, offset: u8);
    fn set_enabled(&mut self, enabled: bool);
}

trait MultipanIdentityProgrammingPort {
    fn enable_context(&mut self, index: usize);
    fn write_pan_id(&mut self, index: usize, pan_id: u16);
    fn write_short_address(&mut self, index: usize, short_address: u16);
    fn write_extended_address_low(&mut self, index: usize, word: u32);
    fn write_extended_address_high(&mut self, index: usize, word: u32);
}

fn execute_multipan_identity_configuration<Port: MultipanIdentityProgrammingPort>(
    port: &mut Port,
    index: usize,
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
) {
    assert!(index < 4, "multipan index exceeds four contexts");
    port.enable_context(index);
    port.write_pan_id(index, pan_id);
    port.enable_context(index);
    port.write_short_address(index, short_address);
    port.enable_context(index);
    port.write_extended_address_low(
        index,
        u32::from_le_bytes([
            extended_address[0],
            extended_address[1],
            extended_address[2],
            extended_address[3],
        ]),
    );
    port.write_extended_address_high(
        index,
        u32::from_le_bytes([
            extended_address[4],
            extended_address[5],
            extended_address[6],
            extended_address[7],
        ]),
    );
}

fn execute_transmit_security_configuration<Port: TransmitSecurityProgrammingPort>(
    port: &mut Port,
    address: &[u8; 8],
    key: &[u8; 16],
    payload_offset: u8,
) {
    assert!(
        payload_offset <= 0x7f,
        "transmit-security payload offset exceeds seven bits"
    );
    port.write_address_low(u32::from_le_bytes([
        address[0], address[1], address[2], address[3],
    ]));
    port.write_address_high(u32::from_le_bytes([
        address[4], address[5], address[6], address[7],
    ]));
    for index in 0..4 {
        let start = index * 4;
        port.write_key_word(
            index,
            u32::from_le_bytes([key[start], key[start + 1], key[start + 2], key[start + 3]]),
        );
    }
    port.write_payload_offset(payload_offset);
    port.set_enabled(true);
}

fn execute_transmit_security_disable<Port: TransmitSecurityProgrammingPort>(port: &mut Port) {
    port.set_enabled(false);
}

/// Execute the closed source-order transmit-security transaction for the
/// restricted parent PAC.
#[doc(hidden)]
fn configure_transmit_security(
    registers: &mut crate::Ieee802154Mac,
    address: &[u8; 8],
    key: &[u8; 16],
    payload_offset: u8,
) {
    execute_transmit_security_configuration(registers, address, key, payload_offset);
}

/// Disable transmit security without claiming key/address zeroization.
#[doc(hidden)]
fn disable_transmit_security(registers: &mut crate::Ieee802154Mac) {
    execute_transmit_security_disable(registers);
}

/// Publish the one source-confirmed enhanced-ACK notification image.
#[doc(hidden)]
fn notify_enhanced_ack_generated(registers: &mut crate::Ieee802154Mac) {
    crate::fixed_register_image::notify_ieee802154_enhanced_ack_generated(registers);
}

/// Replace only the raw eight-bit transmit-power field while preserving every
/// unmodeled bit in its backing word.
#[doc(hidden)]
fn set_tx_power_code(registers: &mut crate::Ieee802154Mac, code: u32) {
    assert!(code <= 0xff, "TX-power code exceeds eight bits");
    crate::masked_register_modify::set_ieee802154_tx_power_code(registers, code);
}

impl TransmitSecurityProgrammingPort for crate::Ieee802154Mac {
    fn write_address_low(&mut self, word: u32) {
        crate::zero_based_field_write::publish_ieee802154_security_address_low(self, word);
    }

    fn write_address_high(&mut self, word: u32) {
        crate::zero_based_field_write::publish_ieee802154_security_address_high(self, word);
    }

    fn write_key_word(&mut self, index: usize, word: u32) {
        assert!(index < 4, "security key index exceeds four words");
        crate::zero_based_field_write::publish_ieee802154_security_key_word(self, index, word);
    }

    fn write_payload_offset(&mut self, offset: u8) {
        // SAFETY: the restricted PAC validates the seven-bit domain before
        // entering this raw backend.
        self.security_control()
            .modify(|_, writer| unsafe { writer.payload_offset().bits(offset) });
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.security_control()
            .modify(|_, writer| writer.tx_enable().bit(enabled));
    }
}

impl MultipanIdentityProgrammingPort for crate::Ieee802154Mac {
    fn enable_context(&mut self, index: usize) {
        self.control().modify(|_, writer| match index {
            0 => writer.multipan0_enabled().set_bit(),
            1 => writer.multipan1_enabled().set_bit(),
            2 => writer.multipan2_enabled().set_bit(),
            3 => writer.multipan3_enabled().set_bit(),
            _ => panic!("multipan index exceeds four contexts"),
        });
    }

    fn write_pan_id(&mut self, index: usize, pan_id: u16) {
        self.multipan_pan_id(index)
            .modify(|_, writer| writer.pan_id().set(pan_id));
    }

    fn write_short_address(&mut self, index: usize, short_address: u16) {
        self.multipan_short_address(index)
            .modify(|_, writer| writer.address().set(short_address));
    }

    fn write_extended_address_low(&mut self, index: usize, word: u32) {
        self.multipan_extended_address_low(index)
            .modify(|_, writer| writer.address_word().set(word));
    }

    fn write_extended_address_high(&mut self, index: usize, word: u32) {
        self.multipan_extended_address_high(index)
            .modify(|_, writer| writer.address_word().set(word));
    }
}

/// Task-side IEEE 802.15.4 MAC register owner.
///
/// The restricted PAC above this raw crate exposes only reviewed command,
/// policy and DMA operations through this handle.
///
/// The complete block cannot be recovered through a shared borrow:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac_raw::ieee802154_mac_ownership::TaskRegisters;
///
/// fn raw_block_escape(task: &TaskRegisters) {
///     let _ = task.registers();
/// }
/// ```
///
/// Nor through a mutable borrow:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac_raw::ieee802154_mac_ownership::TaskRegisters;
///
/// fn mutable_raw_block_escape(task: &mut TaskRegisters) {
///     let _ = task.task_mac_mut();
/// }
/// ```
#[must_use = "the task owner must be reunited with its interrupt owner"]
pub struct TaskRegisters {
    registers: crate::Ieee802154Mac,
}

/// Primitive readback of one source-confirmed PAN context.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultipanIdentityReadback {
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
}

impl MultipanIdentityReadback {
    pub const fn pan_id(self) -> u16 {
        self.pan_id
    }

    pub const fn short_address(self) -> u16 {
        self.short_address
    }

    pub const fn extended_address(self) -> [u8; 8] {
        self.extended_address
    }
}

/// Primitive task-side MAC policy readback.
///
/// This value contains field images only and cannot recover the raw register
/// block. It deliberately excludes interrupt status and abort sidebands.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticMacPolicyReadback {
    frequency_code: u8,
    cca_mode: u8,
    cca_threshold_code: u8,
    ack_timeout: u16,
    auto_ack_tx: bool,
    auto_ack_rx: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    pending_enhanced: bool,
    multipan_enabled: [bool; 4],
    identity: MultipanIdentityReadback,
}

impl StaticMacPolicyReadback {
    pub const fn frequency_code(self) -> u8 {
        self.frequency_code
    }

    pub const fn cca_mode(self) -> u8 {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> u8 {
        self.cca_threshold_code
    }

    pub const fn ack_timeout(self) -> u16 {
        self.ack_timeout
    }

    pub const fn auto_ack_tx(self) -> bool {
        self.auto_ack_tx
    }

    pub const fn auto_ack_rx(self) -> bool {
        self.auto_ack_rx
    }

    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    pub const fn pending_enhanced(self) -> bool {
        self.pending_enhanced
    }

    pub const fn multipan_enabled(self) -> [bool; 4] {
        self.multipan_enabled
    }

    pub const fn identity(self) -> MultipanIdentityReadback {
        self.identity
    }
}

/// Primitive readback of every currently modeled task-side MAC configuration
/// field, including dynamic values that are not part of runtime policy.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacConfigurationReadback {
    frequency_code: u8,
    tx_power_code: u8,
    cca_mode: u8,
    cca_threshold_code: u8,
    ed_sample_rate: u8,
    ack_timeout: u16,
    auto_ack_tx: bool,
    auto_ack_rx: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    pending_enhanced: bool,
    multipan_enabled: [bool; 4],
    identities: [MultipanIdentityReadback; 4],
    frame_pending: bool,
}

impl MacConfigurationReadback {
    pub const fn frequency_code(self) -> u8 {
        self.frequency_code
    }

    pub const fn tx_power_code(self) -> u8 {
        self.tx_power_code
    }

    pub const fn cca_mode(self) -> u8 {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> u8 {
        self.cca_threshold_code
    }

    pub const fn ed_sample_rate(self) -> u8 {
        self.ed_sample_rate
    }

    pub const fn ack_timeout(self) -> u16 {
        self.ack_timeout
    }

    pub const fn auto_ack_tx(self) -> bool {
        self.auto_ack_tx
    }

    pub const fn auto_ack_rx(self) -> bool {
        self.auto_ack_rx
    }

    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    pub const fn pending_enhanced(self) -> bool {
        self.pending_enhanced
    }

    pub const fn multipan_enabled(self) -> [bool; 4] {
        self.multipan_enabled
    }

    pub const fn identity(self, index: usize) -> MultipanIdentityReadback {
        assert!(
            index < self.identities.len(),
            "multipan index exceeds four contexts"
        );
        self.identities[index]
    }

    pub const fn frame_pending(self) -> bool {
        self.frame_pending
    }
}

/// Primitive readback of the interrupt-masked task-side foundation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FoundationReadback {
    events_masked: bool,
    rx_aborts_masked: bool,
    tx_aborts_masked: bool,
    ed_uses_average: bool,
    txrx_pti: u8,
    ack_pti: u8,
}

impl FoundationReadback {
    pub const fn events_masked(self) -> bool {
        self.events_masked
    }

    pub const fn rx_aborts_masked(self) -> bool {
        self.rx_aborts_masked
    }

    pub const fn tx_aborts_masked(self) -> bool {
        self.tx_aborts_masked
    }

    pub const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub const fn txrx_pti(self) -> u8 {
        self.txrx_pti
    }

    pub const fn ack_pti(self) -> u8 {
        self.ack_pti
    }
}

/// Closed classification of `EVENT_ENABLE` for one finite ED/CCA operation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationEventEnableReadback {
    AllMasked,
    EdOperation,
    Unexpected,
}

/// Closed classification of `RX_ABORT_ENABLE` for one finite ED/CCA operation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationRxAbortEnableReadback {
    AllMasked,
    EdOperationReasons,
    Unexpected,
}

/// Closed validation-only classification of `EVENT_ENABLE`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationEventEnableReadback {
    AllMasked,
    TimerPair,
    EdTimerAbort,
    Unexpected,
}

/// Semantic observation shared by `EVENT_ENABLE`, `EVENT_STATUS`, and the
/// generated affine W1C snapshot.
///
/// Register positions never cross this type: every value is obtained through
/// a generated field reader, and only named event predicates are public.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventReadback {
    tx_done: bool,
    rx_done: bool,
    ack_tx_done: bool,
    ack_rx_done: bool,
    rx_abort: bool,
    tx_abort: bool,
    ed_done: bool,
    unclassified_7: bool,
    timer0_overflow: bool,
    timer1_overflow: bool,
    clock_count_match: bool,
    tx_sfd_done: bool,
    rx_sfd_done: bool,
    unclassified_13: bool,
}

impl Ieee802154EventReadback {
    pub(crate) fn from_event_enable(reader: &crate::ieee802154_mac::event_enable::R) -> Self {
        Self {
            tx_done: reader.tx_done().bit_is_set(),
            rx_done: reader.rx_done().bit_is_set(),
            ack_tx_done: reader.ack_tx_done().bit_is_set(),
            ack_rx_done: reader.ack_rx_done().bit_is_set(),
            rx_abort: reader.rx_abort().bit_is_set(),
            tx_abort: reader.tx_abort().bit_is_set(),
            ed_done: reader.ed_done().bit_is_set(),
            unclassified_7: reader.unclassified_7().bit_is_set(),
            timer0_overflow: reader.timer0_overflow().bit_is_set(),
            timer1_overflow: reader.timer1_overflow().bit_is_set(),
            clock_count_match: reader.clock_count_match().bit_is_set(),
            tx_sfd_done: reader.tx_sfd_done().bit_is_set(),
            rx_sfd_done: reader.rx_sfd_done().bit_is_set(),
            unclassified_13: reader.unclassified_13().bit_is_set(),
        }
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn from_event_status(reader: &crate::ieee802154_mac::event_status::R) -> Self {
        Self {
            tx_done: reader.tx_done().bit_is_set(),
            rx_done: reader.rx_done().bit_is_set(),
            ack_tx_done: reader.ack_tx_done().bit_is_set(),
            ack_rx_done: reader.ack_rx_done().bit_is_set(),
            rx_abort: reader.rx_abort().bit_is_set(),
            tx_abort: reader.tx_abort().bit_is_set(),
            ed_done: reader.ed_done().bit_is_set(),
            unclassified_7: reader.unclassified_7().bit_is_set(),
            timer0_overflow: reader.timer0_overflow().bit_is_set(),
            timer1_overflow: reader.timer1_overflow().bit_is_set(),
            clock_count_match: reader.clock_count_match().bit_is_set(),
            tx_sfd_done: reader.tx_sfd_done().bit_is_set(),
            rx_sfd_done: reader.rx_sfd_done().bit_is_set(),
            unclassified_13: reader.unclassified_13().bit_is_set(),
        }
    }

    pub fn from_event_status_snapshot(
        snapshot: &crate::w1c_register_snapshot::Ieee802154EventStatusSnapshot,
    ) -> Self {
        Self {
            tx_done: snapshot.tx_done(),
            rx_done: snapshot.rx_done(),
            ack_tx_done: snapshot.ack_tx_done(),
            ack_rx_done: snapshot.ack_rx_done(),
            rx_abort: snapshot.rx_abort(),
            tx_abort: snapshot.tx_abort(),
            ed_done: snapshot.ed_done(),
            unclassified_7: snapshot.unclassified_7(),
            timer0_overflow: snapshot.timer0_overflow(),
            timer1_overflow: snapshot.timer1_overflow(),
            clock_count_match: snapshot.clock_count_match(),
            tx_sfd_done: snapshot.tx_sfd_done(),
            rx_sfd_done: snapshot.rx_sfd_done(),
            unclassified_13: snapshot.unclassified_13(),
        }
    }

    pub const fn tx_done(self) -> bool {
        self.tx_done
    }

    pub const fn rx_done(self) -> bool {
        self.rx_done
    }

    pub const fn ack_tx_done(self) -> bool {
        self.ack_tx_done
    }

    pub const fn ack_rx_done(self) -> bool {
        self.ack_rx_done
    }

    pub const fn rx_abort(self) -> bool {
        self.rx_abort
    }

    pub const fn tx_abort(self) -> bool {
        self.tx_abort
    }

    pub const fn ed_done(self) -> bool {
        self.ed_done
    }

    pub const fn timer0_overflow(self) -> bool {
        self.timer0_overflow
    }

    pub const fn timer1_overflow(self) -> bool {
        self.timer1_overflow
    }

    pub const fn clock_count_match(self) -> bool {
        self.clock_count_match
    }

    pub const fn tx_sfd_done(self) -> bool {
        self.tx_sfd_done
    }

    pub const fn rx_sfd_done(self) -> bool {
        self.rx_sfd_done
    }

    pub const fn has_unclassified(self) -> bool {
        self.unclassified_7 || self.unclassified_13
    }

    pub const fn is_clear(self) -> bool {
        !self.tx_done
            && !self.rx_done
            && !self.ack_tx_done
            && !self.ack_rx_done
            && !self.rx_abort
            && !self.tx_abort
            && !self.ed_done
            && !self.unclassified_7
            && !self.timer0_overflow
            && !self.timer1_overflow
            && !self.clock_count_match
            && !self.tx_sfd_done
            && !self.rx_sfd_done
            && !self.unclassified_13
    }

    const fn is_ed_operation(self) -> bool {
        self.rx_abort
            && self.ed_done
            && !self.tx_done
            && !self.rx_done
            && !self.ack_tx_done
            && !self.ack_rx_done
            && !self.tx_abort
            && !self.unclassified_7
            && !self.timer0_overflow
            && !self.timer1_overflow
            && !self.clock_count_match
            && !self.tx_sfd_done
            && !self.rx_sfd_done
            && !self.unclassified_13
    }

    const fn is_timer_pair(self) -> bool {
        self.timer0_overflow
            && self.timer1_overflow
            && !self.tx_done
            && !self.rx_done
            && !self.ack_tx_done
            && !self.ack_rx_done
            && !self.rx_abort
            && !self.tx_abort
            && !self.ed_done
            && !self.unclassified_7
            && !self.clock_count_match
            && !self.tx_sfd_done
            && !self.rx_sfd_done
            && !self.unclassified_13
    }

    const fn is_ed_timer_abort(self) -> bool {
        self.rx_abort
            && self.ed_done
            && self.timer0_overflow
            && !self.tx_done
            && !self.rx_done
            && !self.ack_tx_done
            && !self.ack_rx_done
            && !self.tx_abort
            && !self.unclassified_7
            && !self.timer1_overflow
            && !self.clock_count_match
            && !self.tx_sfd_done
            && !self.rx_sfd_done
            && !self.unclassified_13
    }
}

/// Non-secret readable transmit-security control fields.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransmitSecurityControlReadback {
    enabled: bool,
    payload_offset: u8,
}

impl TransmitSecurityControlReadback {
    pub const fn enabled(self) -> bool {
        self.enabled
    }

    pub const fn payload_offset(self) -> u8 {
        self.payload_offset
    }
}

impl TaskRegisters {
    /// Mask every MAC event through the generated closed field variant.
    #[doc(hidden)]
    pub fn mask_all_events(&mut self) {
        self.registers.event_enable().modify(|_, writer| {
            writer.tx_done().clear_bit();
            writer.rx_done().clear_bit();
            writer.ack_tx_done().clear_bit();
            writer.ack_rx_done().clear_bit();
            writer.rx_abort().clear_bit();
            writer.tx_abort().clear_bit();
            writer.ed_done().clear_bit();
            writer.unclassified_7().clear_bit();
            writer.timer0_overflow().clear_bit();
            writer.timer1_overflow().clear_bit();
            writer.clock_count_match().clear_bit();
            writer.tx_sfd_done().clear_bit();
            writer.rx_sfd_done().clear_bit();
            writer.unclassified_13().clear_bit()
        });
    }

    /// Enable the finite ED/CCA operation event set.
    #[doc(hidden)]
    pub fn enable_ed_operation_events(&mut self) {
        self.registers.event_enable().modify(|_, writer| {
            writer.tx_done().clear_bit();
            writer.rx_done().clear_bit();
            writer.ack_tx_done().clear_bit();
            writer.ack_rx_done().clear_bit();
            writer.rx_abort().set_bit();
            writer.tx_abort().clear_bit();
            writer.ed_done().set_bit();
            writer.unclassified_7().clear_bit();
            writer.timer0_overflow().clear_bit();
            writer.timer1_overflow().clear_bit();
            writer.clock_count_match().clear_bit();
            writer.tx_sfd_done().clear_bit();
            writer.rx_sfd_done().clear_bit();
            writer.unclassified_13().clear_bit()
        });
    }

    /// Enable the reviewed runtime event baseline without TIMER0.
    #[doc(hidden)]
    pub fn enable_runtime_events_without_timer0(&mut self) {
        self.registers.event_enable().modify(|_, writer| {
            writer.tx_done().set_bit();
            writer.rx_done().set_bit();
            writer.ack_tx_done().set_bit();
            writer.ack_rx_done().set_bit();
            writer.rx_abort().set_bit();
            writer.tx_abort().set_bit();
            writer.ed_done().set_bit();
            writer.unclassified_7().clear_bit();
            writer.timer0_overflow().clear_bit();
            writer.timer1_overflow().set_bit();
            writer.clock_count_match().clear_bit();
            writer.tx_sfd_done().set_bit();
            writer.rx_sfd_done().set_bit();
            writer.unclassified_13().clear_bit()
        });
    }

    /// Enable the reviewed runtime event baseline with TIMER0.
    #[doc(hidden)]
    pub fn enable_runtime_events_with_timer0(&mut self) {
        self.registers.event_enable().modify(|_, writer| {
            writer.tx_done().set_bit();
            writer.rx_done().set_bit();
            writer.ack_tx_done().set_bit();
            writer.ack_rx_done().set_bit();
            writer.rx_abort().set_bit();
            writer.tx_abort().set_bit();
            writer.ed_done().set_bit();
            writer.unclassified_7().clear_bit();
            writer.timer0_overflow().set_bit();
            writer.timer1_overflow().set_bit();
            writer.clock_count_match().clear_bit();
            writer.tx_sfd_done().set_bit();
            writer.rx_sfd_done().set_bit();
            writer.unclassified_13().clear_bit()
        });
    }

    /// Mask every receive-abort reason through the generated field variant.
    #[doc(hidden)]
    pub fn mask_all_rx_aborts(&mut self) {
        self.registers
            .rx_abort_enable()
            .modify(|_, writer| writer.events().none());
    }

    /// Enable exactly the receive-abort reasons owned by finite ED/CCA.
    #[doc(hidden)]
    pub fn enable_ed_operation_rx_abort_reasons(&mut self) {
        self.registers
            .rx_abort_enable()
            .modify(|_, writer| writer.events().ed_operation_reasons());
    }

    /// Enable the reviewed runtime receive-abort baseline.
    #[doc(hidden)]
    pub fn enable_runtime_rx_aborts(&mut self) {
        self.registers
            .rx_abort_enable()
            .modify(|_, writer| writer.events().runtime_baseline());
    }

    /// Mask every transmit-abort reason through the generated field variant.
    #[doc(hidden)]
    pub fn mask_all_tx_aborts(&mut self) {
        self.registers
            .tx_abort_enable()
            .modify(|_, writer| writer.events().none());
    }

    /// Enable the reviewed runtime transmit-abort baseline.
    #[doc(hidden)]
    pub fn enable_runtime_tx_aborts(&mut self) {
        self.registers
            .tx_abort_enable()
            .modify(|_, writer| writer.events().runtime_baseline());
    }

    /// Select the source-confirmed average ED sample mode.
    #[doc(hidden)]
    pub fn select_average_ed_sampling(&mut self) {
        self.registers
            .ed_config()
            .modify(|_, writer| writer.ed_sample_mode().average());
    }

    /// Read the complete event-delivery state through generated field readers.
    #[doc(hidden)]
    pub fn event_enable_readback(&self) -> Ieee802154EventReadback {
        Ieee802154EventReadback::from_event_enable(&self.registers.event_enable().read())
    }

    /// Classify `EVENT_ENABLE` without exporting its physical field image.
    #[doc(hidden)]
    pub fn operation_event_enable_readback(&self) -> OperationEventEnableReadback {
        let events = self.event_enable_readback();
        if events.is_clear() {
            OperationEventEnableReadback::AllMasked
        } else if events.is_ed_operation() {
            OperationEventEnableReadback::EdOperation
        } else {
            OperationEventEnableReadback::Unexpected
        }
    }

    /// Classify `RX_ABORT_ENABLE` without exporting its physical field image.
    #[doc(hidden)]
    pub fn operation_rx_abort_enable_readback(&self) -> OperationRxAbortEnableReadback {
        let events = self.registers.rx_abort_enable().read().events();
        if events.is_none() {
            OperationRxAbortEnableReadback::AllMasked
        } else if events.is_ed_operation_reasons() {
            OperationRxAbortEnableReadback::EdOperationReasons
        } else {
            OperationRxAbortEnableReadback::Unexpected
        }
    }

    /// Classify the two reset-isolated validation event selections.
    #[doc(hidden)]
    pub fn validation_event_enable_readback(&self) -> ValidationEventEnableReadback {
        let events = self.event_enable_readback();
        if events.is_clear() {
            ValidationEventEnableReadback::AllMasked
        } else if events.is_timer_pair() {
            ValidationEventEnableReadback::TimerPair
        } else if events.is_ed_timer_abort() {
            ValidationEventEnableReadback::EdTimerAbort
        } else {
            ValidationEventEnableReadback::Unexpected
        }
    }

    /// Program the public LL's sixteen-bit subset of the wider ED-duration
    /// register while preserving the unowned high byte.
    #[doc(hidden)]
    pub fn set_ed_duration(&mut self, duration: u16) {
        crate::masked_register_modify::set_ieee802154_ed_duration(
            &self.registers,
            u32::from(duration),
        );
    }

    /// Read the recovered low twenty-four-bit ED duration.
    #[doc(hidden)]
    pub fn ed_duration(&self) -> u32 {
        self.registers.ed_duration().read().duration().bits()
    }

    /// Publish one complete TX DMA address word.
    #[doc(hidden)]
    pub fn publish_transmit_dma_address(&mut self, address: u32) {
        crate::full_register_write::publish_ieee802154_tx_dma_address(&self.registers, address);
    }

    /// Publish one complete RX DMA address word.
    #[doc(hidden)]
    pub fn publish_receive_dma_address(&mut self, address: u32) {
        crate::full_register_write::publish_ieee802154_rx_dma_address(&self.registers, address);
    }

    /// Issue the fixed TX-start image.
    #[doc(hidden)]
    pub fn issue_transmit(&mut self) {
        crate::fixed_register_image::issue_ieee802154_tx_start(&self.registers);
    }

    /// Issue the fixed RX-start image.
    #[doc(hidden)]
    pub fn issue_receive(&mut self) {
        crate::fixed_register_image::issue_ieee802154_rx_start(&self.registers);
    }

    /// Issue the fixed CCA-then-TX image.
    #[doc(hidden)]
    pub fn issue_clear_channel_then_transmit(&mut self) {
        crate::fixed_register_image::issue_ieee802154_cca_tx_start(&self.registers);
    }

    /// Issue the fixed ED-start image.
    #[doc(hidden)]
    pub fn issue_energy_detection(&mut self) {
        crate::fixed_register_image::issue_ieee802154_ed_start(&self.registers);
    }

    /// Issue the fixed state-specific STOP image.
    #[doc(hidden)]
    pub fn issue_stop(&mut self) {
        crate::fixed_register_image::issue_ieee802154_stop(&self.registers);
    }

    /// Publish one complete TIMER0 threshold word.
    #[doc(hidden)]
    pub fn publish_timer0_threshold(&mut self, threshold: u32) {
        crate::full_register_write::publish_ieee802154_timer0_threshold(&self.registers, threshold);
    }

    /// Observe one complete TIMER0 counter word.
    #[doc(hidden)]
    pub fn observe_timer0_value(&self) -> u32 {
        crate::full_register_read::observe_ieee802154_timer0_value(&self.registers)
    }

    /// Issue the fixed TIMER0-start image.
    #[doc(hidden)]
    pub fn issue_timer0_start(&mut self) {
        crate::fixed_register_image::issue_ieee802154_timer0_start(&self.registers);
    }

    /// Issue the fixed TIMER0-stop image.
    #[doc(hidden)]
    pub fn issue_timer0_stop(&mut self) {
        crate::fixed_register_image::issue_ieee802154_timer0_stop(&self.registers);
    }

    /// Publish one complete TIMER1 threshold word.
    #[doc(hidden)]
    pub fn publish_timer1_threshold(&mut self, threshold: u32) {
        crate::full_register_write::publish_ieee802154_timer1_threshold(&self.registers, threshold);
    }

    /// Observe one complete TIMER1 counter word.
    #[doc(hidden)]
    pub fn observe_timer1_value(&self) -> u32 {
        crate::full_register_read::observe_ieee802154_timer1_value(&self.registers)
    }

    /// Issue the fixed TIMER1-start image.
    #[doc(hidden)]
    pub fn issue_timer1_start(&mut self) {
        crate::fixed_register_image::issue_ieee802154_timer1_start(&self.registers);
    }

    /// Issue the fixed TIMER1-stop image.
    #[doc(hidden)]
    pub fn issue_timer1_stop(&mut self) {
        crate::fixed_register_image::issue_ieee802154_timer1_stop(&self.registers);
    }

    /// Replace only the recovered eight-bit channel/frequency field.
    #[doc(hidden)]
    pub fn set_frequency_code(&mut self, code: u8) {
        self.registers
            .channel()
            .modify(|_, writer| writer.frequency_code().set(code));
    }

    /// Replace only the recovered eight-bit raw TX-power field.
    #[doc(hidden)]
    pub fn set_tx_power_code(&mut self, code: u32) {
        set_tx_power_code(&mut self.registers, code);
    }

    /// Replace the two-bit CCA-mode field.
    #[doc(hidden)]
    pub fn set_cca_mode(&mut self, mode: u8) {
        match mode {
            0 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.cca_mode().carrier()),
            1 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.cca_mode().energy_detection()),
            2 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.cca_mode().carrier_or_energy_detection()),
            3 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.cca_mode().carrier_and_energy_detection()),
            _ => panic!("CCA mode exceeds two bits"),
        };
    }

    /// Replace the signed eight-bit CCA threshold field.
    #[doc(hidden)]
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers
            .ed_config()
            .modify(|_, writer| writer.cca_threshold_code().set(threshold as u8));
    }

    /// Replace the two-bit ED sample-rate field.
    #[doc(hidden)]
    pub fn set_ed_sample_rate(&mut self, rate: u8) {
        match rate {
            0 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.ed_sample_rate().one_per_us()),
            1 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.ed_sample_rate().two_per_us()),
            2 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.ed_sample_rate().four_per_us()),
            3 => self
                .registers
                .ed_config()
                .modify(|_, writer| writer.ed_sample_rate().eight_per_us()),
            _ => panic!("ED sample rate exceeds two bits"),
        };
    }

    /// Replace the complete sixteen-bit ACK-timeout field.
    #[doc(hidden)]
    pub fn set_ack_timeout(&mut self, timeout: u16) {
        self.registers
            .ack_timeout()
            .modify(|_, writer| writer.timeout().set(timeout));
    }

    /// Apply all six public PIB control bits in source order.
    #[doc(hidden)]
    pub fn set_mac_control(
        &mut self,
        auto_ack_tx: bool,
        auto_ack_rx: bool,
        enhanced_ack_tx: bool,
        coordinator: bool,
        promiscuous: bool,
        pending_enhanced: bool,
    ) {
        let register = self.registers.control();
        register.modify(|_, writer| writer.auto_ack_tx().bit(auto_ack_tx));
        register.modify(|_, writer| writer.auto_ack_rx().bit(auto_ack_rx));
        register.modify(|_, writer| writer.enhanced_ack_tx().bit(enhanced_ack_tx));
        register.modify(|_, writer| writer.coordinator().bit(coordinator));
        register.modify(|_, writer| writer.promiscuous().bit(promiscuous));
        register.modify(|_, writer| writer.pending_enhanced().bit(pending_enhanced));
    }

    /// Replace all four named multipan enable fields.
    #[doc(hidden)]
    pub fn set_multipan_enabled(&mut self, enabled: [bool; 4]) {
        self.registers.control().modify(|_, writer| {
            writer
                .multipan0_enabled()
                .bit(enabled[0])
                .multipan1_enabled()
                .bit(enabled[1])
                .multipan2_enabled()
                .bit(enabled[2])
                .multipan3_enabled()
                .bit(enabled[3])
        });
    }

    /// Program one complete source-confirmed multipan identity transaction.
    #[doc(hidden)]
    pub fn set_multipan_identity(
        &mut self,
        index: usize,
        pan_id: u16,
        short_address: u16,
        extended_address: [u8; 8],
    ) {
        execute_multipan_identity_configuration(
            &mut self.registers,
            index,
            pan_id,
            short_address,
            extended_address,
        );
    }

    #[doc(hidden)]
    pub fn multipan_identity(&self, index: usize) -> MultipanIdentityReadback {
        assert!(index < 4, "multipan index exceeds four contexts");
        let pan_id = self.registers.multipan_pan_id(index).read();
        let short_address = self.registers.multipan_short_address(index).read();
        let extended_low = self.registers.multipan_extended_address_low(index).read();
        let extended_high = self.registers.multipan_extended_address_high(index).read();
        let low = extended_low.address_word().bits().to_le_bytes();
        let high = extended_high.address_word().bits().to_le_bytes();
        MultipanIdentityReadback {
            pan_id: pan_id.pan_id().bits(),
            short_address: short_address.address().bits(),
            extended_address: [
                low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
            ],
        }
    }

    /// Read all four named multipan enable fields.
    #[doc(hidden)]
    pub fn multipan_enabled(&self) -> [bool; 4] {
        let control = self.registers.control().read();
        [
            control.multipan0_enabled().bit_is_set(),
            control.multipan1_enabled().bit_is_set(),
            control.multipan2_enabled().bit_is_set(),
            control.multipan3_enabled().bit_is_set(),
        ]
    }

    /// Set the outgoing ACK frame-pending bit through a preserving update.
    #[doc(hidden)]
    pub fn set_frame_pending(&mut self, pending: bool) {
        self.registers
            .pending_config()
            .modify(|_, writer| writer.frame_pending().bit(pending));
    }

    /// Read the outgoing ACK frame-pending bit.
    #[doc(hidden)]
    pub fn frame_pending(&self) -> bool {
        self.registers
            .pending_config()
            .read()
            .frame_pending()
            .bit_is_set()
    }

    /// Publish the sole source-confirmed enhanced-ACK notification image.
    #[doc(hidden)]
    pub fn notify_enhanced_ack_generated(&mut self) {
        notify_enhanced_ack_generated(&mut self.registers);
    }

    /// Configure transmit security in exact source order.
    #[doc(hidden)]
    pub fn configure_transmit_security(
        &mut self,
        address: &[u8; 8],
        key: &[u8; 16],
        payload_offset: u8,
    ) {
        configure_transmit_security(&mut self.registers, address, key, payload_offset);
    }

    /// Disable transmit security without claiming register zeroization.
    #[doc(hidden)]
    pub fn disable_transmit_security(&mut self) {
        disable_transmit_security(&mut self.registers);
    }

    /// Read only the non-secret transmit-security control fields.
    #[doc(hidden)]
    pub fn transmit_security_control(&self) -> TransmitSecurityControlReadback {
        let control = self.registers.security_control().read();
        TransmitSecurityControlReadback {
            enabled: control.tx_enable().bit_is_set(),
            payload_offset: control.payload_offset().bits(),
        }
    }

    /// Replace only the generated five-bit TX/RX coexistence PTI field.
    #[doc(hidden)]
    pub fn set_txrx_pti(&mut self, pti: u8) {
        assert!(pti <= 0x1f, "TX/RX PTI exceeds five bits");
        self.registers
            .coex_pti()
            .modify(|_, writer| writer.txrx_pti().set(pti));
    }

    /// Replace only the generated five-bit ACK coexistence PTI field.
    #[doc(hidden)]
    pub fn set_ack_pti(&mut self, pti: u8) {
        assert!(pti <= 0x1f, "ACK PTI exceeds five bits");
        self.registers
            .coex_pti()
            .modify(|_, writer| writer.ack_pti().set(pti));
    }

    /// Read the complete interrupt-masked task-side foundation.
    #[doc(hidden)]
    pub fn foundation_readback(&self) -> FoundationReadback {
        let event_enable = self.registers.event_enable().read();
        let rx_abort_enable = self.registers.rx_abort_enable().read();
        let tx_abort_enable = self.registers.tx_abort_enable().read();
        let ed_config = self.registers.ed_config().read();
        let coex_pti = self.registers.coex_pti().read();
        FoundationReadback {
            events_masked: Ieee802154EventReadback::from_event_enable(&event_enable).is_clear(),
            rx_aborts_masked: rx_abort_enable.events().is_none(),
            tx_aborts_masked: tx_abort_enable.events().is_none(),
            ed_uses_average: ed_config.ed_sample_mode().is_average(),
            txrx_pti: coex_pti.txrx_pti().bits(),
            ack_pti: coex_pti.ack_pti().bits(),
        }
    }

    /// Read the complete task-owned static policy subset once per word.
    #[doc(hidden)]
    pub fn static_mac_policy_readback(&self) -> StaticMacPolicyReadback {
        let channel = self.registers.channel().read();
        let ed_config = self.registers.ed_config().read();
        let ack_timeout = self.registers.ack_timeout().read();
        let control = self.registers.control().read();
        StaticMacPolicyReadback {
            frequency_code: channel.frequency_code().bits(),
            cca_mode: ed_config.cca_mode().bits(),
            cca_threshold_code: ed_config.cca_threshold_code().bits(),
            ack_timeout: ack_timeout.timeout().bits(),
            auto_ack_tx: control.auto_ack_tx().bit_is_set(),
            auto_ack_rx: control.auto_ack_rx().bit_is_set(),
            enhanced_ack_tx: control.enhanced_ack_tx().bit_is_set(),
            coordinator: control.coordinator().bit_is_set(),
            promiscuous: control.promiscuous().bit_is_set(),
            pending_enhanced: control.pending_enhanced().bit_is_set(),
            multipan_enabled: [
                control.multipan0_enabled().bit_is_set(),
                control.multipan1_enabled().bit_is_set(),
                control.multipan2_enabled().bit_is_set(),
                control.multipan3_enabled().bit_is_set(),
            ],
            identity: self.multipan_identity(0),
        }
    }

    /// Read every currently modeled task-side configuration field once per
    /// backing word. This includes dynamic values absent from static policy.
    #[doc(hidden)]
    pub fn mac_configuration_readback(&self) -> MacConfigurationReadback {
        let channel = self.registers.channel().read();
        let tx_power = self.registers.tx_power().read();
        let ed_config = self.registers.ed_config().read();
        let ack_timeout = self.registers.ack_timeout().read();
        let control = self.registers.control().read();
        let pending_config = self.registers.pending_config().read();
        MacConfigurationReadback {
            frequency_code: channel.frequency_code().bits(),
            tx_power_code: tx_power.power_code().bits(),
            cca_mode: ed_config.cca_mode().bits(),
            cca_threshold_code: ed_config.cca_threshold_code().bits(),
            ed_sample_rate: ed_config.ed_sample_rate().bits(),
            ack_timeout: ack_timeout.timeout().bits(),
            auto_ack_tx: control.auto_ack_tx().bit_is_set(),
            auto_ack_rx: control.auto_ack_rx().bit_is_set(),
            enhanced_ack_tx: control.enhanced_ack_tx().bit_is_set(),
            coordinator: control.coordinator().bit_is_set(),
            promiscuous: control.promiscuous().bit_is_set(),
            pending_enhanced: control.pending_enhanced().bit_is_set(),
            multipan_enabled: [
                control.multipan0_enabled().bit_is_set(),
                control.multipan1_enabled().bit_is_set(),
                control.multipan2_enabled().bit_is_set(),
                control.multipan3_enabled().bit_is_set(),
            ],
            identities: [
                self.multipan_identity(0),
                self.multipan_identity(1),
                self.multipan_identity(2),
                self.multipan_identity(3),
            ],
            frame_pending: pending_config.frame_pending().bit_is_set(),
        }
    }

    /// Apply the sole source-confirmed RXON delay image used by IEEE timing.
    #[doc(hidden)]
    pub fn set_rx_on_delay_50(&mut self) {
        crate::masked_register_modify::set_ieee802154_rx_on_delay(&self.registers, 50);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_timer_events(&mut self) {
        crate::ieee802154_event_status_validation::enable_timer_events(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_all_events(&mut self) {
        crate::ieee802154_event_status_validation::disable_all_events(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_timer0_value(&self) -> u32 {
        crate::ieee802154_event_status_validation::timer0_value(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_timer1_value(&self) -> u32 {
        crate::ieee802154_event_status_validation::timer1_value(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_timer_thresholds(&mut self, threshold: u32) {
        crate::ieee802154_event_status_validation::set_timer_thresholds(&self.registers, threshold);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_timer0(&mut self) {
        crate::ieee802154_event_status_validation::start_timer0(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_timer0(&mut self) {
        crate::ieee802154_event_status_validation::stop_timer0(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_timer1(&mut self) {
        crate::ieee802154_event_status_validation::start_timer1(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_timer1(&mut self) {
        crate::ieee802154_event_status_validation::stop_timer1(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_timer_abort_events(&mut self) {
        crate::ieee802154_ed_event_validation::enable_ed_timer_abort_events(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_events(&mut self) {
        crate::ieee802154_ed_event_validation::disable_all_events(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_abort_reasons(&mut self) {
        crate::ieee802154_ed_event_validation::enable_ed_abort_reasons(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_abort_reasons(&mut self) {
        crate::ieee802154_ed_event_validation::disable_all_rx_abort_reasons(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_duration(&self) -> u32 {
        crate::ieee802154_ed_event_validation::ed_duration(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_duration_eight(&mut self) {
        crate::ieee802154_ed_event_validation::set_ed_duration_eight(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_timer0_value(&self) -> u32 {
        crate::ieee802154_ed_event_validation::timer0_value(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_timer0_threshold(&mut self, threshold: u32) {
        crate::ieee802154_ed_event_validation::set_timer0_threshold(&mut self.registers, threshold);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed_timer0(&mut self) {
        crate::ieee802154_ed_event_validation::start_timer0(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_timer0(&mut self) {
        crate::ieee802154_ed_event_validation::stop_timer0(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed(&mut self) {
        crate::ieee802154_ed_event_validation::start_ed(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_operation(&mut self) {
        crate::ieee802154_ed_event_validation::stop_operation(&mut self.registers);
    }
}

/// Inactive or ISR-owned IEEE 802.15.4 event/status capability.
///
/// There is no conversion to the task owner and no raw register accessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptRxStatusReadback {
    bits: u32,
    abort_reason_code: u8,
}

impl InterruptRxStatusReadback {
    pub const fn bits(&self) -> u32 {
        self.bits
    }

    pub const fn abort_reason_code(&self) -> u8 {
        self.abort_reason_code
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptTxStatusReadback {
    bits: u32,
    abort_reason_code: u8,
}

impl InterruptTxStatusReadback {
    pub const fn bits(&self) -> u32 {
        self.bits
    }

    pub const fn abort_reason_code(&self) -> u8 {
        self.abort_reason_code
    }
}

#[must_use = "the interrupt owner must be deactivated and reunited"]
pub struct InterruptRegisters {
    registers: crate::Ieee802154Mac,
}

impl InterruptRegisters {
    /// Sample the complete fourteen-bit event field exactly once.
    #[inline]
    pub fn sample_event_status(
        &self,
    ) -> crate::w1c_register_snapshot::Ieee802154EventStatusSnapshot {
        crate::w1c_register_snapshot::sample_ieee802154_event_status(&self.registers)
    }

    /// Acknowledge exactly one previously sampled event field and consume it.
    #[inline]
    pub fn acknowledge_event_status(
        &mut self,
        snapshot: crate::w1c_register_snapshot::Ieee802154EventStatusSnapshot,
    ) {
        crate::w1c_register_snapshot::acknowledge_ieee802154_event_status(
            &mut self.registers,
            snapshot,
        );
    }

    /// Observe the complete RX status word captured for an RX-abort event.
    #[inline]
    pub fn rx_status_readback(&self) -> InterruptRxStatusReadback {
        let status = self.registers.rx_status().read();
        InterruptRxStatusReadback {
            bits: status.bits(),
            abort_reason_code: status.abort_reason().bits(),
        }
    }

    /// Observe the complete TX status word captured for a TX-abort event.
    #[inline]
    pub fn tx_status_readback(&self) -> InterruptTxStatusReadback {
        let status = self.registers.tx_status().read();
        InterruptTxStatusReadback {
            bits: status.bits(),
            abort_reason_code: status.abort_reason().bits(),
        }
    }

    /// Observe the signed energy-detection result captured for ED-DONE.
    #[inline]
    pub fn ed_rss_code(&self) -> i8 {
        self.registers.ed_config().read().ed_rss_code().bits() as i8
    }

    /// Observe the CCA result captured for ED-DONE.
    #[inline]
    pub fn cca_busy(&self) -> bool {
        self.registers.ed_config().read().cca_busy().bit_is_set()
    }

    /// Observe only the generated RX/TX state subfields from the IRQ-owned
    /// sideband words.
    #[doc(hidden)]
    pub fn state_codes(&self) -> (u8, u8) {
        (
            self.registers.rx_status().read().state().bits(),
            self.registers.tx_status().read().state().bits(),
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_status_events(&self) -> Ieee802154EventReadback {
        crate::ieee802154_event_status_validation::event_status_events(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_timer0_event(&mut self) {
        crate::ieee802154_event_status_validation::write_timer0_event(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_timer1_event(&mut self) {
        crate::ieee802154_event_status_validation::write_timer1_event(&self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_status_events(&self) -> Ieee802154EventReadback {
        crate::ieee802154_ed_event_validation::event_status_events(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_status_raw(&self) -> u32 {
        crate::ieee802154_ed_event_validation::rx_status_raw(&self.registers)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_done_event(&mut self) {
        crate::ieee802154_ed_event_validation::write_ed_done_event(&mut self.registers);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_timer0_event(&mut self) {
        crate::ieee802154_ed_event_validation::write_timer0_event(&mut self.registers);
    }
}

/// Split one unique MAC owner into disjoint task and interrupt roles.
///
/// # Safety invariant
///
/// The only duplicated raw handle is retained privately by
/// [`InterruptRegisters`], which exposes no command, policy, DMA, event-enable,
/// or generic register operation. The high-level PAC similarly keeps the task
/// handle behind a task-only surface. Reuniting consumes both handles and drops
/// the duplicate before returning the original owner.
#[inline]
pub fn split(registers: crate::Ieee802154Mac) -> (TaskRegisters, InterruptRegisters) {
    // SAFETY: `registers` was consumed above. The duplicate remains private in
    // the IRQ role, whose safe methods touch only EVENT_STATUS, RX_STATUS,
    // TX_STATUS and ED_CONFIG observations. The task role exposed by the
    // restricted parent PAC does not offer EVENT_STATUS acknowledge or abort
    // status snapshot methods while the roles are separated.
    let interrupt = unsafe { crate::Ieee802154Mac::steal() };
    (
        TaskRegisters { registers },
        InterruptRegisters {
            registers: interrupt,
        },
    )
}

/// Consume both roles and recover the unique complete MAC owner.
#[inline]
pub fn reunite(task: TaskRegisters, interrupt: InterruptRegisters) -> crate::Ieee802154Mac {
    let TaskRegisters { registers } = task;
    let InterruptRegisters {
        registers: _duplicate,
    } = interrupt;
    registers
}

#[cfg(test)]
mod tests {
    extern crate std;

    use self::std::vec::Vec;
    use super::{
        MultipanIdentityProgrammingPort, TransmitSecurityProgrammingPort,
        execute_multipan_identity_configuration, execute_transmit_security_configuration,
        execute_transmit_security_disable,
    };

    #[derive(Debug, Eq, PartialEq)]
    enum Operation {
        AddressLow(u32),
        AddressHigh(u32),
        KeyWord(usize, u32),
        PayloadOffset(u8),
        Enabled(bool),
    }

    #[derive(Default)]
    struct RecordingPort {
        operations: Vec<Operation>,
    }

    impl TransmitSecurityProgrammingPort for RecordingPort {
        fn write_address_low(&mut self, word: u32) {
            self.operations.push(Operation::AddressLow(word));
        }

        fn write_address_high(&mut self, word: u32) {
            self.operations.push(Operation::AddressHigh(word));
        }

        fn write_key_word(&mut self, index: usize, word: u32) {
            self.operations.push(Operation::KeyWord(index, word));
        }

        fn write_payload_offset(&mut self, offset: u8) {
            self.operations.push(Operation::PayloadOffset(offset));
        }

        fn set_enabled(&mut self, enabled: bool) {
            self.operations.push(Operation::Enabled(enabled));
        }
    }

    #[derive(Debug, Eq, PartialEq)]
    enum MultipanOperation {
        Enable(usize),
        PanId(usize, u16),
        ShortAddress(usize, u16),
        ExtendedLow(usize, u32),
        ExtendedHigh(usize, u32),
    }

    #[derive(Default)]
    struct RecordingMultipanPort {
        operations: Vec<MultipanOperation>,
    }

    impl MultipanIdentityProgrammingPort for RecordingMultipanPort {
        fn enable_context(&mut self, index: usize) {
            self.operations.push(MultipanOperation::Enable(index));
        }

        fn write_pan_id(&mut self, index: usize, pan_id: u16) {
            self.operations
                .push(MultipanOperation::PanId(index, pan_id));
        }

        fn write_short_address(&mut self, index: usize, short_address: u16) {
            self.operations
                .push(MultipanOperation::ShortAddress(index, short_address));
        }

        fn write_extended_address_low(&mut self, index: usize, word: u32) {
            self.operations
                .push(MultipanOperation::ExtendedLow(index, word));
        }

        fn write_extended_address_high(&mut self, index: usize, word: u32) {
            self.operations
                .push(MultipanOperation::ExtendedHigh(index, word));
        }
    }

    #[test]
    fn transmit_security_transaction_matches_source_order_and_little_endian_words() {
        let mut port = RecordingPort::default();
        execute_transmit_security_configuration(
            &mut port,
            &[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
            &[
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ],
            0x6d,
        );

        assert_eq!(
            port.operations,
            [
                Operation::AddressLow(0x7654_3210),
                Operation::AddressHigh(0xfedc_ba98),
                Operation::KeyWord(0, 0x3322_1100),
                Operation::KeyWord(1, 0x7766_5544),
                Operation::KeyWord(2, 0xbbaa_9988),
                Operation::KeyWord(3, 0xffee_ddcc),
                Operation::PayloadOffset(0x6d),
                Operation::Enabled(true),
            ]
        );
    }

    #[test]
    fn transmit_security_disable_only_clears_enable() {
        let mut port = RecordingPort::default();
        execute_transmit_security_disable(&mut port);
        assert_eq!(port.operations, [Operation::Enabled(false)]);
    }

    #[test]
    fn multipan_identity_transaction_has_exact_enable_and_write_order_for_every_context() {
        for index in 0..4 {
            let mut port = RecordingMultipanPort::default();
            execute_multipan_identity_configuration(
                &mut port,
                index,
                0x1234,
                0x5678,
                [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
            );
            assert_eq!(
                port.operations,
                [
                    MultipanOperation::Enable(index),
                    MultipanOperation::PanId(index, 0x1234),
                    MultipanOperation::Enable(index),
                    MultipanOperation::ShortAddress(index, 0x5678),
                    MultipanOperation::Enable(index),
                    MultipanOperation::ExtendedLow(index, 0x7654_3210),
                    MultipanOperation::ExtendedHigh(index, 0xfedc_ba98),
                ]
            );
        }
    }
}
