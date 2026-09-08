//! Actor-local phase adapters for recurring connectable advertising.
//!
//! The chip and thin Embassy layers own HCI and scheduler policy. This module
//! maps each concrete recurrence phase into the sole controller actor without
//! allocating or erasing an affine owner behind a maximum-sized enum.

#![forbid(unsafe_code)]

use core::{cell::RefCell, marker::PhantomData};

use crate::session::advertising::{
    LegacyConnectableAdvertisingRecurringCancellationWait,
    LegacyConnectableAdvertisingRecurringControllerTimeReady,
    LegacyConnectableAdvertisingRecurringControllerTimeWait,
    LegacyConnectableAdvertisingRecurringDriveHandler,
    LegacyConnectableAdvertisingRecurringStopHandler,
    begin_legacy_connectable_advertising_recurring_command_ready_with,
    begin_legacy_connectable_advertising_recurring_response_pending_with,
    cancel_legacy_connectable_advertising_recurring_candidate_with,
    cancel_legacy_connectable_advertising_recurring_graph_prepared_with,
    cancel_legacy_connectable_advertising_recurring_merged_with,
    cancel_legacy_connectable_advertising_recurring_prepared_with,
    cancel_legacy_connectable_advertising_recurring_sequence_pending_with,
    drive_legacy_connectable_advertising_recurring_candidate_with,
    drive_legacy_connectable_advertising_recurring_graph_prepared_with,
    drive_legacy_connectable_advertising_recurring_merged_with,
    drive_legacy_connectable_advertising_recurring_prepared_with,
    retain_legacy_connectable_advertising_recurring_controller_time,
    retain_legacy_connectable_advertising_recurring_retry_for_hci,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
    LeControllerCommandReady, LeControllerResponsePending,
};

use oer_bluetooth_ll::advertising::AdvertisingDelay;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        LegacyAdvertisingDisableResponsePending, LegacyAdvertisingResetCompletionReady,
        LegacyConnectableAdvertisingActiveResponsePending,
        LegacyConnectableAdvertisingHciActiveSession,
        LegacyConnectableAdvertisingNoConnectionReady,
        LegacyConnectableAdvertisingNoConnectionResponsePending,
        LegacyConnectableAdvertisingRecurrenceCandidate,
        LegacyConnectableAdvertisingRecurrenceGraphPrepared,
        LegacyConnectableAdvertisingRecurrenceMerged,
        LegacyConnectableAdvertisingRecurrencePrepared,
        LegacyConnectableAdvertisingRecurrenceSequencePending,
        LegacyConnectableAdvertisingRecurringCommandHandler,
        LegacyConnectableAdvertisingRecurringCommandMismatch,
        LegacyConnectableAdvertisingRecurringFailStopCause,
        LegacyConnectableAdvertisingRecurringHci, LegacyConnectableAdvertisingRecurringHciFailStop,
        LegacyConnectableAdvertisingRecurringHciRetry,
        LegacyConnectableAdvertisingRecurringRetryCause,
        LegacyConnectableAdvertisingRecurringStopping, LegacyConnectableAdvertisingStopOrder,
    },
};

use super::{
    ControllerCommandBoundary, ControllerCommandPhase, ControllerCommandState,
    ControllerCommandStimulus, ControllerCommandTask, ControllerRetry,
};

pub(super) type CommandOrder<'runtime> = LeControllerCommandReady<'runtime, ()>;
pub(super) type ResponseOrder<'runtime> = LeControllerResponsePending<'runtime, ()>;

macro_rules! recurrence_state_aliases {
    ($command:ident, $response:ident, $phase:ident) => {
        pub(super) type $command<'runtime, S, const CAPACITY: usize> =
            LegacyConnectableAdvertisingRecurringHci<
                $phase<'runtime, S, CAPACITY>,
                CommandOrder<'runtime>,
            >;
        pub(super) type $response<'runtime, S, const CAPACITY: usize> =
            LegacyConnectableAdvertisingRecurringHci<
                $phase<'runtime, S, CAPACITY>,
                ResponseOrder<'runtime>,
            >;
    };
}

pub(super) type CommandWait<'runtime, S, const CAPACITY: usize> =
    LegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        CommandOrder<'runtime>,
    >;
