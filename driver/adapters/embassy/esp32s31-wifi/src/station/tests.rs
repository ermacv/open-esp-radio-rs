use core::future::{Future, ready};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_wifi_sta::station::{
    StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffOutcome, StaBackoffReason,
    StaFailureDisposition, StaLifecycleBackend, StaLifecycleStage, StaReconnectPolicy,
};

use super::connected_epoch::{
    coalesce_disconnected_station_command, complete_connected_station_command,
};
use super::*;

struct Backend<'control> {
    control: Esp32s31StationCommandReceiver<'control, NoopRawMutex>,
    fail: bool,
}

impl StaLifecycleBackend for Backend<'_> {
    type Owner = u32;
    type Error = u8;

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + '_ {
        let outcome = if let Some(command) = self.control.try_take() {
            self.control.record_terminal(command);
            StaAttemptOutcome::Stopped { owner }
        } else if self.fail {
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

    fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        _delay_millis: u32,
        _reason: StaBackoffReason,
    ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
        ready(StaBackoffOutcome::Elapsed { owner })
    }
}

fn policy(attempt_limit: u16) -> StaReconnectPolicy {
    StaReconnectPolicy::new(attempt_limit, 1, 1, 1).unwrap()
}

#[test]
fn command_priority_never_downgrades_a_pending_stop() {
    let mut resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split();
    assert!(controller.request_reconnect());
    assert!(controller.request_disconnect());
    assert!(!controller.request_reconnect());
    assert!(controller.request_stop());
    assert!(!controller.request_disconnect());
    assert_eq!(block_on(receiver.wait()), Esp32s31StationCommand::Stop);
}

#[test]
fn peer_disconnect_coalesces_a_pending_reconnect_without_leaking_it() {
    let mut resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split();
    assert!(controller.request_reconnect());
    assert!(matches!(
        coalesce_disconnected_station_command::<(), _>(&mut receiver),
        Esp32s31ConnectedStationExit::ReconnectRequested {
            source: Esp32s31StationReconnectSource::CoalescedDisconnect,
        }
    ));
    assert_eq!(receiver.try_take(), None);
}

#[test]
fn terminal_connected_command_records_the_public_stop_reason() {
    let mut resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = resources.split();
    assert!(matches!(
        complete_connected_station_command::<(), _>(Esp32s31StationCommand::Stop, &mut receiver,),
        Esp32s31ConnectedStationExit::StationStopped(Esp32s31StationCommand::Stop)
    ));
    assert_eq!(receiver.take_terminal(), Some(Esp32s31StationCommand::Stop));
}

#[test]
fn controller_stop_returns_the_exact_owner_and_reason() {
    let mut control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, runner) = Esp32s31Station::new(
        Esp32s31StationConfig::new(policy(2)),
        Esp32s31StationResources::new(41),
        &mut control,
        |control| Backend {
            control,
            fail: false,
        },
    );
    assert!(controller.request_stop());
    let Esp32s31StationExit::Stopped {
        resources,
        progress,
        reason,
    } = block_on(runner.run())
    else {
        panic!("station did not stop");
    };
    assert_eq!(resources.into_owner(), 41);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(
        reason,
        Esp32s31StationStopReason::Requested(Esp32s31StationCommand::Stop)
    );
}

#[test]
fn retry_exhaustion_preserves_failure_and_owner() {
    let mut control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (_controller, runner) = Esp32s31Station::new(
        Esp32s31StationConfig::new(policy(1)),
        Esp32s31StationResources::new(77),
        &mut control,
        |control| Backend {
            control,
            fail: true,
        },
    );
    let Esp32s31StationExit::RetryExhausted {
        resources,
        progress,
        failure,
    } = block_on(runner.run())
    else {
        panic!("station did not exhaust its bounded retry policy");
    };
    assert_eq!(resources.into_owner(), 77);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(failure.stage, StaLifecycleStage::Authentication);
    assert_eq!(failure.error, 9);
}
