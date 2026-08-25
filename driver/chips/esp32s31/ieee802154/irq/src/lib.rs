//! Quiesced interrupt planning for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! This crate deliberately contains no concrete MMIO, interrupt binding,
//! route-enable, or general interrupt-status acknowledgement operation. Public
//! code can inspect the source-confirmed OR-operation order and apply a pure
//! subset readback predicate, but it cannot supply a backend or mint an
//! execution result. A consuming IRQ transaction exists only in unit tests and
//! has no production backend until a target ownership split is reviewed.
//!
//! `EVENT_STATUS` is available through one deliberately narrow boundary outside
//! this crate: a fixed `ED_DONE = 0x0040` selected image is HIL-qualified for a
//! serialized, route-detached ED completion transaction. That result does not
//! classify the register as generally W1C, authorize acknowledgement of any
//! other event, define concurrent-arrival behavior, or establish an active IRQ
//! owner. Those broader capabilities remain unavailable.
//!
//! Register identities and values below are audited against ESP-IDF commit
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`:
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/interrupts.h#L139-L153>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/interrupt_core0_reg.h#L2786-L2805>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/interrupt_core1_reg.h#L2786-L2805>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/reg_base.h#L137-L138>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L43-L122>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/include/esp_intr_alloc.h#L135-L169>,
//! and
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L782-L938>.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(test)]
use core::fmt;

/// The only interrupt source identity represented by this crate.
///
/// No constructor from an integer is provided, so an arbitrary peripheral
/// source cannot be confused with the IEEE 802.15.4 MAC source.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
pub enum Ieee802154InterruptSource {
    /// `ETS_MODEM_ZB_MAC_INTR_SOURCE` in the pinned ESP32-S31 source table.
    ModemZbMac = 132,
}

impl Ieee802154InterruptSource {
    /// Return the audited peripheral interrupt source number.
    pub const fn number(self) -> u16 {
        self as u16
    }
}

/// The audited ESP32-S31 IEEE 802.15.4 MAC interrupt source.
pub const IEEE802154_MAC_INTERRUPT_SOURCE: Ieee802154InterruptSource =
    Ieee802154InterruptSource::ModemZbMac;

/// One of the two ESP32-S31 high-performance CPU cores.
///
/// This enum is used only for route-register geometry. This crate never reads
/// or writes either route register.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154InterruptCore {
    /// High-performance CPU core zero.
    Core0 = 0,
    /// High-performance CPU core one.
    Core1 = 1,
}

/// Audited geometry of the per-core MODEM_ZB_MAC interrupt-map register.
///
/// These constants are descriptive only. There is intentionally no base
/// address, arbitrary-address constructor, register accessor, or route-enable
/// operation in this crate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154RouteGeometry;

impl Ieee802154RouteGeometry {
    /// MODEM_ZB_MAC map-register offset from an interrupt-core base.
    pub const MAP_REGISTER_OFFSET: usize = 0x210;
    /// Distance between the core-zero and core-one interrupt register blocks.
    pub const CORE_REGISTER_STRIDE: usize = 0x800;
    /// Bits `[5:0]`, which hold the selected CPU interrupt number.
    pub const MAP_FIELD_MASK: u32 = 0x3f;
    /// Shift of the interrupt pass/remap-level field.
    pub const PASS_LEVEL_SHIFT: u32 = 8;
    /// Bits `[9:8]`, which hold the interrupt pass/remap level.
    pub const PASS_LEVEL_MASK: u32 = 0x03 << Self::PASS_LEVEL_SHIFT;
    /// Every documented field in one MODEM_ZB_MAC route word.
    pub const DOCUMENTED_FIELD_MASK: u32 = Self::MAP_FIELD_MASK | Self::PASS_LEVEL_MASK;

    /// Return the map-register offset relative to the core-zero block.
    ///
    /// The result is an offset, never an MMIO address.
    pub const fn map_offset_from_core0(core: Ieee802154InterruptCore) -> usize {
        Self::MAP_REGISTER_OFFSET + core as usize * Self::CORE_REGISTER_STRIDE
    }
}

/// One caller-supplied MODEM_ZB_MAC route-register observation.
///
/// This pure value carries no MMIO capability and cannot change a route. Raw
/// bits outside the two fields documented by the pinned ESP32-S31 headers are
/// retained for evidence. They are ignored only by the field decoders; the
/// complete-reset predicate rejects every nonzero raw bit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154RouteWord(u32);

impl Ieee802154RouteWord {
    /// Preserve one complete observed register word for pure classification.
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Return the complete caller-supplied register observation.
    pub const fn raw_bits(self) -> u32 {
        self.0
    }

    /// Return the documented CPU-interrupt destination field, bits `[5:0]`.
    pub const fn map(self) -> u8 {
        (self.0 & Ieee802154RouteGeometry::MAP_FIELD_MASK) as u8
    }

    /// Return the documented pass/remap-level field, bits `[9:8]`.
    pub const fn pass_level(self) -> u8 {
        ((self.0 & Ieee802154RouteGeometry::PASS_LEVEL_MASK)
            >> Ieee802154RouteGeometry::PASS_LEVEL_SHIFT) as u8
    }

    /// Return only the documented MAP and PASS_LEVEL bits.
    pub const fn documented_bits(self) -> u32 {
        self.0 & Ieee802154RouteGeometry::DOCUMENTED_FIELD_MASK
    }

    /// Return whether MAP has the boot/reset unassigned value zero.
    ///
    /// This deliberately checks only MAP. It does not claim the complete word
    /// is at reset, nor does it classify the ESP-IDF runtime allocator's
    /// separate disabled destination (CPU interrupt six).
    pub const fn is_reset_unassigned(self) -> bool {
        self.map() == 0
    }

    /// Return whether the complete observed route word is exactly reset zero.
    ///
    /// Unknown bits are not assumed inert at this safety boundary.
    pub const fn is_full_reset(self) -> bool {
        self.0 == 0
    }
}

/// One ordered readback of the IEEE 802.15.4 route word on both CPU cores.
///
/// The value does not claim the two words were sampled atomically. It only
/// preserves their core identities and applies the pure per-word predicates.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154RouteReadback {
    core0: Ieee802154RouteWord,
    core1: Ieee802154RouteWord,
}

impl Ieee802154RouteReadback {
    /// Pair the core-zero and core-one observations in fixed order.
    pub const fn new(core0: Ieee802154RouteWord, core1: Ieee802154RouteWord) -> Self {
        Self { core0, core1 }
    }

    /// Return the core-zero route observation.
    pub const fn core0(self) -> Ieee802154RouteWord {
        self.core0
    }

    /// Return the core-one route observation.
    pub const fn core1(self) -> Ieee802154RouteWord {
        self.core1
    }

    /// Return whether both MAP fields have the boot/reset unassigned value.
    pub const fn is_reset_unassigned(self) -> bool {
        self.core0.is_reset_unassigned() && self.core1.is_reset_unassigned()
    }

    /// Return whether all documented fields on both cores are at reset.
    pub const fn is_full_reset(self) -> bool {
        self.core0.is_full_reset() && self.core1.is_full_reset()
    }
}

/// One source-confirmed IEEE 802.15.4 MAC event bit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
pub enum Ieee802154Event {
    /// A transmission completed.
    TxDone = 1 << 0,
    /// A reception completed.
    RxDone = 1 << 1,
    /// Automatic ACK transmission completed.
    AckTxDone = 1 << 2,
    /// ACK reception completed.
    AckRxDone = 1 << 3,
    /// Receive processing aborted.
    RxAbort = 1 << 4,
    /// Transmit processing aborted.
    TxAbort = 1 << 5,
    /// Energy detection completed.
    EdDone = 1 << 6,
    /// Timer zero overflowed.
    Timer0Overflow = 1 << 8,
    /// Timer one overflowed.
    Timer1Overflow = 1 << 9,
    /// The MAC clock counter matched its configured value.
    ///
    /// This event is named by the public LL but is not handled by the reviewed
    /// vendor ISR, so dispatch and quiesced plans reject it.
    ClockCountMatch = 1 << 10,
    /// Transmission SFD processing completed.
    TxSfdDone = 1 << 11,
    /// Reception SFD processing completed.
    RxSfdDone = 1 << 12,
}

impl Ieee802154Event {
    /// Return this event's single-bit register image.
    pub const fn bit(self) -> u16 {
        self as u16
    }

    /// Return a validated mask containing only this event.
    pub const fn mask(self) -> Ieee802154EventMask {
        Ieee802154EventMask(self.bit())
    }
}

/// All event bits named by the pinned public low-level header.
pub const NAMED_EVENT_BITS: u16 = 0x1f7f;
/// Event bits consumed by the reviewed vendor ISR.
pub const VENDOR_HANDLED_EVENT_BITS: u16 = 0x1b7f;
/// Raw vendor initialization image after timer zero is initially masked.
///
/// This image includes unnamed bits 7 and 13 and the named-but-unhandled
/// clock-count event, so it cannot form a fail-closed [`Ieee802154EventMask`].
pub const VENDOR_RAW_INIT_NO_TIMER0_EVENT_BITS: u16 = 0x3eff;
/// Handled subset of the raw vendor image with timer zero initially masked.
///
/// This is exactly
/// `VENDOR_RAW_INIT_NO_TIMER0_EVENT_BITS & VENDOR_HANDLED_EVENT_BITS`.
pub const HANDLED_BASELINE_NO_TIMER0_EVENT_BITS: u16 = 0x1a7f;

/// Failure to construct an event mask from unclassified bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventMaskError {
    unsupported_bits: u16,
}

impl Ieee802154EventMaskError {
    /// Return every input bit not named by the pinned public LL.
    pub const fn unsupported_bits(self) -> u16 {
        self.unsupported_bits
    }
}

/// A mask containing only source-confirmed, named MAC event bits.
///
/// Physical field bits 7 and 13 have no public meaning and cannot be
/// represented. Bits above the fourteen-bit field are rejected as well.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventMask(u16);

impl Ieee802154EventMask {
    /// An empty event mask.
    pub const NONE: Self = Self(0);
    /// All named events, including the undispatchable clock-count event.
    pub const NAMED: Self = Self(NAMED_EVENT_BITS);
    /// Every event handled by the reviewed vendor ISR.
    pub const VENDOR_HANDLED: Self = Self(VENDOR_HANDLED_EVENT_BITS);
    /// The fail-closed, handled subset of the raw vendor initialization image.
    pub const HANDLED_BASELINE_NO_TIMER0: Self = Self(HANDLED_BASELINE_NO_TIMER0_EVENT_BITS);

    /// Validate a raw field image against the source-confirmed named bits.
    pub const fn from_named_bits(bits: u16) -> Result<Self, Ieee802154EventMaskError> {
        let unsupported_bits = bits & !NAMED_EVENT_BITS;
        if unsupported_bits == 0 {
            Ok(Self(bits))
        } else {
            Err(Ieee802154EventMaskError { unsupported_bits })
        }
    }

    /// Return the fourteen-bit field image.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Return whether this mask contains an event.
    pub const fn contains(self, event: Ieee802154Event) -> bool {
        self.0 & event.bit() != 0
    }

    /// Combine two already validated named-event masks.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl TryFrom<u16> for Ieee802154EventMask {
    type Error = Ieee802154EventMaskError;

    fn try_from(bits: u16) -> Result<Self, Self::Error> {
        Self::from_named_bits(bits)
    }
}

impl From<Ieee802154Event> for Ieee802154EventMask {
    fn from(event: Ieee802154Event) -> Self {
        event.mask()
    }
}

/// One named receive-abort reason code from the pinned public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154RxAbortReason {
    /// Receive stop command.
    RxStop = 1,
    /// SFD timeout.
    SfdTimeout = 2,
    /// CRC failure.
    CrcError = 3,
    /// Invalid frame length.
    InvalidLength = 4,
    /// Address/filter rejection.
    FilterFail = 5,
    /// RSS was not detected.
    NoRss = 6,
    /// Coexistence interrupted reception.
    CoexistenceBreak = 7,
    /// An ACK was received unexpectedly.
    UnexpectedAck = 8,
    /// Receive processing restarted.
    RxRestart = 9,
    /// ACK transmission timed out.
    TxAckTimeout = 16,
    /// ACK transmission was stopped.
    TxAckStop = 17,
    /// Coexistence interrupted ACK transmission.
    TxAckCoexistenceBreak = 18,
    /// Enhanced-ACK security processing failed.
    EnhancedAckSecurityError = 19,
    /// Energy detection was aborted.
    EdAbort = 24,
    /// Energy detection was stopped.
    EdStop = 25,
    /// Coexistence rejected energy detection.
    EdCoexistenceReject = 26,
}

impl Ieee802154RxAbortReason {
    /// Return the source-confirmed reason code sampled from `RX_STATUS[8:4]`.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Return `BIT(reason - 1)` for `RX_ABORT_INTR_CTRL`.
    pub const fn enable_bit(self) -> u32 {
        1u32 << (self.code() - 1)
    }

    /// Return a validated mask containing only this reason.
    pub const fn mask(self) -> Ieee802154RxAbortMask {
        Ieee802154RxAbortMask(self.enable_bit())
    }
}

/// One named transmit-abort reason code from the pinned public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154TxAbortReason {
    /// ACK reception was stopped.
    RxAckStop = 1,
    /// ACK SFD timed out.
    RxAckSfdTimeout = 2,
    /// The received ACK had a CRC failure.
    RxAckCrcError = 3,
    /// The received ACK had an invalid length.
    RxAckInvalidLength = 4,
    /// The received ACK failed filtering.
    RxAckFilterFail = 5,
    /// RSS was not detected for the ACK.
    RxAckNoRss = 6,
    /// Coexistence interrupted ACK reception.
    RxAckCoexistenceBreak = 7,
    /// The received frame was not an ACK.
    RxAckTypeNotAck = 8,
    /// ACK receive processing restarted.
    RxAckRestart = 9,
    /// ACK reception timed out.
    RxAckTimeout = 16,
    /// Transmission was stopped.
    TxStop = 17,
    /// Coexistence interrupted transmission.
    TxCoexistenceBreak = 18,
    /// Transmission security processing failed.
    TxSecurityError = 19,
    /// CCA failed.
    CcaFailed = 24,
    /// CCA observed a busy channel.
    CcaBusy = 25,
}

impl Ieee802154TxAbortReason {
    /// Return the source-confirmed reason code sampled from `TX_STATUS[8:4]`.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Return `BIT(reason - 1)` for `TX_ABORT_INTERRUPT_CONTROL`.
    pub const fn enable_bit(self) -> u32 {
        1u32 << (self.code() - 1)
    }

    /// Return a validated mask containing only this reason.
    pub const fn mask(self) -> Ieee802154TxAbortMask {
        Ieee802154TxAbortMask(self.enable_bit())
    }
}

const NAMED_RX_ABORT_BITS: u32 = 0x0387_81ff;
const NAMED_TX_ABORT_BITS: u32 = 0x0187_81ff;

/// Vendor receive-abort baseline: reason codes 16 and 18.
pub const VENDOR_RX_ABORT_BASELINE_BITS: u32 = 0x0002_8000;
/// Vendor transmit-abort baseline: reason codes 16, 18, 19, 24, and 25.
pub const VENDOR_TX_ABORT_BASELINE_BITS: u32 = 0x0186_8000;

/// Failure to construct an abort mask from unnamed reason bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154AbortMaskError {
    unsupported_bits: u32,
}

impl Ieee802154AbortMaskError {
    /// Return every input bit without a source-confirmed reason code.
    pub const fn unsupported_bits(self) -> u32 {
        self.unsupported_bits
    }
}

/// A receive-abort enable mask containing only named reason bits.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154RxAbortMask(u32);

impl Ieee802154RxAbortMask {
    /// An empty receive-abort mask.
    pub const NONE: Self = Self(0);
    /// The source-confirmed vendor initialization mask.
    pub const VENDOR_BASELINE: Self = Self(VENDOR_RX_ABORT_BASELINE_BITS);

    /// Validate an enable image against the named receive-abort reasons.
    pub const fn from_named_bits(bits: u32) -> Result<Self, Ieee802154AbortMaskError> {
        let unsupported_bits = bits & !NAMED_RX_ABORT_BITS;
        if unsupported_bits == 0 {
            Ok(Self(bits))
        } else {
            Err(Ieee802154AbortMaskError { unsupported_bits })
        }
    }

    /// Return the low thirty-one-bit enable image.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Return whether this mask contains a reason.
    pub const fn contains(self, reason: Ieee802154RxAbortReason) -> bool {
        self.0 & reason.enable_bit() != 0
    }

    /// Combine two already validated receive-abort masks.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl TryFrom<u32> for Ieee802154RxAbortMask {
    type Error = Ieee802154AbortMaskError;

    fn try_from(bits: u32) -> Result<Self, Self::Error> {
        Self::from_named_bits(bits)
    }
}

impl From<Ieee802154RxAbortReason> for Ieee802154RxAbortMask {
    fn from(reason: Ieee802154RxAbortReason) -> Self {
        reason.mask()
    }
}

/// A transmit-abort enable mask containing only named reason bits.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154TxAbortMask(u32);

impl Ieee802154TxAbortMask {
    /// An empty transmit-abort mask.
    pub const NONE: Self = Self(0);
    /// The source-confirmed vendor initialization mask.
    pub const VENDOR_BASELINE: Self = Self(VENDOR_TX_ABORT_BASELINE_BITS);

    /// Validate an enable image against the named transmit-abort reasons.
    pub const fn from_named_bits(bits: u32) -> Result<Self, Ieee802154AbortMaskError> {
        let unsupported_bits = bits & !NAMED_TX_ABORT_BITS;
        if unsupported_bits == 0 {
            Ok(Self(bits))
        } else {
            Err(Ieee802154AbortMaskError { unsupported_bits })
        }
    }

    /// Return the low thirty-one-bit enable image.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Return whether this mask contains a reason.
    pub const fn contains(self, reason: Ieee802154TxAbortReason) -> bool {
        self.0 & reason.enable_bit() != 0
    }

    /// Combine two already validated transmit-abort masks.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl TryFrom<u32> for Ieee802154TxAbortMask {
    type Error = Ieee802154AbortMaskError;

    fn try_from(bits: u32) -> Result<Self, Self::Error> {
        Self::from_named_bits(bits)
    }
}

impl From<Ieee802154TxAbortReason> for Ieee802154TxAbortMask {
    fn from(reason: Ieee802154TxAbortReason) -> Self {
        reason.mask()
    }
}

/// Failure to form a quiesced plan from events absent from the reviewed ISR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuiescedIrqPlanError {
    unsupported_event_bits: u16,
}

impl QuiescedIrqPlanError {
    /// Return named event bits for which this crate has no dispatch contract.
    pub const fn unsupported_event_bits(self) -> u16 {
        self.unsupported_event_bits
    }
}

/// One non-executable desired-mask operation in reviewed vendor order.
///
/// These variants contain validated semantic masks, not register capabilities.
/// Iterating them neither writes MMIO nor proves that any target observed them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuiescedIrqMaskStep {
    /// OR named handled events into `EVENT_ENABLE`.
    UnionEventEnable(Ieee802154EventMask),
    /// OR named transmit-abort reasons into `TX_ABORT_ENABLE`.
    UnionTxAbortEnable(Ieee802154TxAbortMask),
    /// OR named receive-abort reasons into `RX_ABORT_ENABLE`.
    UnionRxAbortEnable(Ieee802154RxAbortMask),
}

/// A validated desired-mask plan that cannot activate an interrupt.
///
/// This value has no hardware capability. It records only the field images a
/// future, separately audited owner may install while the route is quiesced.
/// The named-but-unhandled clock-count event is rejected.
///
/// There is deliberately no activation transition:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::QuiescedIrqPlan;
///
/// let plan = QuiescedIrqPlan::handled_baseline_without_timer0();
/// plan.activate();
/// ```
///
/// `EVENT_STATUS` is deliberately absent as well:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::QuiescedIrqPlan;
///
/// let plan = QuiescedIrqPlan::handled_baseline_without_timer0();
/// let _ = plan.event_status();
/// ```
///
/// No downstream backend or executable attempt is exported:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::QuiescedIrqMaskInstallAttempt;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuiescedIrqPlan {
    events: Ieee802154EventMask,
    rx_aborts: Ieee802154RxAbortMask,
    tx_aborts: Ieee802154TxAbortMask,
}