pub(super) type ResponseWait<'runtime, S, const CAPACITY: usize> =
    LegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        ResponseOrder<'runtime>,
    >;

recurrence_state_aliases!(
    CommandGraphPrepared,
    ResponseGraphPrepared,
    LegacyConnectableAdvertisingRecurrenceGraphPrepared
);
recurrence_state_aliases!(
    CommandCandidate,
    ResponseCandidate,
    LegacyConnectableAdvertisingRecurrenceCandidate
);
recurrence_state_aliases!(
    CommandPrepared,
    ResponsePrepared,
    LegacyConnectableAdvertisingRecurrencePrepared
);
recurrence_state_aliases!(
    CommandMerged,
    ResponseMerged,
    LegacyConnectableAdvertisingRecurrenceMerged
);

macro_rules! recurrence_mismatch_alias {
    ($name:ident, $phase:ident) => {
        pub(super) type $name<'runtime, 'command, S, const CAPACITY: usize> =
            LegacyConnectableAdvertisingRecurringCommandMismatch<
                'runtime,
                'command,
                $phase<'runtime, S, CAPACITY>,
            >;
    };
}

recurrence_mismatch_alias!(
    SequencePendingMismatch,
    LegacyConnectableAdvertisingRecurrenceSequencePending
);
recurrence_mismatch_alias!(
    GraphPreparedMismatch,
    LegacyConnectableAdvertisingRecurrenceGraphPrepared
);
recurrence_mismatch_alias!(
    CandidateMismatch,
    LegacyConnectableAdvertisingRecurrenceCandidate
);
recurrence_mismatch_alias!(
    PreparedMismatch,
    LegacyConnectableAdvertisingRecurrencePrepared
);
recurrence_mismatch_alias!(MergedMismatch, LegacyConnectableAdvertisingRecurrenceMerged);

fn store_recurring_state<'runtime, S, const CAPACITY: usize>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    state: ControllerCommandState<'runtime, S, CAPACITY>,
) where
    S: SchedulerRunInterruptStorage,
{
    let active = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
    if from == active {
        actor.store_retained_state(active, state);
    } else {
        actor.store_transition(
            from,
            ControllerCommandStimulus::LegacyConnectableAdvertisingActive,
            state,
        );
    }
}

struct CommandDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    actor: RefCell<&'actor mut ControllerCommandTask<'runtime, S, CAPACITY>>,
    from: ControllerCommandPhase,
    _boundary: PhantomData<fn(&'epoch (), &'packet ())>,
}

struct ResponseDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    actor: RefCell<&'actor mut ControllerCommandTask<'runtime, S, CAPACITY>>,
    from: ControllerCommandPhase,
    _boundary: PhantomData<fn(&'epoch (), &'packet ())>,
}

