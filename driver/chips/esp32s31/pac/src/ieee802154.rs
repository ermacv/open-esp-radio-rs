//! Typed lower-level ownership for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! Every MMIO operation in this module is routed through the reviewed
//! generated `IEEE802154_MAC` peripheral. The narrow lease exposes only the
//! first field-sized operations needed by HAL; neither the generated register
//! block nor numeric addresses can escape it.

#![forbid(unsafe_code)]

use core::ops::{Deref, DerefMut};

use super::{Ieee802154InterruptRegisters, Ieee802154InterruptSetup, Ieee802154TaskRegisters};
pub use crate::generated::{Ieee802154EdDurationUnits, Ieee802154TxPowerCode};

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

/// One of the four source-confirmed MAC PAN contexts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanIndex(u8);

impl Ieee802154MultipanIndex {
    pub const COUNT: u8 = 4;
    pub const CONTEXT0: Self = Self(0);
    pub const CONTEXT1: Self = Self(1);
    pub const CONTEXT2: Self = Self(2);
    pub const CONTEXT3: Self = Self(3);

    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    const fn as_usize(self) -> usize {
        self.0 as usize
    }

    const fn enable_bit(self) -> u8 {
        1 << self.0
    }
}

/// Exact four-bit `MULTIPAN_ENABLE_MASK` field image.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanEnableMask(u8);

impl Ieee802154MultipanEnableMask {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0x0f);

    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::ALL.0 == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, index: Ieee802154MultipanIndex) -> bool {
        self.0 & index.enable_bit() != 0
    }

    pub const fn with(self, index: Ieee802154MultipanIndex) -> Self {
        Self(self.0 | index.enable_bit())
    }

    pub const fn without(self, index: Ieee802154MultipanIndex) -> Self {
        Self(self.0 & !index.enable_bit())
    }
}

/// Source-confirmed energy-detection sampling rate.
///
/// The discriminants are the two-bit PAC field values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154EdSampleRate {
    OnePerMicrosecond = 0,
    TwoPerMicrosecond = 1,
    FourPerMicrosecond = 2,
    EightPerMicrosecond = 3,
}

impl Ieee802154EdSampleRate {
    pub const fn field_value(self) -> u8 {
        self as u8
    }

    const fn from_field(value: u8) -> Self {
        match value {
            0 => Self::OnePerMicrosecond,
            1 => Self::TwoPerMicrosecond,
            2 => Self::FourPerMicrosecond,
            3 => Self::EightPerMicrosecond,
            _ => unreachable!(),
        }
    }
}

/// Seven-bit transmit-security payload offset.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154SecurityPayloadOffset(u8);

impl Ieee802154SecurityPayloadOffset {
    pub const MAX: u8 = 0x7f;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Readable transmit-security control state without write-only key material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TransmitSecurityControl {
    enabled: bool,
    payload_offset: Ieee802154SecurityPayloadOffset,
}

impl Ieee802154TransmitSecurityControl {
    const fn new(enabled: bool, payload_offset: Ieee802154SecurityPayloadOffset) -> Self {
        Self {
            enabled,
            payload_offset,
        }
    }

    pub const fn enabled(self) -> bool {
        self.enabled
    }

    pub const fn payload_offset(self) -> Ieee802154SecurityPayloadOffset {
        self.payload_offset
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

/// Source-confirmed command images accepted by the IEEE 802.15.4 MAC.
///
/// Each variant maps to one complete generated `COMMAND` image. There is no
/// integer constructor, so callers cannot publish test-only or unknown
/// opcodes through the production task capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154MacCommand {
    /// Start transmission from the published TX DMA address.
    Transmit,
    /// Start reception into the published RX DMA address.
    Receive,
    /// Perform CCA and transmit when the channel is clear.
    ClearChannelThenTransmit,
    /// Start one configured energy-detection transaction.
    EnergyDetection,
    /// Stop the current state-specific MAC operation.
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
    /// Events handled by the reviewed runtime ISR before TIMER0 is armed.
    ///
    /// This is the exact fail-closed intersection of the public vendor
    /// initialization image with the events currently classified by the
    /// source-owned ISR. It excludes unnamed bits seven and thirteen,
    /// TIMER0, and the named-but-unhandled clock-count event.
    pub const HANDLED_BASELINE_WITHOUT_TIMER0: Self = Self(
        Self::TX_DONE.0
            | Self::RX_DONE.0
            | Self::ACK_TX_DONE.0
            | Self::ACK_RX_DONE.0
            | Self::RX_ABORT.0
            | Self::TX_ABORT.0
            | Self::ED_DONE.0
            | Self::TIMER1_OVERFLOW.0
            | Self::TX_SFD_DONE.0
            | Self::RX_SFD_DONE.0,
    );

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

/// Closed receive-abort image for the production interrupt baseline.
///
/// The image is source-confirmed by the pinned public initialization path.
/// It has no integer constructor and is writable only as part of the complete
/// inactive-route activation transaction below.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ieee802154InterruptRxAbortEnableMask(u32);

impl Ieee802154InterruptRxAbortEnableMask {
    const NONE: Self = Self(0);
    const SOURCE_CONFIRMED_BASELINE: Self = Self(0x0002_8000);

    const fn bits(self) -> u32 {
        self.0
    }
}

/// Closed transmit-abort image for the production interrupt baseline.
///
/// The image is source-confirmed by the pinned public initialization path.
/// It has no integer constructor and is writable only as part of the complete
/// inactive-route activation transaction below.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ieee802154InterruptTxAbortEnableMask(u32);

impl Ieee802154InterruptTxAbortEnableMask {
    const NONE: Self = Self(0);
    const SOURCE_CONFIRMED_BASELINE: Self = Self(0x0186_8000);

    const fn bits(self) -> u32 {
        self.0
    }
}

/// One closed production plan for activating the IEEE 802.15.4 IRQ owner.
///
/// No raw-mask constructor exists. Keeping all three images in one value
/// prevents a caller from combining an event vocabulary with unrelated abort
/// reasons or publishing only part of the reviewed baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ieee802154InterruptActivationPlan {
    events: Ieee802154EventEnableMask,
    rx_aborts: Ieee802154InterruptRxAbortEnableMask,
    tx_aborts: Ieee802154InterruptTxAbortEnableMask,
}

impl Ieee802154InterruptActivationPlan {
    const SOURCE_CONFIRMED_BASELINE: Self = Self {
        events: Ieee802154EventEnableMask::HANDLED_BASELINE_WITHOUT_TIMER0,
        rx_aborts: Ieee802154InterruptRxAbortEnableMask::SOURCE_CONFIRMED_BASELINE,
        tx_aborts: Ieee802154InterruptTxAbortEnableMask::SOURCE_CONFIRMED_BASELINE,
    };

    const fn events(self) -> Ieee802154EventEnableMask {
        self.events
    }

    const fn rx_aborts(self) -> Ieee802154InterruptRxAbortEnableMask {
        self.rx_aborts
    }

