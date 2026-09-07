use oer_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothSchedulerLockModifyInterruptObservation,
    BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyPublished,
    BluetoothSchedulerLockModifyRequest, BluetoothSchedulerLockModifyTaskObservation,
};

use super::{
    SchedulerLockModifyAwaitingPublication, SchedulerLockModifyBackend,
    SchedulerLockModifyBeginError, SchedulerLockModifyEventCell,
    SchedulerLockModifyEventPublication, SchedulerLockModifyInFlight, SchedulerLockModifyProgress,
    SchedulerLockModifyWorker, SchedulerLockModifyWorkerStep,
};

fn request() -> BluetoothSchedulerLockModifyRequest {
    BluetoothSchedulerLockModifyRequest::new(
        BluetoothControllerSramAddress::new(0x2f00_0040).expect("test address is representable"),
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex::new(6)
            .expect("test list index is representable"),
    )
}

#[test]
fn each_busy_edge_returns_control_to_the_executor() {
    let waiting = SchedulerLockModifyAwaitingPublication::new(request());
    let waiting = match waiting
        .observe(BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, true, 0))
    {
        SchedulerLockModifyProgress::Waiting(waiting) => waiting,
        SchedulerLockModifyProgress::Ready(_) => panic!("busy request advanced"),
    };

    let publication = match waiting.observe(
        BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, false, 0),
    ) {
        SchedulerLockModifyProgress::Ready(publication) => publication,
        SchedulerLockModifyProgress::Waiting(_) => panic!("ready request stalled"),
    };
    let _publication_requires_live_hal = publication;
}

#[test]
fn published_request_yields_once_per_busy_event_before_result() {
    let request = request();
    let in_flight = SchedulerLockModifyInFlight {
        _publication: BluetoothSchedulerLockModifyPublished::for_validation(),
        request,
    };
    let in_flight = match in_flight
        .observe(BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, true, 0))
    {
        SchedulerLockModifyProgress::Waiting(in_flight) => in_flight,
        SchedulerLockModifyProgress::Ready(_) => panic!("busy request advanced"),
    };
    let result = match in_flight.observe(
        BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, false, 5),
    ) {
        SchedulerLockModifyProgress::Ready(result) => result,
        SchedulerLockModifyProgress::Waiting(_) => panic!("ready result stalled"),
    };

    assert_eq!(result.code(), 5);
    assert_eq!(result.request(), request);
}

#[test]
fn scheduler_idle_admits_publication_without_raw_register_images() {
    let publication = match SchedulerLockModifyAwaitingPublication::new(request()).observe(
        BluetoothSchedulerLockModifyObservation::from_fields_for_validation(false, true, 0),
    ) {
        SchedulerLockModifyProgress::Ready(publication) => publication,
        SchedulerLockModifyProgress::Waiting(_) => panic!("idle scheduler stalled"),
    };
    let _publication_requires_live_hal = publication;
}

#[test]
fn event_handoff_keeps_the_latest_level_and_reopens_the_wake_epoch() {
    let cell = SchedulerLockModifyEventCell::new();
    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerLockModifyInterruptObservation::from_busy(
            true
        )),
        SchedulerLockModifyEventPublication::WakeWorker
    );
    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerLockModifyInterruptObservation::from_busy(
            false
        )),
        SchedulerLockModifyEventPublication::Coalesced
    );
    assert!(cell.is_pending());
    assert!(
        !cell
            .take()
            .expect("latest event must remain pending")
            .interrupt
            .is_busy()
    );
    assert!(!cell.is_pending());
    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerLockModifyInterruptObservation::from_busy(
            true
        )),
        SchedulerLockModifyEventPublication::WakeWorker
    );
}

struct Backend {
    observations: [BluetoothSchedulerLockModifyTaskObservation; 4],
    next_observation: usize,
    published: usize,
}

impl SchedulerLockModifyBackend for Backend {
    fn capture_task(&mut self) -> BluetoothSchedulerLockModifyTaskObservation {
        let observation = self.observations[self.next_observation];
        self.next_observation += 1;
        observation
    }

    fn publish(
        &mut self,
        _request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        self.published += 1;
        BluetoothSchedulerLockModifyPublished::for_validation()
    }
}

fn event(cell: &SchedulerLockModifyEventCell, busy: bool) -> super::SchedulerLockModifyEvent {
    let _wake = cell.publish_from_interrupt(
        BluetoothSchedulerLockModifyInterruptObservation::from_busy(busy),
    );
    cell.take().expect("published event must be available")
}

#[test]
fn durable_worker_publishes_once_and_retains_result_across_event_steps() {
    let cell = SchedulerLockModifyEventCell::new();
    let mut backend = Backend {
        observations: [
            BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(true, 0),
            BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(false, 0),
            BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(true, 0),
            BluetoothSchedulerLockModifyTaskObservation::from_fields_for_validation(false, 5),
        ],
        next_observation: 0,
        published: 0,
    };
    let mut worker = SchedulerLockModifyWorker::new();
    worker
        .begin_inner(request())
        .expect("idle worker admits request");

    assert_eq!(
        worker.step_with(event(&cell, true), &mut backend),
        SchedulerLockModifyWorkerStep::Waiting
    );
    assert_eq!(
        worker.step_with(event(&cell, true), &mut backend),
        SchedulerLockModifyWorkerStep::Published
    );
    assert_eq!(backend.published, 1);
    assert_eq!(
        worker.begin_inner(request()),
        Err(SchedulerLockModifyBeginError::AlreadyInFlight)
    );

    assert_eq!(
        worker.step_with(event(&cell, true), &mut backend),
        SchedulerLockModifyWorkerStep::Waiting
    );
    assert_eq!(
        worker.step_with(event(&cell, true), &mut backend),
        SchedulerLockModifyWorkerStep::Ready
    );
    assert_eq!(
        worker.begin_inner(request()),
        Err(SchedulerLockModifyBeginError::ResultPending)
    );
    let result = worker.take_result().expect("result is durable");
    assert_eq!(result.code(), 5);
    assert_eq!(result.request(), request());
    assert!(!worker.is_active());
    assert_eq!(backend.published, 1);
}