impl QuiescedIrqPlan {
    /// Validate desired masks without creating an active IRQ capability.
    pub const fn new(
        events: Ieee802154EventMask,
        rx_aborts: Ieee802154RxAbortMask,
        tx_aborts: Ieee802154TxAbortMask,
    ) -> Result<Self, QuiescedIrqPlanError> {
        let unsupported_event_bits = events.bits() & !VENDOR_HANDLED_EVENT_BITS;
        if unsupported_event_bits == 0 {
            Ok(Self {
                events,
                rx_aborts,
                tx_aborts,
            })
        } else {
            Err(QuiescedIrqPlanError {
                unsupported_event_bits,
            })
        }
    }

    /// Return a fail-closed plan derived from vendor initialization.
    ///
    /// ESP-IDF's raw event-enable image after initially masking timer zero is
    /// `0x3eff`. This plan intersects it with the events consumed by the
    /// reviewed ISR, excluding unnamed bits 7 and 13 plus the named-but-
    /// unhandled clock-count event. The RX/TX abort masks remain the exact
    /// vendor initialization images.
    pub const fn handled_baseline_without_timer0() -> Self {
        Self {
            events: Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0,
            rx_aborts: Ieee802154RxAbortMask::VENDOR_BASELINE,
            tx_aborts: Ieee802154TxAbortMask::VENDOR_BASELINE,
        }
    }