    const fn tx_aborts(self) -> Ieee802154InterruptTxAbortEnableMask {
        self.tx_aborts
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

/// Copyable fourteen-bit `EVENT_ENABLE` or `EVENT_STATUS` observation.
///
/// Unlike [`Ieee802154EventEnableMask`], this type preserves unnamed physical
/// bits because observations must not erase unexpected hardware state. It has
/// no public constructor and cannot be passed to a write. W1C acknowledgement
/// consumes a separate affine snapshot.
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

/// One opaque, non-replayable hard-IRQ sample.
///
/// The private raw token retains the exact fourteen-bit W1C image sampled at
/// interrupt entry. Status and ED/CCA observations are captured only when the
/// matching event bit was present. The value is intentionally neither `Copy`
/// nor `Clone`; acknowledgement consumes it.
#[must_use = "an IEEE 802.15.4 interrupt snapshot must be acknowledged"]
#[derive(Debug)]
pub struct Ieee802154InterruptSnapshot {
    acknowledgement: crate::svd::w1c_register_snapshot::Ieee802154EventStatusSnapshot,
    events: Ieee802154EventObservation,
    rx_status: Option<u32>,
    tx_status: Option<u32>,
    ed_rss_code: Option<i8>,
    cca_busy: Option<bool>,
}

impl Ieee802154InterruptSnapshot {
    /// Return the complete sampled fourteen-bit event image.
    pub const fn events(&self) -> Ieee802154EventObservation {
        self.events
    }

    /// Return the complete RX status word only for an RX-abort event.
    pub const fn rx_status_bits(&self) -> Option<u32> {
        self.rx_status
    }

    /// Return the source-defined five-bit RX-abort reason code.
    pub const fn rx_abort_reason_code(&self) -> Option<u8> {
        match self.rx_status {
            Some(bits) => Some(((bits >> 4) & 0x1f) as u8),
            None => None,
        }
    }

    /// Return the complete TX status word only for a TX-abort event.
    pub const fn tx_status_bits(&self) -> Option<u32> {
        self.tx_status
    }

    /// Return the source-defined five-bit TX-abort reason code.
    pub const fn tx_abort_reason_code(&self) -> Option<u8> {
        match self.tx_status {
            Some(bits) => Some(((bits >> 4) & 0x1f) as u8),
            None => None,
        }
    }

    /// Return the signed ED result only for an ED-DONE event.
    pub const fn ed_rss_code(&self) -> Option<i8> {
        self.ed_rss_code
    }

    /// Return the CCA result only for an ED-DONE event.
    pub const fn cca_busy(&self) -> Option<bool> {
        self.cca_busy
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
/// `EVENT_STATUS` remains observation-only in this copyable diagnostic value.
/// Runtime acknowledgement uses the separate affine interrupt snapshot, so a
/// read-only report can never be replayed as a W1C image.
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

    /// Return the complete non-acknowledging `EVENT_STATUS` observation.
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

/// Raw paired CPU-route observation from the read-only PAC sidecar.
///
/// This type contains evidence only: it cannot expose a register pointer or
/// perform a route write. Pure decoding and reset predicates belong to the
/// IEEE 802.15.4 IRQ crate above the PAC boundary.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RouteRawReadback {
    core0: u32,
    core1: u32,
}

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
/// This snapshot deliberately excludes `EVENT_STATUS` because the foundation
/// transition neither owns nor acknowledges pending runtime events. Polled and
/// hard-IRQ paths use the generated affine W1C snapshot transaction.
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
/// This is exactly the subset written and compared by the current runtime
/// refresh transaction. Dynamic frame-pending state and newly modeled
/// diagnostic/configuration fields are intentionally absent.
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

/// Complete read-only observation of the currently modeled MAC configuration.
///
/// This diagnostic DTO is not a static runtime policy and is never compared
/// by command refresh. In particular, `frame_pending` is dynamic per-ACK
/// state. The transmit-power value is raw eight-bit PAC geometry with no dBm
/// or calibration-table claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacConfigurationReadback {
    frequency_code: Ieee802154FrequencyCode,
    tx_power_code: Ieee802154TxPowerCode,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ed_sample_rate: Ieee802154EdSampleRate,
    ack_timeout: Ieee802154AckTimeoutUnits,
    control: Ieee802154MacControl,
    multipan_enable_mask: Ieee802154MultipanEnableMask,
    identities: [Ieee802154PanIdentity; 4],
    frame_pending: bool,
}

fn raw_identity_to_typed(
    readback: crate::svd::ieee802154_mac_ownership::MultipanIdentityReadback,
) -> Ieee802154PanIdentity {
    Ieee802154PanIdentity::new(
        readback.pan_id(),
        readback.short_address(),
        readback.extended_address(),
    )
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

impl Ieee802154MacConfigurationReadback {
    pub const fn frequency_code(self) -> Ieee802154FrequencyCode {
        self.frequency_code
    }

    pub const fn tx_power_code(self) -> Ieee802154TxPowerCode {
        self.tx_power_code
    }

    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub const fn ed_sample_rate(self) -> Ieee802154EdSampleRate {
        self.ed_sample_rate
    }

    pub const fn ack_timeout(self) -> Ieee802154AckTimeoutUnits {
        self.ack_timeout
    }

    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub const fn multipan_enable_mask(self) -> Ieee802154MultipanEnableMask {
        self.multipan_enable_mask
    }

    pub const fn multipan_identity(self, index: Ieee802154MultipanIndex) -> Ieee802154PanIdentity {
        self.identities[index.as_usize()]
    }

    pub const fn frame_pending(self) -> bool {
        self.frame_pending
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
/// The generated peripheral remains inside the active whole-radio route. Only
/// named field operations are available through this lease, so HAL cannot
/// recover its register block, addresses, or raw images.
///
/// Interrupt status and W1C operations require the combined inactive-route
/// lease and cannot be called through this task-only capability:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn sample_irq_status(task: &Ieee802154RegisterLease<'_>) {
///     let _ = task.event_status_observation();
/// }
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn acknowledge_irq_status(task: &mut Ieee802154RegisterLease<'_>) {
///     let _ = task.acknowledge_pending_events();
/// }
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn literal_status_write(task: &mut Ieee802154RegisterLease<'_>) {
///     task.validation_write_event_timer0();
/// }
/// ```
#[must_use = "dropping the lease releases the unique radio-register borrow"]
#[doc(hidden)]
pub struct Ieee802154RegisterLease<'registers> {
    registers: &'registers mut crate::svd::ieee802154_mac_ownership::TaskRegisters,
}

impl Ieee802154RegisterLease<'_> {
    /// Replace the source-confirmed sixteen-bit ED-duration subset.
    ///
    /// The generated masked transaction clears unused bits 23:16 exactly as
    /// the public `uint16_t` bitfield assignment does and preserves the
    /// adjacent unowned high byte.
    pub fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits) {
        let duration = match u16::try_from(duration.get()) {
            Ok(duration) => duration,
            Err(_) => unreachable!("generated ED-duration domain is bounded to sixteen bits"),
        };
        self.registers.set_ed_duration(duration);
    }

    /// Issue one finite ED command through a generated fixed-image bridge.
    ///
    /// `Stop` remains scoped to the finite ED/CCA transaction. This method
    /// does not establish that STOP is synchronous in another MAC state.
    pub fn issue_ed_command(&mut self, command: Ieee802154EdCommand) {
        self.request_mac_command(match command {
            Ieee802154EdCommand::Start => Ieee802154MacCommand::EnergyDetection,
            Ieee802154EdCommand::Stop => Ieee802154MacCommand::Stop,
        });
    }

    /// Publish the complete TX frame-buffer address through its generated
    /// register-specific domain.
    ///
    /// Buffer provenance, DMA accessibility, alignment, and lifetime remain
    /// obligations of the higher DMA owner.
    pub fn publish_transmit_dma_address(&mut self, address: u32) {
        self.registers.publish_transmit_dma_address(address);
    }

    /// Publish the complete RX frame-buffer address through its generated
    /// register-specific domain.
    pub fn publish_receive_dma_address(&mut self, address: u32) {
        self.registers.publish_receive_dma_address(address);
    }

    /// Issue exactly one source-confirmed complete MAC command image.
    pub fn request_mac_command(&mut self, command: Ieee802154MacCommand) {
        match command {
            Ieee802154MacCommand::Transmit => self.registers.issue_transmit(),
            Ieee802154MacCommand::Receive => self.registers.issue_receive(),
            Ieee802154MacCommand::ClearChannelThenTransmit => {
                self.registers.issue_clear_channel_then_transmit();
            }
            Ieee802154MacCommand::EnergyDetection => self.registers.issue_energy_detection(),
            Ieee802154MacCommand::Stop => self.registers.issue_stop(),
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
        self.registers.set_frequency_code(code.value());
    }

    /// Replace only the raw eight-bit transmit-power field.
    ///
    /// This preserving update makes no dBm or calibration-table claim.
    pub fn set_tx_power_code(&mut self, code: Ieee802154TxPowerCode) {
        self.registers.set_tx_power_code(code.get());
    }

    /// Replace the CCA mode through the generated enumerated field.
    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers.set_cca_mode(mode.field_value());
    }

    /// Replace the source-defined signed CCA threshold code.
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers.set_cca_threshold_code(threshold);
    }

    /// Replace the source-confirmed two-bit ED sample-rate field.
    pub fn set_ed_sample_rate(&mut self, rate: Ieee802154EdSampleRate) {
        self.registers.set_ed_sample_rate(rate.field_value());
    }

    /// Replace the ACK timeout field without assigning units at the PAC layer.
    pub fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
        self.registers.set_ack_timeout(timeout.value());
    }

    /// Apply the six public PIB control fields in vendor update order.
    pub fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.registers.set_mac_control(
            control.tx_auto_ack(),
            control.rx_auto_ack(),
            control.enhanced_ack_tx(),
            control.coordinator(),
            control.promiscuous(),
            control.enhanced_pending(),
        );
    }

    /// Replace the complete four-bit multipan-enable field exactly.
    pub fn set_multipan_enable_mask(&mut self, mask: Ieee802154MultipanEnableMask) {
        self.registers.set_multipan_enable_mask(mask.bits());
    }

    /// Program one of the four public PAN identities.
    ///
    /// Matching the public LL, each logical address setter first enables its
    /// context while preserving every other enable bit. Call
    /// [`Self::set_multipan_enable_mask`] afterwards when the caller needs one
    /// exact final enable image independent of identity publication.
    pub fn set_multipan_identity(
        &mut self,
        index: Ieee802154MultipanIndex,
        identity: Ieee802154PanIdentity,
    ) {
        self.registers.set_multipan_identity(
            index.as_usize(),
            identity.pan_id(),
            identity.short_address(),
            identity.extended_address(),
        );
    }

    /// Program the public API's primary PAN identity.
    pub fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.set_multipan_identity(Ieee802154MultipanIndex::CONTEXT0, identity);
    }

