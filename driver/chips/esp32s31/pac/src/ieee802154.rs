//! Typed lower-level ownership for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! Every MMIO operation in this module is routed through the reviewed
//! generated `IEEE802154_MAC` peripheral. The narrow lease exposes only the
//! first field-sized operations needed by HAL; neither the generated register
//! block nor numeric addresses can escape it.

#![forbid(unsafe_code)]

use super::WifiRadioRegisters;
pub use crate::generated::Ieee802154EdDurationUnits;

/// Opaque eight-bit value accepted by the MAC frequency-code register.
///
/// This is deliberately not an IEEE channel number. The checked 2.4 GHz
/// channel mapping is source-confirmed and owned by the HAL; the PAC type
/// still represents the complete recovered field rather than silently
/// narrowing register geometry to one operating mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154FrequencyCode(u8);

impl Ieee802154FrequencyCode {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Return the field value, not a complete register image.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl From<u8> for Ieee802154FrequencyCode {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

/// One five-bit coexistence priority value.
///
/// The value is intentionally not a complete PTI register image. The PAC
/// lease places it through named generated fields and preserves all unrelated
/// bits.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154Pti(u8);

impl Ieee802154Pti {
    pub const MAX: u8 = 0x1f;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the bounded field value, not a shifted register image.
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// One source-confirmed clear-channel-assessment mode.
///
/// The discriminants are field values, not shifted register images.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154CcaMode {
    Carrier = 0,
    EnergyDetection = 1,
    CarrierOrEnergyDetection = 2,
    CarrierAndEnergyDetection = 3,
}

impl Ieee802154CcaMode {
    pub const fn field_value(self) -> u8 {
        self as u8
    }

    const fn from_field(value: u8) -> Self {
        match value {
            0 => Self::Carrier,
            1 => Self::EnergyDetection,
            2 => Self::CarrierOrEnergyDetection,
            3 => Self::CarrierAndEnergyDetection,
            _ => unreachable!(),
        }
    }
}

/// Sixteen-bit ACK-timeout field value.
///
/// The PAC deliberately does not assign physical units. The HAL owns the
/// source-confirmed conversion between microseconds and this field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154AckTimeoutUnits(u16);

impl Ieee802154AckTimeoutUnits {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

impl Ieee802154EdDurationUnits {
    const fn from_field(value: u32) -> Option<Self> {
        Self::new(value)
    }
}

/// One finite energy-detection command accepted by the narrow PAC lease.
///
/// `Stop` maps to the source-confirmed common MAC `STOP` opcode, but this type
/// grants no generic STOP operation to callers outside the ED transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154EdCommand {
    /// Start one configured energy-detection/CCA sampling transaction.
    Start,
    /// Stop the active energy-detection transaction.
    Stop,
}

/// Named `EVENT_ENABLE` bits accepted by the typed PAC API.
///
/// Physical bits seven and thirteen are intentionally absent because the
/// pinned public LL assigns them no meaning. The private image prevents those
/// bits or bits outside the fourteen-bit field from reaching production
/// `EVENT_ENABLE` writes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventEnableMask(u16);

impl Ieee802154EventEnableMask {
    /// Every event bit named by the pinned public LL.
    pub const NAMED_BITS: u16 = 0x1f7f;

    /// Empty event-enable set.
    pub const NONE: Self = Self(0);
    /// Transmission completed, bit zero.
    pub const TX_DONE: Self = Self(1 << 0);
    /// Reception completed, bit one.
    pub const RX_DONE: Self = Self(1 << 1);
    /// Automatic ACK transmission completed, bit two.
    pub const ACK_TX_DONE: Self = Self(1 << 2);
    /// ACK reception completed, bit three.
    pub const ACK_RX_DONE: Self = Self(1 << 3);
    /// Receive processing aborted, bit four.
    pub const RX_ABORT: Self = Self(1 << 4);
    /// Transmit processing aborted, bit five.
    pub const TX_ABORT: Self = Self(1 << 5);
    /// Energy detection completed, bit six.
    pub const ED_DONE: Self = Self(1 << 6);
    /// The complete event window owned by one finite polled ED/CCA operation.
    pub const ED_DONE_AND_RX_ABORT: Self = Self(Self::ED_DONE.0 | Self::RX_ABORT.0);
    /// Timer zero overflowed, bit eight.
    pub const TIMER0_OVERFLOW: Self = Self(1 << 8);
    /// Timer one overflowed, bit nine.
    pub const TIMER1_OVERFLOW: Self = Self(1 << 9);
    /// MAC clock count matched, bit ten.
    pub const CLOCK_COUNT_MATCH: Self = Self(1 << 10);
    /// Transmission SFD processing completed, bit eleven.
    pub const TX_SFD_DONE: Self = Self(1 << 11);
    /// Reception SFD processing completed, bit twelve.
    pub const RX_SFD_DONE: Self = Self(1 << 12);
    /// Every source-confirmed named event.
    pub const ALL_NAMED: Self = Self(Self::NAMED_BITS);

    /// Validate a complete caller-supplied event field image.
    pub const fn from_named_bits(bits: u16) -> Option<Self> {
        if bits & !Self::NAMED_BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Return the exact named field image.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Combine two already validated named-event sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Return whether every event selected by `required` is enabled.
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

/// Closed `RX_ABORT_ENABLE` field image for one finite ED/CCA operation.
///
/// The public LL identifies receive-abort reasons 24, 25, and 26 as ED abort,
/// ED stop, and ED coexistence rejection. Their enable positions are bits
/// 23, 24, and 25. No constructor accepts a caller-provided image, so this
/// type can represent only a fully masked field or that exact three-reason
/// operation set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154RxAbortEnableMask(u32);

impl Ieee802154RxAbortEnableMask {
    /// Every receive-abort reason masked.
    pub const NONE: Self = Self(0);
    /// Exactly ED abort, ED stop, and ED coexistence rejection.
    pub const ED_OPERATION_REASONS: Self = Self(0x0380_0000);

    /// Return the exact closed field image.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Semantic readback of the event-enable field owned by a polled ED/CCA
/// operation.
///
/// `Unexpected` deliberately combines every other fourteen-bit image. It
/// never projects an unexpected image into a writable mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationEventEnableObservation {
    /// Every event is masked.
    AllMasked,
    /// Exactly `ED_DONE` and `RX_ABORT` are enabled.
    EdDoneAndRxAbortOnly,
    /// A required event is missing or at least one other event is enabled.
    Unexpected,
}

impl Ieee802154OperationEventEnableObservation {
    const fn from_field(bits: u16) -> Self {
        match bits {
            0 => Self::AllMasked,
            bits if bits == Ieee802154EventEnableMask::ED_DONE_AND_RX_ABORT.bits() => {
                Self::EdDoneAndRxAbortOnly
            }
            _ => Self::Unexpected,
        }
    }
}

/// Semantic readback of the receive-abort-enable field owned by a polled
/// ED/CCA operation.
///
/// The observation has no conversion to [`Ieee802154RxAbortEnableMask`], so
/// unexpected hardware state cannot accidentally become a writable image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationRxAbortEnableObservation {
    /// Every receive-abort reason is masked.
    AllMasked,
    /// Exactly the three ED-operation reasons are enabled.
    EdOperationReasonsOnly,
    /// A required reason is missing or at least one other reason is enabled.
    Unexpected,
}

impl Ieee802154OperationRxAbortEnableObservation {
    const fn from_field(bits: u32) -> Self {
        match bits {
            0 => Self::AllMasked,
            bits if bits == Ieee802154RxAbortEnableMask::ED_OPERATION_REASONS.bits() => {
                Self::EdOperationReasonsOnly
            }
            _ => Self::Unexpected,
        }
    }
}

/// Read-only fourteen-bit `EVENT_ENABLE` or `EVENT_STATUS` observation.
///
/// Unlike [`Ieee802154EventEnableMask`], this type preserves unnamed physical
/// bits because observations must not erase unexpected hardware state. It has
/// no public constructor and cannot be passed to the production enable write.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventObservation(u16);

impl Ieee802154EventObservation {
    const FIELD_MASK: u16 = 0x3fff;