    /// Return the desired event-enable image.
    pub const fn events(self) -> Ieee802154EventMask {
        self.events
    }

    /// Return the desired receive-abort-enable image.
    pub const fn rx_aborts(self) -> Ieee802154RxAbortMask {
        self.rx_aborts
    }

    /// Return the desired transmit-abort-enable image.
    pub const fn tx_aborts(self) -> Ieee802154TxAbortMask {
        self.tx_aborts
    }

    /// Return the exact non-executable vendor OR-operation order.
    pub const fn ordered_mask_steps(self) -> [QuiescedIrqMaskStep; 3] {
        [
            QuiescedIrqMaskStep::UnionEventEnable(self.events),
            QuiescedIrqMaskStep::UnionTxAbortEnable(self.tx_aborts),
            QuiescedIrqMaskStep::UnionRxAbortEnable(self.rx_aborts),
        ]
    }

    /// Apply the pure required-bit predicate to caller-supplied observations.
    ///
    /// Success is `()` rather than a token: it says only that the three numeric
    /// observations contain the plan bits. It is not evidence that the values
    /// came from MMIO, that unrelated bits are safe, or that an IRQ route, ISR,
    /// acknowledgement operation, PHY/RF state, or ready MAC exists.
    /// Additional observed bits are accepted because the source operations are
    /// OR assignments rather than complete-register replacements.
    pub const fn verify_required_readback(
        self,
        observed_event_enable: u16,
        observed_tx_abort_enable: u32,
        observed_rx_abort_enable: u32,
    ) -> Result<(), QuiescedIrqMaskReadbackError> {
        match require_bits(
            QuiescedIrqMaskReadbackCheckpoint::EventEnable,
            self.events.bits() as u32,
            observed_event_enable as u32,
        ) {
            Err(error) => Err(error),
            Ok(()) => match require_bits(
                QuiescedIrqMaskReadbackCheckpoint::TxAbortEnable,
                self.tx_aborts.bits(),
                observed_tx_abort_enable,
            ) {
                Err(error) => Err(error),
                Ok(()) => require_bits(
                    QuiescedIrqMaskReadbackCheckpoint::RxAbortEnable,
                    self.rx_aborts.bits(),
                    observed_rx_abort_enable,
                ),
            },
        }
    }
}

