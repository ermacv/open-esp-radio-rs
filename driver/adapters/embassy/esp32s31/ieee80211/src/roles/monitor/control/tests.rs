use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

#[test]
fn request_is_idempotent_and_task_acknowledges_the_stop_edge() {
    let resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
    let (mut controller, mut receiver) = resources.split().unwrap();

    assert!(controller.request_stop());
    assert!(!controller.request_stop());
    block_on(receiver.wait_stop());
    receiver.complete(Esp32s31MonitorCompletion::Stopped);

    assert_eq!(
        block_on(controller.wait_completion()),
        Esp32s31MonitorCompletion::Stopped
    );
}

#[test]
fn dropping_the_application_handle_does_not_cancel_the_task_endpoint() {
    let resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split().unwrap();

    drop(controller);
    assert!(!receiver.resources.stop_requested.load(Ordering::Acquire));
    receiver.complete(Esp32s31MonitorCompletion::Faulted);
    assert_eq!(
        Esp32s31MonitorCompletion::decode(receiver.resources.completion.load(Ordering::Acquire)),
        Some(Esp32s31MonitorCompletion::Faulted)
    );
}

#[test]
fn dropping_task_endpoint_publishes_sticky_fault() {
    let resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
    let (mut controller, receiver) = resources.split().unwrap();

    drop(receiver);
    assert_eq!(
        block_on(controller.wait_completion()),
        Esp32s31MonitorCompletion::Faulted
    );
    drop(controller);
    assert!(matches!(
        resources.split(),
        Err(Esp32s31MonitorControlError::Faulted)
    ));
}

#[test]
fn clean_epoch_is_reusable_only_after_both_endpoints_drop() {
    let resources = Esp32s31MonitorControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split().unwrap();
    assert!(matches!(
        resources.split(),
        Err(Esp32s31MonitorControlError::InUse)
    ));

    receiver.complete(Esp32s31MonitorCompletion::Stopped);
    drop(receiver);
    assert!(matches!(
        resources.split(),
        Err(Esp32s31MonitorControlError::InUse)
    ));
    drop(controller);
    assert!(resources.split().is_ok());
}
