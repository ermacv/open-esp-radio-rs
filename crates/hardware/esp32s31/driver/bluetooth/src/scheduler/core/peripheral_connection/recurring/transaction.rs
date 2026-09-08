//! Private combined ownership for one unpublished recurring connection event.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

#[cfg(target_arch = "riscv32")]
use crate::le::peripheral::connection::{
    PeripheralConnectionCompletedEvent, PeripheralConnectionCompletedEventRecurringParts,
    PeripheralConnectionCompletedEventRecurringRemainder,
};
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_ll::connection::LeDataChannelIndex;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    PeripheralConnectionMemoryGraphActiveCpuOwned,
    PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
};

use crate::le::peripheral::connection::{
    PeripheralConnectionPacketStartTiming, PeripheralConnectionRecurringPhase,
    PeripheralConnectionRecurringTimingError, PeripheralConnectionRecurringTimingPolicy,
};

use oer_bluetooth_ll::connection::{
    LePeripheralConnectionEventCompleted, LePeripheralConnectionEventDelta,
    LePeripheralConnectionRecurringEventProvisional, LePeripheralConnectionState,
};

#[cfg(target_arch = "riscv32")]
use crate::scheduler::core::peripheral_connection::PeripheralConnectionSchedulerCompleted;
use oer_esp32s31_bluetooth_memory::{
    PeripheralConnectionDataChannel, PeripheralConnectionEventSpan,
    PeripheralConnectionRecurringReceiveWait,
};

use crate::{
    ControllerSchedulerEpoch, SchedulerInstant,
    scheduler::{SchedulerRawWindow, SchedulerSoftwareConfig},
};

/// Derived recurrence values retained alongside the provisional LL owner.
///
/// This value has no commit operation. Its phase remains only a proposal until
/// a later combined scheduler transition consumes both it and the LL
/// provisional after lower admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PeripheralConnectionRecurringProtocolProposal {
    delta: LePeripheralConnectionEventDelta,
    proposed_phase: PeripheralConnectionRecurringPhase,
    proposed_anchor: SchedulerInstant,
    window: SchedulerRawWindow,
    event_span: PeripheralConnectionEventSpan,
    receive_wait: PeripheralConnectionRecurringReceiveWait,
    window_widening_micros: u32,
    data_channel: PeripheralConnectionDataChannel,
}

/// Pure provisional LL successor paired with original and proposed phase.
#[derive(Debug)]
struct PeripheralConnectionRecurringProtocolCandidate {
    provisional: LePeripheralConnectionRecurringEventProvisional,
    original_phase: PeripheralConnectionRecurringPhase,
    proposal: PeripheralConnectionRecurringProtocolProposal,
}

impl PeripheralConnectionRecurringProtocolCandidate {
    const fn event_counter(&self) -> u16 {
        self.provisional.event_counter()
    }

    #[cfg(target_arch = "riscv32")]
    const fn channel(&self) -> oer_bluetooth_ll::connection::LeDataChannelIndex {
        self.provisional.channel()
    }

    fn cancel(
        self,
    ) -> (
        LePeripheralConnectionEventCompleted,
        PeripheralConnectionRecurringPhase,
        LePeripheralConnectionEventDelta,
    ) {
        (
            self.provisional.cancel(),
            self.original_phase,
            self.proposal.delta,
        )
    }
}

/// Lossless portable/timing rejection before any active memory is changed.
#[derive(Debug)]
struct PeripheralConnectionRecurringProtocolFailure {
    completed: LePeripheralConnectionEventCompleted,
    original_phase: PeripheralConnectionRecurringPhase,
    delta: LePeripheralConnectionEventDelta,
    error: PeripheralConnectionRecurringCandidateError,
}

fn prepare_recurring_protocol_proposal(
    completed: LePeripheralConnectionEventCompleted,
    original_phase: PeripheralConnectionRecurringPhase,
    packet_start: Option<&PeripheralConnectionPacketStartTiming>,
    delta: LePeripheralConnectionEventDelta,
    epoch: ControllerSchedulerEpoch,
    scheduler_config: SchedulerSoftwareConfig,
    timing_policy: PeripheralConnectionRecurringTimingPolicy,
) -> ControlFlow<
    PeripheralConnectionRecurringProtocolFailure,
    PeripheralConnectionRecurringProtocolCandidate,