macro_rules! impl_actor_drive_handler {
    (
        $handler:ident, $order:ty, $running:ty, $running_state:expr,
        $wait_state:expr, $graph_state:expr, $candidate_state:expr,
        $prepared_state:expr, $merged_state:expr, $fail_owner:expr
    ) => {
        impl<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
            $handler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
        where
            S: SchedulerRunInterruptStorage,
        {
            fn new(
                actor: &'actor mut ControllerCommandTask<'runtime, S, CAPACITY>,
                from: ControllerCommandPhase,
            ) -> Self {
                Self {
                    actor: RefCell::new(actor),
                    from,
                    _boundary: PhantomData,
                }
            }
        }

        impl<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
            LegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                $order,
                $running,
            > for $handler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
        where
            S: SchedulerRunInterruptStorage + 'runtime,
        {
            type Output = Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

            fn wait_controller_time(
                &self,
                wait: LegacyConnectableAdvertisingRecurringControllerTimeWait<
                    'runtime,
                    S,
                    CAPACITY,
                    $order,
                >,
            ) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(&mut actor, self.from, $wait_state(wait));
                None
            }

            fn retry_graph_prepared(
                &self,
                retry: LegacyConnectableAdvertisingRecurringHciRetry<
                    LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let cause = retry_cause(retry.cause());
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $graph_state(
                        retain_legacy_connectable_advertising_recurring_retry_for_hci(retry),
                    ),
                );
                Some(actor.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyConnectableAdvertisingRecurring(cause),
                )))
            }

            fn retry_candidate(
                &self,
                retry: LegacyConnectableAdvertisingRecurringHciRetry<
                    LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let cause = retry_cause(retry.cause());
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $candidate_state(
                        retain_legacy_connectable_advertising_recurring_retry_for_hci(retry),
                    ),
                );
                Some(actor.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyConnectableAdvertisingRecurring(cause),
                )))
            }

            fn retry_prepared(
                &self,
                retry: LegacyConnectableAdvertisingRecurringHciRetry<
                    LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let cause = retry_cause(retry.cause());
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $prepared_state(
                        retain_legacy_connectable_advertising_recurring_retry_for_hci(retry),
                    ),
                );
                Some(actor.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyConnectableAdvertisingRecurring(cause),
                )))
            }

            fn retry_merged(
                &self,
                retry: LegacyConnectableAdvertisingRecurringHciRetry<
                    LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let cause = retry_cause(retry.cause());
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $merged_state(
                        retain_legacy_connectable_advertising_recurring_retry_for_hci(retry),
                    ),
                );
                Some(actor.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyConnectableAdvertisingRecurring(cause),
                )))
            }

            fn running(&self, running: $running) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(&mut actor, self.from, $running_state(running));
                Some(ControllerCommandBoundary::LegacyConnectableAdvertisingActive)
            }

            fn fail_stop(
                &self,
                failure: LegacyConnectableAdvertisingRecurringHciFailStop<
                    'runtime,
                    S,
                    CAPACITY,
                    $order,
                >,
            ) -> Self::Output {
                let actor = self.actor.borrow();
                Some(actor.terminal_boundary(
                    self.from,
                    ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringFailStop(
                        AdvertisingRecurringFailStop {
                            _owner: $fail_owner(failure),
                        },
                    ),
                ))
            }
        }
    };
}

impl_actor_drive_handler!(
    CommandDriveHandler,
    CommandOrder<'runtime>,
    LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
    ControllerCommandState::LegacyConnectableAdvertisingActive,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandGraphPrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandCandidate,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandPrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandMerged,
    FailStopOwner::Command
);
impl_actor_drive_handler!(
    ResponseDriveHandler,
    ResponseOrder<'runtime>,
    LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ControllerCommandState::LegacyConnectableAdvertisingActiveResponse,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseGraphPrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseCandidate,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponsePrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseMerged,
    FailStopOwner::Response
);

fn command_drive_handler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &'actor mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
) -> CommandDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    CommandDriveHandler::new(actor, from)
}

fn response_drive_handler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &'actor mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
) -> ResponseDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    ResponseDriveHandler::new(actor, from)
}

pub(super) fn begin_command<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    completed: LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>,
    delay: AdvertisingDelay,
) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    actor.advertising_rejected_packets = actor
        .advertising_rejected_packets
        .and_then(|count| count.checked_add(u32::try_from(completed.rejected_packets()).ok()?));
    if let Some(rejection) = completed.last_receive_rejection() {
        actor.advertising_last_receive_rejection = Some(rejection);
    }
    actor.advertising_completion = Some(completed.scheduler_status());
    begin_legacy_connectable_advertising_recurring_command_ready_with(
        completed,
        delay,
        &command_drive_handler(actor, from),
    )
}

pub(super) fn begin_response<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    completed: LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    delay: AdvertisingDelay,
) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    actor.advertising_rejected_packets = actor
        .advertising_rejected_packets
        .and_then(|count| count.checked_add(u32::try_from(completed.rejected_packets()).ok()?));
    if let Some(rejection) = completed.last_receive_rejection() {
        actor.advertising_last_receive_rejection = Some(rejection);
    }
    actor.advertising_completion = Some(completed.scheduler_status());
    begin_legacy_connectable_advertising_recurring_response_pending_with(
        completed,
        delay,
        &response_drive_handler(actor, from),
    )
}

pub(super) fn resume_command_wait<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    wait: CommandWait<'runtime, S, CAPACITY>,
    ready: LegacyConnectableAdvertisingRecurringControllerTimeReady,
) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    wait.resume_with(ready, &command_drive_handler(actor, from))
}

pub(super) fn resume_response_wait<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    wait: ResponseWait<'runtime, S, CAPACITY>,
    ready: LegacyConnectableAdvertisingRecurringControllerTimeReady,
) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    wait.resume_with(ready, &response_drive_handler(actor, from))
}