    const fn from_field(bits: u16) -> Self {
        Self(bits & Self::FIELD_MASK)
    }

    /// Return the complete observed fourteen-bit field image.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Return whether the complete observed event field is clear.
    pub const fn is_clear(self) -> bool {
        self.0 == 0
    }

    /// Return every observed bit without a source-confirmed event name.
    pub const fn unnamed_bits(self) -> u16 {
        self.0 & !Ieee802154EventEnableMask::NAMED_BITS
    }

    /// Return whether every named event in `required` was observed.
    pub const fn contains(self, required: Ieee802154EventEnableMask) -> bool {
        self.0 & required.bits() == required.bits()
    }

    /// Project only source-confirmed event bits into a writable mask.
    pub const fn named(self) -> Ieee802154EventEnableMask {
        Ieee802154EventEnableMask(self.0 & Ieee802154EventEnableMask::NAMED_BITS)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(bits: u16) -> Option<Self> {
        if bits & !Self::FIELD_MASK == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }
}

/// Complete read-only `RX_STATUS` register observation.
///
/// Abort evidence must retain the whole word, including fields whose
/// semantics are not yet classified. The private image cannot be used for a
/// register write or converted into an enable mask.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154RxStatusObservation(u32);

impl Ieee802154RxStatusObservation {
    const fn from_register(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete observed register word for retained evidence.
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[cfg(test)]
    const fn for_validation(bits: u32) -> Self {
        Self(bits)
    }
}

/// One ordered DMA-free energy-detection/CCA register sample.
///
/// `EVENT_STATUS` is observation only. This value has no acknowledge or write
/// operation because HIL has not established a production access class for
/// the complete fourteen-bit register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EdCcaSnapshot {
    duration: Option<Ieee802154EdDurationUnits>,
    enabled_events: Ieee802154EventObservation,
    pending_events: Ieee802154EventObservation,
    rss_code: i8,
    cca_busy: bool,
}

impl Ieee802154EdCcaSnapshot {
    #[doc(hidden)]
    pub const fn new(
        duration: Option<Ieee802154EdDurationUnits>,
        enabled_events: Ieee802154EventObservation,
        pending_events: Ieee802154EventObservation,
        rss_code: i8,
        cca_busy: bool,
    ) -> Self {
        Self {
            duration,
            enabled_events,
            pending_events,
            rss_code,
            cca_busy,
        }
    }

    /// Return the configured finite LL duration field.
    /// `None` means the observed twenty-four-bit field is outside the strict
    /// public-LL `uint16_t` subset; no truncation is performed.
    pub const fn duration(self) -> Option<Ieee802154EdDurationUnits> {
        self.duration
    }

    /// Return the complete observed `EVENT_ENABLE` field.
    pub const fn enabled_events(self) -> Ieee802154EventObservation {
        self.enabled_events
    }

    /// Return the complete read-only `EVENT_STATUS` observation.
    pub const fn pending_events(self) -> Ieee802154EventObservation {
        self.pending_events
    }

    /// Return the signed source-defined ED RSS code.
    ///
    /// Conversion to dBm remains a HAL/radio-policy responsibility.
    pub const fn rss_code(self) -> i8 {
        self.rss_code
    }

    /// Return the sampled generated `CCA_BUSY` bit.
    pub const fn cca_busy(self) -> bool {
        self.cca_busy
    }
}

/// Source-confirmed MAC control fields programmed as one semantic policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacControl {
    tx_auto_ack: bool,
    rx_auto_ack: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    enhanced_pending: bool,
}

impl Ieee802154MacControl {
    pub const fn new(
        tx_auto_ack: bool,
        rx_auto_ack: bool,
        enhanced_ack_tx: bool,
        coordinator: bool,
        promiscuous: bool,
        enhanced_pending: bool,
    ) -> Self {
        Self {
            tx_auto_ack,
            rx_auto_ack,
            enhanced_ack_tx,
            coordinator,
            promiscuous,
            enhanced_pending,
        }
    }

    pub const fn tx_auto_ack(self) -> bool {
        self.tx_auto_ack
    }

    pub const fn rx_auto_ack(self) -> bool {
        self.rx_auto_ack
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

    pub const fn enhanced_pending(self) -> bool {
        self.enhanced_pending
    }
}

/// Address-filter identity for the public API's primary PAN context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PanIdentity {
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
}

impl Ieee802154PanIdentity {
    pub const fn new(pan_id: u16, short_address: u16, extended_address: [u8; 8]) -> Self {
        Self {
            pan_id,
            short_address,
            extended_address,
        }
    }

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

/// Opaque three-bit receive-state observation.
///
/// Only the comparison around the publicly identified `RECEIVE_SFD` value is
/// exposed. Zero is intentionally not named `idle` until lifecycle evidence
/// proves that interpretation for the ESP32-S31.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStateCode(u8);

impl Ieee802154RxStateCode {
    pub const MAX: u8 = 0x07;
    pub const RECEIVE_SFD: u8 = 1;

    pub const fn is_receive_sfd(self) -> bool {
        self.0 == Self::RECEIVE_SFD
    }

