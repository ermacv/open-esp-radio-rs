//! Executor-neutral A-MPDU completion and retained-retry state.
//!
//! This module owns the finite policy between one detached hardware
//! completion and the next publication decision. It deliberately does not
//! wait for interrupts, access MMIO, mutate DMA storage or choose an EDCA
//! backoff; those remain separate hardware/executor boundaries.

use crate::tx_ampdu::HtAmpduTxCompletion;

const IEEE80211_SEQUENCE_MASK: u16 = 0x0fff;
const HARDWARE_BLOCK_ACK_WINDOW: usize = 32;

/// Policy for retained A-MPDU retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryPolicy {
    /// Maximum number of aggregate publications, including the first one.
    pub attempt_limit: u8,
    /// Keep one missing MPDU in the aggregate owner.
    ///
    /// The recovered HE path requires this because converting a one-member
    /// HE A-MPDU to the ordinary queue first needs the distinct
    /// `ppHEAMPDU2Normal` metadata transition. The qualified HT path instead
    /// sends one remaining MPDU through its ordinary retry owner.
    pub retain_single_mpdu: bool,
}

/// Invalid construction or a disagreement with the pinned DMA owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryError {
    ZeroAttemptLimit,
    EmptyAggregate,
    CapacityExceedsHardwareWindow { capacity: usize },
    AggregateExceedsCapacity { subframes: u8, capacity: usize },
    FrameCountChanged { expected: u8, observed: u8 },
}

/// Driver-owned action after one BlockAck completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryDecision {
    /// Retain and compact the selected MPDUs, then publish another A-MPDU.
    RetainAggregate { retry_mask: u32 },
    /// End aggregate ownership; selected MPDUs require individual retry.
    Finish { retry_mask: u32 },
}

impl AmpduRetryDecision {
    pub const fn retry_mask(self) -> u32 {
        match self {
            Self::RetainAggregate { retry_mask } | Self::Finish { retry_mask } => retry_mask,
        }
    }

    pub const fn missing(self) -> u8 {
        self.retry_mask().count_ones() as u8
    }
}

/// One bounded BlockAck/retry transaction.
///
/// Sequence numbers are kept independently of descriptor indices because a
/// partial BlockAck retry compacts only missing MPDUs toward slot zero.
pub struct AmpduRetryState<const CAPACITY: usize> {
    sequences: [u16; CAPACITY],
    current_subframes: u8,
    policy: AmpduRetryPolicy,
    aggregate_attempts: u8,
    acknowledged: u8,
    block_ack_mpdu_attempts: u16,
}