pub(super) enum StopDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Disable(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    Reset(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
    Wait(LegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, CAPACITY>),
    FailStop(
        LegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            LegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ),
}

pub(super) struct StopDriveHandler;

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY> for StopDriveHandler
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    type Output = StopDrive<'runtime, S, CAPACITY>;

    fn disable_ready(
        &self,
        ready: LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
    ) -> Self::Output {
        StopDrive::Disable(ready)
    }

    fn reset_ready(
        &self,
        ready: LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
    ) -> Self::Output {
        StopDrive::Reset(ready)
    }

    fn wait_cancellation(
        &self,
        wait: LegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, CAPACITY>,
    ) -> Self::Output {
        StopDrive::Wait(wait)
    }

    fn fail_stop(
        &self,
        failure: LegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            LegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ) -> Self::Output {
        StopDrive::FailStop(failure)
    }
}

pub(super) trait RecurrencePhase<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    type Phase;

    fn drive_command<'epoch, 'packet>(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        from: ControllerCommandPhase,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, CommandOrder<'runtime>>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

    fn drive_response<'epoch, 'packet>(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        from: ControllerCommandPhase,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, ResponseOrder<'runtime>>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

    fn retain_command(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, CommandOrder<'runtime>>,
    );

    fn retain_response(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, ResponseOrder<'runtime>>,
    );

    fn cancel<'epoch, 'packet>(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        from: ControllerCommandPhase,
        stopping: LegacyConnectableAdvertisingRecurringStopping<'runtime, Self::Phase>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

    fn mismatch<'command, 'packet>(
        mismatch: LegacyConnectableAdvertisingRecurringCommandMismatch<
            'runtime,
            'command,
            Self::Phase,
        >,
    ) -> ControllerCommandBoundary<'runtime, 'command, 'packet, S, CAPACITY>;
}

pub(super) struct SequencePendingPhase;
pub(super) struct GraphPreparedPhase;
pub(super) struct CandidatePhase;
pub(super) struct PreparedPhase;
pub(super) struct MergedPhase;

impl<'runtime, S, const CAPACITY: usize> RecurrencePhase<'runtime, S, CAPACITY>
    for SequencePendingPhase
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    type Phase = LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>;

    fn drive_command<'epoch, 'packet>(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        _from: ControllerCommandPhase,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, CommandOrder<'runtime>>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        Self::retain_command(actor, state);
        None
    }

    fn drive_response<'epoch, 'packet>(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        _from: ControllerCommandPhase,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, ResponseOrder<'runtime>>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        Self::retain_response(actor, state);
        None
    }

    fn retain_command(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, CommandOrder<'runtime>>,
    ) {
        actor.store_retained_state(
            ControllerCommandPhase::LegacyConnectableAdvertisingActive,
            ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(
                retain_legacy_connectable_advertising_recurring_controller_time(state),
            ),
        );
    }

    fn retain_response(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        state: LegacyConnectableAdvertisingRecurringHci<Self::Phase, ResponseOrder<'runtime>>,
    ) {
        actor.store_retained_state(
            ControllerCommandPhase::LegacyConnectableAdvertisingActive,
            ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(
                retain_legacy_connectable_advertising_recurring_controller_time(state),
            ),
        );
    }

    fn cancel<'epoch, 'packet>(
        actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
        from: ControllerCommandPhase,
        stopping: LegacyConnectableAdvertisingRecurringStopping<'runtime, Self::Phase>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let drive = cancel_legacy_connectable_advertising_recurring_sequence_pending_with(
            stopping,
            &StopDriveHandler,
        );
        actor.store_connectable_recurring_stop_drive(from, drive)
    }

    fn mismatch<'command, 'packet>(
        mismatch: LegacyConnectableAdvertisingRecurringCommandMismatch<
            'runtime,
            'command,
            Self::Phase,
        >,
    ) -> ControllerCommandBoundary<'runtime, 'command, 'packet, S, CAPACITY> {
        ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringSequencePendingCommandEndpointMismatch(mismatch)
    }
}

