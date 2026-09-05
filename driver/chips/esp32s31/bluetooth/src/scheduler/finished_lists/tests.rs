use open_esp_radio_esp32s31_pac::BluetoothSchedulerFinishedListObservation;

use super::{
    BluetoothSchedulerFinishedListBackend, BluetoothSchedulerFinishedListCaptureError,
    BluetoothSchedulerFinishedListWorker, BluetoothSchedulerFinishedListWorkerStep,
};
use crate::{BluetoothSchedulerWakeCell, BluetoothSchedulerWorkerWakeClass};

struct Backend {
    observation: Option<BluetoothSchedulerFinishedListObservation>,
}

impl BluetoothSchedulerFinishedListBackend for Backend {
    fn transfer_scheduler_finished_lists(&mut self) -> BluetoothSchedulerFinishedListObservation {
        self.observation.take().expect("one scripted transfer")
    }
}

fn backend(lists: &[u8]) -> Backend {
    Backend {
        observation: Some(
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(lists)
                .expect("semantic list set is valid"),
        ),
    }
}

fn wake(class: BluetoothSchedulerWorkerWakeClass) -> crate::BluetoothSchedulerWakeBatch {
    let cell = BluetoothSchedulerWakeCell::new();
    let _publication = cell.publish_from_interrupt(class);
    cell.take().expect("one scheduler batch must be pending")
}

fn assert_list(
    step: BluetoothSchedulerFinishedListWorkerStep,
    expected_index: u8,
    expected_more: bool,
) {
    match step {
        BluetoothSchedulerFinishedListWorkerStep::List { observed, more } => {
            assert_eq!(observed.index().get(), expected_index);
            assert_eq!(more, expected_more);
        }
        _ => panic!("the scripted observation must yield one list"),
    }
}

#[test]
fn multiple_lists_are_drained_lowest_first_one_per_step() {
    let mut backend = backend(&[9, 3]);
    let mut worker = BluetoothSchedulerFinishedListWorker::new();

    worker
        .capture_with(
            &mut backend,
            wake(BluetoothSchedulerWorkerWakeClass::Ordinary),
        )
        .expect("idle worker accepts one transfer");
    assert_eq!(
        worker.capture_with(
            &mut backend,
            wake(BluetoothSchedulerWorkerWakeClass::Marked),
        ),
        Err(BluetoothSchedulerFinishedListCaptureError::DrainAlreadyActive)
    );
    assert_list(worker.step(), 3, true);
    assert!(worker.is_active());
    assert_list(worker.step(), 9, false);
    assert!(!worker.is_active());
    assert_eq!(
        worker.step(),
        BluetoothSchedulerFinishedListWorkerStep::Idle
    );
}

#[test]
fn single_list_exhausts_the_capture() {
    let mut backend = backend(&[3]);
    let mut worker = BluetoothSchedulerFinishedListWorker::new();

    worker
        .capture_with(
            &mut backend,
            wake(BluetoothSchedulerWorkerWakeClass::Ordinary),
        )
        .unwrap();
    assert_list(worker.step(), 3, false);
    assert!(!worker.is_active());
    assert_eq!(
        worker.step(),
        BluetoothSchedulerFinishedListWorkerStep::Idle
    );
}

#[test]
fn list_zero_precedes_an_unowned_list_without_losing_the_capture() {
    let mut backend = backend(&[0, 3]);
    let mut worker = BluetoothSchedulerFinishedListWorker::new();

    worker
        .capture_with(
            &mut backend,
            wake(BluetoothSchedulerWorkerWakeClass::Marked),
        )
        .unwrap();
    assert_list(worker.step(), 0, true);
    assert!(worker.is_active());
    assert_list(worker.step(), 3, false);
    assert!(!worker.is_active());
}

#[test]
fn sole_list_zero_exhausts_the_capture() {
    let mut backend = backend(&[0]);
    let mut worker = BluetoothSchedulerFinishedListWorker::new();

    worker
        .capture_with(
            &mut backend,
            wake(BluetoothSchedulerWorkerWakeClass::Ordinary),
        )
        .unwrap();
    assert_list(worker.step(), 0, false);
    assert!(!worker.is_active());
    assert_eq!(
        worker.step(),
        BluetoothSchedulerFinishedListWorkerStep::Idle
    );
}