impl<const CAPACITY: usize> AmpduRetryState<CAPACITY> {
    /// Start at the first Sequence Control value already consumed by the
    /// encoded aggregate.
    pub fn new(
        first_sequence: u16,
        subframes: u8,
        policy: AmpduRetryPolicy,
    ) -> Result<Self, AmpduRetryError> {
        if policy.attempt_limit == 0 {
            return Err(AmpduRetryError::ZeroAttemptLimit);
        }
        if CAPACITY > HARDWARE_BLOCK_ACK_WINDOW {
            return Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: CAPACITY });
        }
        if subframes == 0 {
            return Err(AmpduRetryError::EmptyAggregate);
        }
        if usize::from(subframes) > CAPACITY {
            return Err(AmpduRetryError::AggregateExceedsCapacity {
                subframes,
                capacity: CAPACITY,
            });
        }
        let mut sequences = [0_u16; CAPACITY];
        let mut index = 0_usize;
        while index < subframes as usize {
            sequences[index] = first_sequence.wrapping_add(index as u16) & IEEE80211_SEQUENCE_MASK;
            index += 1;
        }
        Ok(Self {
            sequences,
            current_subframes: subframes,
            policy,
            aggregate_attempts: 1,
            acknowledged: 0,
            block_ack_mpdu_attempts: 0,
        })
    }

    /// Apply one completion after the hardware queue has been detached.
    ///
    /// SOURCE: complete `_oracles/libpp.a[pp.o]::ppResortTxAMPDU` preserves
    /// Sequence Control and compacts only the MPDUs absent from BlockAck.
    /// Complete `_oracles/libpp.a[lmac.o]::lmacRetryTxFrame` skips
    /// `rcGetRate` for the state written by
    /// `lmacProcessLongRetryFail`, so a retained aggregate keeps its PHY rate.
    pub fn observe(
        &mut self,
        completion: HtAmpduTxCompletion,
        observed_subframes: u8,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        if observed_subframes != self.current_subframes {
            return Err(AmpduRetryError::FrameCountChanged {
                expected: self.current_subframes,
                observed: observed_subframes,
            });
        }
        self.block_ack_mpdu_attempts = self
            .block_ack_mpdu_attempts
            .saturating_add(u16::from(observed_subframes));

        let mut retry_mask = 0_u32;
        let mut index = 0_usize;
        while index < observed_subframes as usize {
            if completion.acknowledges(self.sequences[index]) {
                self.acknowledged = self.acknowledged.saturating_add(1);
            } else {
                retry_mask |= 1_u32 << index;
            }
            index += 1;
        }
        let missing = retry_mask.count_ones() as u8;
        let retain = (missing >= 2 || (missing == 1 && self.policy.retain_single_mpdu))
            && self.aggregate_attempts < self.policy.attempt_limit;
        if !retain {
            return Ok(AmpduRetryDecision::Finish { retry_mask });
        }

        let mut destination = 0_usize;
        let mut source = 0_usize;
        while source < observed_subframes as usize {
            if retry_mask & (1_u32 << source) != 0 {
                self.sequences[destination] = self.sequences[source];
                destination += 1;
            }
            source += 1;
        }
        self.current_subframes = missing;
        self.aggregate_attempts = self.aggregate_attempts.saturating_add(1);
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask })
    }

    pub const fn current_subframes(&self) -> u8 {
        self.current_subframes
    }

    pub const fn aggregate_attempts(&self) -> u8 {
        self.aggregate_attempts
    }

    pub const fn acknowledged(&self) -> u8 {
        self.acknowledged
    }

    pub const fn block_ack_mpdu_attempts(&self) -> u16 {
        self.block_ack_mpdu_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tx::{TxCompletion, TxCookie},
        tx_ampdu::{HtAmpduTxCompletion, HtBlockAckRegisters, TxBlockAckBitmap},
    };

    fn completion(status: u8, starting_sequence: u16, bitmap: u64) -> HtAmpduTxCompletion {
        HtAmpduTxCompletion {
            tx: TxCompletion {
                cookie: TxCookie(1),
                status,
                trigger_flow: false,
                used_alternate: false,
                auxiliary_a_word: 0,
                auxiliary_b_word: 0,
                auxiliary_c_word: 0,
                primary_word: 0,
                alternate_word: 0,
            },
            block_ack: HtBlockAckRegisters {
                control: 0,
                block_ack: TxBlockAckBitmap::new(starting_sequence, bitmap),
            },
        }
    }

    const HT_POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
        attempt_limit: 4,
        retain_single_mpdu: false,
    };

    #[test]
    fn partial_block_ack_compacts_sequences_across_retained_attempts() {
        let mut state = AmpduRetryState::<32>::new(0x0ffe, 4, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(0, 0x0ffe, 0b0101), 4),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 })
        );
        assert_eq!(state.current_subframes(), 2);
        assert_eq!(state.acknowledged(), 2);
        assert_eq!(state.block_ack_mpdu_attempts(), 4);

        // The retained sequences are 0x0fff and 1. A BlockAck starting at
        // 0x0fff acknowledges both across the 12-bit wrap.
        assert_eq!(
            state.observe(completion(0, 0x0fff, 0b0101), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
        );
        assert_eq!(state.acknowledged(), 4);
        assert_eq!(state.aggregate_attempts(), 2);
        assert_eq!(state.block_ack_mpdu_attempts(), 6);
    }

    #[test]
    fn nonzero_status_ignores_a_stale_block_ack_bitmap() {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(5, 20, u64::MAX), 2),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
        assert_eq!(state.acknowledged(), 0);
    }

    #[test]
    fn ht_finishes_one_missing_mpdu_but_he_retains_it() {
        let mut ht = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        assert_eq!(
            ht.observe(completion(0, 20, 0b01), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0b10 })
        );

        let mut he = AmpduRetryState::<4>::new(
            20,
            2,
            AmpduRetryPolicy {
                retain_single_mpdu: true,
                ..HT_POLICY
            },
        )
        .unwrap();
        assert_eq!(
            he.observe(completion(0, 20, 0b01), 2),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b10 })
        );
        assert_eq!(he.current_subframes(), 1);
    }

    #[test]
    fn attempt_limit_finishes_the_last_failed_aggregate() {
        let mut state = AmpduRetryState::<4>::new(
            100,
            2,
            AmpduRetryPolicy {
                attempt_limit: 2,
                retain_single_mpdu: false,
            },
        )
        .unwrap();
        assert!(matches!(
            state.observe(completion(5, 100, 0), 2),
            Ok(AmpduRetryDecision::RetainAggregate { .. })
        ));
        assert_eq!(
            state.observe(completion(5, 100, 0), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
        );
        assert_eq!(state.aggregate_attempts(), 2);
    }

    #[test]
    fn construction_and_dma_count_disagreements_fail_closed() {
        assert!(matches!(
            AmpduRetryState::<33>::new(0, 1, HT_POLICY),
            Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: 33 })
        ));
        assert!(matches!(
            AmpduRetryState::<2>::new(0, 3, HT_POLICY),
            Err(AmpduRetryError::AggregateExceedsCapacity { .. })
        ));
        let mut state = AmpduRetryState::<2>::new(0, 2, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(0, 0, 0b11), 1),
            Err(AmpduRetryError::FrameCountChanged {
                expected: 2,
                observed: 1
            })
        );
    }
}
