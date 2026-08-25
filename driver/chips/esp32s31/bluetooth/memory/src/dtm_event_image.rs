//! CPU-owned positional words covered by the reviewed DTM event transforms.
//!
//! These value types describe offsets inside the bound link-state and
//! scheduler-item allocations. Their public fields intentionally remain
//! forgeable so pure upper-layer transforms can construct and test them. Only
//! the memory-graph transition validates allocation-binding anchors; none of
//! these values grants publication or controller ownership.

#![forbid(unsafe_code)]

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

/// The nine scheduler-item words whose DTM event transform is complete.
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
