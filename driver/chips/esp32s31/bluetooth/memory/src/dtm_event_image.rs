//! CPU-owned positional words covered by the reviewed DTM event transforms.
//!
//! These value types describe offsets inside the bound link-state and
//! scheduler-item allocations. Raw words remain private to this memory crate;
//! upper layers can apply only the semantic transforms published below. The
//! memory-graph transition validates allocation-binding anchors, and none of
//! these values grants publication or controller ownership.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerSramLinkAddress,
    le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
    le_tx_power::rounded_tx_power,
};

const LOW_TWENTY_MASK: u32 = 0x000f_ffff;
const LINK_STATE_POWER_MASK: u32 = 0x0f80_0000;
const LINK_STATE_CONFIG_MASK: u32 = 0x3f00_0000;
const SCHEDULER_FREQUENCY_MASK: u32 = 0x0000_7f00;
const SCHEDULER_RATE_LANES_MASK: u32 = 0xf000_0000;
const SCHEDULER_ROUNDED_POWER_REGION_MASK: u32 = 0x0ff0_0000;
const INITIAL_RECEIVER_CONFIGURATION_IMAGE: u32 = 0x000f_0001;
const RECURRING_RECEIVER_CONFIGURATION_IMAGE: u32 = 0x0000_0001;

/// Opaque controller projection of this graph's current TX-header head.
///
/// The memory graph creates this value only while producing a positional
/// event seed. It carries neither a general SRAM address nor publication
/// authority, so it cannot be substituted for the RX-header tail projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxHeaderHeadProjection(u32);

impl BluetoothDtmTxHeaderHeadProjection {
    pub(super) const fn from_bound(address: BluetoothControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn from_link_state_word(word: u32) -> Self {
        Self(word & LOW_TWENTY_MASK)
    }

    const fn apply_to_link_state_word(self, word: u32) -> u32 {
        (word & !LOW_TWENTY_MASK) | self.0
    }
}

/// Opaque controller projection of this graph's current RX-header tail.
///
/// The memory graph creates this value only while producing a positional
/// event seed. It carries neither a general SRAM address nor publication
/// authority, so it cannot be substituted for the TX-header head projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxHeaderTailProjection(u32);

impl BluetoothDtmRxHeaderTailProjection {
    pub(super) const fn from_bound(address: BluetoothControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn from_link_state_word(word: u32) -> Self {
        Self(word & LOW_TWENTY_MASK)
    }

    const fn apply_to_link_state_word(self, word: u32) -> u32 {
        (word & !LOW_TWENTY_MASK) | self.0
    }
}

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

/// PHY choice admitted by a transmitter DTM scheduler item.
///
/// These variants retain the role-specific meaning established by the DTM
/// command validator. They deliberately do not expose the controller field
/// image used by the private SRAM codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerTransmitterPhy {
    /// LE 1M transmitter.
    Le1M,
    /// LE 2M transmitter.
    Le2M,
    /// LE Coded transmitter using S=8 coding.
    LeCodedS8,
    /// LE Coded transmitter using S=2 coding.
    LeCodedS2,
}

/// PHY choice admitted by a receiver DTM scheduler item.
///
/// The receiver command has no S=2 selector. Encoding that restriction in the
/// type prevents a transmitter-only choice from reaching the SRAM codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerReceiverPhy {
    /// LE 1M receiver.
    Le1M,
    /// LE 2M receiver.
    Le2M,
    /// Generic LE Coded receiver.
    LeCoded,
}

/// Private encoding applied to both replicated scheduler PHY lanes.
///
/// Current vendor bodies prove that both lanes receive the same two-bit code,
/// but do not establish independent names for the lanes. Keeping the code in
/// this private value avoids inventing that missing hardware semantic.
#[derive(Clone, Copy)]
struct BluetoothDtmSchedulerPhyEncoding(u32);

impl BluetoothDtmSchedulerPhyEncoding {
    const fn for_transmitter(phy: BluetoothDtmSchedulerTransmitterPhy) -> Self {
        Self(match phy {
            BluetoothDtmSchedulerTransmitterPhy::Le1M => 0,
            BluetoothDtmSchedulerTransmitterPhy::Le2M => 1,
            BluetoothDtmSchedulerTransmitterPhy::LeCodedS8 => 2,
            BluetoothDtmSchedulerTransmitterPhy::LeCodedS2 => 3,
        })
    }

    const fn for_receiver(phy: BluetoothDtmSchedulerReceiverPhy) -> Self {
        Self(match phy {
            BluetoothDtmSchedulerReceiverPhy::Le1M => 0,
            BluetoothDtmSchedulerReceiverPhy::Le2M => 1,
            BluetoothDtmSchedulerReceiverPhy::LeCoded => 3,
        })
    }
}