/// Staged consuming port for the crate-private quiesced transaction model.
///
/// The three mutation methods represent the reviewed vendor OR assignments,
/// not complete-register replacement. Implementations must preserve every bit
/// outside the supplied validated mask and must keep the CPU interrupt route
/// quiesced for the entire transaction. The ordering method is only a Rust and
/// device-access boundary before readback; it does not claim instruction-level
/// equivalence with the vendor implementation.
///
/// There is deliberately no production implementation yet. The only current
/// implementation is the test fake below; downstream crates cannot implement
/// or name this trait and therefore cannot mint an execution result.
#[cfg(test)]
trait QuiescedIrqMaskPort {
    /// OR the validated named-event subset into `EVENT_ENABLE`.
    fn union_event_enable(&mut self, events: Ieee802154EventMask);

    /// OR the validated transmit-abort subset into `TX_ABORT_ENABLE`.
    fn union_tx_abort_enable(&mut self, aborts: Ieee802154TxAbortMask);

    /// OR the validated receive-abort subset into `RX_ABORT_ENABLE`.
    fn union_rx_abort_enable(&mut self, aborts: Ieee802154RxAbortMask);

    /// Order the three mask writes before the following readbacks.
    fn order_mask_writes_before_readback(&mut self);

    /// Read the complete fourteen-bit `EVENT_ENABLE` field image.
    fn event_enable_bits(&mut self) -> u16;

    /// Read the complete low thirty-one-bit `TX_ABORT_ENABLE` field image.
    fn tx_abort_enable_bits(&mut self) -> u32;

    /// Read the complete low thirty-one-bit `RX_ABORT_ENABLE` field image.
    fn rx_abort_enable_bits(&mut self) -> u32;
}

/// Readback checkpoint which did not contain every required plan bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuiescedIrqMaskReadbackCheckpoint {
    /// `EVENT_ENABLE` omitted at least one required named event.
    EventEnable,
    /// `TX_ABORT_ENABLE` omitted at least one required named reason.
    TxAbortEnable,
    /// `RX_ABORT_ENABLE` omitted at least one required named reason.
    RxAbortEnable,
}

/// One fail-closed required-bit readback mismatch.
///
/// Additional observed bits do not fail this check because the reviewed vendor
/// operations are OR assignments and the register reset images are not part of
/// the current evidence set. Such additional bits are observations only; this
/// error type does not classify or authorize them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuiescedIrqMaskReadbackError {
    checkpoint: QuiescedIrqMaskReadbackCheckpoint,
    required_bits: u32,
    observed_bits: u32,
}

impl QuiescedIrqMaskReadbackError {
    /// Return the first failed readback checkpoint in vendor write order.
    pub const fn checkpoint(self) -> QuiescedIrqMaskReadbackCheckpoint {
        self.checkpoint
    }

    /// Return the bits required by the validated plan at this checkpoint.
    pub const fn required_bits(self) -> u32 {
        self.required_bits
    }

    /// Return the complete caller-supplied observed field image.
    pub const fn observed_bits(self) -> u32 {
        self.observed_bits
    }

    /// Return exactly the required bits absent from the observed image.
    pub const fn missing_bits(self) -> u32 {
        self.required_bits & !self.observed_bits
    }
}

/// Unique owner of one prepared quiesced mask-install transaction.
///
/// Construction performs no port operation. [`Self::execute`] consumes the
/// owner and applies the reviewed order: event enables, transmit-abort enables,
/// receive-abort enables, ordering boundary, then subset readback.
#[must_use = "the quiesced mask attempt owns its port until execution or abandonment"]
#[cfg(test)]
struct QuiescedIrqMaskInstallAttempt<Port> {
    port: Port,
    plan: QuiescedIrqPlan,
}

#[cfg(test)]
impl<Port> QuiescedIrqMaskInstallAttempt<Port> {
    /// Bind a validated plan to the exact consuming semantic port owner.
    ///
    /// This constructor creates no target or hardware proof. In particular, a
    /// caller-supplied model port remains only a model port.
    const fn new(port: Port, plan: QuiescedIrqPlan) -> Self {
        Self { port, plan }
    }
}

#[cfg(test)]
impl<Port: QuiescedIrqMaskPort> QuiescedIrqMaskInstallAttempt<Port> {
    /// Execute the bounded quiesced mask transaction through the owned port.
    ///
    /// A readback succeeds when it contains every required bit; unrelated bits
    /// are preserved and do not turn the vendor OR operation into an equality
    /// claim. Any mismatch happens after the first possible write and therefore
    /// returns a terminal opaque failure which retains the port and exposes no
    /// retry, rollback, or decomposition method.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free terminal failure retains the exact port owner and plan"
    )]
    fn execute(
        mut self,
    ) -> Result<QuiescedIrqDesiredMaskModel<Port>, QuiescedIrqDesiredMaskFailure<Port>> {
        self.port.union_event_enable(self.plan.events());
        self.port.union_tx_abort_enable(self.plan.tx_aborts());
        self.port.union_rx_abort_enable(self.plan.rx_aborts());
        self.port.order_mask_writes_before_readback();

        let event_enable = self.port.event_enable_bits();
        let tx_abort_enable = self.port.tx_abort_enable_bits();
        let rx_abort_enable = self.port.rx_abort_enable_bits();

        match self
            .plan
            .verify_required_readback(event_enable, tx_abort_enable, rx_abort_enable)
        {
            Ok(()) => Ok(QuiescedIrqDesiredMaskModel {
                _port: self.port,
                plan: self.plan,
            }),
            Err(error) => Err(QuiescedIrqDesiredMaskFailure {
                _port: self.port,
                plan: self.plan,
                error,
            }),
        }
    }
}

