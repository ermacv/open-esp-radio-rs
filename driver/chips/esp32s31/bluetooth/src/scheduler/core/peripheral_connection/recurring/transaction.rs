//! Private combined ownership for one unpublished recurring connection event.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_ll::connection::LeDataChannelIndex;
use open_esp_radio_bluetooth_ll::connection::{
    LePeripheralConnectionEventCompleted, LePeripheralConnectionEventDelta,
    LePeripheralConnectionRecurringEventProvisional, LePeripheralConnectionState,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionDataChannel, BluetoothPeripheralConnectionEventSpan,
    BluetoothPeripheralConnectionRecurringReceiveWait,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
    BluetoothPeripheralConnectionSchedulerPriority, BluetoothPeripheralConnectionSchedulerWindow,
};

#[cfg(target_arch = "riscv32")]
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionCompletedEvent,
    BluetoothPeripheralConnectionCompletedEventRecurringParts,
    BluetoothPeripheralConnectionCompletedEventRecurringRemainder,
};
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionPacketStartTiming, BluetoothPeripheralConnectionRecurringPhase,
    BluetoothPeripheralConnectionRecurringTimingError,
    BluetoothPeripheralConnectionRecurringTimingPolicy,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::core::peripheral_connection::BluetoothPeripheralConnectionSchedulerCompleted;
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothSchedulerInstant, BluetoothSchedulerRawWindow,
    BluetoothSchedulerSoftwareConfig,
};

/// Derived recurrence values retained alongside the provisional LL owner.
///
/// This value has no commit operation. Its phase remains only a proposal until
/// a later combined scheduler transition consumes both it and the LL
/// provisional after lower admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPeripheralConnectionRecurringProtocolProposal {
    delta: LePeripheralConnectionEventDelta,
    proposed_phase: BluetoothPeripheralConnectionRecurringPhase,
    proposed_anchor: BluetoothSchedulerInstant,
    window: BluetoothSchedulerRawWindow,
    event_span: BluetoothPeripheralConnectionEventSpan,
    receive_wait: BluetoothPeripheralConnectionRecurringReceiveWait,
    window_widening_micros: u32,
    data_channel: BluetoothPeripheralConnectionDataChannel,
}

/// Pure provisional LL successor paired with original and proposed phase.
#[derive(Debug)]
struct BluetoothPeripheralConnectionRecurringProtocolCandidate {
    provisional: LePeripheralConnectionRecurringEventProvisional,
    original_phase: BluetoothPeripheralConnectionRecurringPhase,
    proposal: BluetoothPeripheralConnectionRecurringProtocolProposal,
}

impl BluetoothPeripheralConnectionRecurringProtocolCandidate {
    const fn event_counter(&self) -> u16 {
        self.provisional.event_counter()
    }

    #[cfg(target_arch = "riscv32")]
    const fn channel(&self) -> open_esp_radio_bluetooth_ll::connection::LeDataChannelIndex {
        self.provisional.channel()
    }

