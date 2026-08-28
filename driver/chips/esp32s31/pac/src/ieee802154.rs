//! Typed lower-level ownership for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! Every MMIO operation in this module is routed through the reviewed
//! generated `IEEE802154_MAC` peripheral. The narrow lease exposes only the
//! first field-sized operations needed by HAL; neither the generated register
//! block nor numeric addresses can escape it.

#![forbid(unsafe_code)]

use core::ops::{Deref, DerefMut};

use super::{Ieee802154InterruptRegisters, Ieee802154InterruptSetup, Ieee802154TaskRegisters};
pub use crate::generated::{
    Ieee802154EdDurationUnits, Ieee802154Timer0ThresholdWord, Ieee802154Timer0ValueWord,
    Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord, Ieee802154TxPowerCode,
};

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
}

/// Semantic enable state for the four source-confirmed Multi-PAN contexts.
///
/// Hardware bit positions are owned by generated PAC field accessors. This
/// type stores one boolean per context and cannot represent a register image.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanEnableState([bool; 4]);

impl Ieee802154MultipanEnableState {
    pub const NONE: Self = Self([false; 4]);
    pub const ALL: Self = Self([true; 4]);

    /// Construct an explicit semantic state without exposing register bits.
    pub const fn new(context0: bool, context1: bool, context2: bool, context3: bool) -> Self {
        Self([context0, context1, context2, context3])
    }

    const fn enabled(self) -> [bool; 4] {
        self.0
    }

    pub const fn contains(self, index: Ieee802154MultipanIndex) -> bool {
        self.0[index.as_usize()]
    }

    pub const fn with(self, index: Ieee802154MultipanIndex) -> Self {
        let mut enabled = self.0;
        enabled[index.as_usize()] = true;
        Self(enabled)
    }

    pub const fn without(self, index: Ieee802154MultipanIndex) -> Self {
        let mut enabled = self.0;
        enabled[index.as_usize()] = false;
        Self(enabled)
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

/// Semantic state of the source-132 CPU interrupt route on both cores.
///
/// Register words and field geometry remain private to this PAC domain. Every
/// non-reset state is distinct from [`ResetDetached`](Self::ResetDetached), so
/// callers can fail closed without receiving register images.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RouteState {
    /// Both core routes retain the complete reset-detached state.
    ResetDetached,
    /// At least one route has a CPU-interrupt destination assigned.
    DestinationAssigned,
    /// No destination is assigned, but a pass/remap level is configured.
    PassLevelConfigured,
    /// A non-reset bit outside the reviewed destination and pass-level fields
    /// was observed.
    UnclassifiedNonReset,
}

impl Ieee802154RouteState {
    const fn from_observation(
        both_reset: bool,
        destination_assigned: bool,
        pass_level_configured: bool,
    ) -> Self {
        if both_reset {
            Self::ResetDetached
        } else if destination_assigned {
            Self::DestinationAssigned
        } else if pass_level_configured {
            Self::PassLevelConfigured
        } else {
            Self::UnclassifiedNonReset
        }
    }

    /// Return whether both CPU routes retain the complete reset-detached state.
    pub const fn is_reset_detached(self) -> bool {
        matches!(self, Self::ResetDetached)
    }
}

/// One source-confirmed IEEE 802.15.4 MAC event.
///
/// Register positions remain an implementation detail of this PAC type. The
/// enum itself is the semantic vocabulary consumed by the IRQ state machine.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154Event {
    /// A transmission completed.
    TxDone,
    /// A reception completed.
    RxDone,
    /// Automatic ACK transmission completed.
    AckTxDone,
    /// ACK reception completed.
    AckRxDone,
    /// Receive processing aborted.
    RxAbort,
    /// Transmit processing aborted.
    TxAbort,
    /// Energy detection completed.
    EdDone,
    /// TIMER0 overflowed.
    Timer0Overflow,
    /// TIMER1 overflowed.
    Timer1Overflow,
    /// The MAC clock counter matched its configured value.
    ClockCountMatch,
    /// Transmission SFD processing completed.
    TxSfdDone,
    /// Reception SFD processing completed.
    RxSfdDone,
}

impl Ieee802154Event {
    /// Return this event as a validated semantic event set.
    pub const fn mask(self) -> Ieee802154EventMask {
        Ieee802154EventMask::NONE.with(self)
    }
}

/// A sampled event field contained at least one event without a reviewed
/// semantic identity.
///
/// The physical field image remains private to [`Ieee802154EventObservation`].
/// This error deliberately exposes no raw positions while still forcing every
/// consumer to reject an unclassified sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventObservationError;

/// A semantic set containing only source-confirmed MAC events.
///
/// Unlike [`Ieee802154EventEnableState`], this value is not accepted by any PAC
/// writer. It is suitable for executor-side classification without granting
/// event-enable authority.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventMask {
    tx_done: bool,
    rx_done: bool,
    ack_tx_done: bool,
    ack_rx_done: bool,
    rx_abort: bool,
    tx_abort: bool,
    ed_done: bool,
    timer0_overflow: bool,
    timer1_overflow: bool,
    clock_count_match: bool,
    tx_sfd_done: bool,
    rx_sfd_done: bool,
}

impl Ieee802154EventMask {
    pub const NONE: Self = Self {
        tx_done: false,
        rx_done: false,
        ack_tx_done: false,
        ack_rx_done: false,
        rx_abort: false,
        tx_abort: false,
        ed_done: false,
        timer0_overflow: false,
        timer1_overflow: false,
        clock_count_match: false,
        tx_sfd_done: false,
        rx_sfd_done: false,
    };
    pub const NAMED: Self = Self {
        tx_done: true,
        rx_done: true,
        ack_tx_done: true,
        ack_rx_done: true,
        rx_abort: true,
        tx_abort: true,
        ed_done: true,
        timer0_overflow: true,
        timer1_overflow: true,
        clock_count_match: true,
        tx_sfd_done: true,
        rx_sfd_done: true,
    };
    pub const VENDOR_HANDLED: Self = Self {
        clock_count_match: false,
        ..Self::NAMED
    };
    pub const HANDLED_BASELINE_NO_TIMER0: Self = Self {
        timer0_overflow: false,
        ..Self::VENDOR_HANDLED
    };

