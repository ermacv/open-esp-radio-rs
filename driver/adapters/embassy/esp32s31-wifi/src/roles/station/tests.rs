#![expect(
    clippy::manual_async_fn,
    reason = "station test doubles implement the production borrowed Future contracts"
)]

use core::{
    future::{Future, pending, ready},
    task::{Context, Poll, Waker},
};

use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_wifi_sta::station::{
    StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition,
    StaLifecycleStage, StaReconnectPolicy,
};

use super::connected_epoch::{
    coalesce_disconnected_station_command, complete_connected_station_command,
};
use super::*;

struct Backend {
    fail: bool,
}

struct PendingBackend;

impl Esp32s31StationAttemptRunner<NoopRawMutex> for PendingBackend {
    type Owner = u32;
    type Error = u8;
    type Fault = core::convert::Infallible;

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        async move {
            pending::<()>().await;
            StaAttemptOutcome::Stopped { owner }
        }
    }
}

impl Esp32s31StationAttemptRunner<NoopRawMutex> for Backend {
    type Owner = u32;
    type Error = u8;
    type Fault = core::convert::Infallible;

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        let outcome = if self.fail {
            StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Authentication,
                    StaFailureDisposition::RetryCurrentCandidate,
                    9,
                ),
            }
        } else {
            StaAttemptOutcome::Stopped { owner }
        };
        ready(outcome)
    }
}

fn policy(attempt_limit: u16) -> StaReconnectPolicy {
    StaReconnectPolicy::new(attempt_limit, 1, 1, 1).unwrap()
}

#[test]
fn command_priority_never_downgrades_a_pending_stop() {
    let resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split().unwrap();
    assert!(controller.request_reconnect());
    assert!(controller.request_disconnect());
    assert!(!controller.request_reconnect());
    assert!(controller.request_stop());
    assert!(!controller.request_disconnect());
    assert_eq!(block_on(receiver.wait()), Esp32s31StationCommand::Stop);
}

#[test]
fn peer_disconnect_coalesces_a_pending_reconnect_without_leaking_it() {
    let resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split().unwrap();
    assert!(controller.request_reconnect());
    assert!(matches!(
        coalesce_disconnected_station_command::<(), _>(
            &mut receiver,
            open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::BeaconLoss,
        ),
        Esp32s31ConnectedStationExit::ReconnectRequested {
            source: Esp32s31StationReconnectSource::CoalescedDisconnect,
        }
    ));
    assert_eq!(receiver.try_take(), None);
}

#[test]
fn terminal_connected_command_records_the_public_stop_reason() {
    let resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = resources.split().unwrap();
    assert!(matches!(
        complete_connected_station_command::<(), _>(Esp32s31StationCommand::Stop, &mut receiver,),
        Esp32s31ConnectedStationExit::StationStopped(Esp32s31StationCommand::Stop)
    ));
    assert_eq!(receiver.take_terminal(), Some(Esp32s31StationCommand::Stop));
}

#[test]
fn controller_stop_returns_the_exact_owner_and_reason() {
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut runner) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy(2)),
        Esp32s31StationStartResources::new(41),
        &control,
        Backend { fail: false },
    )
    .unwrap();
    let (completion, exit) = block_on(join(controller.stop(), runner.run()));
    let Esp32s31StationExit::Stopped {
        resources,
        progress,
        reason,
    } = exit
    else {
        panic!("station did not stop");
    };
    assert_eq!(completion, Esp32s31StationCompletion::Stopped);
    let (owner, runner) = resources.into_parts();
    assert_eq!(owner, 41);
    assert!(!runner.fail);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(
        reason,
        Esp32s31StationStopReason::Requested(Esp32s31StationCommand::Stop)
    );
}

#[test]
fn retry_exhaustion_preserves_failure_and_owner() {
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut runner) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy(1)),
        Esp32s31StationStartResources::new(77),
        &control,
        Backend { fail: true },
    )
    .unwrap();
    let exit = block_on(runner.run());
    assert_eq!(
        block_on(controller.wait_completion()),
        Esp32s31StationCompletion::Ended
    );
    let Esp32s31StationExit::RetryExhausted {
        resources,
        progress,
        failure,
    } = exit
    else {
        panic!("station did not exhaust its bounded retry policy");
    };
    let (owner, runner) = resources.into_parts();
    assert_eq!(owner, 77);
    assert!(runner.fail);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(failure.stage, StaLifecycleStage::Authentication);
    assert_eq!(failure.error, 9);
}

#[test]
fn cancelled_station_task_reports_fault_instead_of_claiming_quiescence() {
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut task) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy(1)),
        Esp32s31StationStartResources::new(99),
        &control,
        PendingBackend,
    )
    .unwrap();
    let mut run = std::boxed::Box::pin(task.run());
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    drop(run);

    assert_eq!(
        block_on(controller.wait_completion()),
        Esp32s31StationCompletion::Faulted
    );
    drop(controller);
    assert!(matches!(
        control.split(),
        Err(Esp32s31StationControlError::Faulted)
    ));
}

#[test]
fn clean_station_completion_allows_a_later_epoch() {
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut task) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy(1)),
        Esp32s31StationStartResources::new(17),
        &control,
        Backend { fail: false },
    )
    .unwrap();
    let (completion, _) = block_on(join(controller.stop(), task.run()));
    assert_eq!(completion, Esp32s31StationCompletion::Stopped);
    drop(controller);

    assert!(control.split().is_ok());
}

#[test]
fn later_epoch_waits_for_both_previous_control_endpoints_to_drop() {
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, mut task) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy(1)),
        Esp32s31StationStartResources::new(23),
        &control,
        Backend { fail: false },
    )
    .unwrap();

    assert!(matches!(
        control.split(),
        Err(Esp32s31StationControlError::InUse)
    ));
    let _exit = block_on(task.run());
    assert!(matches!(
        control.split(),
        Err(Esp32s31StationControlError::InUse)
    ));
    drop(controller);
    assert!(control.split().is_ok());
}

#[test]
fn failed_task_prepare_returns_owner_policy_and_runner() {
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (_occupied_controller, _occupied_receiver) = control.split().unwrap();
    let config = Esp32s31StationConfig::new(policy(2));
    let failure = match prepare_esp32s31_station_task(
        config,
        Esp32s31StationStartResources::new(29),
        &control,
        Backend { fail: true },
    ) {
        Ok(_) => panic!("an occupied command domain must reject another station task"),
        Err(failure) => failure,
    };
    let (error, returned_config, resources, runner) = failure.into_parts();
    assert_eq!(error, Esp32s31StationControlError::InUse);
    assert_eq!(returned_config, config);
    assert_eq!(resources.into_owner(), 29);
    assert!(runner.fail);
}