macro_rules! impl_recurrence_phase {
    (
        $marker:ident, $phase:ident, $command_state:expr, $response_state:expr,
        $drive:path, $cancel:path, $mismatch_state:ident
    ) => {
        impl<'runtime, S, const CAPACITY: usize> RecurrencePhase<'runtime, S, CAPACITY> for $marker
        where
            S: SchedulerRunInterruptStorage + 'runtime,
        {
            type Phase = $phase<'runtime, S, CAPACITY>;

            fn drive_command<'epoch, 'packet>(
                actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
                from: ControllerCommandPhase,
                state: LegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    CommandOrder<'runtime>,
                >,
            ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
                $drive(state, &command_drive_handler(actor, from))
            }

            fn drive_response<'epoch, 'packet>(
                actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
                from: ControllerCommandPhase,
                state: LegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    ResponseOrder<'runtime>,
                >,
            ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
                $drive(state, &response_drive_handler(actor, from))
            }

            fn retain_command(
                actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
                state: LegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    CommandOrder<'runtime>,
                >,
            ) {
                actor.store_retained_state(
                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                    $command_state(state),
                );
            }

            fn retain_response(
                actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
                state: LegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    ResponseOrder<'runtime>,
                >,
            ) {
                actor.store_retained_state(
                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                    $response_state(state),
                );
            }

            fn cancel<'epoch, 'packet>(
                actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
                from: ControllerCommandPhase,
                stopping: LegacyConnectableAdvertisingRecurringStopping<'runtime, Self::Phase>,
            ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
                actor.store_connectable_recurring_stop_drive(
                    from,
                    $cancel(stopping, &StopDriveHandler),
                )
            }

            fn mismatch<'command, 'packet>(
                mismatch: LegacyConnectableAdvertisingRecurringCommandMismatch<
                    'runtime,
                    'command,
                    Self::Phase,
                >,
            ) -> ControllerCommandBoundary<'runtime, 'command, 'packet, S, CAPACITY> {
                ControllerCommandBoundary::$mismatch_state(mismatch)
            }
        }
    };
}

impl_recurrence_phase!(
    GraphPreparedPhase,
    LegacyConnectableAdvertisingRecurrenceGraphPrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandGraphPrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseGraphPrepared,
    drive_legacy_connectable_advertising_recurring_graph_prepared_with,
    cancel_legacy_connectable_advertising_recurring_graph_prepared_with,
    LegacyConnectableAdvertisingRecurringGraphPreparedCommandEndpointMismatch
);
impl_recurrence_phase!(
    CandidatePhase,
    LegacyConnectableAdvertisingRecurrenceCandidate,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandCandidate,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseCandidate,
    drive_legacy_connectable_advertising_recurring_candidate_with,
    cancel_legacy_connectable_advertising_recurring_candidate_with,
    LegacyConnectableAdvertisingRecurringCandidateCommandEndpointMismatch
);
impl_recurrence_phase!(
    PreparedPhase,
    LegacyConnectableAdvertisingRecurrencePrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandPrepared,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponsePrepared,
    drive_legacy_connectable_advertising_recurring_prepared_with,
    cancel_legacy_connectable_advertising_recurring_prepared_with,
    LegacyConnectableAdvertisingRecurringPreparedCommandEndpointMismatch
);
impl_recurrence_phase!(
    MergedPhase,
    LegacyConnectableAdvertisingRecurrenceMerged,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandMerged,
    ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseMerged,
    drive_legacy_connectable_advertising_recurring_merged_with,
    cancel_legacy_connectable_advertising_recurring_merged_with,
    LegacyConnectableAdvertisingRecurringMergedCommandEndpointMismatch
);

pub(super) struct CommandRouteOutcome<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    buffer: Option<&'buffer mut [u8]>,
    boundary: Option<ControllerCommandBoundary<'runtime, 'command, 'buffer, S, CAPACITY>>,
}

impl<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
    CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) fn into_parts(
        self,
    ) -> (
        Option<&'buffer mut [u8]>,
        Option<ControllerCommandBoundary<'runtime, 'command, 'buffer, S, CAPACITY>>,
    ) {
        (self.buffer, self.boundary)
    }
}

struct CommandRouteHandler<'actor, 'runtime, 'command, S, const CAPACITY: usize, PhaseMap>
where
    S: SchedulerRunInterruptStorage,
{
    actor: &'actor mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    _phase: PhantomData<fn() -> PhaseMap>,
    _command: PhantomData<&'command ()>,
}

