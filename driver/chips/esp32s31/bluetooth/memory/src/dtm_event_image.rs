//! CPU-owned positional words covered by the reviewed DTM event transforms.
//!
//! These value types describe offsets inside the bound link-state and
//! scheduler-item allocations. Their public fields intentionally remain
//! forgeable so pure upper-layer transforms can construct and test them. Only
//! the memory-graph transition validates allocation-binding anchors; none of
//! these values grants publication or controller ownership.

#![forbid(unsafe_code)]

use crate::BluetoothDtmBoundSramLinkAddress;

const LOW_TWENTY_MASK: u32 = 0x000f_ffff;
const LINK_STATE_POWER_MASK: u32 = 0x0f80_0000;
const LINK_STATE_CONFIG_MASK: u32 = 0x3f00_0000;
const SCHEDULER_FREQUENCY_MASK: u32 = 0x0000_7f00;
const SCHEDULER_RATE_LANES_MASK: u32 = 0xf000_0000;
const SCHEDULER_ROUNDED_POWER_REGION_MASK: u32 = 0x0ff0_0000;
const INITIAL_RECEIVER_CONFIGURATION_IMAGE: u32 = 0x000f_0001;
const RECURRING_RECEIVER_CONFIGURATION_IMAGE: u32 = 0x0000_0001;

/// DTM role shared by the CPU-owned link-state and scheduler-item formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRole {
    /// Repeated transmitter-test events use role image one.
    Transmitter,
    /// Repeated receiver-test events use role image two.
    Receiver,
}

/// Phase selecting the role-private receiver configuration for one DTM event.
///
/// The initial event establishes the complete receiver configuration. After
/// one completion has returned the private graph, the recurring vendor path
/// deliberately replaces it with the narrower re-use configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReceiverEventPhase {
    /// First receiver event in one active test session.
    Initial,
    /// Receiver event prepared from a recycled active-session graph.
    Recurring,
}

/// Role and phase selecting one reviewed scheduler-item event transform.
///
/// Transmitter events have no phase-dependent scheduler-item configuration.
/// Nesting the receiver phase in the receiver variant prevents a caller from
/// attaching receiver-only phase semantics to a transmitter descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerItemEventType {
    /// One transmitter-test scheduler item.
    Transmitter,
    /// One receiver-test scheduler item in its explicit session phase.
    Receiver(BluetoothDtmReceiverEventPhase),
}

impl BluetoothDtmSchedulerItemEventType {
    /// Return the DTM role selected by this event type.
    pub const fn role(self) -> BluetoothDtmRole {
        match self {
            Self::Transmitter => BluetoothDtmRole::Transmitter,
            Self::Receiver(_) => BluetoothDtmRole::Receiver,
        }
    }

    /// Return the receiver phase, or `None` for a transmitter event.
    pub const fn receiver_phase(self) -> Option<BluetoothDtmReceiverEventPhase> {
        match self {
            Self::Transmitter => None,
            Self::Receiver(phase) => Some(phase),
        }
    }
}

/// The eight link-state words whose reset behavior is complete.
///
/// Names are byte offsets, not semantic descriptor fields. The omitted bytes
/// and the hardware consumer remain unresolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmLinkStateReviewedWords {
    /// Complete word at byte offset `+0x00`; low 20 bits carry the TX head.
    pub word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub word_04: u32,
    /// Complete word at byte offset `+0x08`; low 20 bits carry the RX tail.
    pub word_08: u32,
    /// Complete word at byte offset `+0x14`.
    pub word_14: u32,
    /// Complete word at byte offset `+0x2c`.
    pub word_2c: u32,
    /// Complete word at byte offset `+0x34`.
    pub word_34: u32,
    /// Complete word at byte offset `+0x38`.
    pub word_38: u32,
    /// Complete word at byte offset `+0x50`.
    pub word_50: u32,
}