    const fn with(mut self, event: Ieee802154Event) -> Self {
        match event {
            Ieee802154Event::TxDone => self.tx_done = true,
            Ieee802154Event::RxDone => self.rx_done = true,
            Ieee802154Event::AckTxDone => self.ack_tx_done = true,
            Ieee802154Event::AckRxDone => self.ack_rx_done = true,
            Ieee802154Event::RxAbort => self.rx_abort = true,
            Ieee802154Event::TxAbort => self.tx_abort = true,
            Ieee802154Event::EdDone => self.ed_done = true,
            Ieee802154Event::Timer0Overflow => self.timer0_overflow = true,
            Ieee802154Event::Timer1Overflow => self.timer1_overflow = true,
            Ieee802154Event::ClockCountMatch => self.clock_count_match = true,
            Ieee802154Event::TxSfdDone => self.tx_sfd_done = true,
            Ieee802154Event::RxSfdDone => self.rx_sfd_done = true,
        }
        self
    }

    const fn same_as(self, other: Self) -> bool {
        self.tx_done == other.tx_done
            && self.rx_done == other.rx_done
            && self.ack_tx_done == other.ack_tx_done
            && self.ack_rx_done == other.ack_rx_done
            && self.rx_abort == other.rx_abort
            && self.tx_abort == other.tx_abort
            && self.ed_done == other.ed_done
            && self.timer0_overflow == other.timer0_overflow
            && self.timer1_overflow == other.timer1_overflow
            && self.clock_count_match == other.clock_count_match
            && self.tx_sfd_done == other.tx_sfd_done
            && self.rx_sfd_done == other.rx_sfd_done
    }

    /// Return whether the semantic event set is empty.
    pub const fn is_empty(self) -> bool {
        self.same_as(Self::NONE)
    }

    /// Return whether this set contains `event`.
    pub const fn contains(self, event: Ieee802154Event) -> bool {
        match event {
            Ieee802154Event::TxDone => self.tx_done,
            Ieee802154Event::RxDone => self.rx_done,
            Ieee802154Event::AckTxDone => self.ack_tx_done,
            Ieee802154Event::AckRxDone => self.ack_rx_done,
            Ieee802154Event::RxAbort => self.rx_abort,
            Ieee802154Event::TxAbort => self.tx_abort,
            Ieee802154Event::EdDone => self.ed_done,
            Ieee802154Event::Timer0Overflow => self.timer0_overflow,
            Ieee802154Event::Timer1Overflow => self.timer1_overflow,
            Ieee802154Event::ClockCountMatch => self.clock_count_match,
            Ieee802154Event::TxSfdDone => self.tx_sfd_done,
            Ieee802154Event::RxSfdDone => self.rx_sfd_done,
        }
    }

    /// Combine two already classified event sets.
    pub const fn union(self, other: Self) -> Self {
        Self {
            tx_done: self.tx_done || other.tx_done,
            rx_done: self.rx_done || other.rx_done,
            ack_tx_done: self.ack_tx_done || other.ack_tx_done,
            ack_rx_done: self.ack_rx_done || other.ack_rx_done,
            rx_abort: self.rx_abort || other.rx_abort,
            tx_abort: self.tx_abort || other.tx_abort,
            ed_done: self.ed_done || other.ed_done,
            timer0_overflow: self.timer0_overflow || other.timer0_overflow,
            timer1_overflow: self.timer1_overflow || other.timer1_overflow,
            clock_count_match: self.clock_count_match || other.clock_count_match,
            tx_sfd_done: self.tx_sfd_done || other.tx_sfd_done,
            rx_sfd_done: self.rx_sfd_done || other.rx_sfd_done,
        }
    }

    /// Return events present in `self` but absent from `allowed`.
    pub const fn difference(self, allowed: Self) -> Self {
        Self {
            tx_done: self.tx_done && !allowed.tx_done,
            rx_done: self.rx_done && !allowed.rx_done,
            ack_tx_done: self.ack_tx_done && !allowed.ack_tx_done,
            ack_rx_done: self.ack_rx_done && !allowed.ack_rx_done,
            rx_abort: self.rx_abort && !allowed.rx_abort,
            tx_abort: self.tx_abort && !allowed.tx_abort,
            ed_done: self.ed_done && !allowed.ed_done,
            timer0_overflow: self.timer0_overflow && !allowed.timer0_overflow,
            timer1_overflow: self.timer1_overflow && !allowed.timer1_overflow,
            clock_count_match: self.clock_count_match && !allowed.clock_count_match,
            tx_sfd_done: self.tx_sfd_done && !allowed.tx_sfd_done,
            rx_sfd_done: self.rx_sfd_done && !allowed.rx_sfd_done,
        }
    }

    /// Return events present in both semantic sets.
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            tx_done: self.tx_done && other.tx_done,
            rx_done: self.rx_done && other.rx_done,
            ack_tx_done: self.ack_tx_done && other.ack_tx_done,
            ack_rx_done: self.ack_rx_done && other.ack_rx_done,
            rx_abort: self.rx_abort && other.rx_abort,
            tx_abort: self.tx_abort && other.tx_abort,
            ed_done: self.ed_done && other.ed_done,
            timer0_overflow: self.timer0_overflow && other.timer0_overflow,
            timer1_overflow: self.timer1_overflow && other.timer1_overflow,
            clock_count_match: self.clock_count_match && other.clock_count_match,
            tx_sfd_done: self.tx_sfd_done && other.tx_sfd_done,
            rx_sfd_done: self.rx_sfd_done && other.rx_sfd_done,
        }
    }

    /// Return whether the set contains more than one semantic event.
    pub const fn has_multiple(self) -> bool {
        self.tx_done as u8
            + self.rx_done as u8
            + self.ack_tx_done as u8
            + self.ack_rx_done as u8
            + self.rx_abort as u8
            + self.tx_abort as u8
            + self.ed_done as u8
            + self.timer0_overflow as u8
            + self.timer1_overflow as u8
            + self.clock_count_match as u8
            + self.tx_sfd_done as u8
            + self.rx_sfd_done as u8
            > 1
    }

    /// Collapse this named set into the closed diagnostic vocabulary.
    pub const fn state(self) -> Ieee802154ObservedEventState {
        Ieee802154ObservedEventState::from_mask(self)
    }
}

impl From<Ieee802154Event> for Ieee802154EventMask {
    fn from(event: Ieee802154Event) -> Self {
        event.mask()
    }
}