impl<'actor, 'runtime, 'command, 'buffer, S, const CAPACITY: usize, PhaseMap>
    LegacyConnectableAdvertisingRecurringCommandHandler<
        'runtime,
        'command,
        'buffer,
        PhaseMap::Phase,
    > for CommandRouteHandler<'actor, 'runtime, 'command, S, CAPACITY, PhaseMap>
where
    S: SchedulerRunInterruptStorage + 'runtime,
    PhaseMap: RecurrencePhase<'runtime, S, CAPACITY>,
{
    type Output = CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>;

    fn response_pending(
        self,
        state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, ResponseOrder<'runtime>>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output {
        let boundary = PhaseMap::drive_response(self.actor, self.from, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary,
        }
    }

    fn stopping(
        self,
        state: LegacyConnectableAdvertisingRecurringStopping<'runtime, PhaseMap::Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output {
        let boundary = PhaseMap::cancel(self.actor, self.from, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary,
        }
    }

    fn command_mismatch(
        self,
        mismatch: LegacyConnectableAdvertisingRecurringCommandMismatch<
            'runtime,
            'command,
            PhaseMap::Phase,
        >,
        _buffer: &'buffer mut [u8],
    ) -> Self::Output {
        CommandRouteOutcome {
            buffer: None,
            boundary: Some(
                self.actor
                    .terminal_boundary(self.from, PhaseMap::mismatch(mismatch)),
            ),
        }
    }

    fn empty(
        self,
        state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, CommandOrder<'runtime>>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output {
        let boundary = PhaseMap::drive_command(self.actor, self.from, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary,
        }
    }

    fn endpoint_mismatch(
        self,
        state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, CommandOrder<'runtime>>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output {
        PhaseMap::retain_command(self.actor, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary: Some(
                self.actor
                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch),
            ),
        }
    }

    fn channel_fault(
        self,
        state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, CommandOrder<'runtime>>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    ) -> Self::Output {
        PhaseMap::retain_command(self.actor, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary: Some(
                self.actor
                    .retain_boundary(ControllerCommandBoundary::HciFault(error)),
            ),
        }
    }

    fn non_command(
        self,
        state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, CommandOrder<'runtime>>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    ) -> Self::Output {
        PhaseMap::retain_command(self.actor, state);
        CommandRouteOutcome {
            buffer: None,
            boundary: Some(
                self.actor
                    .retain_boundary(ControllerCommandBoundary::NonCommand(frame)),
            ),
        }
    }
}

pub(super) fn route_command<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
    PhaseMap,
    M: RawMutex,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    from: ControllerCommandPhase,
    state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, CommandOrder<'runtime>>,
    controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
    buffer: &'buffer mut [u8],
) -> CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage + 'runtime,
    PhaseMap: RecurrencePhase<'runtime, S, CAPACITY>,
{
    state.try_route_controller_command_with_buffer(
        controller,
        buffer,
        CommandRouteHandler::<S, CAPACITY, PhaseMap> {
            actor,
            from,
            _phase: PhantomData,
            _command: PhantomData,
        },
    )
}

pub(super) fn publish_response<
    'runtime,
    'epoch,
    'packet,
    S,
    const CAPACITY: usize,
    PhaseMap,
    M: RawMutex,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    state: LegacyConnectableAdvertisingRecurringHci<PhaseMap::Phase, ResponseOrder<'runtime>>,
    controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: SchedulerRunInterruptStorage + 'runtime,
    PhaseMap: RecurrencePhase<'runtime, S, CAPACITY>,
{
    let actor = RefCell::new(actor);
    let phase = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
    state.try_publish_response_with(
        controller,
        |state| PhaseMap::drive_command(&mut actor.borrow_mut(), phase, state),
        |state| PhaseMap::drive_response(&mut actor.borrow_mut(), phase, state),
        |state| {
            let mut actor = actor.borrow_mut();
            PhaseMap::retain_response(&mut actor, state);
            Some(actor.retain_boundary(ControllerCommandBoundary::EndpointMismatch))
        },
        |state, error| {
            let mut actor = actor.borrow_mut();
            PhaseMap::retain_response(&mut actor, state);
            Some(actor.retain_boundary(ControllerCommandBoundary::HciFault(error)))
        },
    )
}

