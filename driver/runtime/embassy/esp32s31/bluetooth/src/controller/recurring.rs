//! Actor-local phase adapters for recurring connectable advertising.
//!
//! The chip and thin Embassy layers own HCI and scheduler policy. This module
//! maps each concrete recurrence phase into the sole controller actor without
//! allocating or erasing an affine owner behind a maximum-sized enum.

#![forbid(unsafe_code)]

use core::{cell::RefCell, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
    LeControllerCommandReady, LeControllerResponsePending,
};
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingResetCompletionReady,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending,
    BluetoothLegacyConnectableAdvertisingHciActiveSession,
    BluetoothLegacyConnectableAdvertisingNoConnectionReady,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending,
    BluetoothLegacyConnectableAdvertisingRecurringCommandHandler,
    BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    BluetoothLegacyConnectableAdvertisingRecurringHci,
    BluetoothLegacyConnectableAdvertisingRecurringHciFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringHciRetry,
    BluetoothLegacyConnectableAdvertisingRecurringStopping,
    BluetoothLegacyConnectableAdvertisingStopOrder, BluetoothSchedulerRunInterruptStorage,
};

use crate::{
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler,
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

use super::{
    ControllerCommandStimulus, EmbassyBluetoothControllerCommandBoundary,
    EmbassyBluetoothControllerCommandPhase, EmbassyBluetoothControllerCommandState,
    EmbassyBluetoothControllerCommandTask, EmbassyBluetoothControllerRetry,
};

pub(super) type CommandOrder<'runtime> = LeControllerCommandReady<'runtime, ()>;
pub(super) type ResponseOrder<'runtime> = LeControllerResponsePending<'runtime, ()>;

macro_rules! recurrence_state_aliases {
    ($command:ident, $response:ident, $phase:ident) => {
        pub(super) type $command<'runtime, S, const CAPACITY: usize> =
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                $phase<'runtime, S, CAPACITY>,
                CommandOrder<'runtime>,
            >;
        pub(super) type $response<'runtime, S, const CAPACITY: usize> =
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                $phase<'runtime, S, CAPACITY>,
                ResponseOrder<'runtime>,
            >;
    };
}

pub(super) type CommandWait<'runtime, S, const CAPACITY: usize> =
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        CommandOrder<'runtime>,
    >;
pub(super) type ResponseWait<'runtime, S, const CAPACITY: usize> =
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        ResponseOrder<'runtime>,
    >;

recurrence_state_aliases!(
    CommandGraphPrepared,
    ResponseGraphPrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared
);
recurrence_state_aliases!(
    CommandCandidate,
    ResponseCandidate,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate
);
recurrence_state_aliases!(
    CommandPrepared,
    ResponsePrepared,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared
);
recurrence_state_aliases!(
    CommandMerged,
    ResponseMerged,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged
);

macro_rules! recurrence_mismatch_alias {
    ($name:ident, $phase:ident) => {
        pub(super) type $name<'runtime, 'command, S, const CAPACITY: usize> =
            BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<
                'runtime,
                'command,
                $phase<'runtime, S, CAPACITY>,
            >;
    };
}

recurrence_mismatch_alias!(
    SequencePendingMismatch,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending
);
recurrence_mismatch_alias!(
    GraphPreparedMismatch,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared
);
recurrence_mismatch_alias!(
    CandidateMismatch,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate
);
recurrence_mismatch_alias!(
    PreparedMismatch,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared
);
recurrence_mismatch_alias!(
    MergedMismatch,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged
);

fn store_recurring_state<'runtime, S, const CAPACITY: usize>(
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    state: EmbassyBluetoothControllerCommandState<'runtime, S, CAPACITY>,
) where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let active = EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive;
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
    S: BluetoothSchedulerRunInterruptStorage,
{
    actor: RefCell<&'actor mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>>,
    from: EmbassyBluetoothControllerCommandPhase,
    _boundary: PhantomData<fn(&'epoch (), &'packet ())>,
}

