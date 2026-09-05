use super::{
    BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
    BluetoothSchedulerWorkerWakeClass,
};

#[test]
fn first_publication_wakes_and_ordinary_duplicates_coalesce() {
    let cell = BluetoothSchedulerWakeCell::new();

    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
        BluetoothSchedulerWakePublication::WakeWorker
    );
    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
        BluetoothSchedulerWakePublication::Coalesced
    );
    assert!(cell.is_pending());
    assert!(!cell.take().expect("one batch must be pending").is_marked());
    assert!(!cell.is_pending());
}

#[test]
fn marker_is_sticky_for_both_publication_orders() {
    for classes in [
        [
            BluetoothSchedulerWorkerWakeClass::Ordinary,
            BluetoothSchedulerWorkerWakeClass::Marked,
        ],
        [
            BluetoothSchedulerWorkerWakeClass::Marked,
            BluetoothSchedulerWorkerWakeClass::Ordinary,
        ],
    ] {
        let cell = BluetoothSchedulerWakeCell::new();
        assert_eq!(
            cell.publish_from_interrupt(classes[0]),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert_eq!(
            cell.publish_from_interrupt(classes[1]),
            BluetoothSchedulerWakePublication::Coalesced
        );
        assert!(cell.take().expect("one batch must be pending").is_marked());
    }
}

#[test]
fn dequeue_closes_the_epoch_and_the_next_publication_wakes_again() {
    let cell = BluetoothSchedulerWakeCell::new();
    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Marked),
        BluetoothSchedulerWakePublication::WakeWorker
    );
    assert!(
        cell.take()
            .expect("first batch must be pending")
            .is_marked()
    );
    assert_eq!(cell.take(), None);

    assert_eq!(
        cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
        BluetoothSchedulerWakePublication::WakeWorker
    );
    assert!(
        !cell
            .take()
            .expect("second batch must be pending")
            .is_marked()
    );
}