/// Closed semantic summary of one complete MAC event observation.
///
/// Register positions remain private to the PAC. The two validation probes and
/// production ED diagnostics need only these exact relations; every other
/// source-confirmed combination is retained as `UnexpectedNamed`, while an
/// unnamed physical event remains fail-closed as `Unclassified`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154ObservedEventState {
    #[default]
    Clear,
    Timer0Only,
    Timer1Only,
    Timer0AndTimer1,
    EdDoneOnly,
    EdDoneAndTimer0,
    RxAbortOnly,
    RxAbortWithOther,
    EdDoneWithOther,
    EdDoneAndRxAbortWithOther,
    UnexpectedNamed,
    Unclassified,
}

impl Ieee802154ObservedEventState {
    const fn from_mask(events: Ieee802154EventMask) -> Self {
        if events.is_empty() {
            Self::Clear
        } else if events.same_as(Ieee802154Event::Timer0Overflow.mask()) {
            Self::Timer0Only
        } else if events.same_as(Ieee802154Event::Timer1Overflow.mask()) {
            Self::Timer1Only
        } else if events.same_as(
            Ieee802154Event::Timer0Overflow
                .mask()
                .union(Ieee802154Event::Timer1Overflow.mask()),
        ) {
            Self::Timer0AndTimer1
        } else if events.same_as(Ieee802154Event::EdDone.mask()) {
            Self::EdDoneOnly
        } else if events.same_as(
            Ieee802154Event::EdDone
                .mask()
                .union(Ieee802154Event::Timer0Overflow.mask()),
        ) {
            Self::EdDoneAndTimer0
        } else if events.same_as(Ieee802154Event::RxAbort.mask()) {
            Self::RxAbortOnly
        } else if events.contains(Ieee802154Event::EdDone)
            && events.contains(Ieee802154Event::RxAbort)
        {
            Self::EdDoneAndRxAbortWithOther
        } else if events.contains(Ieee802154Event::RxAbort) {
            Self::RxAbortWithOther
        } else if events.contains(Ieee802154Event::EdDone) {
            Self::EdDoneWithOther
        } else {
            Self::UnexpectedNamed
        }
    }

    /// Return whether the observation is clear.
    pub const fn is_clear(self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Return whether TIMER0 is part of this classified observation.
    pub const fn has_timer0(self) -> bool {
        matches!(
            self,
            Self::Timer0Only | Self::Timer0AndTimer1 | Self::EdDoneAndTimer0
        )
    }

    /// Return whether TIMER1 is part of this classified observation.
    pub const fn has_timer1(self) -> bool {
        matches!(self, Self::Timer1Only | Self::Timer0AndTimer1)
    }

    /// Return whether ED-DONE is part of this classified observation.
    pub const fn has_ed_done(self) -> bool {
        matches!(
            self,
            Self::EdDoneOnly
                | Self::EdDoneAndTimer0
                | Self::EdDoneWithOther
                | Self::EdDoneAndRxAbortWithOther
        )
    }

    /// Return whether RX-ABORT is part of this classified observation.
    pub const fn has_rx_abort(self) -> bool {
        matches!(
            self,
            Self::RxAbortOnly | Self::RxAbortWithOther | Self::EdDoneAndRxAbortWithOther
        )
    }

    /// Return whether this is exactly the classified RX-abort observation.
    pub const fn is_rx_abort_only(self) -> bool {
        matches!(self, Self::RxAbortOnly)
    }

    /// Combine observations without exposing their physical encoding.
    pub fn union(self, other: Self) -> Self {
        use Ieee802154ObservedEventState as State;
        if matches!(self, State::Unclassified) || matches!(other, State::Unclassified) {
            return State::Unclassified;
        }
        if self == State::Clear {
            return other;
        }
        if other == State::Clear || self == other {
            return self;
        }

        let ed_done = self.has_ed_done() || other.has_ed_done();
        let rx_abort = self.has_rx_abort() || other.has_rx_abort();
        let timer0 = self.has_timer0() || other.has_timer0();
        let timer1 = self.has_timer1() || other.has_timer1();
        let has_opaque_other = matches!(
            self,
            State::RxAbortWithOther
                | State::EdDoneWithOther
                | State::EdDoneAndRxAbortWithOther
                | State::UnexpectedNamed
        ) || matches!(
            other,
            State::RxAbortWithOther
                | State::EdDoneWithOther
                | State::EdDoneAndRxAbortWithOther
                | State::UnexpectedNamed
        );

        if ed_done && rx_abort {
            State::EdDoneAndRxAbortWithOther
        } else if has_opaque_other && rx_abort {
            State::RxAbortWithOther
        } else if has_opaque_other && ed_done {
            State::EdDoneWithOther
        } else if has_opaque_other {
            State::UnexpectedNamed
        } else if ed_done && timer0 && !timer1 {
            State::EdDoneAndTimer0
        } else if rx_abort && !ed_done && !timer0 && !timer1 {
            State::RxAbortOnly
        } else if rx_abort {
            State::RxAbortWithOther
        } else if ed_done && !timer0 && !timer1 {
            State::EdDoneOnly
        } else if ed_done {
            State::EdDoneWithOther
        } else if timer0 && timer1 {
            State::Timer0AndTimer1
        } else if timer0 {
            State::Timer0Only
        } else if timer1 {
            State::Timer1Only
        } else {
            State::UnexpectedNamed
        }
    }
}

/// Semantic readback of the validation-owned event-enable field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154ValidationEventEnableState {
    #[default]
    AllMasked,
    TimerPairOnly,
    EdDoneTimer0RxAbortOnly,
    Unexpected,
}

impl Ieee802154ValidationEventEnableState {
    #[cfg(feature = "validation-probes")]
    const fn from_raw(
        readback: crate::svd::ieee802154_mac_ownership::ValidationEventEnableReadback,
    ) -> Self {
        match readback {
            crate::svd::ieee802154_mac_ownership::ValidationEventEnableReadback::AllMasked => {
                Self::AllMasked
            }
            crate::svd::ieee802154_mac_ownership::ValidationEventEnableReadback::TimerPair => {
                Self::TimerPairOnly
            }
            crate::svd::ieee802154_mac_ownership::ValidationEventEnableReadback::EdTimerAbort => {
                Self::EdDoneTimer0RxAbortOnly
            }
            crate::svd::ieee802154_mac_ownership::ValidationEventEnableReadback::Unexpected => {
                Self::Unexpected
            }
        }
    }
}

/// Semantic readback of the fixed validation ED duration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154ValidationEdDurationState {
    ValidationEight,
    #[default]
    Other,
}

