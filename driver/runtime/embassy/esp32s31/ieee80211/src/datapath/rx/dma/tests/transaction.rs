use super::*;

const TRANSACTION_COUNT: usize = 2;
const TRANSACTION_CAPACITY: usize = 64;
const TRANSACTION_STORAGE: usize = 68;
const TRANSACTION_ADDRESSES: [u32; TRANSACTION_COUNT] = [0x2f00_2000, 0x2f00_2200];
const TRANSACTION_PAYLOAD: [u8; 16] = [1, 3, 5, 7, 9, 11, 13, 15, 2, 4, 6, 8, 10, 12, 14, 16];

type TransactionStorage =
    Esp32s31RxDmaStorage<TRANSACTION_COUNT, TRANSACTION_CAPACITY, TRANSACTION_STORAGE>;

fn transaction_fixture() -> (
    &'static TransactionStorage,
    RxRingLive<'static, TRANSACTION_COUNT>,
    MockRxDma,
    *const u8,
) {
    let storage = Box::leak(Box::new(TransactionStorage::new()));
    let pointer = storage.buffer_mut(0).unwrap().as_ptr();
    storage.buffer_mut(0).unwrap()[..TRANSACTION_PAYLOAD.len()]
        .copy_from_slice(&TRANSACTION_PAYLOAD);
    let mut hardware = MockRxDma::default();
    let ring = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &TRANSACTION_ADDRESSES,
        TRANSACTION_CAPACITY as u32,
        |_| Ok(()),
    )
    .unwrap()
    .try_start(&mut hardware)
    .map_err(|(_, error)| error)
    .unwrap();
    // Model a completed frame. Assertions below exercise lease ownership and
    // payload identity, not the descriptor representation used by the fixture.
    storage.descriptors()[0].write_word0(
        TRANSACTION_CAPACITY as u32
            | ((TRANSACTION_PAYLOAD.len() as u32) << LENGTH_SHIFT)
            | BIT_30
            | BIT_31,
    );
    hardware.release_through(TRANSACTION_COUNT - 1, None);
    (storage, ring, hardware, pointer)
}

#[test]
fn adapter_service_publishes_the_original_buffer_before_its_future_is_polled() {
    let (storage, ring, mut hardware, pointer) = transaction_fixture();
    let pool = RxStagePool::<1, TRANSACTION_CAPACITY>::new();
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, TRANSACTION_CAPACITY, 1>::new();
    let (sender, receiver) = queue.split();
    let mut producer = Esp32s31StagedRxProducer::new(ring, storage, &pool, NoDelay, sender);

    let completion = producer.service(&mut hardware);
    let frame = receiver
        .try_receive()
        .expect("the synchronous transaction runs before any future poll");
    assert_eq!(frame.segment().buffer.as_ptr(), pointer);
    assert_eq!(frame.segment().buffer, TRANSACTION_PAYLOAD);
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(storage.detached_buffer_count(), 1);
    assert_eq!(storage.released_buffer_count(), 0);

    // Cancellation of this ready adapter result does not revoke publication.
    drop(completion);
    assert_eq!(producer.work_counters().completed_units, 1);
    assert_eq!(producer.work_counters().staged_bytes, 16);
    let mut producer = match producer.try_stop(&mut hardware) {
        Ok(_) => panic!("a live upper lease must reject physical stop"),
        Err((producer, error)) => {
            assert_eq!(error, RxRingError::Busy);
            producer
        }
    };
    assert!(hardware.walker);
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(frame.segment().buffer, TRANSACTION_PAYLOAD);
    drop(frame);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(storage.released_buffer_count(), 1);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(storage.detached_buffer_count(), 0);
    producer
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("returned lease leaves the producer stoppable"));
}

use core::cell::{Cell, RefCell};
use open_esp_radio_esp32s31_wifi::rx::transaction::{self, Publisher};
use open_esp_radio_esp32s31_wifi_mac::rx::pool::NetworkRxFrame;

type TransactionFrame<'pool> = NetworkRxFrame<'pool, 1, TRANSACTION_CAPACITY>;

struct RecordingPublisher<'pool> {
    accept: bool,
    pointer: *const u8,
    calls: Cell<usize>,
    seen_slot: Cell<Option<usize>>,
    retained: RefCell<Option<TransactionFrame<'pool>>>,
}