impl BluetoothDtmLinkStateReviewedWords {
    /// Apply the complete reviewed DTM reset to this CPU-owned word subset.
    ///
    /// All positional encoding stays inside the controller-memory codec. The
    /// caller supplies only validated semantic values and bound graph links.
    pub const fn apply_reset(
        self,
        tx_head: Option<BluetoothDtmBoundSramLinkAddress>,
        rx_tail: Option<BluetoothDtmBoundSramLinkAddress>,
        rounded_power: u8,
        config: u8,
        role: BluetoothDtmRole,
    ) -> Self {
        let tx_head = compressed_or_zero(tx_head);
        let rx_tail = compressed_or_zero(rx_tail);
        let word_00_with_link = (self.word_00 & !LOW_TWENTY_MASK) | tx_head;
        let transformed_high_half = (((word_00_with_link >> 16) as u16 & 0x600f) | 0x8ff0) as u32;

        Self {
            word_00: (word_00_with_link & 0x0000_ffff) | (transformed_high_half << 16),
            word_04: (self.word_04 & !LINK_STATE_POWER_MASK) | ((rounded_power as u32) << 23),
            word_08: (self.word_08 & 0xf000_0000) | 0x0ff0_0000 | rx_tail,
            word_14: self.word_14 | 0xc000_0000,
            word_2c: (self.word_2c & 0xff00_0000) | 0x0055_5555,
            word_34: match role {
                BluetoothDtmRole::Transmitter => self.word_34,
                BluetoothDtmRole::Receiver => 0,
            },
            word_38: 0x7176_4129,
            word_50: (self.word_50 & !LINK_STATE_CONFIG_MASK) | ((config as u32) << 24),
        }
    }

    /// Apply the role-specific link-state write performed while constructing
    /// one event after reset.
    ///
    /// The reviewed receiver path replaces word `+0x34` with the raw
    /// controller-time projection of scheduler origin zero. The transmitter
    /// path does not write this word. This remains a CPU-owned positional
    /// transform and grants no descriptor-publication authority.
    pub const fn apply_event_context(
        mut self,
        role: BluetoothDtmRole,
        raw_scheduler_origin: u32,
    ) -> Self {
        if matches!(role, BluetoothDtmRole::Receiver) {
            self.word_34 = raw_scheduler_origin;
        }
        self
    }

    /// Return the five-bit rounded-power value consumed by scheduler insert.
    pub const fn rounded_power(self) -> u8 {
        ((self.word_04 & LINK_STATE_POWER_MASK) >> 23) as u8
    }

    /// Return the compressed private TX-head link image.
    pub const fn tx_head_link_image(self) -> u32 {
        self.word_00 & LOW_TWENTY_MASK
    }

    /// Return the compressed private RX-tail link image.
    pub const fn rx_tail_link_image(self) -> u32 {
        self.word_08 & LOW_TWENTY_MASK
    }
}

/// The eleven scheduler-item words whose DTM event transform is complete.
///
/// Names are byte offsets. This is not the complete scheduler object and has
/// no list-linkage or hardware-ownership authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerItemReviewedWords {
    /// Complete word at byte offset `+0x00`; only byte `+0x02` is transformed.
    pub word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub word_04: u32,
    /// Complete word at byte offset `+0x08`; low 20 bits retain link-state.
    pub word_08: u32,
    /// Complete sequence-start word at byte offset `+0x0c`.
    pub word_0c: u32,
    /// Complete sequence-duration word at byte offset `+0x10`.
    pub word_10: u32,
    /// Complete word at byte offset `+0x14`.
    pub word_14: u32,
    /// Complete word at byte offset `+0x18`.
    pub word_18: u32,
    /// Complete word at byte offset `+0x2c`.
    pub word_2c: u32,
    /// Complete raw-time word at byte offset `+0x44`.
    pub word_44: u32,
    /// Complete raw-time word at byte offset `+0x48`.
    pub word_48: u32,
    /// Complete word at byte offset `+0x4c`; only its low byte is cleared.
    pub word_4c: u32,
}