impl Ieee802154ValidationEdDurationState {
    #[cfg(feature = "validation-probes")]
    const fn from_field(value: u32) -> Self {
        if value == 8 {
            Self::ValidationEight
        } else {
            Self::Other
        }
    }
}

/// One source-confirmed receive-abort reason.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortReason {
    /// Receive stop command.
    RxStop,
    /// SFD timeout.
    SfdTimeout,
    /// CRC failure.
    CrcError,
    /// Invalid frame length.
    InvalidLength,
    /// Address or filter rejection.
    FilterFail,
    /// RSS was not detected.
    NoRss,
    /// Coexistence interrupted reception.
    CoexistenceBreak,
    /// An ACK was received unexpectedly.
    UnexpectedAck,
    /// Receive processing restarted.
    RxRestart,
    /// ACK transmission timed out.
    TxAckTimeout,
    /// ACK transmission was stopped.
    TxAckStop,
    /// Coexistence interrupted ACK transmission.
    TxAckCoexistenceBreak,
    /// Enhanced-ACK security processing failed.
    EnhancedAckSecurityError,
    /// Energy detection was aborted.
    EdAbort,
    /// Energy detection was stopped.
    EdStop,
    /// Coexistence rejected energy detection.
    EdCoexistenceReject,
}

impl Ieee802154RxAbortReason {
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::RxStop),
            2 => Some(Self::SfdTimeout),
            3 => Some(Self::CrcError),
            4 => Some(Self::InvalidLength),
            5 => Some(Self::FilterFail),
            6 => Some(Self::NoRss),
            7 => Some(Self::CoexistenceBreak),
            8 => Some(Self::UnexpectedAck),
            9 => Some(Self::RxRestart),
            16 => Some(Self::TxAckTimeout),
            17 => Some(Self::TxAckStop),
            18 => Some(Self::TxAckCoexistenceBreak),
            19 => Some(Self::EnhancedAckSecurityError),
            24 => Some(Self::EdAbort),
            25 => Some(Self::EdStop),
            26 => Some(Self::EdCoexistenceReject),
            _ => None,
        }
    }
}

/// One source-confirmed transmit-abort reason.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154TxAbortReason {
    /// ACK reception was stopped.
    RxAckStop,
    /// ACK SFD timed out.
    RxAckSfdTimeout,
    /// The received ACK had a CRC failure.
    RxAckCrcError,
    /// The received ACK had an invalid length.
    RxAckInvalidLength,
    /// The received ACK failed filtering.
    RxAckFilterFail,
    /// RSS was not detected for the ACK.
    RxAckNoRss,
    /// Coexistence interrupted ACK reception.
    RxAckCoexistenceBreak,
    /// The received frame was not an ACK.
    RxAckTypeNotAck,
    /// ACK receive processing restarted.
    RxAckRestart,
    /// ACK reception timed out.
    RxAckTimeout,
    /// Transmission was stopped.
    TxStop,
    /// Coexistence interrupted transmission.
    TxCoexistenceBreak,
    /// Transmission security processing failed.
    TxSecurityError,
    /// CCA failed.
    CcaFailed,
    /// CCA observed a busy channel.
    CcaBusy,
}

impl Ieee802154TxAbortReason {
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::RxAckStop),
            2 => Some(Self::RxAckSfdTimeout),
            3 => Some(Self::RxAckCrcError),
            4 => Some(Self::RxAckInvalidLength),
            5 => Some(Self::RxAckFilterFail),
            6 => Some(Self::RxAckNoRss),
            7 => Some(Self::RxAckCoexistenceBreak),
            8 => Some(Self::RxAckTypeNotAck),
            9 => Some(Self::RxAckRestart),
            16 => Some(Self::RxAckTimeout),
            17 => Some(Self::TxStop),
            18 => Some(Self::TxCoexistenceBreak),
            19 => Some(Self::TxSecurityError),
            24 => Some(Self::CcaFailed),
            25 => Some(Self::CcaBusy),
            _ => None,
        }
    }
}

/// Semantic classification of one sampled RX-abort reason field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortReasonObservation {
    /// The field matched a source-confirmed reason.
    Named(Ieee802154RxAbortReason),
    /// The field value has no reviewed semantic identity.
    Unclassified,
}

impl Ieee802154RxAbortReasonObservation {
    const fn from_field(code: u8) -> Self {
        match Ieee802154RxAbortReason::from_code(code) {
            Some(reason) => Self::Named(reason),
            None => Self::Unclassified,
        }
    }
}

impl From<Ieee802154RxAbortReason> for Ieee802154RxAbortReasonObservation {
    fn from(reason: Ieee802154RxAbortReason) -> Self {
        Self::Named(reason)
    }
}

/// Semantic classification of one sampled TX-abort reason field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154TxAbortReasonObservation {
    /// The field matched a source-confirmed reason.
    Named(Ieee802154TxAbortReason),
    /// The field value has no reviewed semantic identity.
    Unclassified,
}

impl Ieee802154TxAbortReasonObservation {
    const fn from_field(code: u8) -> Self {
        match Ieee802154TxAbortReason::from_code(code) {
            Some(reason) => Self::Named(reason),
            None => Self::Unclassified,
        }
    }
}

impl From<Ieee802154TxAbortReason> for Ieee802154TxAbortReasonObservation {
    fn from(reason: Ieee802154TxAbortReason) -> Self {
        Self::Named(reason)
    }
}

/// Closed semantic `EVENT_ENABLE` states accepted by finite polled operations.
///
/// Register geometry and physical images remain exclusively in generated PAC
/// accessors. No integer conversion exists in either direction.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154EventEnableState {
    #[default]
    AllMasked,
    EdOperation,
}

/// Closed semantic `RX_ABORT_ENABLE` states accepted by finite ED/CCA work.
///
/// The runtime interrupt baseline is owned by the complete activation
/// transaction and is intentionally not constructible through this API.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortEnableState {
    #[default]
    AllMasked,
    EdOperationReasons,
}

/// One closed production plan for activating the IEEE 802.15.4 IRQ owner.
///
/// Register images are selected only by generated accessors inside the raw PAC
/// owner; this marker grants no field or integer authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ieee802154InterruptActivationPlan;