    /// Read one complete PAN identity without changing its enable bit.
    pub fn multipan_identity(&self, index: Ieee802154MultipanIndex) -> Ieee802154PanIdentity {
        raw_identity_to_typed(self.registers.multipan_identity(index.as_usize()))
    }

    /// Read the complete four-bit multipan enable field.
    pub fn multipan_enable_mask(&self) -> Ieee802154MultipanEnableMask {
        Ieee802154MultipanEnableMask(self.registers.multipan_enable_mask())
    }

    /// Set the outgoing ACK frame-pending bit through a preserving update.
    pub fn set_frame_pending(&mut self, pending: bool) {
        self.registers.set_frame_pending(pending);
    }

    /// Read the outgoing ACK frame-pending bit.
    pub fn frame_pending(&self) -> bool {
        self.registers.frame_pending()
    }

    /// Publish exactly one enhanced-ACK-generation-done notification write.
    ///
    /// The only accepted image is the public LL's complete value one. This
    /// method does not infer whether hardware self-clears or handshakes the
    /// write; operation lifecycle must make each call non-replayable.
    pub fn notify_enhanced_ack_generated(&mut self) {
        self.registers.notify_enhanced_ack_generated();
    }

    /// Configure and enable transmit security in exact public-driver order.
    ///
    /// The borrowed key is written as four little-endian words and is never
    /// stored in a PAC value or exposed through `Debug`. The transaction is
    /// address low/high, key words zero through three, payload offset, then
    /// `TX_ENABLE = 1`.
    pub fn configure_transmit_security(
        &mut self,
        address: &[u8; 8],
        key: &[u8; 16],
        payload_offset: Ieee802154SecurityPayloadOffset,
    ) {
        self.registers
            .configure_transmit_security(address, key, payload_offset.value());
    }

    /// Disable transmit security without claiming key/address zeroization.
    ///
    /// Pinned ESP-IDF `ieee802154_sec_clear()` clears only `TX_ENABLE`; no
    /// source proves that writing zero to the write-only address/key registers
    /// is a safe hardware zeroization transaction. This method therefore
    /// preserves those registers and makes no erasure claim.
    ///
    /// No misleading clear/zeroize operation exists:
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_pac::Ieee802154RegisterLease;
    ///
    /// fn unsupported_zeroization(lease: &mut Ieee802154RegisterLease<'_>) {
    ///     lease.clear_transmit_security();
    /// }
    /// ```
    pub fn disable_transmit_security(&mut self) {
        self.registers.disable_transmit_security();
    }

    /// Read only the non-secret transmit-security control fields.
    pub fn transmit_security_control(&self) -> Ieee802154TransmitSecurityControl {
        let control = self.registers.transmit_security_control();
        Ieee802154TransmitSecurityControl::new(
            control.enabled(),
            Ieee802154SecurityPayloadOffset(control.payload_offset()),
        )
    }

    /// Replace only the generated five-bit TX/RX coexistence PTI field.
    pub fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.registers.set_txrx_pti(pti.value());
    }

    /// Replace only the generated five-bit ACK coexistence PTI field.
    pub fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.registers.set_ack_pti(pti.value());
    }