/// Crate-private desired-mask model after a successful subset readback.
///
/// This type retains the exact port owner and has no decomposition method. It
/// records only that this internal model reported the plan's required bits. It
/// is not evidence of target MMIO execution, an interrupt route,
/// `EVENT_STATUS` access, an ISR epoch, PHY/RF qualification, or readiness.
#[must_use = "the desired-mask model retains the exact quiesced port owner"]
#[cfg(test)]
struct QuiescedIrqDesiredMaskModel<Port> {
    _port: Port,
    plan: QuiescedIrqPlan,
}

#[cfg(test)]
impl<Port> QuiescedIrqDesiredMaskModel<Port> {
    /// Return the desired plan accepted by the internal subset model.
    const fn plan(&self) -> QuiescedIrqPlan {
        self.plan
    }
}

#[cfg(test)]
impl<Port> fmt::Debug for QuiescedIrqDesiredMaskModel<Port> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuiescedIrqDesiredMaskModel")
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

/// Terminal failed quiesced mask transaction retaining the exact port owner.
///
/// At least the event-enable OR operation may already have reached the port.
/// Consequently this type exposes neither retry nor rollback nor its port.
#[must_use = "the terminal failure retains a possibly partially modified port owner"]
#[cfg(test)]
struct QuiescedIrqDesiredMaskFailure<Port> {
    _port: Port,
    plan: QuiescedIrqPlan,
    error: QuiescedIrqMaskReadbackError,
}

#[cfg(test)]
impl<Port> QuiescedIrqDesiredMaskFailure<Port> {
    /// Return the first required-bit readback mismatch.
    const fn error(&self) -> QuiescedIrqMaskReadbackError {
        self.error
    }

    /// Borrow the immutable desired plan without exposing the retained port.
    const fn plan(&self) -> QuiescedIrqPlan {
        self.plan
    }
}

#[cfg(test)]
impl<Port> fmt::Debug for QuiescedIrqDesiredMaskFailure<Port> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuiescedIrqDesiredMaskFailure")
            .field("plan", &self.plan)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

const fn require_bits(
    checkpoint: QuiescedIrqMaskReadbackCheckpoint,
    required_bits: u32,
    observed_bits: u32,
) -> Result<(), QuiescedIrqMaskReadbackError> {
    if observed_bits & required_bits == required_bits {
        Ok(())
    } else {
        Err(QuiescedIrqMaskReadbackError {
            checkpoint,
            required_bits,
            observed_bits,
        })
    }
}

/// One callback position in the reviewed vendor event-dispatch order.
///
/// The closed enum makes the two receive-abort phases explicit and makes no
/// claim about status acknowledgement or next-operation policy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154DispatchedEvent {
    /// Receive-abort processing before SFD and completion events.
    RxAbortPhase1,
    /// RX SFD completion.
    RxSfdDone,
    /// TX SFD completion.
    TxSfdDone,
    /// TX completion.
    TxDone,
    /// RX completion.
    RxDone,
    /// ACK TX completion.
    AckTxDone,
    /// ACK RX completion.
    AckRxDone,
    /// Receive-abort processing after ACK completion events.
    RxAbortPhase2,
    /// TX abort processing.
    TxAbort,
    /// Energy-detection completion.
    EdDone,
    /// Timer-zero overflow.
    Timer0Overflow,
    /// Timer-one overflow.
    Timer1Overflow,
}

/// Consumer of the closed, pure event-dispatch sequence.
pub trait Ieee802154EventSink {
    /// Observe one source-confirmed callback position.
    fn on_event(&mut self, event: Ieee802154DispatchedEvent);
}

/// Failure to dispatch a batch containing a named but unsupported event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154DispatchError {
    unsupported_event_bits: u16,
}

impl Ieee802154DispatchError {
    /// Return every bit rejected before dispatch began.
    pub const fn unsupported_event_bits(self) -> u16 {
        self.unsupported_event_bits
    }
}