> {
    // A missed first event leaves the connection in `Created`, so the peer's
    // actual anchor is still unknown inside the initial WinSize interval. The
    // reviewed software-widening profile is valid only after an actual packet
    // start has established that reference; do not turn the earliest planned
    // first-window instant into a fictitious anchor.
    if packet_start.is_none()
        && matches!(
            completed.connection_state(),
            LePeripheralConnectionState::Created
        )
    {
        return ControlFlow::Break(PeripheralConnectionRecurringProtocolFailure {
            completed,
            original_phase,
            delta,
            error: PeripheralConnectionRecurringCandidateError::InitialAnchorUnavailable,
        });
    }
    let provisional = completed.prepare_recurring_event(delta);
    let planning_phase = match packet_start {
        Some(packet_start) => original_phase.correct_from_normalized_packet_start(packet_start),
        None => original_phase,
    };
    let plan = match planning_phase.plan(
        provisional.request(),
        delta,
        epoch,
        scheduler_config,
        timing_policy,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return ControlFlow::Break(PeripheralConnectionRecurringProtocolFailure {
                completed: provisional.cancel(),
                original_phase,
                delta,
                error: PeripheralConnectionRecurringCandidateError::Timing(error),
            });
        }
    };
    let data_channel = PeripheralConnectionDataChannel::new(provisional.channel().get())
        .expect("a portable LE data channel is always one of the 37 S31 data channels");
    let window_widening_micros = plan.window_widening_micros();
    let (planned_delta, proposed_phase, proposed_anchor, window, event_span, receive_wait) =
        plan.into_parts();
    ControlFlow::Continue(PeripheralConnectionRecurringProtocolCandidate {
        provisional,
        original_phase,
        proposal: PeripheralConnectionRecurringProtocolProposal {
            delta: planned_delta,
            proposed_phase,
            proposed_anchor,
            window,
            event_span,
            receive_wait,
            window_widening_micros,
            data_channel,
        },
    })
}

/// Why a completed chip event could not form a recurring candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionRecurringCandidateError {
    /// No packet has established the peer-selected anchor inside the initial
    /// transmit window, so the zero-uncertainty software profile is unsound.
    InitialAnchorUnavailable,
    Timing(PeripheralConnectionRecurringTimingError),
}

