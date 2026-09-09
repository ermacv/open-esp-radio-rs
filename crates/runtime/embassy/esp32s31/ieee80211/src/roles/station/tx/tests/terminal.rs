use super::*;

#[test]
fn exhausted_aggregate_emits_one_terminal_receipt() {
    let (mut device, network) = make_network();
    for marker in 1..=3 {
        send_frame(&mut device, marker);
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(network.tx_consumer().promotion_capacity(), 0);
    assert!(device.transmit(&mut context()).is_some());

    hardware.aggregate_completion = Some(aggregate_completion(8, 0b00));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    let work = tx.aggregate_work();
    let status = tx.take_last_aggregate_status().unwrap();
    assert_eq!(*observer.terminal.lock().unwrap(), [status]);
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert_eq!(work.publications, 2);
    assert_eq!(work.mpdus, 5);
    assert!(work.psdu_bytes > 0);
    assert!(work.nominal_data_micros > 0);
    assert_eq!(work.unestimated_publications, 0);
    assert_eq!(
        Some(status),
        Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::Incomplete,
            original_subframes: 3,
            aggregate_attempts: 2,
            aggregate_rate: TxPhyRate::Ht(TEST_RATE),
            block_acknowledged_subframes: 1,
            ordinary_retry: None,
        })
    );
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    send_frame(&mut device, 6);
    assert!(device.transmit(&mut context()).is_none());
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx_direct().unwrap());
    }
}
