//! Lazy construction of one radio-selected prefix in reserved physical storage.

use core::{
    cell::RefCell,
    num::{NonZeroU16, NonZeroU32},
};

use crate::{
    EgressSelection, EgressWorkProvider, FillStopReason, PhysicalTxSource, ReservedTxBatch,
};

/// Construction accounting when the radio releases a selected source.
///
/// A missing stop means that the radio stopped requesting frames before the
/// selection was exhausted. Unrequested work remains with the provider.
#[derive(Debug)]
#[must_use = "construction failures must be handled independently of radio completion"]
pub struct SelectedTxReport<WriteError> {
    pub frames: u16,
    pub bytes: u32,
    pub stop: Option<Result<FillStopReason, WriteError>>,
}

/// Builds only the frames actually requested by the radio.
///
/// The caller selects the current radio key and reserves physical storage
/// before constructing this source. The exclusive provider borrow freezes
/// that selection for this synchronous service turn. Dropping the source
/// returns unused reservations; it never removes unrequested network work.
/// Taken physical owners may outlive the source and retain their own credits.
/// The caller must authorize the key against the live radio epoch and supply
/// a batch for that interface; this source does not own radio lifecycle state.
pub struct SelectedTxSource<'provider, Provider: EgressWorkProvider, Batch> {
    state: RefCell<SelectedState<'provider, Provider, Batch>>,
}

struct SelectedState<'provider, Provider: EgressWorkProvider, Batch> {
    provider: &'provider mut Provider,
    batch: Batch,
    selection: EgressSelection,
    pending: u16,
    report: SelectedTxReport<Provider::WriteError>,
}

impl<'provider, Provider, Batch> SelectedTxSource<'provider, Provider, Batch>
where
    Provider: EgressWorkProvider,
    Batch: ReservedTxBatch + PhysicalTxSource,
{
    pub fn new(
        provider: &'provider mut Provider,
        selection: EgressSelection,
        batch: Batch,
    ) -> Self {
        assert_eq!(
            batch.pending_frames(),
            0,
            "lazy selection requires an empty reserved batch"
        );
        // The exclusive provider borrow permits one demand snapshot per turn;
        // aggregate lookahead does not rescan the flow table for every MPDU.
        let mut pending = 0;
        provider.visit_demands(|demand| {
            if demand.key == selection.key {
                pending = demand.ready_frames.min(selection.max_frames.get());
            }
        });
        Self {
            state: RefCell::new(SelectedState {
                provider,
                batch,
                selection,
                pending,
                report: SelectedTxReport {
                    frames: 0,
                    bytes: 0,
                    stop: None,
                },
            }),
        }
    }

    /// End this selection, release unused destinations, and report construction
    /// progress. This does not imply radio completion or delivery.
    pub fn finish(self) -> SelectedTxReport<Provider::WriteError> {
        self.state.into_inner().report
    }
}

impl<Provider, Batch> PhysicalTxSource for SelectedTxSource<'_, Provider, Batch>
where
    Provider: EgressWorkProvider,
    Batch: ReservedTxBatch + PhysicalTxSource,
{
    type Frame = Batch::Frame;

    fn pending_frames(&self) -> usize {
        let state = self.state.borrow();
        if state.report.stop.is_some() {
            return 0;
        }
        usize::from(state.pending)
    }

    fn try_take_physical(&self) -> Option<Self::Frame> {
        let mut state = self.state.borrow_mut();
        if state.report.stop.is_some() {
            return None;
        }
        let budget = state.selection.max_bytes.get() - state.report.bytes;
        let Some(max_bytes) = NonZeroU32::new(budget) else {
            state.report.stop = Some(Ok(FillStopReason::ByteBudget));
            return None;
        };
        let selection = EgressSelection {
            key: state.selection.key,
            max_frames: NonZeroU16::new(1).unwrap(),
            max_bytes,
        };
        let SelectedState {
            provider, batch, ..
        } = &mut *state;
        match provider.fill_selected(selection, batch) {
            Ok(outcome) => {
                state.report.frames += outcome.frames;
                state.report.bytes += outcome.bytes;
                state.pending = outcome
                    .source_remaining
                    .min(state.selection.max_frames.get() - state.report.frames);
                if outcome.frames == 0 || outcome.stop != FillStopReason::SelectionSatisfied {
                    state.report.stop = Some(Ok(outcome.stop));
                } else if state.report.frames == state.selection.max_frames.get() {
                    state.report.stop = Some(Ok(FillStopReason::SelectionSatisfied));
                }
                if outcome.frames == 0 {
                    return None;
                }
                Some(
                    state
                        .batch
                        .try_take_physical()
                        .expect("successful single fill publishes one owner"),
                )
            }
            Err(failure) => {
                // A single-frame fill has no committed prefix on writer error.
                debug_assert_eq!(failure.committed_frames, 0);
                state.report.stop = Some(Err(failure.error));
                None
            }
        }
    }
}