    pub const fn is_after_receive_sfd(self) -> bool {
        self.0 > Self::RECEIVE_SFD
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    const fn from_field(value: u8) -> Self {
        Self(value)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// Opaque four-bit transmit-state observation.
///
/// No individual value is assigned a lifecycle meaning by this foundation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxStateCode(u8);

impl Ieee802154TxStateCode {
    pub const MAX: u8 = 0x0f;

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    const fn from_field(value: u8) -> Self {
        Self(value)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// One paired receive/transmit state sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154StateSnapshot {
    rx: Ieee802154RxStateCode,
    tx: Ieee802154TxStateCode,
}

/// Raw paired CPU-route observation from the validation-only PAC sidecar.
///
/// This type contains evidence only: it cannot expose a register pointer or
/// perform a route write. Pure decoding and reset predicates belong to the
/// IEEE 802.15.4 IRQ crate above the PAC boundary.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RouteRawReadback {
    core0: u32,
    core1: u32,
}

#[cfg(feature = "validation-probes")]
impl Ieee802154RouteRawReadback {
    /// Return the complete core-zero source-132 route word.
    pub const fn core0_bits(self) -> u32 {
        self.core0
    }

    /// Return the complete core-one source-132 route word.
    pub const fn core1_bits(self) -> u32 {
        self.core1
    }
}

/// Read-back image of the interrupt-masked IEEE 802.15.4 MAC foundation.
///
/// This snapshot deliberately excludes `EVENT_STATUS`: the pinned public LL
/// performs a masked self-write there, but the underlying modified-write
/// semantics are not authoritative yet.  Event clearing belongs to the later
/// IRQ ownership transition, after that gap is closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154FoundationSnapshot {
    enabled_events: u16,
    enabled_rx_aborts: u32,
    enabled_tx_aborts: u32,
    ed_uses_average: bool,
    txrx_pti: Ieee802154Pti,
    ack_pti: Ieee802154Pti,
}

/// Readback of the static, interrupt-masked MAC policy subset.
///
/// TX power is deliberately absent: its dBm-to-code table remains an opaque
/// RF/BTBB dependency, so this snapshot is not a complete vendor PIB image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacPolicySnapshot {
    frequency_code: Ieee802154FrequencyCode,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ack_timeout: Ieee802154AckTimeoutUnits,
    control: Ieee802154MacControl,
    multipan_enable_mask: u8,
    identity: Ieee802154PanIdentity,
}

impl Ieee802154MacPolicySnapshot {
    #[doc(hidden)]
    pub const fn new(
        frequency_code: Ieee802154FrequencyCode,
        cca_mode: Ieee802154CcaMode,
        cca_threshold_code: i8,
        ack_timeout: Ieee802154AckTimeoutUnits,
        control: Ieee802154MacControl,
        multipan_enable_mask: u8,
        identity: Ieee802154PanIdentity,
    ) -> Self {
        Self {
            frequency_code,
            cca_mode,
            cca_threshold_code,
            ack_timeout,
            control,
            multipan_enable_mask,
            identity,
        }
    }

    pub const fn frequency_code(self) -> Ieee802154FrequencyCode {
        self.frequency_code
    }

    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub const fn ack_timeout(self) -> Ieee802154AckTimeoutUnits {
        self.ack_timeout
    }

    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub const fn multipan_enable_mask(self) -> u8 {
        self.multipan_enable_mask
    }

    pub const fn identity(self) -> Ieee802154PanIdentity {
        self.identity
    }
}

impl Ieee802154FoundationSnapshot {
    /// Construct a field-level image for a platform-independent read-back.
    ///
    /// The arguments are bounded semantic fields, never shifted or complete
    /// register images. Production snapshots are sampled by the PAC lease;
    /// this constructor also lets the HAL verify its transition against a
    /// host backend without duplicating the register model.
    #[doc(hidden)]
    pub const fn new(
        enabled_events: u16,
        enabled_rx_aborts: u32,
        enabled_tx_aborts: u32,
        ed_uses_average: bool,
        txrx_pti: Ieee802154Pti,
        ack_pti: Ieee802154Pti,
    ) -> Self {
        Self {
            enabled_events,
            enabled_rx_aborts,
            enabled_tx_aborts,
            ed_uses_average,
            txrx_pti,
            ack_pti,
        }
    }

    pub const fn enabled_events(self) -> u16 {
        self.enabled_events
    }

    pub const fn enabled_rx_aborts(self) -> u32 {
        self.enabled_rx_aborts
    }

    pub const fn enabled_tx_aborts(self) -> u32 {
        self.enabled_tx_aborts
    }

    pub const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub const fn txrx_pti(self) -> Ieee802154Pti {
        self.txrx_pti
    }

    pub const fn ack_pti(self) -> Ieee802154Pti {
        self.ack_pti
    }
}

impl Ieee802154StateSnapshot {
    pub const fn new(rx: Ieee802154RxStateCode, tx: Ieee802154TxStateCode) -> Self {
        Self { rx, tx }
    }

    pub const fn rx(self) -> Ieee802154RxStateCode {
        self.rx
    }

    pub const fn tx(self) -> Ieee802154TxStateCode {
        self.tx
    }

    /// Test only the observed numeric state codes.
    ///
    /// This is not a reset-readiness or quiescence claim. Those semantic
    /// predicates require a reviewed lifecycle and shared-reset model.
    pub const fn all_codes_zero(self) -> bool {
        self.rx.is_zero() && self.tx.is_zero()
    }
}

/// Narrow borrow reserving the unique radio-register owner for one
/// IEEE 802.15.4 transaction.
///
/// The generated peripheral remains inside [`WifiRadioRegisters`]. Only named
/// field operations are available through this lease, so HAL cannot recover
/// its register block, addresses, or raw images.
#[must_use = "dropping the lease releases the unique radio-register borrow"]
#[doc(hidden)]
pub struct Ieee802154RegisterLease<'registers> {
    registers: &'registers mut WifiRadioRegisters,
}

impl Ieee802154RegisterLease<'_> {
    /// Mask every MAC event while the IRQ ownership split is not active.
    ///
    /// This touches `EVENT_ENABLE`, never the unresolved `EVENT_STATUS`
    /// modified-write register.
    pub fn mask_all_events(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .event_enable()
            .modify(|_, writer| writer.events().set(0));
    }

    /// Mask every receive-abort source before a receive dataplane exists.
    pub fn mask_all_rx_aborts(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .rx_abort_enable()
            .modify(|_, writer| writer.events().set(0));
    }

    /// Mask every transmit-abort source before a transmit dataplane exists.
    pub fn mask_all_tx_aborts(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .tx_abort_enable()
            .modify(|_, writer| writer.events().set(0));
    }

    /// Select the vendor foundation's average energy-detection sampler.
    pub fn select_average_ed_sampling(&mut self) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ed_config()
            .modify(|_, writer| writer.ed_sample_mode().average());
    }