struct ResponseDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    actor: RefCell<&'actor mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>>,
    from: EmbassyBluetoothControllerCommandPhase,
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
            S: BluetoothSchedulerRunInterruptStorage,
        {
            fn new(
                actor: &'actor mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
                from: EmbassyBluetoothControllerCommandPhase,
            ) -> Self {
                Self {
                    actor: RefCell::new(actor),
                    from,
                    _boundary: PhantomData,
                }
            }
        }

        impl<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
            EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                $order,
                $running,
            > for $handler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
        where
            S: BluetoothSchedulerRunInterruptStorage + 'runtime,
        {
            type Output = Option<
                EmbassyBluetoothControllerCommandBoundary<
                    'runtime,
                    'epoch,
                    'packet,
                    S,
                    CAPACITY,
                >,
            >;

            fn wait_controller_time(
                &self,
                wait: EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
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
                retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<
                        'runtime,
                        S,
                        CAPACITY,
                    >,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $graph_state(retain_legacy_connectable_advertising_recurring_retry_for_hci(
                        retry,
                    )),
                );
                Some(actor.retain_boundary(
                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyConnectableAdvertisingRecurring,
                    ),
                ))
            }

            fn retry_candidate(
                &self,
                retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $candidate_state(
                        retain_legacy_connectable_advertising_recurring_retry_for_hci(retry),
                    ),
                );
                Some(actor.retain_boundary(
                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyConnectableAdvertisingRecurring,
                    ),
                ))
            }

            fn retry_prepared(
                &self,
                retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                    BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $prepared_state(
                        retain_legacy_connectable_advertising_recurring_retry_for_hci(retry),
                    ),
                );
                Some(actor.retain_boundary(
                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyConnectableAdvertisingRecurring,
                    ),
                ))
            }

            fn retry_merged(
                &self,
                retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                    BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                    S::Error,
                    $order,
                >,
            ) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(
                    &mut actor,
                    self.from,
                    $merged_state(retain_legacy_connectable_advertising_recurring_retry_for_hci(
                        retry,
                    )),
                );
                Some(actor.retain_boundary(
                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyConnectableAdvertisingRecurring,
                    ),
                ))
            }

            fn running(&self, running: $running) -> Self::Output {
                let mut actor = self.actor.borrow_mut();
                store_recurring_state(&mut actor, self.from, $running_state(running));
                None
            }

            fn fail_stop(
                &self,
                failure: BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                    'runtime,
                    S,
                    CAPACITY,
                    $order,
                >,
            ) -> Self::Output {
                let actor = self.actor.borrow();
                Some(actor.terminal_boundary(
                    self.from,
                    EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingRecurringFailStop(
                        EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop {
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
    BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandGraphPrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandCandidate,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandPrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandMerged,
    FailStopOwner::Command
);
impl_actor_drive_handler!(
    ResponseDriveHandler,
    ResponseOrder<'runtime>,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseGraphPrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseCandidate,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponsePrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseMerged,
    FailStopOwner::Response
);

fn command_drive_handler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &'actor mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
) -> CommandDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    CommandDriveHandler::new(actor, from)
}

fn response_drive_handler<'actor, 'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &'actor mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
) -> ResponseDriveHandler<'actor, 'runtime, 'epoch, 'packet, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponseDriveHandler::new(actor, from)
}

pub(super) fn begin_command<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    completed: BluetoothLegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>,
    delay: AdvertisingDelay,
) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    begin_legacy_connectable_advertising_recurring_command_ready_with(
        completed,
        delay,
        &command_drive_handler(actor, from),
    )
}

pub(super) fn begin_response<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    completed: BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<
        'runtime,
        S,
        CAPACITY,
    >,
    delay: AdvertisingDelay,
) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    begin_legacy_connectable_advertising_recurring_response_pending_with(
        completed,
        delay,
        &response_drive_handler(actor, from),
    )
}

pub(super) fn resume_command_wait<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    wait: CommandWait<'runtime, S, CAPACITY>,
    ready: EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady,
) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    wait.resume_with(ready, &command_drive_handler(actor, from))
}

pub(super) fn resume_response_wait<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>(
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    wait: ResponseWait<'runtime, S, CAPACITY>,
    ready: EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady,
) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    wait.resume_with(ready, &response_drive_handler(actor, from))
}