/// Private codec for the link-state word changed by the DTM reset profile.
///
/// Both reviewed implementations select the same two-bit image in this word,
/// and the complete descriptor is subsequently reachable by the controller.
/// No field-specific hardware reader or independent bit names have been
/// recovered, so only the observed aggregate DTM selection is exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothDtmLinkStateProfileWord(u32);

impl BluetoothDtmLinkStateProfileWord {
    const DIRECT_TEST_MODE_MASK: u32 = 0x0c00_0000;

    pub(super) const fn from_storage(word: u32) -> Self {
        Self(word)
    }

    const fn select_direct_test_mode(self) -> Self {
        Self((self.0 & !Self::DIRECT_TEST_MODE_MASK) | Self::DIRECT_TEST_MODE_MASK)
    }

    pub(super) const fn into_storage(self) -> u32 {
        self.0
    }

    #[cfg(test)]
    pub(super) const fn direct_test_mode_is_selected(self) -> bool {
        self.0 & Self::DIRECT_TEST_MODE_MASK == Self::DIRECT_TEST_MODE_MASK
    }
}

/// Role and phase selecting one reviewed scheduler-item event transform.
///
/// Transmitter events have no phase-dependent scheduler-item configuration.
/// Nesting the receiver phase in the receiver variant prevents a caller from
/// attaching receiver-only phase semantics to a transmitter descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerItemEventType {
    /// One transmitter-test scheduler item.
    Transmitter(BluetoothDtmSchedulerTransmitterPhy),
    /// One receiver-test scheduler item in its explicit session phase.
    Receiver {
        /// Initial or recurring receiver configuration.
        phase: BluetoothDtmReceiverEventPhase,
        /// Receiver-valid PHY retained without exposing its field image.
        phy: BluetoothDtmSchedulerReceiverPhy,
    },
}

