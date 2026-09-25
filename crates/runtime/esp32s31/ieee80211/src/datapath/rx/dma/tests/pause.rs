use super::*;

#[allow(
    clippy::result_large_err,
    reason = "the regression retains the entire no-alloc DMA owner on the error branch"
)]
#[test]
fn connected_service_can_change_dma_state_without_replacing_other_role_owners() {
    use crate::datapath::services::SingleRoleServices;
    use std::rc::Rc;

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<2>::new()));
    let mut hardware = MockRxDma::default();
    let ring = storage
        .prepare_ring(&mut hardware, BASE, &[0x2f00_2000, 0x2f00_3200])
        .unwrap()
        .try_start(&mut hardware)
        .unwrap_or_else(|_| panic!("start"));
    let pool = RxStagePool::new();
    let queue = StagedRxQueue::<NoopRawMutex, 2>::new();
    let (sender, _receiver) = queue.split();
    let dma = StagedRxProducer::new(ring, storage, &pool, NoDelay, sender);
    let protocol = Rc::new(17_u8);
    let tx = Rc::new(23_u8);
    let control = Rc::new(31_u8);
    let services = SingleRoleServices::with_control(
        hardware,
        ConnectedStaRxService::new(dma, protocol.clone()),
        tx.clone(),
        control.clone(),
    );
    let failed = services
        .try_map_rx(|_, rx| rx.try_map_dma(Err::<(), _>))
        .err()
        .expect("preserve protocol/TX/control on transition failure");
    let services = failed
        .try_map_rx(|_, rx| rx.try_map_dma(Ok::<_, ()>))
        .unwrap_or_else(|_| panic!("recover services"));
    let paused = services.map_rx(|hardware, rx| {
        rx.map_dma(|dma| dma.try_pause(hardware).unwrap_or_else(|_| panic!("pause")))
    });
    assert!(!paused.hardware().walker);
    assert!(Rc::ptr_eq(paused.rx().protocol(), &protocol));
    assert!(Rc::ptr_eq(paused.tx(), &tx));
    assert!(Rc::ptr_eq(paused.control(), &control));
    let services = paused.map_rx(|hardware, rx| {
        rx.map_dma(|dma| {
            dma.try_resume(hardware)
                .unwrap_or_else(|_| panic!("resume"))
        })
    });
    let (mut hardware, rx, returned_tx, returned_control) = services.into_parts();
    let (dma, returned_protocol) = rx.into_parts();
    assert!(Rc::ptr_eq(&returned_protocol, &protocol));
    assert!(Rc::ptr_eq(&returned_tx, &tx));
    assert!(Rc::ptr_eq(&returned_control, &control));
    assert!(dma.try_stop(&mut hardware).is_ok());
}

#[test]
fn producer_pause_preserves_queue_admission_and_progress_with_outstanding_leases() {
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<2>::new()));
    let mut hardware = MockRxDma::default();
    let ring = storage
        .prepare_ring(&mut hardware, BASE, &[0x2f00_2000, 0x2f00_3200])
        .unwrap()
        .try_start(&mut hardware)
        .unwrap_or_else(|_| panic!("start"));
    let pool = RxStagePool::new();
    let queue = StagedRxQueue::<NoopRawMutex, 2>::new();
    let (sender, receiver) = queue.split();
    let admission = CountingUnavailableAdmission::default();
    let mut producer = StagedRxProducer::new(ring, storage, &pool, NoDelay, sender)
        .with_stage_admission_policy(&admission);
    for descriptor in storage.descriptors() {
        descriptor
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    }
    hardware.release_through(1, None);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    let counters = producer.work_counters();
    let descriptors = producer.serviced_descriptors();
    assert_eq!(counters.completed_units, 2);
    let held = receiver.try_receive().expect("first consumer lease");
    assert_eq!(pool.claimed_slots(), 2);
    let paused = producer
        .try_pause(&mut hardware)
        .unwrap_or_else(|_| panic!("pause with leases"));
    assert!(!hardware.walker);
    let (paused, error) = paused
        .try_stop(&mut hardware)
        .err()
        .expect("no terminal stop with leases");
    assert_eq!(error, RxRingError::Busy);
    drop(held);
    let mut producer = paused
        .try_resume(&mut hardware)
        .unwrap_or_else(|_| panic!("resume"));
    assert!(hardware.walker);
    assert_eq!(producer.work_counters(), counters);
    assert_eq!(producer.serviced_descriptors(), descriptors);
    assert!(core::ptr::eq(producer.admission, &admission));
    let queued = receiver.try_receive().expect("second lease survives pause");
    assert_eq!(queued.length(), 8);
    drop(queued);
    assert_eq!(pool.claimed_slots(), 0);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    assert_eq!(
        producer.work_counters(),
        counters,
        "resume cannot redeliver old completions"
    );
    assert!(producer.try_stop(&mut hardware).is_ok());
}

#[test]
fn failed_resume_retains_queued_storage_until_terminal_stop_is_possible() {
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<2>::new()));
    let mut hardware = MockRxDma::default();
    let ring = storage
        .prepare_ring(&mut hardware, BASE, &[0x2f00_2000, 0x2f00_3200])
        .unwrap()
        .try_start(&mut hardware)
        .unwrap_or_else(|_| panic!("start"));
    let pool = RxStagePool::new();
    let queue = StagedRxQueue::<NoopRawMutex, 2>::new();
    let (sender, receiver) = queue.split();
    let mut producer = StagedRxProducer::new(ring, storage, &pool, NoDelay, sender);
    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    hardware.release_through(1, None);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    let paused = producer
        .try_pause(&mut hardware)
        .unwrap_or_else(|_| panic!("pause"));
    hardware.fail_enable = true;
    let failure = paused
        .try_resume(&mut hardware)
        .err()
        .expect("failed enable");
    assert_eq!(
        failure.resume_error(),
        oer_esp32s31_wifi_dma::rx_ring::RxResumeError::EnableUnconfirmed
    );
    let (failure, error) = failure
        .try_stop(&mut hardware)
        .err()
        .expect("retain queue on fault");
    assert_eq!(error, RxRingError::Busy);
    assert_eq!(pool.claimed_slots(), 1);
    drop(receiver.try_receive().expect("retained lease"));
    assert!(failure.try_stop(&mut hardware).is_ok());
    assert_eq!(
        storage.lifecycle_state(),
        oer_esp32s31_wifi_dma::rx_ring::RxDmaArenaState::ResetRequired
    );
}