pub(super) fn is_retry<S, const CAPACITY: usize>(
    state: &ControllerCommandState<'_, S, CAPACITY>,
) -> bool
where
    S: SchedulerRunInterruptStorage,
{
    use ControllerCommandState::*;
    matches!(
        state,
        LegacyConnectableAdvertisingRecurringCommandGraphPrepared(_)
            | LegacyConnectableAdvertisingRecurringCommandCandidate(_)
            | LegacyConnectableAdvertisingRecurringCommandPrepared(_)
            | LegacyConnectableAdvertisingRecurringCommandMerged(_)
            | LegacyConnectableAdvertisingRecurringResponseGraphPrepared(_)
            | LegacyConnectableAdvertisingRecurringResponseCandidate(_)
            | LegacyConnectableAdvertisingRecurringResponsePrepared(_)
            | LegacyConnectableAdvertisingRecurringResponseMerged(_)
    )
}

/// Give HCI one opportunity before retrying the retained hardware transaction.
/// A full response queue must not prevent controller-time or scheduler progress.
pub(super) fn retry_ready<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
    M: RawMutex,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
>(
    actor: &mut ControllerCommandTask<'runtime, S, CAPACITY>,
    controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
    buffer: &'buffer mut [u8],
) -> CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    use ControllerCommandState::*;
    let phase = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
    macro_rules! command {
        ($state:expr, $phase:ty) => {
            route_command::<S, CAPACITY, $phase, M, H2C, C2H, PACKET>(
                actor, phase, $state, controller, buffer,
            )
        };
    }
    macro_rules! response {
        ($state:expr, $phase:ty) => {
            CommandRouteOutcome {
                boundary: publish_response::<S, CAPACITY, $phase, M, H2C, C2H, PACKET>(
                    actor, $state, controller,
                ),
                buffer: Some(buffer),
            }
        };
    }
    match actor.owner.take() {
        LegacyConnectableAdvertisingRecurringCommandGraphPrepared(state) => {
            command!(state, GraphPreparedPhase)
        }
        LegacyConnectableAdvertisingRecurringCommandCandidate(state) => {
            command!(state, CandidatePhase)
        }
        LegacyConnectableAdvertisingRecurringCommandPrepared(state) => {
            command!(state, PreparedPhase)
        }
        LegacyConnectableAdvertisingRecurringCommandMerged(state) => command!(state, MergedPhase),
        LegacyConnectableAdvertisingRecurringResponseGraphPrepared(state) => {
            response!(state, GraphPreparedPhase)
        }
        LegacyConnectableAdvertisingRecurringResponseCandidate(state) => {
            response!(state, CandidatePhase)
        }
        LegacyConnectableAdvertisingRecurringResponsePrepared(state) => {
            response!(state, PreparedPhase)
        }
        LegacyConnectableAdvertisingRecurringResponseMerged(state) => response!(state, MergedPhase),
        _ => unreachable!("retry dispatch only consumes a retained recurrence retry"),
    }
}

pub(super) enum FailStopOwner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Command(
        LegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            CommandOrder<'runtime>,
        >,
    ),
    Response(
        LegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            ResponseOrder<'runtime>,
        >,
    ),
    Stopping(
        LegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            LegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ),
}

/// Opaque terminal recurrence failure retaining the exact radio and HCI axes.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct AdvertisingRecurringFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) _owner: FailStopOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> AdvertisingRecurringFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectableAdvertisingRecurringFailStopCause {
        match &self._owner {
            FailStopOwner::Command(failure) => failure.cause(),
            FailStopOwner::Response(failure) => failure.cause(),
            FailStopOwner::Stopping(failure) => failure.cause(),
        }
    }
}

/// Preserve semantic retry details without requiring the storage error to be Copy.
fn retry_cause<E>(
    cause: &LegacyConnectableAdvertisingRecurringRetryCause<E>,
) -> LegacyConnectableAdvertisingRecurringRetryCause<()> {
    use LegacyConnectableAdvertisingRecurringRetryCause::*;
    match cause {
        TimingWindow => TimingWindow,
        Timeline(error) => Timeline(*error),
        Sequence(error) => Sequence(*error),
        EventFields(error) => EventFields(*error),
        EmptyList(error) => EmptyList(*error),
        SchedulerHead(error) => SchedulerHead(*error),
        SchedulerInterrupts(_) => SchedulerInterrupts(()),
    }
}