    /// Replace the complete named `EVENT_ENABLE` set exactly.
    ///
    /// This is a full field replacement, not a union operation. Unnamed bits
    /// seven and thirteen cannot be represented by the input type.
    pub fn set_event_enable(&mut self, events: Ieee802154EventEnableMask) {
        self.registers
            .peripherals
            .ieee802154_mac
            .event_enable()
            .modify(|_, writer| writer.events().set(events.bits()));
    }

    /// Replace the complete receive-abort-enable field with one closed image.
    ///
    /// Callers can select only [`Ieee802154RxAbortEnableMask::NONE`] or the
    /// exact three-reason ED/CCA operation set. The generated field update
    /// preserves the unowned high bit of the backing word.
    pub fn set_rx_abort_enable(&mut self, reasons: Ieee802154RxAbortEnableMask) {
        self.registers
            .peripherals
            .ieee802154_mac
            .rx_abort_enable()
            .modify(|_, writer| writer.events().set(reasons.bits()));
    }

    /// Classify the complete `EVENT_ENABLE` field for a finite polled ED/CCA
    /// operation.
    ///
    /// This samples the backing word once and never reads or acknowledges
    /// `EVENT_STATUS`.
    pub fn operation_event_enable_observation(&self) -> Ieee802154OperationEventEnableObservation {
        let bits = self
            .registers
            .peripherals
            .ieee802154_mac
            .event_enable()
            .read()
            .events()
            .bits();
        Ieee802154OperationEventEnableObservation::from_field(bits)
    }

    /// Classify the complete `RX_ABORT_ENABLE` field for a finite polled
    /// ED/CCA operation.
    ///
    /// Every image other than the two values expressible by
    /// [`Ieee802154RxAbortEnableMask`] is retained semantically as
    /// `Unexpected`, never projected into a writable mask.
    pub fn operation_rx_abort_enable_observation(
        &self,
    ) -> Ieee802154OperationRxAbortEnableObservation {
        let bits = self
            .registers
            .peripherals
            .ieee802154_mac
            .rx_abort_enable()
            .read()
            .events()
            .bits();
        Ieee802154OperationRxAbortEnableObservation::from_field(bits)
    }

    /// Observe the complete fourteen-bit `EVENT_STATUS` field without
    /// acknowledging any event.
    pub fn event_status_observation(&self) -> Ieee802154EventObservation {
        let bits = self
            .registers
            .peripherals
            .ieee802154_mac
            .event_status()
            .read()
            .events()
            .bits();
        Ieee802154EventObservation::from_field(bits)
    }

    /// Observe the complete `RX_STATUS` word for terminal abort evidence.
    pub fn rx_status_observation(&self) -> Ieee802154RxStatusObservation {
        let bits = self
            .registers
            .peripherals
            .ieee802154_mac
            .rx_status()
            .read()
            .bits();
        Ieee802154RxStatusObservation::from_register(bits)
    }

    /// Replace the source-confirmed sixteen-bit ED-duration subset.
    ///
    /// The generated masked transaction clears unused bits 23:16 exactly as
    /// the public `uint16_t` bitfield assignment does and preserves the
    /// adjacent unowned high byte.
    pub fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits) {
        crate::generated::set_ieee802154_ed_duration(
            &self.registers.peripherals.ieee802154_mac,
            duration,
        );
    }

    /// Issue one finite ED command through a generated fixed-image bridge.
    ///
    /// `Stop` remains scoped to the finite ED/CCA transaction. This method
    /// does not establish that STOP is synchronous in another MAC state.
    pub fn issue_ed_command(&mut self, command: Ieee802154EdCommand) {
        let mac = &self.registers.peripherals.ieee802154_mac;
        match command {
            Ieee802154EdCommand::Start => {
                crate::svd::fixed_register_image::start_ieee802154_energy_detection(mac);
            }
            Ieee802154EdCommand::Stop => {
                crate::svd::fixed_register_image::stop_ieee802154_energy_detection(mac);
            }
        }
    }

    /// Issue exactly the source-confirmed finite `ED_START` command.
    ///
    /// This operation-specific entry point prevents a polled backend from
    /// selecting `STOP` while still reusing the same generated command leaf.
    pub fn request_ed_start(&mut self) {
        self.issue_ed_command(Ieee802154EdCommand::Start);
    }

    /// Replace only the generated eight-bit MAC frequency-code field.
    pub fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.registers
            .peripherals
            .ieee802154_mac
            .channel()
            .modify(|_, writer| writer.frequency_code().set(code.value()));
    }