pub(super) enum StopDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Disable(BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    Reset(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
    Wait(
        EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait<
            'runtime,
            S,
            CAPACITY,
        >,
    ),
    FailStop(
        BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ),
}

pub(super) struct StopDriveHandler;

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>
    for StopDriveHandler
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    type Output = StopDrive<'runtime, S, CAPACITY>;

    fn disable_ready(
        &self,
        ready: BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
    ) -> Self::Output {
        StopDrive::Disable(ready)
    }

    fn reset_ready(
        &self,
        ready: BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
    ) -> Self::Output {
        StopDrive::Reset(ready)
    }

    fn wait_cancellation(
        &self,
        wait: EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> Self::Output {
        StopDrive::Wait(wait)
    }

    fn fail_stop(
        &self,
        failure: BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ) -> Self::Output {
        StopDrive::FailStop(failure)
    }
}

pub(super) trait RecurrencePhase<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    type Phase;

    fn drive_command<'epoch, 'packet>(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        from: EmbassyBluetoothControllerCommandPhase,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            CommandOrder<'runtime>,
        >,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

    fn drive_response<'epoch, 'packet>(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        from: EmbassyBluetoothControllerCommandPhase,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            ResponseOrder<'runtime>,
        >,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

    fn retain_command(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            CommandOrder<'runtime>,
        >,
    );

    fn retain_response(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            ResponseOrder<'runtime>,
        >,
    );

    fn cancel<'epoch, 'packet>(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        from: EmbassyBluetoothControllerCommandPhase,
        stopping: BluetoothLegacyConnectableAdvertisingRecurringStopping<'runtime, Self::Phase>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>;

    fn mismatch<'command, 'packet>(
        mismatch: BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<
            'runtime,
            'command,
            Self::Phase,
        >,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'command, 'packet, S, CAPACITY>;
}

pub(super) struct SequencePendingPhase;
pub(super) struct GraphPreparedPhase;
pub(super) struct CandidatePhase;
pub(super) struct PreparedPhase;
pub(super) struct MergedPhase;

impl<'runtime, S, const CAPACITY: usize> RecurrencePhase<'runtime, S, CAPACITY>
    for SequencePendingPhase
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    type Phase =
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>;

    fn drive_command<'epoch, 'packet>(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        _from: EmbassyBluetoothControllerCommandPhase,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            CommandOrder<'runtime>,
        >,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        Self::retain_command(actor, state);
        None
    }

    fn drive_response<'epoch, 'packet>(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        _from: EmbassyBluetoothControllerCommandPhase,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            ResponseOrder<'runtime>,
        >,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        Self::retain_response(actor, state);
        None
    }

    fn retain_command(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            CommandOrder<'runtime>,
        >,
    ) {
        actor.store_retained_state(
            EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
            EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(
                retain_legacy_connectable_advertising_recurring_controller_time(state),
            ),
        );
    }

    fn retain_response(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            Self::Phase,
            ResponseOrder<'runtime>,
        >,
    ) {
        actor.store_retained_state(
            EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
            EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(
                retain_legacy_connectable_advertising_recurring_controller_time(state),
            ),
        );
    }

    fn cancel<'epoch, 'packet>(
        actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
        from: EmbassyBluetoothControllerCommandPhase,
        stopping: BluetoothLegacyConnectableAdvertisingRecurringStopping<'runtime, Self::Phase>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        let drive = cancel_legacy_connectable_advertising_recurring_sequence_pending_with(
            stopping,
            &StopDriveHandler,
        );
        actor.store_connectable_recurring_stop_drive(from, drive)
    }

    fn mismatch<'command, 'packet>(
        mismatch: BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<
            'runtime,
            'command,
            Self::Phase,
        >,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'command, 'packet, S, CAPACITY> {
        EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingRecurringSequencePendingCommandEndpointMismatch(mismatch)
    }
}

macro_rules! impl_recurrence_phase {
    (
        $marker:ident, $phase:ident, $command_state:expr, $response_state:expr,
        $drive:path, $cancel:path, $mismatch_state:ident
    ) => {
        impl<'runtime, S, const CAPACITY: usize> RecurrencePhase<'runtime, S, CAPACITY> for $marker
        where
            S: BluetoothSchedulerRunInterruptStorage + 'runtime,
        {
            type Phase = $phase<'runtime, S, CAPACITY>;

            fn drive_command<'epoch, 'packet>(
                actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
                from: EmbassyBluetoothControllerCommandPhase,
                state: BluetoothLegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    CommandOrder<'runtime>,
                >,
            ) -> Option<
                EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
            > {
                $drive(state, &command_drive_handler(actor, from))
            }

            fn drive_response<'epoch, 'packet>(
                actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
                from: EmbassyBluetoothControllerCommandPhase,
                state: BluetoothLegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    ResponseOrder<'runtime>,
                >,
            ) -> Option<
                EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
            > {
                $drive(state, &response_drive_handler(actor, from))
            }

            fn retain_command(
                actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
                state: BluetoothLegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    CommandOrder<'runtime>,
                >,
            ) {
                actor.store_retained_state(
                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                    $command_state(state),
                );
            }

            fn retain_response(
                actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
                state: BluetoothLegacyConnectableAdvertisingRecurringHci<
                    Self::Phase,
                    ResponseOrder<'runtime>,
                >,
            ) {
                actor.store_retained_state(
                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                    $response_state(state),
                );
            }

            fn cancel<'epoch, 'packet>(
                actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
                from: EmbassyBluetoothControllerCommandPhase,
                stopping: BluetoothLegacyConnectableAdvertisingRecurringStopping<
                    'runtime,
                    Self::Phase,
                >,
            ) -> Option<
                EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
            > {
                actor.store_connectable_recurring_stop_drive(
                    from,
                    $cancel(stopping, &StopDriveHandler),
                )
            }

            fn mismatch<'command, 'packet>(
                mismatch: BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<
                    'runtime,
                    'command,
                    Self::Phase,
                >,
            ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'command, 'packet, S, CAPACITY>
            {
                EmbassyBluetoothControllerCommandBoundary::$mismatch_state(mismatch)
            }
        }
    };
}

impl_recurrence_phase!(
    GraphPreparedPhase,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandGraphPrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseGraphPrepared,
    drive_legacy_connectable_advertising_recurring_graph_prepared_with,
    cancel_legacy_connectable_advertising_recurring_graph_prepared_with,
    LegacyConnectableAdvertisingRecurringGraphPreparedCommandEndpointMismatch
);
impl_recurrence_phase!(
    CandidatePhase,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandCandidate,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseCandidate,
    drive_legacy_connectable_advertising_recurring_candidate_with,
    cancel_legacy_connectable_advertising_recurring_candidate_with,
    LegacyConnectableAdvertisingRecurringCandidateCommandEndpointMismatch
);
impl_recurrence_phase!(
    PreparedPhase,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandPrepared,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponsePrepared,
    drive_legacy_connectable_advertising_recurring_prepared_with,
    cancel_legacy_connectable_advertising_recurring_prepared_with,
    LegacyConnectableAdvertisingRecurringPreparedCommandEndpointMismatch
);
impl_recurrence_phase!(
    MergedPhase,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandMerged,
    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseMerged,
    drive_legacy_connectable_advertising_recurring_merged_with,
    cancel_legacy_connectable_advertising_recurring_merged_with,
    LegacyConnectableAdvertisingRecurringMergedCommandEndpointMismatch
);

pub(super) struct CommandRouteOutcome<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    buffer: Option<&'buffer mut [u8]>,
    boundary:
        Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'command, 'buffer, S, CAPACITY>>,
}

impl<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
    CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) fn into_parts(
        self,
    ) -> (
        Option<&'buffer mut [u8]>,
        Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'command, 'buffer, S, CAPACITY>>,
    ) {
        (self.buffer, self.boundary)
    }
}

struct CommandRouteHandler<'actor, 'runtime, 'command, S, const CAPACITY: usize, PhaseMap>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    actor: &'actor mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    _phase: PhantomData<fn() -> PhaseMap>,
    _command: PhantomData<&'command ()>,
}