impl BluetoothDtmSchedulerItemEventType {
    /// Return the DTM role selected by this event type.
    pub const fn role(self) -> BluetoothDtmRole {
        match self {
            Self::Transmitter(_) => BluetoothDtmRole::Transmitter,
            Self::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    /// Return the receiver phase, or `None` for a transmitter event.
    pub const fn receiver_phase(self) -> Option<BluetoothDtmReceiverEventPhase> {
        match self {
            Self::Transmitter(_) => None,
            Self::Receiver { phase, .. } => Some(phase),
        }
    }

    const fn phy_encoding(self) -> BluetoothDtmSchedulerPhyEncoding {
        match self {
            Self::Transmitter(phy) => BluetoothDtmSchedulerPhyEncoding::for_transmitter(phy),
            Self::Receiver { phy, .. } => BluetoothDtmSchedulerPhyEncoding::for_receiver(phy),
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
    pub(crate) word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub(crate) word_04: u32,
    /// Complete word at byte offset `+0x08`; low 20 bits carry the RX tail.
    pub(crate) word_08: u32,
    /// Private word at byte offset `+0x14`; its field-specific hardware
    /// meaning remains unresolved.
    pub(super) profile_word_14: BluetoothDtmLinkStateProfileWord,
    /// Protocol-level CRC initialization value encoded in the low 24 bits by
    /// the private SRAM codec. The high byte remains opaque and preserved.
    pub(super) crc_init: BluetoothLeCrcInit,
    /// Complete word at byte offset `+0x34`.
    pub(crate) word_34: u32,
    /// Protocol-level synchronization value encoded by the private SRAM
    /// codec. This does not grant hardware-consumer authority.
    pub(super) access_address: BluetoothLeAccessAddress,
    /// Complete word at byte offset `+0x50`.
    pub(crate) word_50: u32,
}

impl BluetoothDtmLinkStateReviewedWords {
    /// Apply the complete reviewed DTM reset to this CPU-owned word subset.
    ///
    /// All positional encoding stays inside the controller-memory codec. The
    /// caller supplies only validated semantic values and bound graph links.
    pub const fn apply_reset(
        mut self,
        tx_head: Option<BluetoothDtmTxHeaderHeadProjection>,
        rx_tail: Option<BluetoothDtmRxHeaderTailProjection>,
        default_tx_power_dbm: i8,
        config: u8,
        role: BluetoothDtmRole,
    ) -> Self {
        let word_00_with_link = match tx_head {
            Some(tx_head) => tx_head.apply_to_link_state_word(self.word_00),
            None => self.word_00 & !LOW_TWENTY_MASK,
        };
        let transformed_high_half = (((word_00_with_link >> 16) as u16 & 0x600f) | 0x8ff0) as u32;

        self.word_00 = (word_00_with_link & 0x0000_ffff) | (transformed_high_half << 16);
        self.word_04 = (self.word_04 & !LINK_STATE_POWER_MASK)
            | ((rounded_tx_power(default_tx_power_dbm) as u32) << 23);
        self.word_08 = match rx_tail {
            Some(rx_tail) => rx_tail.apply_to_link_state_word(self.word_08),
            None => self.word_08 & !LOW_TWENTY_MASK,
        };
        self.word_08 = (self.word_08 & 0xf00f_ffff) | 0x0ff0_0000;
        self.profile_word_14 = self.profile_word_14.select_direct_test_mode();
        if matches!(role, BluetoothDtmRole::Receiver) {
            self.word_34 = 0;
        }
        self.word_50 = (self.word_50 & !LINK_STATE_CONFIG_MASK) | ((config as u32) << 24);
        self.with_crc_init(BluetoothLeCrcInit::LE_PRESET)
            .with_access_address(BluetoothLeAccessAddress::DIRECT_TEST_MODE)
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
    const fn rounded_power(self) -> u8 {
        ((self.word_04 & LINK_STATE_POWER_MASK) >> 23) as u8
    }

    pub(super) const fn access_address(self) -> BluetoothLeAccessAddress {
        self.access_address
    }

    pub(super) const fn crc_init(self) -> BluetoothLeCrcInit {
        self.crc_init
    }

    const fn with_crc_init(mut self, crc_init: BluetoothLeCrcInit) -> Self {
        self.crc_init = crc_init;
        self
    }

    const fn with_access_address(mut self, access_address: BluetoothLeAccessAddress) -> Self {
        self.access_address = access_address;
        self
    }
}

/// The eleven scheduler-item words whose DTM event transform is complete.
///
/// Names are byte offsets. This is not the complete scheduler object and has
/// no list-linkage or hardware-ownership authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerItemReviewedWords {
    /// Complete word at byte offset `+0x00`; only byte `+0x02` is transformed.
    pub(crate) word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub(crate) word_04: u32,
    /// Complete word at byte offset `+0x08`; low 20 bits retain link-state.
    pub(crate) word_08: u32,
    /// Complete sequence-start word at byte offset `+0x0c`.
    pub(crate) word_0c: u32,
    /// Complete sequence-duration word at byte offset `+0x10`.
    pub(crate) word_10: u32,
    /// Complete word at byte offset `+0x14`.
    pub(crate) word_14: u32,
    /// Complete word at byte offset `+0x18`.
    pub(crate) word_18: u32,
    /// Complete word at byte offset `+0x2c`.
    pub(crate) word_2c: u32,
    /// Complete raw-time word at byte offset `+0x44`.
    pub(crate) word_44: u32,
    /// Complete raw-time word at byte offset `+0x48`.
    pub(crate) word_48: u32,
    /// Complete word at byte offset `+0x4c`; only its low byte is cleared.
    pub(crate) word_4c: u32,
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
        let rate = event_type.phy_encoding().0;

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
                BluetoothDtmSchedulerItemEventType::Transmitter(_) => self.word_2c,
                BluetoothDtmSchedulerItemEventType::Receiver {
                    phase: BluetoothDtmReceiverEventPhase::Initial,
                    ..
                } => INITIAL_RECEIVER_CONFIGURATION_IMAGE,
                BluetoothDtmSchedulerItemEventType::Receiver {
                    phase: BluetoothDtmReceiverEventPhase::Recurring,
                    ..
                } => RECURRING_RECEIVER_CONFIGURATION_IMAGE,
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

    pub(super) const fn tx_header_head_projection(self) -> BluetoothDtmTxHeaderHeadProjection {
        BluetoothDtmTxHeaderHeadProjection::from_link_state_word(self.link_state.word_00)
    }

    pub(super) const fn rx_header_tail_projection(self) -> BluetoothDtmRxHeaderTailProjection {
        BluetoothDtmRxHeaderTailProjection::from_link_state_word(self.link_state.word_08)
    }

    pub(super) const fn scheduler_item_retains_link_state(
        self,
        link_state: BluetoothControllerSramLinkAddress,
    ) -> bool {
        self.scheduler_item.word_08 & LOW_TWENTY_MASK == link_state.compressed_image()
    }
}

/// Private codec for the complete scheduler-item word containing the
/// hardware-consumed successor link.
///
/// The common scheduler treats the low twenty bits as a compressed successor
/// and preserves the independent prefix. Keeping the complete word inside
/// this type lets the memory owner terminate or roll back the chain without
/// exposing the field image to its typestate transitions.
#[derive(Clone, Copy)]
pub(super) struct BluetoothDtmSchedulerHardwareChainWord(u32);

impl BluetoothDtmSchedulerHardwareChainWord {
    pub(super) const fn from_storage(word: u32) -> Self {
        Self(word)
    }

    pub(super) const fn terminate(self) -> Self {
        Self(self.0 & !LOW_TWENTY_MASK)
    }

    pub(super) const fn into_storage(self) -> u32 {
        self.0
    }
}
