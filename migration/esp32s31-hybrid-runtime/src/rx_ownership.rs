//! Safe ownership state for fixed receive buffers.
//!
//! Storage addresses and ABI layout remain in the chip adapter. This module
//! contains only the host-testable ownership transitions, independent of raw
//! pointers and vendor state. Two bitmaps encode three states without one
//! atomic object per frame:
//!
//! ```text
//! claimed  network
//!    0         0     Free
//!    1         0     Radio
//!    1         1     Network
//! ```

use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RxBufferOwner {
    Free,
    Radio,
    Network,
}

pub(crate) struct RxBufferOwnershipWord {
    claimed: AtomicUsize,
    network: AtomicUsize,
}

impl RxBufferOwnershipWord {
    pub(crate) const fn new() -> Self {
        Self {
            claimed: AtomicUsize::new(0),
            network: AtomicUsize::new(0),
        }
    }

    pub(crate) fn claimed_bits(&self) -> usize {
        self.claimed.load(Ordering::Acquire)
    }

    pub(crate) fn owner(&self, slot: usize) -> Option<RxBufferOwner> {
        let bit = slot_bit(slot)?;
        let claimed = self.claimed.load(Ordering::Acquire) & bit != 0;
        let network = self.network.load(Ordering::Acquire) & bit != 0;
        match (claimed, network) {
            (false, false) => Some(RxBufferOwner::Free),
            (true, false) => Some(RxBufferOwner::Radio),
            (true, true) => Some(RxBufferOwner::Network),
            // Network ownership without a claim is only a transient internal
            // release state and is never exposed as usable ownership.
            (false, true) => None,
        }
    }

    /// Attempt exactly one Free -> Radio transition without waiting.
    pub(crate) fn try_claim_radio(&self, slot: usize) -> bool {
        let Some(bit) = slot_bit(slot) else {
            return false;
        };
        if self.claimed.fetch_or(bit, Ordering::AcqRel) & bit != 0 {
            return false;
        }
        if self.network.load(Ordering::Acquire) & bit == 0 {
            return true;
        }
        // A Network -> Free release may have cleared `claimed` just before
        // clearing `network`. Undo this one failed admission; the caller can
        // inspect another bounded slot and never waits for this one.
        self.claimed.fetch_and(!bit, Ordering::AcqRel);
        false
    }

    /// Attempt exactly one Radio -> Network transition without waiting.
    pub(crate) fn try_transfer_to_network(&self, slot: usize) -> bool {
        let Some(bit) = slot_bit(slot) else {
            return false;
        };
        if self.claimed.load(Ordering::Acquire) & bit == 0
            || self.network.fetch_or(bit, Ordering::AcqRel) & bit != 0
        {
            return false;
        }
        if self.claimed.load(Ordering::Acquire) & bit != 0 {
            return true;
        }
        self.network.fetch_and(!bit, Ordering::AcqRel);
        false
    }

    pub(crate) fn try_release(&self, slot: usize, owner: RxBufferOwner) -> bool {
        let Some(bit) = slot_bit(slot) else {
            return false;
        };
        match owner {
            RxBufferOwner::Free => false,
            RxBufferOwner::Radio => {
                if self.network.load(Ordering::Acquire) & bit != 0 {
                    return false;
                }
                self.claimed.fetch_and(!bit, Ordering::AcqRel) & bit != 0
            }
            RxBufferOwner::Network => {
                if self.network.load(Ordering::Acquire) & bit == 0 {
                    return false;
                }
                // Make the slot unavailable to pointer lookup before clearing
                // the network bit. A concurrent allocator can fail one claim
                // during this bounded transition but cannot alias the frame.
                if self.claimed.fetch_and(!bit, Ordering::AcqRel) & bit == 0 {
                    return false;
                }
                self.network.fetch_and(!bit, Ordering::AcqRel) & bit != 0
            }
        }
    }
}

fn slot_bit(slot: usize) -> Option<usize> {
    (slot < usize::BITS as usize).then(|| 1_usize << slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_transfer_is_unique_and_must_release_to_free() {
        let ownership = RxBufferOwnershipWord::new();
        assert!(ownership.try_claim_radio(3));
        assert_eq!(ownership.owner(3), Some(RxBufferOwner::Radio));
        assert!(ownership.try_transfer_to_network(3));
        assert_eq!(ownership.owner(3), Some(RxBufferOwner::Network));
        assert!(!ownership.try_transfer_to_network(3));
        assert!(!ownership.try_release(3, RxBufferOwner::Radio));
        assert!(ownership.try_release(3, RxBufferOwner::Network));
        assert_eq!(ownership.owner(3), Some(RxBufferOwner::Free));
    }

    #[test]
    fn slots_in_one_word_are_independent() {
        let ownership = RxBufferOwnershipWord::new();
        assert!(ownership.try_claim_radio(0));
        assert!(ownership.try_claim_radio(usize::BITS as usize - 1));
        assert!(!ownership.try_claim_radio(0));
        assert!(!ownership.try_release(0, RxBufferOwner::Network));
        assert_eq!(ownership.owner(0), Some(RxBufferOwner::Radio));
        assert_eq!(ownership.claimed_bits().count_ones(), 2);
        assert!(ownership.try_release(0, RxBufferOwner::Radio));
        assert_eq!(
            ownership.owner(usize::BITS as usize - 1),
            Some(RxBufferOwner::Radio)
        );
    }
}