    /// Sample the complete static MAC-policy subset once per backing word.
    pub fn mac_policy_snapshot(&self) -> Ieee802154MacPolicySnapshot {
        let readback = self.registers.static_mac_policy_readback();
        let identity = readback.identity();
        Ieee802154MacPolicySnapshot {
            frequency_code: Ieee802154FrequencyCode(readback.frequency_code()),
            cca_mode: Ieee802154CcaMode::from_field(readback.cca_mode()),
            cca_threshold_code: readback.cca_threshold_code() as i8,
            ack_timeout: Ieee802154AckTimeoutUnits(readback.ack_timeout()),
            control: Ieee802154MacControl::new(
                readback.auto_ack_tx(),
                readback.auto_ack_rx(),
                readback.enhanced_ack_tx(),
                readback.coordinator(),
                readback.promiscuous(),
                readback.pending_enhanced(),
            ),
            multipan_enable_mask: readback.multipan_enable_mask(),
            identity: Ieee802154PanIdentity::new(
                identity.pan_id(),
                identity.short_address(),
                identity.extended_address(),
            ),
        }
    }

    /// Sample every currently modeled task-side MAC configuration field.
    ///
    /// This diagnostic readback is deliberately separate from
    /// [`Self::mac_policy_snapshot`]: dynamic frame-pending state and fields
    /// not written by runtime refresh cannot silently join policy equality.
    pub fn mac_configuration_readback(&self) -> Ieee802154MacConfigurationReadback {
        let readback = self.registers.mac_configuration_readback();
        let identities = [
            raw_identity_to_typed(readback.identity(0)),
            raw_identity_to_typed(readback.identity(1)),
            raw_identity_to_typed(readback.identity(2)),
            raw_identity_to_typed(readback.identity(3)),
        ];
        Ieee802154MacConfigurationReadback {
            frequency_code: Ieee802154FrequencyCode(readback.frequency_code()),
            tx_power_code: Ieee802154TxPowerCode::new(u32::from(readback.tx_power_code()))
                .expect("raw PAC field is eight bits"),
            cca_mode: Ieee802154CcaMode::from_field(readback.cca_mode()),
            cca_threshold_code: readback.cca_threshold_code() as i8,
            ed_sample_rate: Ieee802154EdSampleRate::from_field(readback.ed_sample_rate()),
            ack_timeout: Ieee802154AckTimeoutUnits(readback.ack_timeout()),
            control: Ieee802154MacControl::new(
                readback.auto_ack_tx(),
                readback.auto_ack_rx(),
                readback.enhanced_ack_tx(),
                readback.coordinator(),
                readback.promiscuous(),
                readback.pending_enhanced(),
            ),
            multipan_enable_mask: Ieee802154MultipanEnableMask(readback.multipan_enable_mask()),
            identities,
            frame_pending: readback.frame_pending(),
        }
    }

    /// Order memory and device accesses at a descriptor/MMIO boundary.
    pub fn order_device_accesses(&mut self) {
        crate::device_fence();
    }
}

/// Cold/polled MAC lease that borrows both disjoint ownership halves.
///
/// Construction requires the inactive [`Ieee802154InterruptSetup`]. Because
/// activation consumes that setup, `EVENT_STATUS`, affine W1C acknowledge and
/// RX/TX interrupt sidebands are statically unavailable while a hard-IRQ owner
/// exists. Task-only command, DMA and policy operations remain reachable via
/// the embedded [`Ieee802154RegisterLease`].
#[must_use = "dropping the polled lease releases both inactive ownership borrows"]
#[doc(hidden)]
pub struct Ieee802154PolledRegisterLease<'registers> {
    task: Ieee802154RegisterLease<'registers>,
    interrupt: &'registers mut crate::svd::ieee802154_mac_ownership::InterruptRegisters,
}

impl<'registers> Deref for Ieee802154PolledRegisterLease<'registers> {
    type Target = Ieee802154RegisterLease<'registers>;

    fn deref(&self) -> &Self::Target {
        &self.task
    }
}

impl DerefMut for Ieee802154PolledRegisterLease<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.task
    }
}