impl Ieee802154InterruptActivationPlan {
    const SOURCE_CONFIRMED_BASELINE: Self = Self;
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
    const fn from_raw(
        readback: crate::svd::ieee802154_mac_ownership::OperationEventEnableReadback,
    ) -> Self {
        match readback {
            crate::svd::ieee802154_mac_ownership::OperationEventEnableReadback::AllMasked => {
                Self::AllMasked
            }
            crate::svd::ieee802154_mac_ownership::OperationEventEnableReadback::EdOperation => {
                Self::EdDoneAndRxAbortOnly
            }
            crate::svd::ieee802154_mac_ownership::OperationEventEnableReadback::Unexpected => {
                Self::Unexpected
            }
        }
    }
}

/// Semantic readback of the receive-abort-enable field owned by a polled
/// ED/CCA operation.
///
/// The observation has no conversion to [`Ieee802154RxAbortEnableState`], so
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
    const fn from_raw(
        readback: crate::svd::ieee802154_mac_ownership::OperationRxAbortEnableReadback,
    ) -> Self {
        match readback {
            crate::svd::ieee802154_mac_ownership::OperationRxAbortEnableReadback::AllMasked => {
                Self::AllMasked
            }
            crate::svd::ieee802154_mac_ownership::OperationRxAbortEnableReadback::EdOperationReasons => {
                Self::EdOperationReasonsOnly
            }
            crate::svd::ieee802154_mac_ownership::OperationRxAbortEnableReadback::Unexpected => {
                Self::Unexpected
            }
        }
    }
}

/// Copyable semantic `EVENT_ENABLE` or `EVENT_STATUS` observation.
///
/// Unlike [`Ieee802154EventEnableState`], this type preserves unnamed physical
/// bits because observations must not erase unexpected hardware state. It has
/// no public constructor and cannot be passed to a write. W1C acknowledgement
/// consumes a separate affine snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventObservation {
    events: Ieee802154EventMask,
    has_unclassified: bool,
}

impl Ieee802154EventObservation {
    const fn from_readback(
        readback: crate::svd::ieee802154_mac_ownership::Ieee802154EventReadback,
    ) -> Self {
        let mut events = Ieee802154EventMask::NONE;
        if readback.tx_done() {
            events = events.with(Ieee802154Event::TxDone);
        }
        if readback.rx_done() {
            events = events.with(Ieee802154Event::RxDone);
        }
        if readback.ack_tx_done() {
            events = events.with(Ieee802154Event::AckTxDone);
        }
        if readback.ack_rx_done() {
            events = events.with(Ieee802154Event::AckRxDone);
        }
        if readback.rx_abort() {
            events = events.with(Ieee802154Event::RxAbort);
        }
        if readback.tx_abort() {
            events = events.with(Ieee802154Event::TxAbort);
        }
        if readback.ed_done() {
            events = events.with(Ieee802154Event::EdDone);
        }
        if readback.timer0_overflow() {
            events = events.with(Ieee802154Event::Timer0Overflow);
        }
        if readback.timer1_overflow() {
            events = events.with(Ieee802154Event::Timer1Overflow);
        }
        if readback.clock_count_match() {
            events = events.with(Ieee802154Event::ClockCountMatch);
        }
        if readback.tx_sfd_done() {
            events = events.with(Ieee802154Event::TxSfdDone);
        }
        if readback.rx_sfd_done() {
            events = events.with(Ieee802154Event::RxSfdDone);
        }
        Self {
            events,
            has_unclassified: readback.has_unclassified(),
        }
    }

    fn from_snapshot(
        snapshot: &crate::svd::w1c_register_snapshot::Ieee802154EventStatusSnapshot,
    ) -> Self {
        Self::from_readback(
            crate::svd::ieee802154_mac_ownership::Ieee802154EventReadback::from_event_status_snapshot(
                snapshot,
            ),
        )
    }

    /// Return whether the complete observed event field is clear.
    pub const fn is_clear(self) -> bool {
        self.events.is_empty() && !self.has_unclassified
    }

    /// Return whether every named event in `required` was observed.
    pub const fn contains(self, required: Ieee802154Event) -> bool {
        self.events.contains(required)
    }

