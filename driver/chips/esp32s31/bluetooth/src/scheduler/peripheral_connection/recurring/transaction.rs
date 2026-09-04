//! Private combined ownership for one unpublished recurring connection event.

#![forbid(unsafe_code)]

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
use crate::scheduler::peripheral_connection::BluetoothPeripheralConnectionSchedulerCompleted;
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

#[allow(
    clippy::result_large_err,
    reason = "the no-alloc failure retains the exact portable completion owner"
)]
fn prepare_recurring_protocol_proposal(
    completed: LePeripheralConnectionEventCompleted,
    original_phase: BluetoothPeripheralConnectionRecurringPhase,
    packet_start: Option<&BluetoothPeripheralConnectionPacketStartTiming>,
    delta: LePeripheralConnectionEventDelta,
    epoch: BluetoothControllerSchedulerEpoch,
    scheduler_config: BluetoothSchedulerSoftwareConfig,
    timing_policy: BluetoothPeripheralConnectionRecurringTimingPolicy,
) -> Result<
    BluetoothPeripheralConnectionRecurringProtocolCandidate,
    BluetoothPeripheralConnectionRecurringProtocolFailure,
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
        return Err(BluetoothPeripheralConnectionRecurringProtocolFailure {
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
            return Err(BluetoothPeripheralConnectionRecurringProtocolFailure {
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
    Ok(BluetoothPeripheralConnectionRecurringProtocolCandidate {
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
) -> Result<
    BluetoothPeripheralConnectionRecurringEventCandidate,
    BluetoothPeripheralConnectionRecurringCandidateFailure,
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
        Ok(protocol) => Ok(BluetoothPeripheralConnectionRecurringEventCandidate {
            graph,
            remainder,
            protocol,
        }),
        Err(failure) => {
            debug_assert_eq!(failure.original_phase, phase);
            let event = BluetoothPeripheralConnectionCompletedEvent::from_recurring_parts(
                BluetoothPeripheralConnectionCompletedEventRecurringParts {
                    graph,
                    event: failure.completed,
                    remainder,
                },
            );
            Err(BluetoothPeripheralConnectionRecurringCandidateFailure {
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
mod tests {
    use open_esp_radio_bluetooth_ll::connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeDataChannelMap,
        LeLegacyConnectionRequest, LePeripheralConnection, LePeripheralConnectionEventCompleted,
        LePeripheralConnectionEventDelta, LePeripheralConnectionEventPeerActivity,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{
        BluetoothPeripheralConnectionRecurringCandidateError, prepare_recurring_protocol_proposal,
    };
    use crate::peripheral_connection::{
        BluetoothPeripheralConnectionLocalSleepClockAccuracy,
        BluetoothPeripheralConnectionPacketStartTiming,
        BluetoothPeripheralConnectionRecurringPhase,
        BluetoothPeripheralConnectionRecurringTimingError,
        BluetoothPeripheralConnectionRecurringTimingPolicy,
        BluetoothPeripheralConnectionWindowWideningMode,
    };
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
        BluetoothSchedulerSoftwareConfig,
    };

    fn request(interval_units: u16, central_sca: u8) -> LeLegacyConnectionRequest {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = 0x25;
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&interval_units.to_le_bytes());
        pdu[28..30].copy_from_slice(&3200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&LeDataChannelMap::all().wire_bytes());
        pdu[35] = 5 | (central_sca << 5);
        LeLegacyConnectionRequest::decode(&pdu).expect("the connection request is valid")
    }

    fn epoch(micros_anchor: u32) -> BluetoothControllerSchedulerEpoch {
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            micros_anchor,
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        )
    }

    fn software_policy() -> BluetoothPeripheralConnectionRecurringTimingPolicy {
        BluetoothPeripheralConnectionRecurringTimingPolicy::new(
            Some(
                BluetoothPeripheralConnectionLocalSleepClockAccuracy::new(60)
                    .expect("60 ppm is a valid local accuracy"),
            ),
            BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
        )
    }

    fn phase(packet_start_micros: u32) -> BluetoothPeripheralConnectionRecurringPhase {
        BluetoothPeripheralConnectionRecurringPhase::from_nominal_anchor(
            crate::BluetoothSchedulerInstant::from_image(packet_start_micros),
        )
    }

    fn completed_event(request: LeLegacyConnectionRequest) -> LePeripheralConnectionEventCompleted {
        LePeripheralConnection::from_request(request)
            .prepare_event()
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Observed)
    }

    fn missed_first_event(
        request: LeLegacyConnectionRequest,
    ) -> LePeripheralConnectionEventCompleted {
        LePeripheralConnection::from_request(request)
            .prepare_event()
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Missed)
    }

    #[test]
    fn missed_first_event_cannot_invent_an_anchor_for_recurrence() {
        let request = request(24, 4);
        let original_phase = phase(10_000);
        let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
        let expected_completed = missed_first_event(request);
        let failure = prepare_recurring_protocol_proposal(
            missed_first_event(request),
            original_phase,
            None,
            delta,
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            software_policy(),
        )
        .expect_err("the first peer-selected anchor has not been observed");

        assert_eq!(failure.completed, expected_completed);
        assert_eq!(failure.original_phase, original_phase);
        assert_eq!(failure.delta, delta);
        assert_eq!(
            failure.error,
            BluetoothPeripheralConnectionRecurringCandidateError::InitialAnchorUnavailable
        );
    }

    #[test]
    fn protocol_proposal_keeps_completed_owner_provisional_while_capture_advances_full_delta() {
        let request = request(24, 4);
        let original_phase = phase(9_900);
        let actual = BluetoothPeripheralConnectionPacketStartTiming::from_scheduler_micros(10_007);
        let delta = LePeripheralConnectionEventDelta::new(3).unwrap();
        let expected_completed = completed_event(request);
        let candidate = prepare_recurring_protocol_proposal(
            completed_event(request),
            original_phase,
            Some(&actual),
            delta,
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            software_policy(),
        )
        .expect("known timing authority forms a provisional proposal");

        assert_eq!(candidate.event_counter(), delta.get());
        assert_eq!(candidate.proposal.delta, delta);
        assert_eq!(
            candidate.proposal.proposed_anchor.image(),
            actual
                .scheduler_instant()
                .image()
                .wrapping_add(request.timing().interval_micros() * u32::from(delta.get()))
        );
        let (completed, restored_phase, restored_delta) = candidate.cancel();
        assert_eq!(completed, expected_completed);
        assert_eq!(restored_phase, original_phase);
        assert_eq!(restored_delta, delta);
    }

    #[test]
    fn protocol_planning_failure_returns_exact_completion_phase_and_delta() {
        let request = request(24, 4);
        let original_phase = phase(10_000);
        let delta = LePeripheralConnectionEventDelta::new(5).unwrap();
        let expected_completed = completed_event(request);
        let failure = prepare_recurring_protocol_proposal(
            completed_event(request),
            original_phase,
            None,
            delta,
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothPeripheralConnectionRecurringTimingPolicy::new(
                None,
                BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
            ),
        )
        .expect_err("missing local SCA must reject the proposal");

        assert_eq!(failure.completed, expected_completed);
        assert_eq!(failure.original_phase, original_phase);
        assert_eq!(failure.delta, delta);
        assert_eq!(
            failure.error,
            BluetoothPeripheralConnectionRecurringCandidateError::Timing(
                BluetoothPeripheralConnectionRecurringTimingError::LocalSleepClockAccuracyUnknown
            )
        );
    }

    #[test]
    fn scheduler_rejection_can_restore_and_retry_with_a_different_typed_delta() {
        let request = request(24, 4);
        let original_phase = phase(20_000);
        let rejected_delta = LePeripheralConnectionEventDelta::new(4).unwrap();
        let rejected = prepare_recurring_protocol_proposal(
            completed_event(request),
            original_phase,
            None,
            rejected_delta,
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            software_policy(),
        )
        .expect("the first scheduler candidate is representable");
        assert_eq!(rejected.event_counter(), 4);
        assert_eq!(rejected.proposal.delta, rejected_delta);

        let (restored, restored_phase, restored_delta) = rejected.cancel();
        assert_eq!(restored, completed_event(request));
        assert_eq!(restored_phase, original_phase);
        assert_eq!(restored_delta, rejected_delta);
        let retry_delta = LePeripheralConnectionEventDelta::new(1).unwrap();
        let retry = prepare_recurring_protocol_proposal(
            restored,
            restored_phase,
            None,
            retry_delta,
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            software_policy(),
        )
        .expect("the restored owner accepts a different retry");

        assert_eq!(retry.event_counter(), 1);
        assert_eq!(retry.proposal.delta, retry_delta);
        assert_eq!(
            retry.proposal.proposed_anchor.image(),
            20_000 + request.timing().interval_micros()
        );
    }
}