/// Exact completed event, phase and requested distance restored by cancellation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the restored completed event and phase must be retained"]
pub(crate) struct PeripheralConnectionRecurringCandidateFailure {
    completed: PeripheralConnectionSchedulerCompleted,
    delta: LePeripheralConnectionEventDelta,
    error: PeripheralConnectionRecurringCandidateError,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionRecurringCandidateFailure {
    pub(crate) const fn error(&self) -> PeripheralConnectionRecurringCandidateError {
        self.error
    }

    pub(crate) fn into_retry_parts(
        self,
    ) -> (
        PeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        (self.completed, self.delta)
    }
}

/// Completed chip event split into an active graph and provisional successor.
#[cfg(target_arch = "riscv32")]
#[must_use = "the recurring candidate must advance or restore its exact completion"]
pub struct PeripheralConnectionRecurringEventCandidate {
    graph: PeripheralConnectionMemoryGraphActiveCpuOwned,
    remainder: PeripheralConnectionCompletedEventRecurringRemainder,
    protocol: PeripheralConnectionRecurringProtocolCandidate,
}

#[cfg(target_arch = "riscv32")]
pub(super) fn prepare_recurring_event_candidate(
    completed: PeripheralConnectionSchedulerCompleted,
    delta: LePeripheralConnectionEventDelta,
    epoch: ControllerSchedulerEpoch,
    scheduler_config: SchedulerSoftwareConfig,
    timing_policy: PeripheralConnectionRecurringTimingPolicy,
) -> ControlFlow<
    PeripheralConnectionRecurringCandidateFailure,
    PeripheralConnectionRecurringEventCandidate,
> {
    // Form one unpublished candidate without changing active memory or
    // committing either portable state or connection phase.
    let phase = completed.event.recurring_phase();
    let PeripheralConnectionCompletedEventRecurringParts {
        graph,
        event,
        remainder,
    } = completed.event.into_recurring_parts();
    match prepare_recurring_protocol_proposal(
        event,
        phase,
        remainder.packet_start(),
        delta,
        epoch,
        scheduler_config,
        timing_policy,
    ) {
        ControlFlow::Continue(protocol) => {
            ControlFlow::Continue(PeripheralConnectionRecurringEventCandidate {
                graph,
                remainder,
                protocol,
            })
        }
        ControlFlow::Break(failure) => {
            debug_assert_eq!(failure.original_phase, phase);
            let event = PeripheralConnectionCompletedEvent::from_recurring_parts(
                PeripheralConnectionCompletedEventRecurringParts {
                    graph,
                    event: failure.completed,
                    remainder,
                },
            );
            ControlFlow::Break(PeripheralConnectionRecurringCandidateFailure {
                completed: PeripheralConnectionSchedulerCompleted { event },
                delta: failure.delta,
                error: failure.error,
            })
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionRecurringEventCandidate {
    pub const fn event_counter(&self) -> u16 {
        self.protocol.event_counter()
    }

    pub const fn delta(&self) -> LePeripheralConnectionEventDelta {
        self.protocol.proposal.delta
    }

    pub const fn raw_window(&self) -> SchedulerRawWindow {
        self.protocol.proposal.window
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.protocol.channel()
    }

    pub const fn proposed_anchor_micros(&self) -> u32 {
        self.protocol.proposal.proposed_anchor.image()
    }

    pub const fn window_widening_micros(&self) -> u32 {
        self.protocol.proposal.window_widening_micros
    }

    /// Encode recurring fields after exact non-displacing timeline admission.
    ///
    /// The plan already owns a nonempty forward-half-range raw window, which is
    /// the identical invariant required by the memory semantic type.
    pub(super) fn prepare_event_fields(
        self,
        raw_sequence_lead: u32,
    ) -> PeripheralConnectionRecurringEventFieldsPrepared {
        let window = PeripheralConnectionSchedulerWindow::new(
            self.protocol.proposal.window.start(),
            self.protocol.proposal.window.end(),
        )
        .expect("a scheduler raw window has the memory window's identical invariant");
        let Self {
            graph,
            remainder,
            protocol,
        } = self;
        let graph = graph.prepare_reviewed_recurring_event_fields(
            protocol.proposal.data_channel,
            protocol.proposal.event_span,
            window,
            protocol.proposal.receive_wait,
            PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
            raw_sequence_lead,
        );
        let PeripheralConnectionRecurringProtocolCandidate {
            provisional,
            original_phase,
            proposal,
        } = protocol;
        PeripheralConnectionRecurringEventFieldsPrepared {
            graph,
            remainder,
            provisional,
            original_phase,
            proposed_phase: proposal.proposed_phase,
            delta: proposal.delta,
        }
    }

    /// Restore the exact completed chip event and input phase.
    pub(crate) fn cancel(
        self,
    ) -> (
        PeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let Self {
            graph,
            remainder,
            protocol,
        } = self;
        let (event, phase, delta) = protocol.cancel();
        let completed = PeripheralConnectionCompletedEvent::from_recurring_parts(
            PeripheralConnectionCompletedEventRecurringParts {
                graph,
                event,
                remainder,
            },
        );
        debug_assert_eq!(completed.recurring_phase(), phase);
        (
            PeripheralConnectionSchedulerCompleted { event: completed },
            delta,
        )
    }
}

/// Recurring descriptor fields paired with provisional protocol and phase state.
#[cfg(target_arch = "riscv32")]
#[must_use = "the recurring fields must detach or restore their exact completion"]
pub(super) struct PeripheralConnectionRecurringEventFieldsPrepared {
    graph: PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
    remainder: PeripheralConnectionCompletedEventRecurringRemainder,
    provisional: LePeripheralConnectionRecurringEventProvisional,
    original_phase: PeripheralConnectionRecurringPhase,
    proposed_phase: PeripheralConnectionRecurringPhase,
    delta: LePeripheralConnectionEventDelta,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionRecurringEventFieldsPrepared {
    pub(super) const fn event_counter(&self) -> u16 {
        self.provisional.event_counter()
    }

    pub(super) const fn channel(&self) -> LeDataChannelIndex {
        self.provisional.channel()
    }

    /// Split only for the connection scheduler's combined admission owner.
    pub(super) fn into_scheduler_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
        PeripheralConnectionRecurringEventSchedulerHandoff,
    ) {
        (
            self.graph,
            PeripheralConnectionRecurringEventSchedulerHandoff {
                remainder: self.remainder,
                provisional: self.provisional,
                original_phase: self.original_phase,
                proposed_phase: self.proposed_phase,
                delta: self.delta,
            },
        )
    }

    /// Rejoin only the exact parts returned by [`Self::into_scheduler_parts`].
    pub(super) fn from_scheduler_parts(
        graph: PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
        handoff: PeripheralConnectionRecurringEventSchedulerHandoff,
    ) -> Self {
        Self {
            graph,
            remainder: handoff.remainder,
            provisional: handoff.provisional,
            original_phase: handoff.original_phase,
            proposed_phase: handoff.proposed_phase,
            delta: handoff.delta,
        }
    }

    /// Remove the unpublished fields and recover the exact completed event.
    pub(super) fn cancel(
        self,
    ) -> (
        PeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let completed = PeripheralConnectionCompletedEvent::from_recurring_parts(
            PeripheralConnectionCompletedEventRecurringParts {
                graph: self.graph.cancel(),
                event: self.provisional.cancel(),
                remainder: self.remainder,
            },
        );
        debug_assert_eq!(completed.recurring_phase(), self.original_phase);
        (
            PeripheralConnectionSchedulerCompleted { event: completed },
            self.delta,
        )
    }
}

/// One-shot affine handoff into the private connection scheduler transaction.
#[cfg(target_arch = "riscv32")]
#[must_use = "the scheduler must retain or rejoin every affine handoff part"]
pub(super) struct PeripheralConnectionRecurringEventSchedulerHandoff {
    pub(super) remainder: PeripheralConnectionCompletedEventRecurringRemainder,
    pub(super) provisional: LePeripheralConnectionRecurringEventProvisional,
    pub(super) original_phase: PeripheralConnectionRecurringPhase,
    pub(super) proposed_phase: PeripheralConnectionRecurringPhase,
    pub(super) delta: LePeripheralConnectionEventDelta,
}

#[cfg(test)]
#[path = "transaction/tests.rs"]
mod tests;