impl Ieee802154PolledRegisterLease<'_> {
    /// Mask every MAC event while the inactive interrupt owner is borrowed.
    pub fn mask_all_events(&mut self) {
        self.task.registers.replace_event_enable(0);
    }

    /// Mask every receive-abort source before a receive dataplane exists.
    pub fn mask_all_rx_aborts(&mut self) {
        self.task.registers.replace_rx_abort_enable(0);
    }

    /// Mask every transmit-abort source before a transmit dataplane exists.
    pub fn mask_all_tx_aborts(&mut self) {
        self.task.registers.replace_tx_abort_enable(0);
    }

    /// Select the vendor foundation's average energy-detection sampler.
    pub fn select_average_ed_sampling(&mut self) {
        self.task.registers.select_average_ed_sampling();
    }

    /// Replace the complete named `EVENT_ENABLE` set exactly.
    pub fn set_event_enable(&mut self, events: Ieee802154EventEnableMask) {
        self.task.registers.replace_event_enable(events.bits());
    }

    /// Replace the complete receive-abort field with one closed image.
    pub fn set_rx_abort_enable(&mut self, reasons: Ieee802154RxAbortEnableMask) {
        self.task.registers.replace_rx_abort_enable(reasons.bits());
    }

    /// Classify the complete event-delivery field for a finite polled ED/CCA
    /// operation without sampling `EVENT_STATUS`.
    pub fn operation_event_enable_observation(&self) -> Ieee802154OperationEventEnableObservation {
        Ieee802154OperationEventEnableObservation::from_field(self.task.registers.event_enable())
    }

    /// Classify the complete RX-abort delivery field for a finite polled
    /// ED/CCA operation.
    pub fn operation_rx_abort_enable_observation(
        &self,
    ) -> Ieee802154OperationRxAbortEnableObservation {
        Ieee802154OperationRxAbortEnableObservation::from_field(
            self.task.registers.rx_abort_enable(),
        )
    }

    /// Observe the complete fourteen-bit event field without acknowledging it.
    pub fn event_status_observation(&self) -> Ieee802154EventObservation {
        let snapshot = self.interrupt.sample_event_status();
        Ieee802154EventObservation::from_field(snapshot.bits() as u16)
    }

    /// Observe the complete IRQ-owned RX sideband word.
    pub fn rx_status_observation(&self) -> Ieee802154RxStatusObservation {
        Ieee802154RxStatusObservation::from_register(self.interrupt.rx_status_bits())
    }

    /// Sample only fields written by the interrupt-masked foundation.
    pub fn foundation_snapshot(&self) -> Ieee802154FoundationSnapshot {
        let readback = self.task.registers.foundation_readback();
        Ieee802154FoundationSnapshot {
            enabled_events: readback.event_enable(),
            enabled_rx_aborts: readback.rx_abort_enable(),
            enabled_tx_aborts: readback.tx_abort_enable(),
            ed_uses_average: readback.ed_uses_average(),
            txrx_pti: Ieee802154Pti(readback.txrx_pti()),
            ack_pti: Ieee802154Pti(readback.ack_pti()),
        }
    }

    /// Sample the generated receive and transmit state fields once each.
    pub fn state_snapshot(&self) -> Ieee802154StateSnapshot {
        let (rx, tx) = self.interrupt.state_codes();
        Ieee802154StateSnapshot::new(
            Ieee802154RxStateCode::from_field(rx),
            Ieee802154TxStateCode::from_field(tx),
        )
    }

    /// Sample the DMA-free ED/CCA surface while the interrupt half is inactive.
    pub fn ed_cca_snapshot(&self) -> Ieee802154EdCcaSnapshot {
        let event_status = self.interrupt.sample_event_status();
        Ieee802154EdCcaSnapshot::new(
            Ieee802154EdDurationUnits::from_field(self.task.registers.ed_duration()),
            Ieee802154EventObservation::from_field(self.task.registers.event_enable()),
            Ieee802154EventObservation::from_field(event_status.bits() as u16),
            self.interrupt.ed_rss_code(),
            self.interrupt.cca_busy(),
        )
    }

    /// Sample only the signed ED RSS sideband after `ED_DONE`.
    pub fn ed_rss_code(&self) -> i8 {
        self.interrupt.ed_rss_code()
    }

    /// Sample only the CCA-busy sideband after `ED_DONE`.
    pub fn cca_busy(&self) -> bool {
        self.interrupt.cca_busy()
    }

    /// Sample and consume one complete affine W1C event snapshot.
    #[doc(hidden)]
    pub fn acknowledge_pending_events(&mut self) -> Ieee802154EventObservation {
        crate::device_fence();
        let snapshot = self.interrupt.sample_event_status();
        let events = Ieee802154EventObservation::from_field(snapshot.bits() as u16);
        self.interrupt.acknowledge_event_status(snapshot);
        crate::device_fence();
        events
    }

    /// Sample the source-132 route words without exposing either pointer.
    #[doc(hidden)]
    pub fn interrupt_route_readback(&self) -> Ieee802154RouteRawReadback {
        let readback = self.task.registers.interrupt_route_readback();
        Ieee802154RouteRawReadback {
            core0: readback.core0_bits(),
            core1: readback.core1_bits(),
        }
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_enable_events(&self) -> u16 {
        self.task.registers.event_enable()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_timer_events(&mut self) {
        self.task.registers.validation_enable_timer_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_all_events(&mut self) {
        self.task.registers.validation_disable_all_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_status_events(&self) -> u16 {
        self.interrupt.validation_event_status_events()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer0_value(&self) -> u32 {
        self.task.registers.validation_timer0_value()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer1_value(&self) -> u32 {
        self.task.registers.validation_timer1_value()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_event_timer_thresholds(&mut self, threshold: u32) {
        self.task
            .registers
            .validation_set_timer_thresholds(threshold);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer0(&mut self) {
        self.task.registers.validation_start_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer0(&mut self) {
        self.task.registers.validation_stop_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer1(&mut self) {
        self.task.registers.validation_start_timer1();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer1(&mut self) {
        self.task.registers.validation_stop_timer1();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer0(&mut self) {
        self.interrupt.validation_write_timer0_event();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer1(&mut self) {
        self.interrupt.validation_write_timer1_event();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_enable_events(&self) -> u16 {
        self.task.registers.event_enable()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_timer_abort_events(&mut self) {
        self.task
            .registers
            .validation_enable_ed_timer_abort_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_events(&mut self) {
        self.task.registers.validation_disable_ed_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_abort_enable_events(&self) -> u32 {
        self.task.registers.validation_ed_rx_abort_enable()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_abort_reasons(&mut self) {
        self.task.registers.validation_enable_ed_abort_reasons();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_abort_reasons(&mut self) {
        self.task.registers.validation_disable_ed_abort_reasons();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_status_events(&self) -> u16 {
        self.interrupt.validation_ed_event_status_events()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_status_raw(&self) -> u32 {
        self.interrupt.validation_ed_rx_status_raw()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_duration(&self) -> u32 {
        self.task.registers.validation_ed_duration()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_duration_eight(&mut self) {
        self.task.registers.validation_set_ed_duration_eight();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_timer0_value(&self) -> u32 {
        self.task.registers.validation_ed_timer0_value()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_timer0_threshold(&mut self, threshold: u32) {
        self.task
            .registers
            .validation_set_ed_timer0_threshold(threshold);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed_timer0(&mut self) {
        self.task.registers.validation_start_ed_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_timer0(&mut self) {
        self.task.registers.validation_stop_ed_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed(&mut self) {
        self.task.registers.validation_start_ed();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_operation(&mut self) {
        self.task.registers.validation_stop_ed_operation();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_done_event(&mut self) {
        self.interrupt.validation_write_ed_done_event();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_timer0_event(&mut self) {
        self.interrupt.validation_write_ed_timer0_event();
    }
}

/// Internal execution port shared by both PAC ownership transitions and their
/// host ordering model.
///
/// The event snapshot is an associated affine type. An executor can therefore
/// acknowledge only the value returned by `sample_events`; no integer event
/// image can cross this boundary.
trait Ieee802154InterruptTransitionPort {
    type EventSnapshot;

    fn replace_event_enable(&mut self, events: Ieee802154EventEnableMask);
    fn replace_tx_abort_enable(&mut self, aborts: Ieee802154InterruptTxAbortEnableMask);
    fn replace_rx_abort_enable(&mut self, aborts: Ieee802154InterruptRxAbortEnableMask);
    fn order_device_accesses(&mut self);
    fn sample_events(&mut self) -> Self::EventSnapshot;
    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot);
}

/// Execute the complete activation while the platform CPU route is disabled.
///
/// `EVENT_ENABLE` remains zero while both abort fields and the stale affine
/// W1C transaction are updated. The reviewed event baseline is published only
/// after the exact sampled status has been consumed, and the final fence
/// precedes transfer of the hard-IRQ owner.
fn execute_interrupt_activation<Port>(port: &mut Port, plan: Ieee802154InterruptActivationPlan)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.replace_event_enable(Ieee802154EventEnableMask::NONE);
    port.replace_tx_abort_enable(plan.tx_aborts());
    port.replace_rx_abort_enable(plan.rx_aborts());
    port.order_device_accesses();

    let stale = port.sample_events();
    port.acknowledge_events(stale);
    port.replace_event_enable(plan.events());
    port.order_device_accesses();
}

/// Execute the complete teardown after the platform CPU route is disabled.
///
/// All three enable fields are replaced with their closed zero images before
/// one final affine W1C sample is consumed. Both ordering boundaries precede
/// transfer back to inactive setup ownership.
fn execute_interrupt_deactivation<Port>(port: &mut Port)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.replace_event_enable(Ieee802154EventEnableMask::NONE);
    port.replace_tx_abort_enable(Ieee802154InterruptTxAbortEnableMask::NONE);
    port.replace_rx_abort_enable(Ieee802154InterruptRxAbortEnableMask::NONE);
    port.order_device_accesses();

    let pending = port.sample_events();
    port.acknowledge_events(pending);
    port.order_device_accesses();
}

/// Borrowed task owner plus the disjoint raw interrupt partition for one
/// activation or teardown transaction.
struct Ieee802154PacInterruptTransitionPort<'task> {
    task: &'task mut Ieee802154TaskRegisters,
    registers: crate::svd::ieee802154_mac_ownership::InterruptRegisters,
}

impl Ieee802154InterruptTransitionPort for Ieee802154PacInterruptTransitionPort<'_> {
    type EventSnapshot = crate::svd::w1c_register_snapshot::Ieee802154EventStatusSnapshot;

    fn replace_event_enable(&mut self, events: Ieee802154EventEnableMask) {
        self.task
            .peripherals
            .ieee802154_mac
            .replace_event_enable(events.bits());
    }

    fn replace_tx_abort_enable(&mut self, aborts: Ieee802154InterruptTxAbortEnableMask) {
        self.task
            .peripherals
            .ieee802154_mac
            .replace_tx_abort_enable(aborts.bits());
    }

    fn replace_rx_abort_enable(&mut self, aborts: Ieee802154InterruptRxAbortEnableMask) {
        self.task
            .peripherals
            .ieee802154_mac
            .replace_rx_abort_enable(aborts.bits());
    }

    fn order_device_accesses(&mut self) {
        crate::device_fence();
    }

    fn sample_events(&mut self) -> Self::EventSnapshot {
        self.registers.sample_event_status()
    }

    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot) {
        self.registers.acknowledge_event_status(snapshot);
    }
}

impl Ieee802154InterruptSetup {
    /// Borrow both disjoint halves for one inactive-route polled transaction.
    ///
    /// The returned lease cannot outlive either owner. Calling
    /// [`Self::activate`] consumes this setup, so no polled `EVENT_STATUS` or
    /// W1C operation can coexist with the active hard-IRQ capability.
    #[doc(hidden)]
    pub fn polled_register_lease<'registers>(
        &'registers mut self,
        task: &'registers mut Ieee802154TaskRegisters,
    ) -> Ieee802154PolledRegisterLease<'registers> {
        Ieee802154PolledRegisterLease {
            task: task.ieee802154_register_lease(),
            interrupt: &mut self.registers,
        }
    }

    /// Install the source-confirmed runtime baseline and create the finite
    /// hard-IRQ owner.
    ///
    /// The platform CPU route must remain disabled until the returned value is
    /// installed in its final storage. This single consuming transition writes
    /// exact field images for `EVENT_ENABLE = 0x1a7f`,
    /// `RX_ABORT_ENABLE = 0x0002_8000`, and
    /// `TX_ABORT_ENABLE = 0x0186_8000`. It keeps event delivery masked while
    /// replacing both abort fields, consumes one complete stale affine W1C
    /// snapshot, publishes the event image last, and orders those writes
    /// before returning.
    ///
    /// There is no caller-selected mask argument: runtime code cannot activate
    /// an incomplete abort vocabulary or an event absent from the reviewed ISR.
    pub fn activate(self, task: &mut Ieee802154TaskRegisters) -> Ieee802154InterruptRegisters {
        let mut port = Ieee802154PacInterruptTransitionPort {
            task,
            registers: self.registers,
        };
        execute_interrupt_activation(
            &mut port,
            Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
        );
        Ieee802154InterruptRegisters {
            registers: port.registers,
        }
    }
}

impl Ieee802154InterruptRegisters {
    /// Capture one ISR event/status batch before acknowledging any event.
    ///
    /// This follows the pinned public ISR ordering: sample the complete event
    /// image, capture RX/TX abort evidence selected by that image, and retain
    /// the exact W1C token for a later consuming acknowledge.
    pub fn sample_interrupt(&self) -> Ieee802154InterruptSnapshot {
        let acknowledgement = self.registers.sample_event_status();
        let events = Ieee802154EventObservation::from_field(acknowledgement.bits() as u16);
        let rx_abort = events.contains(Ieee802154EventEnableMask::RX_ABORT);
        let tx_abort = events.contains(Ieee802154EventEnableMask::TX_ABORT);
        let ed_done = events.contains(Ieee802154EventEnableMask::ED_DONE);

        Ieee802154InterruptSnapshot {
            acknowledgement,
            events,
            rx_status: rx_abort.then(|| self.registers.rx_status_bits()),
            tx_status: tx_abort.then(|| self.registers.tx_status_bits()),
            ed_rss_code: ed_done.then(|| self.registers.ed_rss_code()),
            cca_busy: ed_done.then(|| self.registers.cca_busy()),
        }
    }

    /// Acknowledge exactly one sampled W1C event image and consume it.
    pub fn acknowledge_interrupt(&mut self, snapshot: Ieee802154InterruptSnapshot) {
        self.registers
            .acknowledge_event_status(snapshot.acknowledgement);
        crate::device_fence();
    }

    /// Close one finite hard-IRQ epoch and return inactive setup ownership.
    ///
    /// The caller must disable the platform CPU route before this method. The
    /// transition replaces event, transmit-abort, and receive-abort enables
    /// with exact zero images, consumes one final complete affine W1C snapshot,
    /// and orders both phases before returning task-side setup authority.
    pub fn deactivate(self, task: &mut Ieee802154TaskRegisters) -> Ieee802154InterruptSetup {
        let mut port = Ieee802154PacInterruptTransitionPort {
            task,
            registers: self.registers,
        };
        execute_interrupt_deactivation(&mut port);
        Ieee802154InterruptSetup {
            registers: port.registers,
        }
    }
}

impl Ieee802154TaskRegisters {
    /// Borrow the MAC capability from the dedicated IEEE 802.15.4 route.
    ///
    /// No Wi-Fi or Bluetooth-controller operation is reachable through the
    /// returned narrow lease.
    #[doc(hidden)]
    pub fn ieee802154_register_lease(&mut self) -> Ieee802154RegisterLease<'_> {
        Ieee802154RegisterLease {
            registers: &mut self.peripherals.ieee802154_mac,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdCcaSnapshot, Ieee802154EdCommand,
        Ieee802154EdDurationUnits, Ieee802154EdSampleRate, Ieee802154EventEnableMask,
        Ieee802154EventObservation, Ieee802154FoundationSnapshot, Ieee802154FrequencyCode,
        Ieee802154InterruptActivationPlan, Ieee802154InterruptRegisters,
        Ieee802154InterruptRxAbortEnableMask, Ieee802154InterruptSetup,
        Ieee802154InterruptSnapshot, Ieee802154InterruptTransitionPort,
        Ieee802154InterruptTxAbortEnableMask, Ieee802154MacCommand,
        Ieee802154MacConfigurationReadback, Ieee802154MacControl, Ieee802154MultipanEnableMask,
        Ieee802154MultipanIndex, Ieee802154OperationEventEnableObservation,
        Ieee802154OperationRxAbortEnableObservation, Ieee802154PanIdentity,
        Ieee802154PolledRegisterLease, Ieee802154Pti, Ieee802154RegisterLease,
        Ieee802154RxAbortEnableMask, Ieee802154RxStateCode, Ieee802154RxStatusObservation,
        Ieee802154SecurityPayloadOffset, Ieee802154StateSnapshot, Ieee802154TaskRegisters,
        Ieee802154TxPowerCode, Ieee802154TxStateCode, execute_interrupt_activation,
        execute_interrupt_deactivation,
    };
    use crate::RadioHardware;
    use std::vec::Vec;

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
    fn complete_configuration_readback_keeps_typed_fields_and_all_identities() {
        let control = Ieee802154MacControl::new(true, false, true, false, true, false);
        let identities = [
            Ieee802154PanIdentity::new(
                0x1234,
                0xabcd,
                [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
            ),
            Ieee802154PanIdentity::new(1, 2, [3; 8]),
            Ieee802154PanIdentity::new(4, 5, [6; 8]),
            Ieee802154PanIdentity::new(7, 8, [9; 8]),
        ];
        let snapshot = Ieee802154MacConfigurationReadback {
            frequency_code: Ieee802154FrequencyCode::new(78),
            tx_power_code: Ieee802154TxPowerCode::new(0xa5).expect("eight-bit TX-power code"),
            cca_mode: Ieee802154CcaMode::CarrierAndEnergyDetection,
            cca_threshold_code: -75,
            ed_sample_rate: Ieee802154EdSampleRate::FourPerMicrosecond,
            ack_timeout: Ieee802154AckTimeoutUnits::new(108),
            control,
            multipan_enable_mask: Ieee802154MultipanEnableMask::from_bits(0b0101)
                .expect("four-bit mask"),
            identities,
            frame_pending: true,
        };

        assert_eq!(snapshot.frequency_code().value(), 78);
        assert_eq!(snapshot.tx_power_code().get(), 0xa5);
        assert_eq!(
            snapshot.cca_mode(),
            Ieee802154CcaMode::CarrierAndEnergyDetection
        );
        assert_eq!(snapshot.cca_threshold_code(), -75);
        assert_eq!(
            snapshot.ed_sample_rate(),
            Ieee802154EdSampleRate::FourPerMicrosecond
        );
        assert_eq!(snapshot.ack_timeout().value(), 108);
        assert_eq!(snapshot.control(), control);
        assert_eq!(snapshot.multipan_enable_mask().bits(), 0b0101);
        for index in 0..Ieee802154MultipanIndex::COUNT {
            let index = Ieee802154MultipanIndex::new(index).expect("bounded context");
            assert_eq!(
                snapshot.multipan_identity(index),
                identities[index.as_usize()]
            );
        }
        assert!(snapshot.frame_pending());
    }

    #[test]
    fn pac_geometry_constructors_are_exhaustive_over_every_u8() {
        for raw in u8::MIN..=u8::MAX {
            assert_eq!(
                Ieee802154TxPowerCode::new(u32::from(raw)).map(Ieee802154TxPowerCode::get),
                Some(u32::from(raw))
            );
            assert_eq!(
                Ieee802154MultipanIndex::new(raw).map(Ieee802154MultipanIndex::value),
                (raw < 4).then_some(raw)
            );
            assert_eq!(
                Ieee802154MultipanEnableMask::from_bits(raw)
                    .map(Ieee802154MultipanEnableMask::bits),
                (raw < 16).then_some(raw)
            );
            assert_eq!(
                Ieee802154SecurityPayloadOffset::new(raw)
                    .map(Ieee802154SecurityPayloadOffset::value),
                (raw <= Ieee802154SecurityPayloadOffset::MAX).then_some(raw)
            );
        }
        assert_eq!(Ieee802154TxPowerCode::new(0x100), None);
    }

    #[test]
    fn multipan_masks_address_all_four_contexts_without_truncation() {
        let mut mask = Ieee802154MultipanEnableMask::NONE;
        for raw in 0..Ieee802154MultipanIndex::COUNT {
            let index = Ieee802154MultipanIndex::new(raw).expect("all four indices are valid");
            assert!(!mask.contains(index));
            mask = mask.with(index);
            assert!(mask.contains(index));
        }
        assert_eq!(mask, Ieee802154MultipanEnableMask::ALL);
        for raw in 0..Ieee802154MultipanIndex::COUNT {
            let index = Ieee802154MultipanIndex::new(raw).expect("all four indices are valid");
            mask = mask.without(index);
            assert!(!mask.contains(index));
        }
        assert_eq!(mask, Ieee802154MultipanEnableMask::NONE);
    }

    #[test]
    fn ed_sample_rate_values_cover_the_complete_two_bit_field() {
        for (raw, rate) in [
            Ieee802154EdSampleRate::OnePerMicrosecond,
            Ieee802154EdSampleRate::TwoPerMicrosecond,
            Ieee802154EdSampleRate::FourPerMicrosecond,
            Ieee802154EdSampleRate::EightPerMicrosecond,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(rate.field_value(), raw as u8);
            assert_eq!(Ieee802154EdSampleRate::from_field(raw as u8), rate);
        }
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
        assert_eq!(
            Ieee802154EventEnableMask::HANDLED_BASELINE_WITHOUT_TIMER0.bits(),
            0x1a7f
        );
        assert!(
            !Ieee802154EventEnableMask::HANDLED_BASELINE_WITHOUT_TIMER0
                .contains(Ieee802154EventEnableMask::TIMER0_OVERFLOW)
        );
        assert!(
            !Ieee802154EventEnableMask::HANDLED_BASELINE_WITHOUT_TIMER0
                .contains(Ieee802154EventEnableMask::CLOCK_COUNT_MATCH)
        );
    }

    #[test]
    fn interrupt_activation_plan_is_the_exact_closed_source_baseline() {
        let plan = Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE;

        assert_eq!(plan.events().bits(), 0x1a7f);
        assert_eq!(plan.rx_aborts().bits(), 0x0002_8000);
        assert_eq!(plan.tx_aborts().bits(), 0x0186_8000);
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
    fn dedicated_route_lends_the_same_narrow_mac_surface() {
        let mut cold = RadioHardware::for_validation().into_ieee802154();
        let mut lease = cold.radio_mut().ieee802154_register_lease();

        // The host reaches only the architecture-neutral fence. The lease is
        // backed by the dedicated route rather than by a second raw singleton.
        lease.order_device_accesses();
        let _hardware = cold.release();
    }

    #[test]
    fn cold_w1c_boundary_accepts_no_caller_supplied_image() {
        let _acknowledge: fn(
            &mut Ieee802154PolledRegisterLease<'static>,
        ) -> Ieee802154EventObservation = Ieee802154PolledRegisterLease::acknowledge_pending_events;
    }

    #[test]
    fn polled_operation_pac_surface_uses_only_closed_typed_writes() {
        let _set_events: fn(
            &mut Ieee802154PolledRegisterLease<'static>,
            Ieee802154EventEnableMask,
        ) = Ieee802154PolledRegisterLease::set_event_enable;
        let _set_rx_aborts: fn(
            &mut Ieee802154PolledRegisterLease<'static>,
            Ieee802154RxAbortEnableMask,
        ) = Ieee802154PolledRegisterLease::set_rx_abort_enable;
        let _event_enable: fn(
            &Ieee802154PolledRegisterLease<'static>,
        ) -> Ieee802154OperationEventEnableObservation =
            Ieee802154PolledRegisterLease::operation_event_enable_observation;
        let _rx_abort_enable: fn(
            &Ieee802154PolledRegisterLease<'static>,
        ) -> Ieee802154OperationRxAbortEnableObservation =
            Ieee802154PolledRegisterLease::operation_rx_abort_enable_observation;
        let _event_status: fn(
            &Ieee802154PolledRegisterLease<'static>,
        ) -> Ieee802154EventObservation = Ieee802154PolledRegisterLease::event_status_observation;
        let _rx_status: fn(
            &Ieee802154PolledRegisterLease<'static>,
        ) -> Ieee802154RxStatusObservation = Ieee802154PolledRegisterLease::rx_status_observation;
        let _set_duration: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154EdDurationUnits) =
            Ieee802154RegisterLease::set_ed_duration;
        let _sample_ed: fn(&Ieee802154PolledRegisterLease<'static>) -> Ieee802154EdCcaSnapshot =
            Ieee802154PolledRegisterLease::ed_cca_snapshot;
        let _start: fn(&mut Ieee802154RegisterLease<'static>) =
            Ieee802154RegisterLease::request_ed_start;
    }

    #[derive(Debug, Eq, PartialEq)]
    enum InterruptTransitionOperation {
        ReplaceEvents(u16),
        ReplaceTxAborts(u32),
        ReplaceRxAborts(u32),
        OrderDeviceAccesses,
        SampleStaleEvents(u8),
        AcknowledgeStaleEvents(u8),
    }

    #[derive(Debug, Eq, PartialEq)]
    struct RecordingEventSnapshot(u8);

    struct RecordingInterruptTransitionPort {
        event_identity: u8,
        operations: Vec<InterruptTransitionOperation>,
    }

    impl Ieee802154InterruptTransitionPort for RecordingInterruptTransitionPort {
        type EventSnapshot = RecordingEventSnapshot;

        fn replace_event_enable(&mut self, events: Ieee802154EventEnableMask) {
            self.operations
                .push(InterruptTransitionOperation::ReplaceEvents(events.bits()));
        }

        fn replace_tx_abort_enable(&mut self, aborts: Ieee802154InterruptTxAbortEnableMask) {
            self.operations
                .push(InterruptTransitionOperation::ReplaceTxAborts(aborts.bits()));
        }

        fn replace_rx_abort_enable(&mut self, aborts: Ieee802154InterruptRxAbortEnableMask) {
            self.operations
                .push(InterruptTransitionOperation::ReplaceRxAborts(aborts.bits()));
        }

        fn order_device_accesses(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::OrderDeviceAccesses);
        }

        fn sample_events(&mut self) -> Self::EventSnapshot {
            self.operations
                .push(InterruptTransitionOperation::SampleStaleEvents(
                    self.event_identity,
                ));
            RecordingEventSnapshot(self.event_identity)
        }

        fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot) {
            self.operations
                .push(InterruptTransitionOperation::AcknowledgeStaleEvents(
                    snapshot.0,
                ));
        }
    }

    #[test]
    fn production_activation_executor_masks_then_acks_the_exact_affine_sample() {
        let mut port = RecordingInterruptTransitionPort {
            event_identity: 0xa5,
            operations: Vec::new(),
        };
        execute_interrupt_activation(
            &mut port,
            Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
        );

        assert_eq!(
            port.operations,
            [
                InterruptTransitionOperation::ReplaceEvents(0),
                InterruptTransitionOperation::ReplaceTxAborts(0x0186_8000),
                InterruptTransitionOperation::ReplaceRxAborts(0x0002_8000),
                InterruptTransitionOperation::OrderDeviceAccesses,
                InterruptTransitionOperation::SampleStaleEvents(0xa5),
                InterruptTransitionOperation::AcknowledgeStaleEvents(0xa5),
                InterruptTransitionOperation::ReplaceEvents(0x1a7f),
                InterruptTransitionOperation::OrderDeviceAccesses,
            ]
        );
    }

    #[test]
    fn production_deactivation_executor_clears_every_mask_before_final_affine_ack() {
        let mut port = RecordingInterruptTransitionPort {
            event_identity: 0x3c,
            operations: Vec::new(),
        };
        execute_interrupt_deactivation(&mut port);

        assert_eq!(
            port.operations,
            [
                InterruptTransitionOperation::ReplaceEvents(0),
                InterruptTransitionOperation::ReplaceTxAborts(0),
                InterruptTransitionOperation::ReplaceRxAborts(0),
                InterruptTransitionOperation::OrderDeviceAccesses,
                InterruptTransitionOperation::SampleStaleEvents(0x3c),
                InterruptTransitionOperation::AcknowledgeStaleEvents(0x3c),
                InterruptTransitionOperation::OrderDeviceAccesses,
            ]
        );
    }

    #[test]
    fn running_task_and_irq_surfaces_are_owned_and_disjoint() {
        let _activate: fn(
            Ieee802154InterruptSetup,
            &mut Ieee802154TaskRegisters,
        ) -> Ieee802154InterruptRegisters = Ieee802154InterruptSetup::activate;
        let _sample: fn(&Ieee802154InterruptRegisters) -> Ieee802154InterruptSnapshot =
            Ieee802154InterruptRegisters::sample_interrupt;
        let _acknowledge: fn(&mut Ieee802154InterruptRegisters, Ieee802154InterruptSnapshot) =
            Ieee802154InterruptRegisters::acknowledge_interrupt;
        let _deactivate: fn(
            Ieee802154InterruptRegisters,
            &mut Ieee802154TaskRegisters,
        ) -> Ieee802154InterruptSetup = Ieee802154InterruptRegisters::deactivate;
        let _command: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154MacCommand) =
            Ieee802154RegisterLease::request_mac_command;
        let _tx_dma: fn(&mut Ieee802154RegisterLease<'static>, u32) =
            Ieee802154RegisterLease::publish_transmit_dma_address;
        let _rx_dma: fn(&mut Ieee802154RegisterLease<'static>, u32) =
            Ieee802154RegisterLease::publish_receive_dma_address;
        let _tx_power: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154TxPowerCode) =
            Ieee802154RegisterLease::set_tx_power_code;
        let _ed_rate: fn(&mut Ieee802154RegisterLease<'static>, Ieee802154EdSampleRate) =
            Ieee802154RegisterLease::set_ed_sample_rate;
        let _multipan_identity: fn(
            &mut Ieee802154RegisterLease<'static>,
            Ieee802154MultipanIndex,
            Ieee802154PanIdentity,
        ) = Ieee802154RegisterLease::set_multipan_identity;
        let _multipan_mask: fn(
            &mut Ieee802154RegisterLease<'static>,
            Ieee802154MultipanEnableMask,
        ) = Ieee802154RegisterLease::set_multipan_enable_mask;
        let _frame_pending: fn(&mut Ieee802154RegisterLease<'static>, bool) =
            Ieee802154RegisterLease::set_frame_pending;
        let _notify: fn(&mut Ieee802154RegisterLease<'static>) =
            Ieee802154RegisterLease::notify_enhanced_ack_generated;
        let _security_config: fn(
            &mut Ieee802154RegisterLease<'static>,
            &[u8; 8],
            &[u8; 16],
            Ieee802154SecurityPayloadOffset,
        ) = Ieee802154RegisterLease::configure_transmit_security;
        let _security_disable: fn(&mut Ieee802154RegisterLease<'static>) =
            Ieee802154RegisterLease::disable_transmit_security;
    }
}