impl<'pool> Publisher<'pool, TRANSACTION_CAPACITY, 1> for RecordingPublisher<'pool> {
    const DEPTH: usize = 1;

    fn free_capacity(&self) -> usize {
        usize::from(self.retained.borrow().is_none())
    }

    fn preview(&self, unit: transaction::CompletedUnit, _bytes: [u8; 24]) -> transaction::Preview {
        self.unclassified_preview(unit)
    }

    fn unclassified_preview(&self, unit: transaction::CompletedUnit) -> transaction::Preview {
        transaction::Preview {
            unit,
            frame_control: None,
            class: transaction::IngressClass::Critical,
            route: transaction::IngressRoute::Standalone,
        }
    }

    fn try_send(&self, frame: TransactionFrame<'pool>) -> Result<(), TransactionFrame<'pool>> {
        assert_eq!(frame.segment().buffer.as_ptr(), self.pointer);
        assert_eq!(frame.segment().buffer, TRANSACTION_PAYLOAD);
        self.calls.set(self.calls.get() + 1);
        self.seen_slot.set(Some(frame.slot()));
        if self.accept {
            assert!(self.retained.borrow_mut().replace(frame).is_none());
            Ok(())
        } else {
            Err(frame)
        }
    }
}

fn run_transaction<'pool>(
    ring: &mut RxRingLive<'static, TRANSACTION_COUNT>,
    storage: &'static TransactionStorage,
    pool: &'pool RxStagePool<1, TRANSACTION_CAPACITY>,
    publisher: &RecordingPublisher<'pool>,
    totals: &mut [u64; 3],
    hardware: &mut MockRxDma,
) -> Result<DatapathRxProgress, RxStageTransactionError> {
    let [descriptors, units, bytes] = totals;
    transaction::service(
        ring,
        storage,
        pool,
        publisher,
        &transaction::AdmitAll,
        transaction::Counters {
            descriptors,
            units,
            bytes,
        },
        hardware,
        (),
    )
}

#[test]
fn chip_transaction_releases_a_rejected_original_lease_for_later_ring_reclaim() {
    let (storage, mut ring, mut hardware, pointer) = transaction_fixture();
    let pool = RxStagePool::<1, TRANSACTION_CAPACITY>::new();
    let publisher = RecordingPublisher {
        accept: false,
        pointer,
        calls: Cell::new(0),
        seen_slot: Cell::new(None),
        retained: RefCell::new(None),
    };
    let mut totals = [0; 3];

    assert_eq!(
        run_transaction(
            &mut ring,
            storage,
            &pool,
            &publisher,
            &mut totals,
            &mut hardware
        ),
        Err(RxStageTransactionError::Ring(RxRingError::Corrupt)),
    );
    assert_eq!(publisher.calls.get(), 1);
    assert!(publisher.seen_slot.get().is_some());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(storage.released_buffer_count(), 1);
    assert_eq!(storage.detached_buffer_count(), 0);
    assert!(hardware.walker);
    assert_eq!(
        totals, [0; 3],
        "publication failure precedes the counter commit"
    );

    run_transaction(
        &mut ring,
        storage,
        &pool,
        &publisher,
        &mut totals,
        &mut hardware,
    )
    .unwrap();
    assert_eq!(
        publisher.calls.get(),
        1,
        "reclaim must not republish a rejected frame"
    );
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(pool.claimed_slots(), 0);
    ring.try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("rejected lease returned to the original ring"));
}

#[test]
fn chip_transaction_cannot_reclaim_the_buffer_retained_by_its_publisher() {
    let (storage, mut ring, mut hardware, pointer) = transaction_fixture();
    let pool = RxStagePool::<1, TRANSACTION_CAPACITY>::new();
    let publisher = RecordingPublisher {
        accept: true,
        pointer,
        calls: Cell::new(0),
        seen_slot: Cell::new(None),
        retained: RefCell::new(None),
    };
    let mut totals = [0; 3];

    run_transaction(
        &mut ring,
        storage,
        &pool,
        &publisher,
        &mut totals,
        &mut hardware,
    )
    .unwrap();
    assert_eq!(totals, [1, 1, 16]);
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(storage.detached_buffer_count(), 1);
    run_transaction(
        &mut ring,
        storage,
        &pool,
        &publisher,
        &mut totals,
        &mut hardware,
    )
    .unwrap();
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(storage.detached_buffer_count(), 1);
    assert_eq!(publisher.calls.get(), 1);
    assert_eq!(
        publisher
            .retained
            .borrow()
            .as_ref()
            .unwrap()
            .segment()
            .buffer,
        TRANSACTION_PAYLOAD
    );

    drop(publisher.retained.borrow_mut().take());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(storage.released_buffer_count(), 1);
    run_transaction(
        &mut ring,
        storage,
        &pool,
        &publisher,
        &mut totals,
        &mut hardware,
    )
    .unwrap();
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(storage.detached_buffer_count(), 0);
    assert_eq!(totals, [1, 1, 16]);
    ring.try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("accepted lease returned before shutdown"));
}