impl BluetoothDtmSchedulerItemReviewedWords {
    /// Apply every complete reviewed DTM event transform before insertion.
    ///
    /// `frequency` and `rate` are already validated controller images;
    /// `event_type` carries the role and the receiver's initial/recurring
    /// configuration choice; scheduler times are raw values derived from the
    /// owned epoch.
    pub const fn apply_event(
        self,
        frequency: u8,
        rate: u8,
        event_type: BluetoothDtmSchedulerItemEventType,
        scheduler_start: u32,
        scheduler_end: u32,
    ) -> Self {
        let role = event_type.role();
        let current_role_byte = ((self.word_00 >> 16) & 0xff) as u8;
        let role_byte = (current_role_byte & 0xaf)
            | match role {
                BluetoothDtmRole::Transmitter => 0x10,
                BluetoothDtmRole::Receiver => 0x40,
            };
        let rate = rate as u32;

        Self {
            word_00: (self.word_00 & 0xff00_ffff) | ((role_byte as u32) << 16),
            word_04: self.word_04 | 0x8000_0000,
            word_08: self.word_08 & 0xff0f_ffff,
            word_0c: self.word_0c,
            word_10: self.word_10,
            word_14: (self.word_14 & !SCHEDULER_RATE_LANES_MASK) | (rate << 28) | (rate << 30),
            word_18: (self.word_18 & !(SCHEDULER_FREQUENCY_MASK | 0x0f))
                | ((frequency as u32) << 8)
                | 0x03,
            word_2c: match event_type {
                BluetoothDtmSchedulerItemEventType::Transmitter => self.word_2c,
                BluetoothDtmSchedulerItemEventType::Receiver(
                    BluetoothDtmReceiverEventPhase::Initial,
                ) => INITIAL_RECEIVER_CONFIGURATION_IMAGE,
                BluetoothDtmSchedulerItemEventType::Receiver(
                    BluetoothDtmReceiverEventPhase::Recurring,
                ) => RECURRING_RECEIVER_CONFIGURATION_IMAGE,
            },
            word_44: scheduler_start,
            word_48: scheduler_end,
            word_4c: self.word_4c & 0xffff_ff00,
        }
    }

    /// Apply the complete per-item sequence-time projection.
    ///
    /// The common scheduler adds its configured raw-time lead to the already
    /// projected start and stores the wrapping window length separately. The
    /// broker notification performed between these two writes belongs to the
    /// scheduler runtime, not this CPU-owned memory codec.
    pub const fn apply_sequence_timing(mut self, raw_sequence_lead: u32) -> Self {
        self.word_0c = self.word_44.wrapping_add(raw_sequence_lead);
        self.word_10 = self.word_48.wrapping_sub(self.word_44);
        self
    }

    /// Apply the common overlap-insertion rounded-power projection.
    pub const fn apply_overlap_insertion_power(
        mut self,
        link_state: BluetoothDtmLinkStateReviewedWords,
    ) -> Self {
        let rounded_power = link_state.rounded_power() as u32;
        self.word_14 =
            (self.word_14 & !SCHEDULER_ROUNDED_POWER_REGION_MASK) | (rounded_power << 20);
        self
    }

    /// Return the compressed link-state link image retained by this item.
    pub const fn link_state_link_image(self) -> u32 {
        self.word_08 & LOW_TWENTY_MASK
    }
}

const fn compressed_or_zero(address: Option<BluetoothDtmBoundSramLinkAddress>) -> u32 {
    match address {
        Some(address) => address.compressed_image(),
        None => 0,
    }
}

/// Complete CPU-side DTM event word subset accepted by the memory graph.
///
/// This aggregate is positional, not proof that the upper DTM transforms were
/// used. The consuming graph transition separately validates its three bound
/// links before writing any word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmPositionalEventWords {
    link_state: BluetoothDtmLinkStateReviewedWords,
    scheduler_item: BluetoothDtmSchedulerItemReviewedWords,
}

impl BluetoothDtmPositionalEventWords {
    /// Pair the two positional word subsets without publishing either object.
    pub const fn new(
        link_state: BluetoothDtmLinkStateReviewedWords,
        scheduler_item: BluetoothDtmSchedulerItemReviewedWords,
    ) -> Self {
        Self {
            link_state,
            scheduler_item,
        }
    }

    /// Return the complete reviewed link-state subset.
    pub const fn link_state(self) -> BluetoothDtmLinkStateReviewedWords {
        self.link_state
    }

    /// Return the complete reviewed scheduler-item subset.
    pub const fn scheduler_item(self) -> BluetoothDtmSchedulerItemReviewedWords {
        self.scheduler_item
    }
}