/// Dispatch one already sampled event batch in reviewed vendor order.
///
/// Validation is transactional: if any named event lacks a reviewed handler,
/// the function returns before invoking the sink. This pure function neither
/// reads nor acknowledges `EVENT_STATUS` and does not run a next-operation
/// policy.
pub fn dispatch_event_batch<S: Ieee802154EventSink + ?Sized>(
    batch: Ieee802154EventMask,
    sink: &mut S,
) -> Result<(), Ieee802154DispatchError> {
    let unsupported_event_bits = batch.bits() & !VENDOR_HANDLED_EVENT_BITS;
    if unsupported_event_bits != 0 {
        return Err(Ieee802154DispatchError {
            unsupported_event_bits,
        });
    }

    if batch.contains(Ieee802154Event::RxAbort) {
        sink.on_event(Ieee802154DispatchedEvent::RxAbortPhase1);
    }
    if batch.contains(Ieee802154Event::RxSfdDone) {
        sink.on_event(Ieee802154DispatchedEvent::RxSfdDone);
    }
    if batch.contains(Ieee802154Event::TxSfdDone) {
        sink.on_event(Ieee802154DispatchedEvent::TxSfdDone);
    }
    if batch.contains(Ieee802154Event::TxDone) {
        sink.on_event(Ieee802154DispatchedEvent::TxDone);
    }
    if batch.contains(Ieee802154Event::RxDone) {
        sink.on_event(Ieee802154DispatchedEvent::RxDone);
    }
    if batch.contains(Ieee802154Event::AckTxDone) {
        sink.on_event(Ieee802154DispatchedEvent::AckTxDone);
    }
    if batch.contains(Ieee802154Event::AckRxDone) {
        sink.on_event(Ieee802154DispatchedEvent::AckRxDone);
    }
    if batch.contains(Ieee802154Event::RxAbort) {
        sink.on_event(Ieee802154DispatchedEvent::RxAbortPhase2);
    }
    if batch.contains(Ieee802154Event::TxAbort) {
        sink.on_event(Ieee802154DispatchedEvent::TxAbort);
    }
    if batch.contains(Ieee802154Event::EdDone) {
        sink.on_event(Ieee802154DispatchedEvent::EdDone);
    }
    if batch.contains(Ieee802154Event::Timer0Overflow) {
        sink.on_event(Ieee802154DispatchedEvent::Timer0Overflow);
    }
    if batch.contains(Ieee802154Event::Timer1Overflow) {
        sink.on_event(Ieee802154DispatchedEvent::Timer1Overflow);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::*;

    const RX_REASONS: [Ieee802154RxAbortReason; 16] = [
        Ieee802154RxAbortReason::RxStop,
        Ieee802154RxAbortReason::SfdTimeout,
        Ieee802154RxAbortReason::CrcError,
        Ieee802154RxAbortReason::InvalidLength,
        Ieee802154RxAbortReason::FilterFail,
        Ieee802154RxAbortReason::NoRss,
        Ieee802154RxAbortReason::CoexistenceBreak,
        Ieee802154RxAbortReason::UnexpectedAck,
        Ieee802154RxAbortReason::RxRestart,
        Ieee802154RxAbortReason::TxAckTimeout,
        Ieee802154RxAbortReason::TxAckStop,
        Ieee802154RxAbortReason::TxAckCoexistenceBreak,
        Ieee802154RxAbortReason::EnhancedAckSecurityError,
        Ieee802154RxAbortReason::EdAbort,
        Ieee802154RxAbortReason::EdStop,
        Ieee802154RxAbortReason::EdCoexistenceReject,
    ];

    const TX_REASONS: [Ieee802154TxAbortReason; 15] = [
        Ieee802154TxAbortReason::RxAckStop,
        Ieee802154TxAbortReason::RxAckSfdTimeout,
        Ieee802154TxAbortReason::RxAckCrcError,
        Ieee802154TxAbortReason::RxAckInvalidLength,
        Ieee802154TxAbortReason::RxAckFilterFail,
        Ieee802154TxAbortReason::RxAckNoRss,
        Ieee802154TxAbortReason::RxAckCoexistenceBreak,
        Ieee802154TxAbortReason::RxAckTypeNotAck,
        Ieee802154TxAbortReason::RxAckRestart,
        Ieee802154TxAbortReason::RxAckTimeout,
        Ieee802154TxAbortReason::TxStop,
        Ieee802154TxAbortReason::TxCoexistenceBreak,
        Ieee802154TxAbortReason::TxSecurityError,
        Ieee802154TxAbortReason::CcaFailed,
        Ieee802154TxAbortReason::CcaBusy,
    ];

    #[test]
    fn source_and_route_geometry_are_exact() {
        assert_eq!(IEEE802154_MAC_INTERRUPT_SOURCE.number(), 132);
        assert_eq!(Ieee802154RouteGeometry::MAP_REGISTER_OFFSET, 0x210);
        assert_eq!(Ieee802154RouteGeometry::CORE_REGISTER_STRIDE, 0x800);
        assert_eq!(Ieee802154RouteGeometry::MAP_FIELD_MASK, 0x3f);
        assert_eq!(Ieee802154RouteGeometry::PASS_LEVEL_SHIFT, 8);
        assert_eq!(Ieee802154RouteGeometry::PASS_LEVEL_MASK, 0x300);
        assert_eq!(Ieee802154RouteGeometry::DOCUMENTED_FIELD_MASK, 0x33f);
        assert_eq!(
            Ieee802154RouteGeometry::map_offset_from_core0(Ieee802154InterruptCore::Core0),
            0x210
        );
        assert_eq!(
            Ieee802154RouteGeometry::map_offset_from_core0(Ieee802154InterruptCore::Core1),
            0xa10
        );
    }

    #[test]
    fn route_readback_distinguishes_unassigned_map_from_full_reset() {
        let reset = Ieee802154RouteWord::from_raw(0);
        assert_eq!(reset.raw_bits(), 0);
        assert_eq!(reset.map(), 0);
        assert_eq!(reset.pass_level(), 0);
        assert!(reset.is_reset_unassigned());
        assert!(reset.is_full_reset());

        let remapped_unassigned = Ieee802154RouteWord::from_raw(0x200);
        assert_eq!(remapped_unassigned.map(), 0);
        assert_eq!(remapped_unassigned.pass_level(), 2);
        assert!(remapped_unassigned.is_reset_unassigned());
        assert!(!remapped_unassigned.is_full_reset());

        let allocator_disabled_destination = Ieee802154RouteWord::from_raw(6);
        assert_eq!(allocator_disabled_destination.map(), 6);
        assert!(!allocator_disabled_destination.is_reset_unassigned());
        assert!(!allocator_disabled_destination.is_full_reset());

        let reserved_bits_only = Ieee802154RouteWord::from_raw(!0x33f);
        assert_eq!(reserved_bits_only.documented_bits(), 0);
        assert!(!reserved_bits_only.is_full_reset());

        for bit in 0..u32::BITS {
            assert!(!Ieee802154RouteWord::from_raw(1 << bit).is_full_reset());
        }
    }

    #[test]
    fn dual_route_readback_preserves_core_identity_and_requires_both_cores() {
        let reset = Ieee802154RouteWord::from_raw(0);
        let core1_active = Ieee802154RouteWord::from_raw(0x103);
        let readback = Ieee802154RouteReadback::new(reset, core1_active);

        assert_eq!(readback.core0(), reset);
        assert_eq!(readback.core1(), core1_active);
        assert!(!readback.is_reset_unassigned());
        assert!(!readback.is_full_reset());

        let reset_both = Ieee802154RouteReadback::new(reset, reset);
        assert!(reset_both.is_reset_unassigned());
        assert!(reset_both.is_full_reset());
    }

    #[test]
    fn event_masks_match_the_reviewed_sets() {
        assert_eq!(Ieee802154EventMask::NAMED.bits(), 0x1f7f);
        assert_eq!(Ieee802154EventMask::VENDOR_HANDLED.bits(), 0x1b7f);
        assert_eq!(
            Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0.bits(),
            0x1a7f
        );
        assert_eq!(VENDOR_RAW_INIT_NO_TIMER0_EVENT_BITS, 0x3eff);
        assert_eq!(
            VENDOR_RAW_INIT_NO_TIMER0_EVENT_BITS & VENDOR_HANDLED_EVENT_BITS,
            HANDLED_BASELINE_NO_TIMER0_EVENT_BITS
        );
        assert_eq!(
            VENDOR_RAW_INIT_NO_TIMER0_EVENT_BITS & !NAMED_EVENT_BITS,
            (1 << 7) | (1 << 13)
        );
        assert_eq!(
            VENDOR_RAW_INIT_NO_TIMER0_EVENT_BITS & NAMED_EVENT_BITS & !VENDOR_HANDLED_EVENT_BITS,
            Ieee802154Event::ClockCountMatch.bit()
        );
        assert!(Ieee802154EventMask::NAMED.contains(Ieee802154Event::ClockCountMatch));
        assert!(!Ieee802154EventMask::VENDOR_HANDLED.contains(Ieee802154Event::ClockCountMatch));
        assert!(Ieee802154EventMask::VENDOR_HANDLED.contains(Ieee802154Event::Timer0Overflow));
        assert!(
            !Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0
                .contains(Ieee802154Event::Timer0Overflow)
        );
    }

    #[test]
    fn event_mask_constructor_rejects_unnamed_and_out_of_field_bits() {
        for bit in [7u32, 13, 14, 15] {
            let raw = 1u16 << bit;
            assert_eq!(
                Ieee802154EventMask::from_named_bits(raw),
                Err(Ieee802154EventMaskError {
                    unsupported_bits: raw,
                })
            );
        }
        assert_eq!(
            Ieee802154EventMask::from_named_bits(NAMED_EVENT_BITS),
            Ok(Ieee802154EventMask::NAMED)
        );
    }

    #[test]
    fn receive_abort_reason_mapping_is_exhaustive_and_bounded() {
        let expected_codes = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 24, 25, 26];
        let mut combined = 0u32;
        for (reason, expected_code) in RX_REASONS.into_iter().zip(expected_codes) {
            assert_eq!(reason.code(), expected_code);
            assert_eq!(reason.enable_bit(), 1u32 << (u32::from(expected_code) - 1));
            assert_eq!(combined & reason.enable_bit(), 0);
            combined |= reason.enable_bit();
        }
        assert_eq!(combined, NAMED_RX_ABORT_BITS);
        assert_eq!(combined & !0x7fff_ffff, 0);
        assert!(Ieee802154RxAbortMask::from_named_bits(combined).is_ok());
        assert_eq!(
            Ieee802154RxAbortMask::from_named_bits(1 << 9)
                .expect_err("reason code 10 is unnamed")
                .unsupported_bits(),
            1 << 9
        );
        assert_eq!(
            Ieee802154RxAbortMask::from_named_bits(1 << 31)
                .expect_err("bit 31 is outside the physical field")
                .unsupported_bits(),
            1 << 31
        );
    }

    #[test]
    fn transmit_abort_reason_mapping_is_exhaustive_and_bounded() {
        let expected_codes = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19, 24, 25];
        let mut combined = 0u32;
        for (reason, expected_code) in TX_REASONS.into_iter().zip(expected_codes) {
            assert_eq!(reason.code(), expected_code);
            assert_eq!(reason.enable_bit(), 1u32 << (u32::from(expected_code) - 1));
            assert_eq!(combined & reason.enable_bit(), 0);
            combined |= reason.enable_bit();
        }
        assert_eq!(combined, NAMED_TX_ABORT_BITS);
        assert_eq!(combined & !0x7fff_ffff, 0);
        assert!(Ieee802154TxAbortMask::from_named_bits(combined).is_ok());
        assert_eq!(
            Ieee802154TxAbortMask::from_named_bits(1 << 25)
                .expect_err("reason code 26 is unnamed for TX")
                .unsupported_bits(),
            1 << 25
        );
        assert_eq!(
            Ieee802154TxAbortMask::from_named_bits(u32::MAX)
                .expect_err("unnamed and out-of-field bits must fail closed")
                .unsupported_bits(),
            !NAMED_TX_ABORT_BITS
        );
    }

    #[test]
    fn vendor_abort_baselines_are_exact_and_named() {
        assert_eq!(VENDOR_RX_ABORT_BASELINE_BITS, 0x0002_8000);
        assert_eq!(VENDOR_TX_ABORT_BASELINE_BITS, 0x0186_8000);
        assert_eq!(Ieee802154RxAbortMask::VENDOR_BASELINE.bits(), 0x0002_8000);
        assert_eq!(Ieee802154TxAbortMask::VENDOR_BASELINE.bits(), 0x0186_8000);
        assert!(
            Ieee802154RxAbortMask::VENDOR_BASELINE.contains(Ieee802154RxAbortReason::TxAckTimeout)
        );
        assert!(
            Ieee802154RxAbortMask::VENDOR_BASELINE
                .contains(Ieee802154RxAbortReason::TxAckCoexistenceBreak)
        );
        assert!(
            Ieee802154TxAbortMask::VENDOR_BASELINE.contains(Ieee802154TxAbortReason::RxAckTimeout)
        );
        assert!(
            Ieee802154TxAbortMask::VENDOR_BASELINE
                .contains(Ieee802154TxAbortReason::TxCoexistenceBreak)
        );
        assert!(
            Ieee802154TxAbortMask::VENDOR_BASELINE
                .contains(Ieee802154TxAbortReason::TxSecurityError)
        );
        assert!(
            Ieee802154TxAbortMask::VENDOR_BASELINE.contains(Ieee802154TxAbortReason::CcaFailed)
        );
        assert!(Ieee802154TxAbortMask::VENDOR_BASELINE.contains(Ieee802154TxAbortReason::CcaBusy));
    }

    #[test]
    fn quiesced_plan_rejects_named_but_unhandled_event() {
        let events = Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0
            .union(Ieee802154Event::ClockCountMatch.mask());
        let error = QuiescedIrqPlan::new(
            events,
            Ieee802154RxAbortMask::VENDOR_BASELINE,
            Ieee802154TxAbortMask::VENDOR_BASELINE,
        )
        .expect_err("clock-count event has no reviewed vendor handler");
        assert_eq!(
            error.unsupported_event_bits(),
            Ieee802154Event::ClockCountMatch.bit()
        );
    }

    #[test]
    fn quiesced_handled_plan_uses_safe_event_subset_and_exact_abort_masks() {
        let plan = QuiescedIrqPlan::handled_baseline_without_timer0();
        assert_eq!(plan.events().bits(), 0x1a7f);
        assert_eq!(plan.rx_aborts().bits(), 0x0002_8000);
        assert_eq!(plan.tx_aborts().bits(), 0x0186_8000);
        assert_eq!(
            plan.ordered_mask_steps(),
            [
                QuiescedIrqMaskStep::UnionEventEnable(
                    Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0,
                ),
                QuiescedIrqMaskStep::UnionTxAbortEnable(Ieee802154TxAbortMask::VENDOR_BASELINE,),
                QuiescedIrqMaskStep::UnionRxAbortEnable(Ieee802154RxAbortMask::VENDOR_BASELINE,),
            ]
        );
        assert_eq!(
            plan.verify_required_readback(0x1a7f, 0x0186_8000, 0x0002_8000),
            Ok(())
        );
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Vec<Ieee802154DispatchedEvent>,
    }

    impl Ieee802154EventSink for RecordingSink {
        fn on_event(&mut self, event: Ieee802154DispatchedEvent) {
            self.events.push(event);
        }
    }

    #[test]
    fn full_batch_dispatches_in_exact_vendor_order() {
        let mut sink = RecordingSink::default();
        dispatch_event_batch(Ieee802154EventMask::VENDOR_HANDLED, &mut sink)
            .expect("all events have reviewed handlers");

        assert_eq!(
            sink.events,
            [
                Ieee802154DispatchedEvent::RxAbortPhase1,
                Ieee802154DispatchedEvent::RxSfdDone,
                Ieee802154DispatchedEvent::TxSfdDone,
                Ieee802154DispatchedEvent::TxDone,
                Ieee802154DispatchedEvent::RxDone,
                Ieee802154DispatchedEvent::AckTxDone,
                Ieee802154DispatchedEvent::AckRxDone,
                Ieee802154DispatchedEvent::RxAbortPhase2,
                Ieee802154DispatchedEvent::TxAbort,
                Ieee802154DispatchedEvent::EdDone,
                Ieee802154DispatchedEvent::Timer0Overflow,
                Ieee802154DispatchedEvent::Timer1Overflow,
            ]
        );
    }

    #[test]
    fn receive_abort_dispatches_both_phases() {
        let mut sink = RecordingSink::default();
        dispatch_event_batch(Ieee802154Event::RxAbort.mask(), &mut sink)
            .expect("RX abort is handled");
        assert_eq!(
            sink.events,
            [
                Ieee802154DispatchedEvent::RxAbortPhase1,
                Ieee802154DispatchedEvent::RxAbortPhase2,
            ]
        );
    }

    #[test]
    fn unsupported_batch_is_rejected_before_any_callback() {
        let mut sink = RecordingSink::default();
        let error = dispatch_event_batch(Ieee802154Event::ClockCountMatch.mask(), &mut sink)
            .expect_err("clock-count event is named but has no reviewed handler");
        assert_eq!(
            error.unsupported_event_bits(),
            Ieee802154Event::ClockCountMatch.bit()
        );
        assert!(sink.events.is_empty());
    }

    #[test]
    fn empty_batch_is_a_noop() {
        let mut sink = RecordingSink::default();
        dispatch_event_batch(Ieee802154EventMask::NONE, &mut sink)
            .expect("empty batch is supported");
        assert!(sink.events.is_empty());
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MaskPortOperation {
        UnionEvents(u16),
        UnionTxAborts(u32),
        UnionRxAborts(u32),
        OrderBeforeReadback,
        ReadEvents,
        ReadTxAborts,
        ReadRxAborts,
    }

    struct RecordingMaskPort {
        identity: u8,
        event_enable: u16,
        tx_abort_enable: u32,
        rx_abort_enable: u32,
        reported_events: Option<u16>,
        reported_tx_aborts: Option<u32>,
        reported_rx_aborts: Option<u32>,
        operations: Vec<MaskPortOperation>,
    }

    impl RecordingMaskPort {
        fn new(
            identity: u8,
            event_enable: u16,
            tx_abort_enable: u32,
            rx_abort_enable: u32,
        ) -> Self {
            Self {
                identity,
                event_enable,
                tx_abort_enable,
                rx_abort_enable,
                reported_events: None,
                reported_tx_aborts: None,
                reported_rx_aborts: None,
                operations: Vec::new(),
            }
        }
    }

    impl QuiescedIrqMaskPort for RecordingMaskPort {
        fn union_event_enable(&mut self, events: Ieee802154EventMask) {
            self.operations
                .push(MaskPortOperation::UnionEvents(events.bits()));
            self.event_enable |= events.bits();
        }

        fn union_tx_abort_enable(&mut self, aborts: Ieee802154TxAbortMask) {
            self.operations
                .push(MaskPortOperation::UnionTxAborts(aborts.bits()));
            self.tx_abort_enable |= aborts.bits();
        }

        fn union_rx_abort_enable(&mut self, aborts: Ieee802154RxAbortMask) {
            self.operations
                .push(MaskPortOperation::UnionRxAborts(aborts.bits()));
            self.rx_abort_enable |= aborts.bits();
        }

        fn order_mask_writes_before_readback(&mut self) {
            self.operations.push(MaskPortOperation::OrderBeforeReadback);
        }

        fn event_enable_bits(&mut self) -> u16 {
            self.operations.push(MaskPortOperation::ReadEvents);
            self.reported_events.unwrap_or(self.event_enable)
        }

        fn tx_abort_enable_bits(&mut self) -> u32 {
            self.operations.push(MaskPortOperation::ReadTxAborts);
            self.reported_tx_aborts.unwrap_or(self.tx_abort_enable)
        }

        fn rx_abort_enable_bits(&mut self) -> u32 {
            self.operations.push(MaskPortOperation::ReadRxAborts);
            self.reported_rx_aborts.unwrap_or(self.rx_abort_enable)
        }
    }

    fn baseline_attempt(
        port: RecordingMaskPort,
    ) -> QuiescedIrqMaskInstallAttempt<RecordingMaskPort> {
        QuiescedIrqMaskInstallAttempt::new(port, QuiescedIrqPlan::handled_baseline_without_timer0())
    }

    #[test]
    fn quiesced_install_uses_vendor_order_and_preserves_unrelated_bits() {
        let unrelated_event = 1 << 7;
        let unrelated_tx_abort = 1 << 25;
        let unrelated_rx_abort = 1 << 9;
        let port =
            RecordingMaskPort::new(17, unrelated_event, unrelated_tx_abort, unrelated_rx_abort);

        let installation = baseline_attempt(port)
            .execute()
            .expect("the model reports every required bit");

        assert_eq!(
            installation.plan(),
            QuiescedIrqPlan::handled_baseline_without_timer0()
        );
        assert_eq!(installation._port.identity, 17);
        assert_eq!(
            installation._port.event_enable,
            unrelated_event | HANDLED_BASELINE_NO_TIMER0_EVENT_BITS
        );
        assert_eq!(
            installation._port.tx_abort_enable,
            unrelated_tx_abort | VENDOR_TX_ABORT_BASELINE_BITS
        );
        assert_eq!(
            installation._port.rx_abort_enable,
            unrelated_rx_abort | VENDOR_RX_ABORT_BASELINE_BITS
        );
        assert_eq!(
            installation._port.operations,
            [
                MaskPortOperation::UnionEvents(HANDLED_BASELINE_NO_TIMER0_EVENT_BITS),
                MaskPortOperation::UnionTxAborts(VENDOR_TX_ABORT_BASELINE_BITS),
                MaskPortOperation::UnionRxAborts(VENDOR_RX_ABORT_BASELINE_BITS),
                MaskPortOperation::OrderBeforeReadback,
                MaskPortOperation::ReadEvents,
                MaskPortOperation::ReadTxAborts,
                MaskPortOperation::ReadRxAborts,
            ]
        );
    }

    #[test]
    fn subset_readback_accepts_additional_observed_bits() {
        let plan = QuiescedIrqPlan::handled_baseline_without_timer0();
        assert_eq!(
            plan.verify_required_readback(0x3fff, 0x7fff_ffff, 0x7fff_ffff),
            Ok(())
        );

        let mut port = RecordingMaskPort::new(23, 0, 0, 0);
        port.reported_events = Some(0x3fff);
        port.reported_tx_aborts = Some(0x7fff_ffff);
        port.reported_rx_aborts = Some(0x7fff_ffff);

        let installation = baseline_attempt(port)
            .execute()
            .expect("OR readback is a subset check, not equality");
        assert_eq!(installation._port.identity, 23);
    }

    fn failed_baseline_install(
        port: RecordingMaskPort,
    ) -> QuiescedIrqDesiredMaskFailure<RecordingMaskPort> {
        match baseline_attempt(port).execute() {
            Ok(_) => panic!("forced missing required bit must fail"),
            Err(failure) => failure,
        }
    }

    #[test]
    fn every_required_bit_checkpoint_fails_closed_and_retains_the_port() {
        let mut event_port = RecordingMaskPort::new(31, 0, 0, 0);
        event_port.reported_events =
            Some(HANDLED_BASELINE_NO_TIMER0_EVENT_BITS & !Ieee802154Event::TxDone.bit());
        let event_failure = failed_baseline_install(event_port);
        assert_eq!(event_failure._port.identity, 31);
        assert_eq!(
            event_failure.error().checkpoint(),
            QuiescedIrqMaskReadbackCheckpoint::EventEnable
        );
        assert_eq!(
            event_failure.error().missing_bits(),
            u32::from(Ieee802154Event::TxDone.bit())
        );
        assert_eq!(
            event_failure._port.operations,
            [
                MaskPortOperation::UnionEvents(HANDLED_BASELINE_NO_TIMER0_EVENT_BITS),
                MaskPortOperation::UnionTxAborts(VENDOR_TX_ABORT_BASELINE_BITS),
                MaskPortOperation::UnionRxAborts(VENDOR_RX_ABORT_BASELINE_BITS),
                MaskPortOperation::OrderBeforeReadback,
                MaskPortOperation::ReadEvents,
                MaskPortOperation::ReadTxAborts,
                MaskPortOperation::ReadRxAborts,
            ]
        );

        let missing_tx_bit = Ieee802154TxAbortReason::TxSecurityError.enable_bit();
        let mut tx_port = RecordingMaskPort::new(32, 0, 0, 0);
        tx_port.reported_tx_aborts = Some(VENDOR_TX_ABORT_BASELINE_BITS & !missing_tx_bit);
        let tx_failure = failed_baseline_install(tx_port);
        assert_eq!(tx_failure._port.identity, 32);
        assert_eq!(
            tx_failure.error().checkpoint(),
            QuiescedIrqMaskReadbackCheckpoint::TxAbortEnable
        );
        assert_eq!(tx_failure.error().missing_bits(), missing_tx_bit);

        let missing_rx_bit = Ieee802154RxAbortReason::TxAckTimeout.enable_bit();
        let mut rx_port = RecordingMaskPort::new(33, 0, 0, 0);
        rx_port.reported_rx_aborts = Some(VENDOR_RX_ABORT_BASELINE_BITS & !missing_rx_bit);
        let rx_failure = failed_baseline_install(rx_port);
        assert_eq!(rx_failure._port.identity, 33);
        assert_eq!(
            rx_failure.error().checkpoint(),
            QuiescedIrqMaskReadbackCheckpoint::RxAbortEnable
        );
        assert_eq!(rx_failure.error().missing_bits(), missing_rx_bit);
        assert_eq!(
            rx_failure.plan(),
            QuiescedIrqPlan::handled_baseline_without_timer0()
        );
    }
}