    /// Classify the complete observation as a semantic event set.
    ///
    /// Any unnamed physical event produces an opaque error. Neither branch
    /// exposes the underlying register image, and the named set is not write
    /// authority.
    pub const fn classification(
        self,
    ) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        if self.has_unclassified {
            Err(Ieee802154EventObservationError)
        } else {
            Ok(self.events)
        }
    }

    /// Collapse the complete observation into the closed semantic vocabulary.
    pub const fn state(self) -> Ieee802154ObservedEventState {
        match self.classification() {
            Ok(events) => Ieee802154ObservedEventState::from_mask(events),
            Err(_) => Ieee802154ObservedEventState::Unclassified,
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
    rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
    tx_abort_reason: Option<Ieee802154TxAbortReasonObservation>,
    ed_rss_code: Option<i8>,
    cca_busy: Option<bool>,
}

impl Ieee802154InterruptSnapshot {
    /// Classify the sampled event field without exposing its register image.
    pub const fn event_classification(
        &self,
    ) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        self.events.classification()
    }

    /// Return typed RX-abort evidence only for an RX-abort event.
    pub const fn rx_abort_reason(&self) -> Option<Ieee802154RxAbortReasonObservation> {
        self.rx_abort_reason
    }

    /// Return typed TX-abort evidence only for a TX-abort event.
    pub const fn tx_abort_reason(&self) -> Option<Ieee802154TxAbortReasonObservation> {
        self.tx_abort_reason
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

/// Read-back image of the interrupt-masked IEEE 802.15.4 MAC foundation.
///
/// This snapshot deliberately excludes `EVENT_STATUS` because the foundation
/// transition neither owns nor acknowledges pending runtime events. Polled and
/// hard-IRQ paths use the generated affine W1C snapshot transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154FoundationSnapshot {
    events_masked: bool,
    rx_aborts_masked: bool,
    tx_aborts_masked: bool,
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
    multipan_enable_state: Ieee802154MultipanEnableState,
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
    multipan_enable_state: Ieee802154MultipanEnableState,
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
        multipan_enable_state: Ieee802154MultipanEnableState,
        identity: Ieee802154PanIdentity,
    ) -> Self {
        Self {
            frequency_code,
            cca_mode,
            cca_threshold_code,
            ack_timeout,
            control,
            multipan_enable_state,
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

    pub const fn multipan_enable_state(self) -> Ieee802154MultipanEnableState {
        self.multipan_enable_state
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

    pub const fn multipan_enable_state(self) -> Ieee802154MultipanEnableState {
        self.multipan_enable_state
    }

    pub const fn multipan_identity(self, index: Ieee802154MultipanIndex) -> Ieee802154PanIdentity {
        self.identities[index.as_usize()]
    }

    pub const fn frame_pending(self) -> bool {
        self.frame_pending
    }
}

impl Ieee802154FoundationSnapshot {
    /// Construct a semantic platform-independent readback.
    ///
    /// Production snapshots are sampled by the PAC lease; this constructor
    /// also lets the HAL verify its transition against a host backend without
    /// duplicating the register model.
    #[doc(hidden)]
    pub const fn new(
        events_masked: bool,
        rx_aborts_masked: bool,
        tx_aborts_masked: bool,
        ed_uses_average: bool,
        txrx_pti: Ieee802154Pti,
        ack_pti: Ieee802154Pti,
    ) -> Self {
        Self {
            events_masked,
            rx_aborts_masked,
            tx_aborts_masked,
            ed_uses_average,
            txrx_pti,
            ack_pti,
        }
    }

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
    interrupt_route: &'registers crate::svd::Ieee802154InterruptRoute,
}

/// Exclusive task-side lease for the two reviewed MAC timers.
///
/// The lease exposes complete register-specific timer words and fixed command
/// images only. It cannot sample or acknowledge interrupt status:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::Ieee802154TimerLease;
///
/// fn sample_irq(timer: &Ieee802154TimerLease<'_>) {
///     let _ = timer.event_status_observation();
/// }
/// ```
///
/// The parent task lease cannot be mutably borrowed twice while a timer lease
/// is alive:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn overlap(task: &mut Ieee802154RegisterLease<'_>) {
///     let first = task.timer_lease();
///     let second = task.timer_lease();
///     let _ = (first, second);
/// }
/// ```
#[must_use = "dropping the timer lease releases its exclusive task-register borrow"]
pub struct Ieee802154TimerLease<'registers> {
    registers: &'registers mut crate::svd::ieee802154_mac_ownership::TaskRegisters,
}

impl Ieee802154TimerLease<'_> {
    /// Add TIMER0 to the closed runtime interrupt baseline before deadline
    /// sampling begins.
    #[doc(hidden)]
    pub fn enable_acknowledgement_watchdog_event(&mut self) {
        self.registers.enable_runtime_events_with_timer0();
    }

    /// Publish the already-derived TIMER0 threshold and fixed start image.
    ///
    /// This PAC layer assigns no unit to `threshold`; the runtime owns the
    /// source-defined monotonic-clock conversion.
    #[doc(hidden)]
    pub fn start_acknowledgement_watchdog(&mut self, threshold: Ieee802154Timer0ThresholdWord) {
        self.set_timer0_threshold(threshold);
        crate::device_fence();
        self.start_timer0();
        crate::device_fence();
    }

    /// Stop TIMER0 and restore the reviewed runtime baseline without TIMER0.
    #[doc(hidden)]
    pub fn disarm_acknowledgement_watchdog(&mut self) {
        self.stop_timer0();
        self.registers.enable_runtime_events_without_timer0();
        crate::device_fence();
    }

    /// Publish one complete TIMER0 threshold without assigning clock units.
    pub fn set_timer0_threshold(&mut self, threshold: Ieee802154Timer0ThresholdWord) {
        self.registers.publish_timer0_threshold(threshold.get());
    }

    /// Observe one complete TIMER0 counter word without assigning clock units.
    pub fn timer0_value(&self) -> Ieee802154Timer0ValueWord {
        Ieee802154Timer0ValueWord::new(self.registers.observe_timer0_value())
    }

    /// Issue exactly the reviewed TIMER0-start command image.
    pub fn start_timer0(&mut self) {
        self.registers.issue_timer0_start();
    }

    /// Issue exactly the reviewed TIMER0-stop command image.
    pub fn stop_timer0(&mut self) {
        self.registers.issue_timer0_stop();
    }

    /// Publish one complete TIMER1 threshold without assigning clock units.
    pub fn set_timer1_threshold(&mut self, threshold: Ieee802154Timer1ThresholdWord) {
        self.registers.publish_timer1_threshold(threshold.get());
    }

    /// Observe one complete TIMER1 counter word without assigning clock units.
    pub fn timer1_value(&self) -> Ieee802154Timer1ValueWord {
        Ieee802154Timer1ValueWord::new(self.registers.observe_timer1_value())
    }

    /// Issue exactly the reviewed TIMER1-start command image.
    pub fn start_timer1(&mut self) {
        self.registers.issue_timer1_start();
    }

    /// Issue exactly the reviewed TIMER1-stop command image.
    pub fn stop_timer1(&mut self) {
        self.registers.issue_timer1_stop();
    }
}

impl Ieee802154RegisterLease<'_> {
    /// Exclusively borrow both reviewed MAC timers from the task owner.
    pub fn timer_lease(&mut self) -> Ieee802154TimerLease<'_> {
        Ieee802154TimerLease {
            registers: self.registers,
        }
    }

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

    /// Replace all four named multipan-enable fields exactly.
    pub fn set_multipan_enable_state(&mut self, state: Ieee802154MultipanEnableState) {
        self.registers.set_multipan_enabled(state.enabled());
    }

    /// Program one of the four public PAN identities.
    ///
    /// Matching the public LL, each logical address setter first enables its
    /// context while preserving every other enable bit. Call
    /// [`Self::set_multipan_enable_state`] afterwards when the caller needs one
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

    /// Read all four named multipan enable fields.
    pub fn multipan_enable_state(&self) -> Ieee802154MultipanEnableState {
        Ieee802154MultipanEnableState(self.registers.multipan_enabled())
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
            multipan_enable_state: Ieee802154MultipanEnableState(readback.multipan_enabled()),
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
            multipan_enable_state: Ieee802154MultipanEnableState(readback.multipan_enabled()),
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
        self.task.registers.mask_all_events();
    }

    /// Mask every receive-abort source before a receive dataplane exists.
    pub fn mask_all_rx_aborts(&mut self) {
        self.task.registers.mask_all_rx_aborts();
    }

    /// Mask every transmit-abort source before a transmit dataplane exists.
    pub fn mask_all_tx_aborts(&mut self) {
        self.task.registers.mask_all_tx_aborts();
    }

    /// Select the vendor foundation's average energy-detection sampler.
    pub fn select_average_ed_sampling(&mut self) {
        self.task.registers.select_average_ed_sampling();
    }

    /// Select one closed finite-operation `EVENT_ENABLE` state.
    pub fn set_event_enable(&mut self, state: Ieee802154EventEnableState) {
        match state {
            Ieee802154EventEnableState::AllMasked => self.task.registers.mask_all_events(),
            Ieee802154EventEnableState::EdOperation => {
                self.task.registers.enable_ed_operation_events()
            }
        }
    }

    /// Select one closed finite-operation `RX_ABORT_ENABLE` state.
    pub fn set_rx_abort_enable(&mut self, state: Ieee802154RxAbortEnableState) {
        match state {
            Ieee802154RxAbortEnableState::AllMasked => self.task.registers.mask_all_rx_aborts(),
            Ieee802154RxAbortEnableState::EdOperationReasons => {
                self.task.registers.enable_ed_operation_rx_abort_reasons()
            }
        }
    }

    /// Classify the complete event-delivery field for a finite polled ED/CCA
    /// operation without sampling `EVENT_STATUS`.
    pub fn operation_event_enable_observation(&self) -> Ieee802154OperationEventEnableObservation {
        Ieee802154OperationEventEnableObservation::from_raw(
            self.task.registers.operation_event_enable_readback(),
        )
    }

    /// Classify the complete RX-abort delivery field for a finite polled
    /// ED/CCA operation.
    pub fn operation_rx_abort_enable_observation(
        &self,
    ) -> Ieee802154OperationRxAbortEnableObservation {
        Ieee802154OperationRxAbortEnableObservation::from_raw(
            self.task.registers.operation_rx_abort_enable_readback(),
        )
    }

    /// Observe the complete fourteen-bit event field without acknowledging it.
    pub fn event_status_observation(&self) -> Ieee802154EventObservation {
        let snapshot = self.interrupt.sample_event_status();
        Ieee802154EventObservation::from_snapshot(&snapshot)
    }

    /// Classify the IRQ-owned RX-abort reason field without exporting the
    /// containing register image.
    pub fn rx_abort_reason_observation(&self) -> Ieee802154RxAbortReasonObservation {
        Ieee802154RxAbortReasonObservation::from_field(
            self.interrupt.rx_status_readback().abort_reason_code(),
        )
    }

    /// Sample only fields written by the interrupt-masked foundation.
    pub fn foundation_snapshot(&self) -> Ieee802154FoundationSnapshot {
        let readback = self.task.registers.foundation_readback();
        Ieee802154FoundationSnapshot {
            events_masked: readback.events_masked(),
            rx_aborts_masked: readback.rx_aborts_masked(),
            tx_aborts_masked: readback.tx_aborts_masked(),
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
            Ieee802154EventObservation::from_readback(self.task.registers.event_enable_readback()),
            Ieee802154EventObservation::from_snapshot(&event_status),
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
        let events = Ieee802154EventObservation::from_snapshot(&snapshot);
        self.interrupt.acknowledge_event_status(snapshot);
        crate::device_fence();
        events
    }

    /// Classify both source-132 routes without exposing register images.
    #[doc(hidden)]
    pub fn interrupt_route_state(&self) -> Ieee802154RouteState {
        let (core0_map, core0_unclassified_6_7, core0_pass_level, core0_unclassified_10_31) =
            crate::svd::field_snapshot_read::observe_ieee802154_core0_route(
                self.task.interrupt_route,
            );
        let (core1_map, core1_unclassified_6_7, core1_pass_level, core1_unclassified_10_31) =
            crate::svd::field_snapshot_read::observe_ieee802154_core1_route(
                self.task.interrupt_route,
            );
        Ieee802154RouteState::from_observation(
            core0_map == 0
                && core0_unclassified_6_7 == 0
                && core0_pass_level == 0
                && core0_unclassified_10_31 == 0
                && core1_map == 0
                && core1_unclassified_6_7 == 0
                && core1_pass_level == 0
                && core1_unclassified_10_31 == 0,
            core0_map != 0 || core1_map != 0,
            core0_pass_level != 0 || core1_pass_level != 0,
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_enable_state(&self) -> Ieee802154ValidationEventEnableState {
        Ieee802154ValidationEventEnableState::from_raw(
            self.task.registers.validation_event_enable_readback(),
        )
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
    pub fn validation_event_status_state(&self) -> Ieee802154ObservedEventState {
        Ieee802154EventObservation::from_readback(self.interrupt.validation_event_status_events())
            .state()
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
    pub fn validation_ed_event_enable_state(&self) -> Ieee802154ValidationEventEnableState {
        Ieee802154ValidationEventEnableState::from_raw(
            self.task.registers.validation_event_enable_readback(),
        )
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
    pub fn validation_ed_rx_abort_enable_state(
        &self,
    ) -> Ieee802154OperationRxAbortEnableObservation {
        Ieee802154OperationRxAbortEnableObservation::from_raw(
            self.task.registers.operation_rx_abort_enable_readback(),
        )
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
    pub fn validation_ed_event_status_state(&self) -> Ieee802154ObservedEventState {
        Ieee802154EventObservation::from_readback(
            self.interrupt.validation_ed_event_status_events(),
        )
        .state()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        self.rx_abort_reason_observation()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_duration_state(&self) -> Ieee802154ValidationEdDurationState {
        Ieee802154ValidationEdDurationState::from_field(
            self.task.registers.validation_ed_duration(),
        )
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

    fn stop_operation(&mut self);
    fn stop_timer0(&mut self);
    fn stop_timer1(&mut self);
    fn mask_all_events(&mut self);
    fn enable_runtime_events(&mut self);
    fn mask_all_tx_aborts(&mut self);
    fn enable_runtime_tx_aborts(&mut self);
    fn mask_all_rx_aborts(&mut self);
    fn enable_runtime_rx_aborts(&mut self);
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
fn execute_interrupt_activation<Port>(port: &mut Port, _plan: Ieee802154InterruptActivationPlan)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.mask_all_events();
    port.enable_runtime_tx_aborts();
    port.enable_runtime_rx_aborts();
    port.order_device_accesses();

    let stale = port.sample_events();
    port.acknowledge_events(stale);
    port.enable_runtime_events();
    port.order_device_accesses();
}

/// Execute the complete teardown after the platform CPU route is disabled.
///
/// The operation and both MAC timers are stopped before all three enable
/// fields are replaced with their closed zero images. One final affine W1C
/// sample is then consumed. Both ordering boundaries precede transfer back to
/// inactive setup ownership.
fn execute_interrupt_deactivation<Port>(port: &mut Port)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.stop_operation();
    port.stop_timer0();
    port.stop_timer1();
    port.mask_all_events();
    port.mask_all_tx_aborts();
    port.mask_all_rx_aborts();
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

    fn stop_operation(&mut self) {
        self.task.peripherals.ieee802154_mac.issue_stop();
    }

    fn stop_timer0(&mut self) {
        self.task.peripherals.ieee802154_mac.issue_timer0_stop();
    }

    fn stop_timer1(&mut self) {
        self.task.peripherals.ieee802154_mac.issue_timer1_stop();
    }

    fn mask_all_events(&mut self) {
        self.task.peripherals.ieee802154_mac.mask_all_events();
    }

    fn enable_runtime_events(&mut self) {
        self.task
            .peripherals
            .ieee802154_mac
            .enable_runtime_events_without_timer0();
    }

    fn mask_all_tx_aborts(&mut self) {
        self.task.peripherals.ieee802154_mac.mask_all_tx_aborts();
    }

    fn enable_runtime_tx_aborts(&mut self) {
        self.task
            .peripherals
            .ieee802154_mac
            .enable_runtime_tx_aborts();
    }

    fn mask_all_rx_aborts(&mut self) {
        self.task.peripherals.ieee802154_mac.mask_all_rx_aborts();
    }

    fn enable_runtime_rx_aborts(&mut self) {
        self.task
            .peripherals
            .ieee802154_mac
            .enable_runtime_rx_aborts();
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
    /// installed in its final storage. This single consuming transition uses
    /// only the generated runtime-baseline accessors. It keeps event delivery
    /// masked while configuring both abort fields, consumes one complete stale
    /// affine W1C snapshot, publishes runtime events last, and orders those
    /// writes before returning.
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
        let events = Ieee802154EventObservation::from_snapshot(&acknowledgement);
        let rx_abort = events.contains(Ieee802154Event::RxAbort);
        let tx_abort = events.contains(Ieee802154Event::TxAbort);
        let ed_done = events.contains(Ieee802154Event::EdDone);
        let rx_status = rx_abort.then(|| self.registers.rx_status_readback());
        let tx_status = tx_abort.then(|| self.registers.tx_status_readback());

        Ieee802154InterruptSnapshot {
            acknowledgement,
            events,
            rx_abort_reason: rx_status.map(|status| {
                Ieee802154RxAbortReasonObservation::from_field(status.abort_reason_code())
            }),
            tx_abort_reason: tx_status.map(|status| {
                Ieee802154TxAbortReasonObservation::from_field(status.abort_reason_code())
            }),
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
    /// transition stops the active operation and both MAC timers, replaces
    /// event, transmit-abort, and receive-abort enables with exact zero images,
    /// consumes one final complete affine W1C snapshot, and orders both phases
    /// before returning task-side setup authority.
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
            interrupt_route: &self.peripherals.ieee802154_interrupt_route,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Ieee802154InterruptActivationPlan, Ieee802154InterruptTransitionPort,
        Ieee802154ObservedEventState, Ieee802154RxStateCode, Ieee802154StateSnapshot,
        Ieee802154TxStateCode, execute_interrupt_activation, execute_interrupt_deactivation,
    };
    use crate::RadioHardware;
    use std::vec::Vec;

    #[test]
    fn semantic_event_union_never_loses_ed_done_or_rx_abort_presence() {
        use Ieee802154ObservedEventState as State;

        assert_eq!(
            State::EdDoneAndRxAbortWithOther.union(State::RxAbortOnly),
            State::EdDoneAndRxAbortWithOther
        );
        assert_eq!(
            State::RxAbortWithOther.union(State::EdDoneWithOther),
            State::EdDoneAndRxAbortWithOther
        );
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
        let _hardware = cold
            .release()
            .expect("an untouched IEEE 802.15.4 route can be released");
    }

    #[derive(Debug, Eq, PartialEq)]
    enum InterruptTransitionOperation {
        StopOperation,
        StopTimer0,
        StopTimer1,
        MaskEvents,
        EnableRuntimeEvents,
        MaskTxAborts,
        EnableRuntimeTxAborts,
        MaskRxAborts,
        EnableRuntimeRxAborts,
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

        fn stop_operation(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::StopOperation);
        }

        fn stop_timer0(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::StopTimer0);
        }

        fn stop_timer1(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::StopTimer1);
        }

        fn mask_all_events(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::MaskEvents);
        }

        fn enable_runtime_events(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::EnableRuntimeEvents);
        }

        fn mask_all_tx_aborts(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::MaskTxAborts);
        }

        fn enable_runtime_tx_aborts(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::EnableRuntimeTxAborts);
        }

        fn mask_all_rx_aborts(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::MaskRxAborts);
        }

        fn enable_runtime_rx_aborts(&mut self) {
            self.operations
                .push(InterruptTransitionOperation::EnableRuntimeRxAborts);
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
                InterruptTransitionOperation::MaskEvents,
                InterruptTransitionOperation::EnableRuntimeTxAborts,
                InterruptTransitionOperation::EnableRuntimeRxAborts,
                InterruptTransitionOperation::OrderDeviceAccesses,
                InterruptTransitionOperation::SampleStaleEvents(0xa5),
                InterruptTransitionOperation::AcknowledgeStaleEvents(0xa5),
                InterruptTransitionOperation::EnableRuntimeEvents,
                InterruptTransitionOperation::OrderDeviceAccesses,
            ]
        );
    }

    #[test]
    fn production_deactivation_stops_every_engine_before_final_affine_ack() {
        let mut port = RecordingInterruptTransitionPort {
            event_identity: 0x3c,
            operations: Vec::new(),
        };
        execute_interrupt_deactivation(&mut port);

        assert_eq!(
            port.operations,
            [
                InterruptTransitionOperation::StopOperation,
                InterruptTransitionOperation::StopTimer0,
                InterruptTransitionOperation::StopTimer1,
                InterruptTransitionOperation::MaskEvents,
                InterruptTransitionOperation::MaskTxAborts,
                InterruptTransitionOperation::MaskRxAborts,
                InterruptTransitionOperation::OrderDeviceAccesses,
                InterruptTransitionOperation::SampleStaleEvents(0x3c),
                InterruptTransitionOperation::AcknowledgeStaleEvents(0x3c),
                InterruptTransitionOperation::OrderDeviceAccesses,
            ]
        );
    }
}