    fn cancel(
        self,
    ) -> (
        LePeripheralConnectionEventCompleted,
        BluetoothPeripheralConnectionRecurringPhase,
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
struct BluetoothPeripheralConnectionRecurringProtocolFailure {
    completed: LePeripheralConnectionEventCompleted,
    original_phase: BluetoothPeripheralConnectionRecurringPhase,
    delta: LePeripheralConnectionEventDelta,
    error: BluetoothPeripheralConnectionRecurringCandidateError,
}

fn prepare_recurring_protocol_proposal(
    completed: LePeripheralConnectionEventCompleted,
    original_phase: BluetoothPeripheralConnectionRecurringPhase,
    packet_start: Option<&BluetoothPeripheralConnectionPacketStartTiming>,
    delta: LePeripheralConnectionEventDelta,
    epoch: BluetoothControllerSchedulerEpoch,
    scheduler_config: BluetoothSchedulerSoftwareConfig,
    timing_policy: BluetoothPeripheralConnectionRecurringTimingPolicy,
) -> ControlFlow<
    BluetoothPeripheralConnectionRecurringProtocolFailure,
    BluetoothPeripheralConnectionRecurringProtocolCandidate,
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
        return ControlFlow::Break(BluetoothPeripheralConnectionRecurringProtocolFailure {
            completed,
            original_phase,
            delta,
            error: BluetoothPeripheralConnectionRecurringCandidateError::InitialAnchorUnavailable,
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
            return ControlFlow::Break(BluetoothPeripheralConnectionRecurringProtocolFailure {
                completed: provisional.cancel(),
                original_phase,
                delta,
                error: BluetoothPeripheralConnectionRecurringCandidateError::Timing(error),
            });
        }
    };
    let data_channel = BluetoothPeripheralConnectionDataChannel::new(provisional.channel().get())
        .expect("a portable LE data channel is always one of the 37 S31 data channels");
    let window_widening_micros = plan.window_widening_micros();
    let (planned_delta, proposed_phase, proposed_anchor, window, event_span, receive_wait) =
        plan.into_parts();
    ControlFlow::Continue(BluetoothPeripheralConnectionRecurringProtocolCandidate {
        provisional,
        original_phase,
        proposal: BluetoothPeripheralConnectionRecurringProtocolProposal {
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
pub enum BluetoothPeripheralConnectionRecurringCandidateError {
    /// No packet has established the peer-selected anchor inside the initial
    /// transmit window, so the zero-uncertainty software profile is unsound.
    InitialAnchorUnavailable,
    Timing(BluetoothPeripheralConnectionRecurringTimingError),
}

/// Exact completed event, phase and requested distance restored by cancellation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the restored completed event and phase must be retained"]
pub(crate) struct BluetoothPeripheralConnectionRecurringCandidateFailure {
    completed: BluetoothPeripheralConnectionSchedulerCompleted,
    delta: LePeripheralConnectionEventDelta,
    error: BluetoothPeripheralConnectionRecurringCandidateError,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionRecurringCandidateFailure {
    pub(crate) const fn error(&self) -> BluetoothPeripheralConnectionRecurringCandidateError {
        self.error
    }

    pub(crate) fn into_retry_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        (self.completed, self.delta)
    }
}

/// Completed chip event split into an active graph and provisional successor.
#[cfg(target_arch = "riscv32")]
#[must_use = "the recurring candidate must advance or restore its exact completion"]
pub struct BluetoothPeripheralConnectionRecurringEventCandidate {
    graph: BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    remainder: BluetoothPeripheralConnectionCompletedEventRecurringRemainder,
    protocol: BluetoothPeripheralConnectionRecurringProtocolCandidate,
}

#[cfg(target_arch = "riscv32")]
pub(super) fn prepare_recurring_event_candidate(
    completed: BluetoothPeripheralConnectionSchedulerCompleted,
    delta: LePeripheralConnectionEventDelta,
    epoch: BluetoothControllerSchedulerEpoch,
    scheduler_config: BluetoothSchedulerSoftwareConfig,
    timing_policy: BluetoothPeripheralConnectionRecurringTimingPolicy,
) -> ControlFlow<
    BluetoothPeripheralConnectionRecurringCandidateFailure,
    BluetoothPeripheralConnectionRecurringEventCandidate,
> {
    // Form one unpublished candidate without changing active memory or
    // committing either portable state or connection phase.
    let phase = completed.event.recurring_phase();
    let BluetoothPeripheralConnectionCompletedEventRecurringParts {
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
            ControlFlow::Continue(BluetoothPeripheralConnectionRecurringEventCandidate {
                graph,
                remainder,
                protocol,
            })
        }
        ControlFlow::Break(failure) => {
            debug_assert_eq!(failure.original_phase, phase);
            let event = BluetoothPeripheralConnectionCompletedEvent::from_recurring_parts(
                BluetoothPeripheralConnectionCompletedEventRecurringParts {
                    graph,
                    event: failure.completed,
                    remainder,
                },
            );
            ControlFlow::Break(BluetoothPeripheralConnectionRecurringCandidateFailure {
                completed: BluetoothPeripheralConnectionSchedulerCompleted { event },
                delta: failure.delta,
                error: failure.error,
            })
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionRecurringEventCandidate {
    pub const fn event_counter(&self) -> u16 {
        self.protocol.event_counter()
    }

    pub const fn delta(&self) -> LePeripheralConnectionEventDelta {
        self.protocol.proposal.delta
    }

    pub const fn raw_window(&self) -> BluetoothSchedulerRawWindow {
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
    ) -> BluetoothPeripheralConnectionRecurringEventFieldsPrepared {
        let window = BluetoothPeripheralConnectionSchedulerWindow::new(
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
            BluetoothPeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        );
        let BluetoothPeripheralConnectionRecurringProtocolCandidate {
            provisional,
            original_phase,
            proposal,
        } = protocol;
        BluetoothPeripheralConnectionRecurringEventFieldsPrepared {
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
        BluetoothPeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let Self {
            graph,
            remainder,
            protocol,
        } = self;
        let (event, phase, delta) = protocol.cancel();
        let completed = BluetoothPeripheralConnectionCompletedEvent::from_recurring_parts(
            BluetoothPeripheralConnectionCompletedEventRecurringParts {
                graph,
                event,
                remainder,
            },
        );
        debug_assert_eq!(completed.recurring_phase(), phase);
        (
            BluetoothPeripheralConnectionSchedulerCompleted { event: completed },
            delta,
        )
    }
}

/// Recurring descriptor fields paired with provisional protocol and phase state.
#[cfg(target_arch = "riscv32")]
#[must_use = "the recurring fields must detach or restore their exact completion"]
pub(super) struct BluetoothPeripheralConnectionRecurringEventFieldsPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
    remainder: BluetoothPeripheralConnectionCompletedEventRecurringRemainder,
    provisional: LePeripheralConnectionRecurringEventProvisional,
    original_phase: BluetoothPeripheralConnectionRecurringPhase,
    proposed_phase: BluetoothPeripheralConnectionRecurringPhase,
    delta: LePeripheralConnectionEventDelta,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionRecurringEventFieldsPrepared {
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
        BluetoothPeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
        BluetoothPeripheralConnectionRecurringEventSchedulerHandoff,
    ) {
        (
            self.graph,
            BluetoothPeripheralConnectionRecurringEventSchedulerHandoff {
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
        graph: BluetoothPeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
        handoff: BluetoothPeripheralConnectionRecurringEventSchedulerHandoff,
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
        BluetoothPeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let completed = BluetoothPeripheralConnectionCompletedEvent::from_recurring_parts(
            BluetoothPeripheralConnectionCompletedEventRecurringParts {
                graph: self.graph.cancel(),
                event: self.provisional.cancel(),
                remainder: self.remainder,
            },
        );
        debug_assert_eq!(completed.recurring_phase(), self.original_phase);
        (
            BluetoothPeripheralConnectionSchedulerCompleted { event: completed },
            self.delta,
        )
    }
}

/// One-shot affine handoff into the private connection scheduler transaction.
#[cfg(target_arch = "riscv32")]
#[must_use = "the scheduler must retain or rejoin every affine handoff part"]
pub(super) struct BluetoothPeripheralConnectionRecurringEventSchedulerHandoff {
    pub(super) remainder: BluetoothPeripheralConnectionCompletedEventRecurringRemainder,
    pub(super) provisional: LePeripheralConnectionRecurringEventProvisional,
    pub(super) original_phase: BluetoothPeripheralConnectionRecurringPhase,
    pub(super) proposed_phase: BluetoothPeripheralConnectionRecurringPhase,
    pub(super) delta: LePeripheralConnectionEventDelta,
}

#[cfg(test)]
#[path = "transaction/tests.rs"]
mod tests;
