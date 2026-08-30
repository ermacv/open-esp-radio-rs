//! Role-neutral state for one fused protocol/DMA/protocol RX turn.
//!
//! STA and AP have different protocol consumers, but they must share the
//! physical scheduling invariants: frame credit is consumed only by staged
//! owners, one DMA frontier is serviced between the pre- and post-protocol
//! phases, and a synchronously returned staging credit keeps a blocked
//! frontier runnable.

use crate::datapath::{DatapathRxProgress, DatapathRxServiceContext};

/// Resolve a role-local frame bound against the scheduler's remaining credit.
///
/// Zero is never a useful service bound. Both a zero role limit and an
/// explicit zero scheduler credit therefore preserve one finite progress
/// opportunity instead of creating an owner which can never drain.
pub(crate) const fn protocol_frame_limit(
    context: DatapathRxServiceContext,
    role_limit: usize,
) -> usize {
    let role_limit = if role_limit == 0 { 1 } else { role_limit };
    match context.maximum_protocol_frames {
        Some(0) => 1,
        Some(limit) if limit < role_limit => limit,
        _ => role_limit,
    }
}

/// State of one bounded fused RX turn.
///
/// This value owns no packet, descriptor or hardware capability. It records
/// the scheduling proof around those typed owners so role adapters cannot
/// silently diverge in budget accounting or completion mapping.
pub(crate) struct FusedRxTurn {
    frame_limit: usize,
    serviced_frames: usize,
    serviced_after_dma: usize,
    dma_progress: Option<DatapathRxProgress>,
    protocol_work_remaining: bool,
}

impl FusedRxTurn {
    pub(crate) const fn new(frame_limit: usize) -> Self {
        Self {
            frame_limit: if frame_limit == 0 { 1 } else { frame_limit },
            serviced_frames: 0,
            serviced_after_dma: 0,
            dma_progress: None,
            protocol_work_remaining: false,
        }
    }

    pub(crate) const fn from_context(context: DatapathRxServiceContext, role_limit: usize) -> Self {
        Self::new(protocol_frame_limit(context, role_limit))
    }

    pub(crate) const fn has_protocol_budget(&self) -> bool {
        self.serviced_frames < self.frame_limit
    }

    pub(crate) const fn remaining_protocol_frames(&self) -> usize {
        self.frame_limit.saturating_sub(self.serviced_frames)
    }

    /// Record staged owners actually consumed by the protocol phase.
    ///
    /// Observation passes and empty probes do not spend frame credit. A role
    /// which needs a non-frame action bound must keep that bound in its own
    /// protocol consumer; it must not make DMA cadence depend on empty calls.
    pub(crate) fn observe_protocol(&mut self, serviced_frames: usize, work_remaining: bool) {
        self.serviced_frames = self.serviced_frames.saturating_add(serviced_frames);
        if self.dma_progress.is_some() {
            self.serviced_after_dma = self.serviced_after_dma.saturating_add(serviced_frames);
        }
        self.protocol_work_remaining |= work_remaining;
    }

    pub(crate) const fn dma_service_required(&self) -> bool {
        self.dma_progress.is_none()
    }

    /// Close the single physical DMA phase of this fused turn.
    pub(crate) fn observe_dma(&mut self, progress: DatapathRxProgress) {
        assert!(
            self.dma_progress.is_none(),
            "one fused RX turn cannot service two DMA frontiers"
        );
        self.dma_progress = Some(progress);
    }

    pub(crate) const fn dma_progress(&self) -> Option<DatapathRxProgress> {
        self.dma_progress
    }

    /// Resolve the physical result at the fused owner boundary.
    ///
    /// `StageCapacityBlocked` is an observation made inside the DMA phase. If
    /// post-DMA protocol work has already returned a staging credit, waiting
    /// for another capacity edge would lose a runnable continuation which is
    /// already owned by this same turn.
    pub(crate) fn finish(self, additional_protocol_work: bool) -> DatapathRxProgress {
        let dma_progress = self
            .dma_progress
            .expect("fused RX completion requires one observed DMA frontier");
        let stage_capacity_released = dma_progress == DatapathRxProgress::StageCapacityBlocked
            && self.serviced_after_dma != 0;
        if self.protocol_work_remaining || additional_protocol_work || stage_capacity_released {
            DatapathRxProgress::BudgetExhausted
        } else {
            dma_progress
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_credit_caps_the_role_limit_and_never_creates_zero_budget() {
        assert_eq!(
            protocol_frame_limit(
                DatapathRxServiceContext {
                    maximum_protocol_frames: None,
                },
                32,
            ),
            32
        );
        assert_eq!(
            protocol_frame_limit(
                DatapathRxServiceContext {
                    maximum_protocol_frames: Some(4),
                },
                32,
            ),
            4
        );
        assert_eq!(
            protocol_frame_limit(
                DatapathRxServiceContext {
                    maximum_protocol_frames: Some(0),
                },
                32,
            ),
            1
        );
        assert_eq!(
            protocol_frame_limit(
                DatapathRxServiceContext {
                    maximum_protocol_frames: None,
                },
                0,
            ),
            1
        );
    }

    #[test]
    fn empty_protocol_observations_do_not_spend_frame_credit() {
        let mut turn = FusedRxTurn::new(2);

        turn.observe_protocol(0, false);
        turn.observe_protocol(0, false);

        assert!(turn.has_protocol_budget());
        assert_eq!(turn.remaining_protocol_frames(), 2);
    }

    #[test]
    fn only_post_dma_consumption_releases_an_observed_capacity_block() {
        let mut blocked = FusedRxTurn::new(4);
        blocked.observe_protocol(1, false);
        blocked.observe_dma(DatapathRxProgress::StageCapacityBlocked);
        assert_eq!(
            blocked.finish(false),
            DatapathRxProgress::StageCapacityBlocked
        );

        let mut released = FusedRxTurn::new(4);
        released.observe_dma(DatapathRxProgress::StageCapacityBlocked);
        released.observe_protocol(1, false);
        assert_eq!(released.finish(false), DatapathRxProgress::BudgetExhausted);
    }

    #[test]
    fn protocol_backlog_preserves_a_runnable_continuation() {
        let mut turn = FusedRxTurn::new(4);
        turn.observe_protocol(4, true);
        turn.observe_dma(DatapathRxProgress::Drained);

        assert_eq!(turn.finish(false), DatapathRxProgress::BudgetExhausted);
    }

    #[test]
    #[should_panic(expected = "one fused RX turn cannot service two DMA frontiers")]
    fn a_fused_turn_rejects_a_second_dma_phase() {
        let mut turn = FusedRxTurn::new(4);
        turn.observe_dma(DatapathRxProgress::Drained);
        turn.observe_dma(DatapathRxProgress::Drained);
    }
}