    /// Replace the CCA mode through the generated enumerated field.
    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ed_config()
            .modify(|_, writer| match mode {
                Ieee802154CcaMode::Carrier => writer.cca_mode().carrier(),
                Ieee802154CcaMode::EnergyDetection => writer.cca_mode().energy_detection(),
                Ieee802154CcaMode::CarrierOrEnergyDetection => {
                    writer.cca_mode().carrier_or_energy_detection()
                }
                Ieee802154CcaMode::CarrierAndEnergyDetection => {
                    writer.cca_mode().carrier_and_energy_detection()
                }
            });
    }

    /// Replace the source-defined signed CCA threshold code.
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ed_config()
            .modify(|_, writer| writer.cca_threshold_code().set(threshold as u8));
    }

    /// Replace the ACK timeout field without assigning units at the PAC layer.
    pub fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
        self.registers
            .peripherals
            .ieee802154_mac
            .ack_timeout()
            .modify(|_, writer| writer.timeout().set(timeout.value()));
    }

    /// Apply the six public PIB control fields in vendor update order.
    pub fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        let register = self.registers.peripherals.ieee802154_mac.control();
        register.modify(|_, writer| writer.auto_ack_tx().bit(control.tx_auto_ack()));
        register.modify(|_, writer| writer.auto_ack_rx().bit(control.rx_auto_ack()));
        register.modify(|_, writer| writer.enhanced_ack_tx().bit(control.enhanced_ack_tx()));
        register.modify(|_, writer| writer.coordinator().bit(control.coordinator()));
        register.modify(|_, writer| writer.promiscuous().bit(control.promiscuous()));
        register.modify(|_, writer| writer.pending_enhanced().bit(control.enhanced_pending()));
    }

    /// Program the public API's primary PAN identity.
    ///
    /// Each address setter first enables context zero, matching the public LL
    /// and preserving any other enabled contexts.
    pub fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let enable_primary = || {
            mac.control().modify(|reader, writer| {
                writer
                    .multipan_enable_mask()
                    .set(reader.multipan_enable_mask().bits() | 1)
            });
        };

        enable_primary();
        mac.multipan0_pan_id()
            .modify(|_, writer| writer.pan_id().set(identity.pan_id()));
        enable_primary();
        mac.multipan0_short_address()
            .modify(|_, writer| writer.address().set(identity.short_address()));
        enable_primary();
        let address = identity.extended_address();
        mac.multipan0_extended_address_low().modify(|_, writer| {
            writer.address_word().set(u32::from_le_bytes([
                address[0], address[1], address[2], address[3],
            ]))
        });
        mac.multipan0_extended_address_high().modify(|_, writer| {
            writer.address_word().set(u32::from_le_bytes([
                address[4], address[5], address[6], address[7],
            ]))
        });
    }

    /// Replace only the generated five-bit TX/RX coexistence PTI field.
    pub fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.registers
            .peripherals
            .ieee802154_mac
            .coex_pti()
            .modify(|_, writer| writer.txrx_pti().set(pti.value()));
    }

    /// Replace only the generated five-bit ACK coexistence PTI field.
    pub fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.registers
            .peripherals
            .ieee802154_mac
            .coex_pti()
            .modify(|_, writer| writer.ack_pti().set(pti.value()));
    }

    /// Sample only fields written by the interrupt-masked foundation.
    pub fn foundation_snapshot(&self) -> Ieee802154FoundationSnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let event_enable = mac.event_enable().read();
        let rx_abort_enable = mac.rx_abort_enable().read();
        let tx_abort_enable = mac.tx_abort_enable().read();
        let ed_config = mac.ed_config().read();
        let coex_pti = mac.coex_pti().read();

        Ieee802154FoundationSnapshot {
            enabled_events: event_enable.events().bits(),
            enabled_rx_aborts: rx_abort_enable.events().bits(),
            enabled_tx_aborts: tx_abort_enable.events().bits(),
            ed_uses_average: ed_config.ed_sample_mode().is_average(),
            txrx_pti: Ieee802154Pti(coex_pti.txrx_pti().bits()),
            ack_pti: Ieee802154Pti(coex_pti.ack_pti().bits()),
        }
    }

    /// Sample the complete static MAC-policy subset once per backing word.
    pub fn mac_policy_snapshot(&self) -> Ieee802154MacPolicySnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let channel = mac.channel().read();
        let ed_config = mac.ed_config().read();
        let ack_timeout = mac.ack_timeout().read();
        let control = mac.control().read();
        let pan_id = mac.multipan0_pan_id().read();
        let short_address = mac.multipan0_short_address().read();
        let extended_low = mac.multipan0_extended_address_low().read();
        let extended_high = mac.multipan0_extended_address_high().read();
        let low = extended_low.address_word().bits().to_le_bytes();
        let high = extended_high.address_word().bits().to_le_bytes();

        Ieee802154MacPolicySnapshot {
            frequency_code: Ieee802154FrequencyCode(channel.frequency_code().bits()),
            cca_mode: Ieee802154CcaMode::from_field(ed_config.cca_mode().bits()),
            cca_threshold_code: ed_config.cca_threshold_code().bits() as i8,
            ack_timeout: Ieee802154AckTimeoutUnits(ack_timeout.timeout().bits()),
            control: Ieee802154MacControl::new(
                control.auto_ack_tx().bit_is_set(),
                control.auto_ack_rx().bit_is_set(),
                control.enhanced_ack_tx().bit_is_set(),
                control.coordinator().bit_is_set(),
                control.promiscuous().bit_is_set(),
                control.pending_enhanced().bit_is_set(),
            ),
            multipan_enable_mask: control.multipan_enable_mask().bits(),
            identity: Ieee802154PanIdentity::new(
                pan_id.pan_id().bits(),
                short_address.address().bits(),
                [
                    low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
                ],
            ),
        }
    }

    /// Sample the generated receive and transmit state fields once each.
    pub fn state_snapshot(&self) -> Ieee802154StateSnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let rx = Ieee802154RxStateCode::from_field(mac.rx_status().read().state().bits());
        let tx = Ieee802154TxStateCode::from_field(mac.tx_status().read().state().bits());
        Ieee802154StateSnapshot::new(rx, tx)
    }

    /// Sample the DMA-free energy-detection and CCA surface.
    ///
    /// Each backing word is read once. This snapshot only observes
    /// `EVENT_STATUS`; it never invokes the separate fixed selected-image
    /// operation.
    pub fn ed_cca_snapshot(&self) -> Ieee802154EdCcaSnapshot {
        let mac = &self.registers.peripherals.ieee802154_mac;
        let duration = mac.ed_duration().read();
        let event_enable = mac.event_enable().read();
        let event_status = mac.event_status().read();
        let ed_config = mac.ed_config().read();

        Ieee802154EdCcaSnapshot::new(
            Ieee802154EdDurationUnits::from_field(duration.duration().bits()),
            Ieee802154EventObservation::from_field(event_enable.events().bits()),
            Ieee802154EventObservation::from_field(event_status.events().bits()),
            ed_config.ed_rss_code().bits() as i8,
            ed_config.cca_busy().bit_is_set(),
        )
    }

    /// Publish the single HIL-qualified ED-DONE selected image.
    ///
    /// This is not a general `EVENT_STATUS` acknowledgement API. The raw PAC
    /// operation fixes the complete image to ED-DONE (`0x0000_0040`), while
    /// this lease supplies the unique IEEE 802.15.4 peripheral borrow and
    /// orders device accesses on both sides of that exact write.
    #[doc(hidden)]
    pub fn write_ed_done_selected_image(&mut self) {
        self.registers.order_device_accesses();
        crate::svd::selected_register_write::write_ieee802154_ed_done_selected_image(
            &mut self.registers.peripherals.ieee802154_mac,
        );
        self.registers.order_device_accesses();
    }

    /// Observe whether MAC event delivery is still masked for the closed
    /// `EVENT_STATUS` validation transaction.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_enable_events(&self) -> u16 {
        crate::svd::ieee802154_event_status_validation::event_enable_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Enable exactly the two timer events for the closed validation probe.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_timer_events(&mut self) {
        crate::svd::ieee802154_event_status_validation::enable_timer_events(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Mask every event before the validation probe cleans selected status.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_all_events(&mut self) {
        crate::svd::ieee802154_event_status_validation::disable_all_events(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Sample the source-132 interrupt-route words for both CPU cores without
    /// changing either route.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_interrupt_route_readback(&self) -> Ieee802154RouteRawReadback {
        let readback = crate::svd::ieee802154_route_validation::read_route_words(
            &self.registers.peripherals.ieee802154_mac,
        );
        Ieee802154RouteRawReadback {
            core0: readback.core0_bits(),
            core1: readback.core1_bits(),
        }
    }

    /// Sample `EVENT_STATUS` without assigning an acknowledge access class.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_status_events(&self) -> u16 {
        crate::svd::ieee802154_event_status_validation::event_status_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Sample the complete timer-zero counter during the closed validation
    /// transaction.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer0_value(&self) -> u32 {
        crate::svd::ieee802154_event_status_validation::timer0_value(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Sample the complete timer-one counter during the closed validation
    /// transaction.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer1_value(&self) -> u32 {
        crate::svd::ieee802154_event_status_validation::timer1_value(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Program both independent event-status validation timers.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_event_timer_thresholds(&mut self, threshold: u32) {
        crate::svd::ieee802154_event_status_validation::set_timer_thresholds(
            &self.registers.peripherals.ieee802154_mac,
            threshold,
        );
    }

    /// Start validation timer zero without enabling event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer0(&mut self) {
        crate::svd::ieee802154_event_status_validation::start_timer0(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Stop validation timer zero without changing event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer0(&mut self) {
        crate::svd::ieee802154_event_status_validation::stop_timer0(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Start validation timer one without enabling event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer1(&mut self) {
        crate::svd::ieee802154_event_status_validation::start_timer1(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Stop validation timer one without changing event delivery.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer1(&mut self) {
        crate::svd::ieee802154_event_status_validation::stop_timer1(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Write only timer-zero's event bit in the validation-only raw API.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer0(&mut self) {
        crate::svd::ieee802154_event_status_validation::write_timer0_event(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Write only timer-one's event bit in the validation-only raw API.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer1(&mut self) {
        crate::svd::ieee802154_event_status_validation::write_timer1_event(
            &self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Observe the complete event-enable field for the closed ED validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_enable_events(&self) -> u16 {
        crate::svd::ieee802154_ed_event_validation::event_enable_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Enable only RX-ABORT, ED-DONE and TIMER0 for the closed ED validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_timer_abort_events(&mut self) {
        crate::svd::ieee802154_ed_event_validation::enable_ed_timer_abort_events(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Mask all event delivery during ED validation cleanup.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_events(&mut self) {
        crate::svd::ieee802154_ed_event_validation::disable_all_events(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Observe the complete RX-abort-enable field for the ED discriminator.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_abort_enable_events(&self) -> u32 {
        crate::svd::ieee802154_ed_event_validation::rx_abort_enable_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Enable only the three source-confirmed ED abort reasons.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_abort_reasons(&mut self) {
        crate::svd::ieee802154_ed_event_validation::enable_ed_abort_reasons(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Mask every RX-abort reason during terminal cleanup.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_abort_reasons(&mut self) {
        crate::svd::ieee802154_ed_event_validation::disable_all_rx_abort_reasons(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Observe the complete event-status field for the ED discriminator.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_status_events(&self) -> u16 {
        crate::svd::ieee802154_ed_event_validation::event_status_events(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Observe the complete RX status when ED terminates through RX-ABORT.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_status_raw(&self) -> u32 {
        crate::svd::ieee802154_ed_event_validation::rx_status_raw(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Observe the complete public ED-duration field.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_duration(&self) -> u32 {
        crate::svd::ieee802154_ed_event_validation::ed_duration(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Program the fixed public standalone-CCA ED duration, exactly eight.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_duration_eight(&mut self) {
        crate::svd::ieee802154_ed_event_validation::set_ed_duration_eight(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Observe TIMER0 while it supplies the independent event latch.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_timer0_value(&self) -> u32 {
        crate::svd::ieee802154_ed_event_validation::timer0_value(
            &self.registers.peripherals.ieee802154_mac,
        )
    }

    /// Program TIMER0's threshold for the ED event discriminator.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_timer0_threshold(&mut self, threshold: u32) {
        crate::svd::ieee802154_ed_event_validation::set_timer0_threshold(
            &mut self.registers.peripherals.ieee802154_mac,
            threshold,
        );
    }

    /// Start TIMER0 before the ED stimulus.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed_timer0(&mut self) {
        crate::svd::ieee802154_ed_event_validation::start_timer0(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Stop TIMER0 during terminal cleanup.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_timer0(&mut self) {
        crate::svd::ieee802154_ed_event_validation::stop_timer0(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Start the fixed-duration energy-detection stimulus.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed(&mut self) {
        crate::svd::ieee802154_ed_event_validation::start_ed(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Issue best-effort STOP after a bounded ED timeout.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_operation(&mut self) {
        crate::svd::ieee802154_ed_event_validation::stop_operation(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Exercise the production selected-image boundary for ED-DONE.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_done_event(&mut self) {
        self.write_ed_done_selected_image();
    }

    /// Write only TIMER0 in the ED validation-only raw status vocabulary.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_timer0_event(&mut self) {
        crate::svd::ieee802154_ed_event_validation::write_timer0_event(
            &mut self.registers.peripherals.ieee802154_mac,
        );
    }

    /// Order memory and device accesses at a descriptor/MMIO boundary.
    pub fn order_device_accesses(&mut self) {
        self.registers.order_device_accesses();
    }
}

impl WifiRadioRegisters {
    /// Borrow the reserved IEEE 802.15.4 register capability.
    ///
    /// No generic PAC or register block can be recovered from the result.
    #[doc(hidden)]
    pub fn ieee802154_register_lease(&mut self) -> Ieee802154RegisterLease<'_> {
        Ieee802154RegisterLease { registers: self }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdCcaSnapshot, Ieee802154EdCommand,
        Ieee802154EdDurationUnits, Ieee802154EventEnableMask, Ieee802154EventObservation,
        Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154MacControl,
        Ieee802154MacPolicySnapshot, Ieee802154OperationEventEnableObservation,
        Ieee802154OperationRxAbortEnableObservation, Ieee802154PanIdentity, Ieee802154Pti,
        Ieee802154RegisterLease, Ieee802154RxAbortEnableMask, Ieee802154RxStateCode,
        Ieee802154RxStatusObservation, Ieee802154StateSnapshot, Ieee802154TxStateCode,
    };
    use crate::RadioHardware;

    #[test]
    fn frequency_code_does_not_claim_an_ieee_channel_mapping() {
        assert_eq!(Ieee802154FrequencyCode::new(0).value(), 0);
        assert_eq!(Ieee802154FrequencyCode::new(u8::MAX).value(), u8::MAX);
    }

    #[test]
    fn foundation_snapshot_exposes_fields_without_complete_register_images() {
        let snapshot = Ieee802154FoundationSnapshot::new(
            0,
            0,
            0,
            true,
            Ieee802154Pti::new(3).expect("five-bit PTI"),
            Ieee802154Pti::new(3).expect("five-bit PTI"),
        );

        assert_eq!(snapshot.enabled_events(), 0);
        assert_eq!(snapshot.enabled_rx_aborts(), 0);
        assert_eq!(snapshot.enabled_tx_aborts(), 0);
        assert!(snapshot.ed_uses_average());
        assert_eq!(snapshot.txrx_pti().value(), 3);
        assert_eq!(snapshot.ack_pti().value(), 3);
    }

    #[test]
    fn mac_policy_snapshot_keeps_typed_fields_and_little_endian_identity() {
        let control = Ieee802154MacControl::new(true, false, true, false, true, false);
        let identity = Ieee802154PanIdentity::new(
            0x1234,
            0xabcd,
            [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
        );
        let snapshot = Ieee802154MacPolicySnapshot::new(
            Ieee802154FrequencyCode::new(78),
            Ieee802154CcaMode::CarrierAndEnergyDetection,
            -75,
            Ieee802154AckTimeoutUnits::new(108),
            control,
            0b0101,
            identity,
        );

        assert_eq!(snapshot.frequency_code().value(), 78);
        assert_eq!(
            snapshot.cca_mode(),
            Ieee802154CcaMode::CarrierAndEnergyDetection
        );
        assert_eq!(snapshot.cca_threshold_code(), -75);
        assert_eq!(snapshot.ack_timeout().value(), 108);
        assert_eq!(snapshot.control(), control);
        assert_eq!(snapshot.multipan_enable_mask(), 0b0101);
        assert_eq!(snapshot.identity(), identity);
    }

    #[test]
    fn pti_constructor_never_creates_a_shifted_or_oversized_image() {
        assert_eq!(Ieee802154Pti::new(0).map(Ieee802154Pti::value), Some(0));
        assert_eq!(
            Ieee802154Pti::new(Ieee802154Pti::MAX).map(Ieee802154Pti::value),
            Some(Ieee802154Pti::MAX)
        );
        assert_eq!(Ieee802154Pti::new(Ieee802154Pti::MAX + 1), None);
    }

    #[test]
    fn ed_duration_never_truncates_the_wider_physical_field() {
        assert_eq!(
            Ieee802154EdDurationUnits::new(0).map(|value| value.get()),
            Some(0)
        );
        assert_eq!(
            Ieee802154EdDurationUnits::new(u32::from(u16::MAX)).map(|value| value.get()),
            Some(u32::from(u16::MAX))
        );
        assert_eq!(
            Ieee802154EdDurationUnits::from_field(u16::MAX as u32),
            Ieee802154EdDurationUnits::new(u16::MAX as u32)
        );
        assert_eq!(
            Ieee802154EdDurationUnits::from_field(u16::MAX as u32 + 1),
            None
        );
    }

    #[test]
    fn event_enable_mask_accepts_only_named_finite_bits() {
        let ed_and_timers = Ieee802154EventEnableMask::ED_DONE
            .union(Ieee802154EventEnableMask::TIMER0_OVERFLOW)
            .union(Ieee802154EventEnableMask::TIMER1_OVERFLOW);
        assert_eq!(ed_and_timers.bits(), (1 << 6) | (1 << 8) | (1 << 9));
        assert!(ed_and_timers.contains(Ieee802154EventEnableMask::ED_DONE));
        assert_eq!(
            Ieee802154EventEnableMask::from_named_bits(Ieee802154EventEnableMask::ALL_NAMED.bits()),
            Some(Ieee802154EventEnableMask::ALL_NAMED)
        );
        for unsupported in [1 << 7, 1 << 13, 1 << 14, u16::MAX] {
            assert_eq!(
                Ieee802154EventEnableMask::from_named_bits(unsupported),
                None
            );
        }
    }

    #[test]
    fn polled_event_enable_readback_classifies_every_fourteen_bit_image() {
        let operation_bits = Ieee802154EventEnableMask::ED_DONE
            .union(Ieee802154EventEnableMask::RX_ABORT)
            .bits();
        assert_eq!(
            Ieee802154EventEnableMask::ED_DONE_AND_RX_ABORT.bits(),
            operation_bits
        );

        for bits in 0..=Ieee802154EventObservation::FIELD_MASK {
            let expected = match bits {
                0 => Ieee802154OperationEventEnableObservation::AllMasked,
                bits if bits == operation_bits => {
                    Ieee802154OperationEventEnableObservation::EdDoneAndRxAbortOnly
                }
                _ => Ieee802154OperationEventEnableObservation::Unexpected,
            };
            assert_eq!(
                Ieee802154OperationEventEnableObservation::from_field(bits),
                expected,
                "misclassified EVENT_ENABLE field {bits:#06x}"
            );
        }
    }

    #[test]
    fn rx_abort_enable_domain_has_only_the_two_operation_images() {
        assert_eq!(Ieee802154RxAbortEnableMask::NONE.bits(), 0);
        assert_eq!(
            Ieee802154RxAbortEnableMask::ED_OPERATION_REASONS.bits(),
            0x0380_0000
        );
        assert_eq!(
            Ieee802154OperationRxAbortEnableObservation::from_field(
                Ieee802154RxAbortEnableMask::NONE.bits()
            ),
            Ieee802154OperationRxAbortEnableObservation::AllMasked
        );
        assert_eq!(
            Ieee802154OperationRxAbortEnableObservation::from_field(
                Ieee802154RxAbortEnableMask::ED_OPERATION_REASONS.bits()
            ),
            Ieee802154OperationRxAbortEnableObservation::EdOperationReasonsOnly
        );
    }

    #[test]
    fn rx_abort_enable_readback_rejects_every_single_bit_divergence() {
        let expected = Ieee802154RxAbortEnableMask::ED_OPERATION_REASONS.bits();

        // The generated RX_ABORT_ENABLE field owns bits 0 through 30. Bit 31
        // is outside the field and is preserved by the masked setter.
        for bit in 0..31 {
            let singleton = 1_u32 << bit;
            assert_eq!(
                Ieee802154OperationRxAbortEnableObservation::from_field(singleton),
                Ieee802154OperationRxAbortEnableObservation::Unexpected,
                "accepted singleton RX_ABORT_ENABLE bit {bit}"
            );

            let divergent = expected ^ singleton;
            assert_eq!(
                Ieee802154OperationRxAbortEnableObservation::from_field(divergent),
                Ieee802154OperationRxAbortEnableObservation::Unexpected,
                "accepted one-bit RX_ABORT_ENABLE divergence at bit {bit}"
            );
        }
    }

    #[test]
    fn event_observation_preserves_unnamed_bits_but_projects_only_named_writes() {
        let bits = Ieee802154EventEnableMask::ED_DONE.bits() | (1 << 7) | (1 << 13);
        let observation =
            Ieee802154EventObservation::for_validation(bits).expect("fourteen-bit field");
        assert_eq!(observation.bits(), bits);
        assert_eq!(observation.unnamed_bits(), (1 << 7) | (1 << 13));
        assert!(observation.contains(Ieee802154EventEnableMask::ED_DONE));
        assert!(!observation.is_clear());
        assert_eq!(observation.named(), Ieee802154EventEnableMask::ED_DONE);
        assert!(
            Ieee802154EventObservation::for_validation(0)
                .expect("clear fourteen-bit field")
                .is_clear()
        );
        assert_eq!(Ieee802154EventObservation::for_validation(1 << 14), None);
    }

    #[test]
    fn rx_status_observation_preserves_the_complete_word() {
        for bits in [0, 1, 0x0380_0000, 0x8000_0000, u32::MAX] {
            assert_eq!(
                Ieee802154RxStatusObservation::for_validation(bits).bits(),
                bits
            );
        }
    }

    #[test]
    fn ed_cca_snapshot_keeps_status_observational_and_codes_uninterpreted() {
        let enabled =
            Ieee802154EventObservation::for_validation(1 << 6).expect("fourteen-bit field");
        let pending = Ieee802154EventObservation::for_validation((1 << 6) | (1 << 7))
            .expect("fourteen-bit field");
        let snapshot = Ieee802154EdCcaSnapshot::new(
            Ieee802154EdDurationUnits::new(128),
            enabled,
            pending,
            -91,
            true,
        );

        assert_eq!(snapshot.duration().map(|value| value.get()), Some(128));
        assert_eq!(snapshot.enabled_events(), enabled);
        assert_eq!(snapshot.pending_events(), pending);
        assert_eq!(snapshot.pending_events().unnamed_bits(), 1 << 7);
        assert_eq!(snapshot.rss_code(), -91);
        assert!(snapshot.cca_busy());
        assert_ne!(Ieee802154EdCommand::Start, Ieee802154EdCommand::Stop);
    }

    #[test]
    fn state_codes_are_bounded_and_expose_only_reviewed_predicates() {
        let zero_rx = Ieee802154RxStateCode::for_validation(0).expect("three-bit state");
        let zero_tx = Ieee802154TxStateCode::for_validation(0).expect("four-bit state");
        let sfd = Ieee802154RxStateCode::for_validation(1).expect("three-bit state");
        let after_sfd = Ieee802154RxStateCode::for_validation(2).expect("three-bit state");

        assert!(Ieee802154StateSnapshot::new(zero_rx, zero_tx).all_codes_zero());
        assert!(sfd.is_receive_sfd());
        assert!(!sfd.is_after_receive_sfd());
        assert!(after_sfd.is_after_receive_sfd());
        assert_eq!(after_sfd.value(), 2);
        assert_eq!(zero_tx.value(), 0);
        assert_eq!(
            Ieee802154RxStateCode::for_validation(Ieee802154RxStateCode::MAX + 1),
            None
        );
        assert_eq!(
            Ieee802154TxStateCode::for_validation(Ieee802154TxStateCode::MAX + 1),
            None
        );
    }

    #[test]
    fn nonzero_state_code_fails_only_the_numeric_zero_predicate() {
        let rx = Ieee802154RxStateCode::for_validation(0).expect("three-bit state");
        let tx = Ieee802154TxStateCode::for_validation(1).expect("four-bit state");
        let snapshot = Ieee802154StateSnapshot::new(rx, tx);

        assert!(!snapshot.all_codes_zero());
        assert_eq!(snapshot.rx().value(), 0);
        assert_eq!(snapshot.tx().value(), 1);
    }

    #[test]
    fn register_lease_borrows_the_existing_unique_radio_owner() {
        let mut cold = RadioHardware::for_validation().into_wifi();
        let mut lease = cold.radio_mut().ieee802154_register_lease();

        // Host execution reaches only the existing architecture-neutral
        // device fence; MMIO operations remain compiled but are not executed.
        lease.order_device_accesses();
    }

    #[test]
    fn selected_ed_done_boundary_accepts_no_caller_supplied_image() {
        let _write: fn(&mut Ieee802154RegisterLease<'static>) =
            Ieee802154RegisterLease::write_ed_done_selected_image;
    }

    #[test]
    fn polled_operation_pac_surface_uses_only_closed_typed_writes() {
        let _set_events: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154EventEnableMask) =
            Ieee802154RegisterLease::set_event_enable;
        let _set_rx_aborts: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154RxAbortEnableMask) =
            Ieee802154RegisterLease::set_rx_abort_enable;
        let _event_enable: fn(
            &Ieee802154RegisterLease<'static>,
        ) -> Ieee802154OperationEventEnableObservation =
            Ieee802154RegisterLease::operation_event_enable_observation;
        let _rx_abort_enable: fn(
            &Ieee802154RegisterLease<'static>,
        ) -> Ieee802154OperationRxAbortEnableObservation =
            Ieee802154RegisterLease::operation_rx_abort_enable_observation;
        let _event_status: fn(&Ieee802154RegisterLease<'static>) -> Ieee802154EventObservation =
            Ieee802154RegisterLease::event_status_observation;
        let _rx_status: fn(&Ieee802154RegisterLease<'static>) -> Ieee802154RxStatusObservation =
            Ieee802154RegisterLease::rx_status_observation;
        let _set_duration: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154EdDurationUnits) =
            Ieee802154RegisterLease::set_ed_duration;
        let _sample_ed: fn(&Ieee802154RegisterLease<'static>) -> Ieee802154EdCcaSnapshot =
            Ieee802154RegisterLease::ed_cca_snapshot;
        let _start: fn(&mut Ieee802154RegisterLease<'static>) =
            Ieee802154RegisterLease::request_ed_start;
    }

    #[test]
    fn generated_mac_geometry_is_owned_by_the_radio_partition() {
        let cold = RadioHardware::for_validation().into_wifi();
        let mac = &cold.radio().peripherals.ieee802154_mac;

        // Pointer inspection performs no volatile access on the host.
        assert_eq!(mac.channel().as_ptr() as usize, 0x2010_3048);
        assert_eq!(mac.command().as_ptr() as usize, 0x2010_3000);
        assert_eq!(mac.ed_duration().as_ptr() as usize, 0x2010_3050);
        assert_eq!(mac.ed_config().as_ptr() as usize, 0x2010_3054);
        assert_eq!(mac.event_enable().as_ptr() as usize, 0x2010_3060);
        assert_eq!(mac.event_status().as_ptr() as usize, 0x2010_3064);
        assert_eq!(mac.rx_abort_enable().as_ptr() as usize, 0x2010_3068);
        assert_eq!(mac.coex_pti().as_ptr() as usize, 0x2010_3070);
        assert_eq!(mac.tx_abort_enable().as_ptr() as usize, 0x2010_3078);
        assert_eq!(mac.rx_status().as_ptr() as usize, 0x2010_3080);
        assert_eq!(mac.tx_status().as_ptr() as usize, 0x2010_3084);
    }
}