impl<'actor, 'runtime, 'command, 'buffer, S, const CAPACITY: usize, PhaseMap>
    BluetoothLegacyConnectableAdvertisingRecurringCommandHandler<
        'runtime,
        'command,
        'buffer,
        PhaseMap::Phase,
    > for CommandRouteHandler<'actor, 'runtime, 'command, S, CAPACITY, PhaseMap>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
    PhaseMap: RecurrencePhase<'runtime, S, CAPACITY>,
{
    type Output = CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>;

    fn response_pending(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            PhaseMap::Phase,
            ResponseOrder<'runtime>,
        >,
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
        state: BluetoothLegacyConnectableAdvertisingRecurringStopping<'runtime, PhaseMap::Phase>,
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
        mismatch: BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<
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
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            PhaseMap::Phase,
            CommandOrder<'runtime>,
        >,
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
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            PhaseMap::Phase,
            CommandOrder<'runtime>,
        >,
        buffer: &'buffer mut [u8],
    ) -> Self::Output {
        PhaseMap::retain_command(self.actor, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary: Some(
                self.actor
                    .retain_boundary(EmbassyBluetoothControllerCommandBoundary::EndpointMismatch),
            ),
        }
    }

    fn channel_fault(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            PhaseMap::Phase,
            CommandOrder<'runtime>,
        >,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    ) -> Self::Output {
        PhaseMap::retain_command(self.actor, state);
        CommandRouteOutcome {
            buffer: Some(buffer),
            boundary: Some(
                self.actor
                    .retain_boundary(EmbassyBluetoothControllerCommandBoundary::HciFault(error)),
            ),
        }
    }

    fn non_command(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            PhaseMap::Phase,
            CommandOrder<'runtime>,
        >,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    ) -> Self::Output {
        PhaseMap::retain_command(self.actor, state);
        CommandRouteOutcome {
            buffer: None,
            boundary: Some(
                self.actor
                    .retain_boundary(EmbassyBluetoothControllerCommandBoundary::NonCommand(frame)),
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
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    from: EmbassyBluetoothControllerCommandPhase,
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        PhaseMap::Phase,
        CommandOrder<'runtime>,
    >,
    controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
    buffer: &'buffer mut [u8],
) -> CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
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
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        PhaseMap::Phase,
        ResponseOrder<'runtime>,
    >,
    controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
    PhaseMap: RecurrencePhase<'runtime, S, CAPACITY>,
{
    let actor = RefCell::new(actor);
    let phase = EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive;
    state.try_publish_response_with(
        controller,
        |state| PhaseMap::drive_command(&mut actor.borrow_mut(), phase, state),
        |state| PhaseMap::drive_response(&mut actor.borrow_mut(), phase, state),
        |state| {
            let mut actor = actor.borrow_mut();
            PhaseMap::retain_response(&mut actor, state);
            Some(actor.retain_boundary(EmbassyBluetoothControllerCommandBoundary::EndpointMismatch))
        },
        |state, error| {
            let mut actor = actor.borrow_mut();
            PhaseMap::retain_response(&mut actor, state);
            Some(actor.retain_boundary(EmbassyBluetoothControllerCommandBoundary::HciFault(error)))
        },
    )
}

pub(super) fn is_retry<S, const CAPACITY: usize>(
    state: &EmbassyBluetoothControllerCommandState<'_, S, CAPACITY>,
) -> bool
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    use EmbassyBluetoothControllerCommandState::*;
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
    actor: &mut EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>,
    controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
    buffer: &'buffer mut [u8],
) -> CommandRouteOutcome<'runtime, 'command, 'buffer, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    use EmbassyBluetoothControllerCommandState::*;
    let phase = EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive;
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
    S: BluetoothSchedulerRunInterruptStorage,
{
    Command(
        BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            CommandOrder<'runtime>,
        >,
    ),
    Response(
        BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            ResponseOrder<'runtime>,
        >,
    ),
    Stopping(
        BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ),
}

/// Opaque terminal recurrence failure retaining the exact radio and HCI axes.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) _owner: FailStopOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectableAdvertisingRecurringFailStopCause {
        match &self._owner {
            FailStopOwner::Command(failure) => failure.cause(),
            FailStopOwner::Response(failure) => failure.cause(),
            FailStopOwner::Stopping(failure) => failure.cause(),
        }
    }
}
