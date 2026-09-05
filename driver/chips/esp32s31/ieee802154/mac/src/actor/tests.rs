use super::*;
use open_esp_radio_esp32s31_ieee802154_dma::{
    DMA_LOW, DmaFrameAddress, PinnedRxPool, PinnedTxBuffer, PreparedTx, RxPoolStorage, TxStorage,
};
use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154EventMask;

fn tx_owner(address: u32) -> PinnedTxBuffer {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(TxStorage::new()));
    TxStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
}

fn arm_no_ack<'owner>(
    owner: &'owner mut PinnedTxBuffer,
    frame: &[u8],
) -> TxArmed<'owner, TxAckNotRequested> {
    let PreparedTx::AckNotRequested(prepared) = owner.prepare(frame).unwrap() else {
        panic!("fixture must not request an ACK");
    };
    prepared.arm()
}

fn arm_with_ack<'owner>(
    owner: &'owner mut PinnedTxBuffer,
    frame: &[u8],
) -> TxArmed<'owner, TxAckRequested> {
    let PreparedTx::AckRequested(prepared) = owner.prepare(frame).unwrap() else {
        panic!("fixture must request an ACK");
    };
    prepared.arm()
}

fn rx_pool<const COUNT: usize>(address: u32) -> PinnedRxPool<COUNT> {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::new()));
    RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap()).unwrap()
}

fn batch(events: Ieee802154EventMask) -> MacEventBatch {
    MacEventBatch::new(events, None, None, None).unwrap()
}

fn rx_abort(reason: Ieee802154RxAbortReason) -> MacEventBatch {
    MacEventBatch::new(Ieee802154Event::RxAbort.mask(), Some(reason), None, None).unwrap()
}

fn tx_abort(reason: Ieee802154TxAbortReason) -> MacEventBatch {
    MacEventBatch::new(Ieee802154Event::TxAbort.mask(), None, Some(reason), None).unwrap()
}

fn cca_done(sample: MacCcaSample) -> MacEventBatch {
    MacEventBatch::new(
        Ieee802154Event::EdDone.mask(),
        None,
        None,
        Some(MacMeasurementSample::ClearChannel(sample)),
    )
    .unwrap()
}

fn ed_done(sample: i8) -> MacEventBatch {
    MacEventBatch::new(
        Ieee802154Event::EdDone.mask(),
        None,
        None,
        Some(MacMeasurementSample::Energy(
            MacEnergySample::from_raw_code(sample),
        )),
    )
    .unwrap()
}

fn deferred<R>(outcome: MacBatchOutcome<R>) -> MacDeferred<R> {
    match outcome {
        MacBatchOutcome::Deferred(deferred) => deferred,
        MacBatchOutcome::Pending(_) => panic!("expected terminal batch"),
    }
}

fn pending<R>(outcome: MacBatchOutcome<R>) -> MacActive<R> {
    match outcome {
        MacBatchOutcome::Pending(active) => active,
        MacBatchOutcome::Deferred(_) => panic!("expected progress batch"),
    }
}

#[test]
fn partial_start_plans_have_deterministic_order_and_bound_addresses() {
    let pool = rx_pool::<1>(DMA_LOW);
    let rx = pool.arm_next().unwrap();
    let active = MacReady::new().request_receive_without_auto_ack(rx);
    let plan = active.start_plan().unwrap();
    assert_eq!(plan.step_count(), 4);
    assert!(matches!(
        plan.step(0),
        Some(MacIntentStep::RequireStateSpecificQuiescence)
    ));
    assert!(matches!(
        plan.step(1),
        Some(MacIntentStep::RefreshStaticPolicy)
    ));
    let Some(MacIntentStep::PublishReceiveAddress(address)) = plan.step(2) else {
        panic!();
    };
    assert_eq!(address.as_u32(), DMA_LOW);
    assert!(matches!(
        plan.step(3),
        Some(MacIntentStep::RequestCommand(MacCommandIntent::Receive))
    ));
    assert!(plan.step(4).is_none());

    let mut tx_owner = tx_owner(DMA_LOW + 128);
    let tx = arm_no_ack(&mut tx_owner, &[1]);
    let tx_active =
        MacReady::new().request_transmit_without_ack(tx, MacTransmitAccess::ClearChannelAssessment);
    let tx_plan = tx_active.start_plan().unwrap();
    assert_eq!(tx_plan.step_count(), 5);
    assert!(matches!(
        tx_plan.step(3),
        Some(MacIntentStep::ConfigureEnergyDetectionDuration(8))
    ));
    assert!(matches!(
        tx_plan.step(4),
        Some(MacIntentStep::RequestCommand(
            MacCommandIntent::TransmitWithClearChannelAssessment
        ))
    ));
}

#[test]
fn public_ed_duration_narrowing_fails_closed() {
    for units in [0_u32, 1, u16::MAX as u32] {
        let duration = MacEnergyDetectionDuration::try_from_public_units(units).unwrap();
        assert_eq!(u32::from(duration.hardware_units()), units);
    }
    for units in [u16::MAX as u32 + 1, u32::MAX] {
        assert_eq!(
            MacEnergyDetectionDuration::try_from_public_units(units),
            Err(MacEnergyDetectionDurationError::OutOfHardwareSubset { units })
        );
    }
}

#[test]
fn transmit_with_ack_plan_publishes_tx_before_rx() {
    let mut tx_owner = tx_owner(DMA_LOW);
    let tx = arm_with_ack(&mut tx_owner, &[0x21]);
    let pool = rx_pool::<1>(DMA_LOW + 128);
    let rx = pool.arm_next().unwrap();
    let active = MacReady::new().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
    let plan = active.start_plan().unwrap();
    assert_eq!(plan.step_count(), 5);
    let Some(MacIntentStep::PublishTransmitAddress(tx_address)) = plan.step(2) else {
        panic!();
    };
    let Some(MacIntentStep::PublishReceiveAddress(rx_address)) = plan.step(3) else {
        panic!();
    };
    assert_eq!(tx_address.as_u32(), DMA_LOW);
    assert_eq!(rx_address.as_u32(), DMA_LOW + 128);
}

#[test]
fn receive_progress_terminal_rejection_and_reclaim_are_linear() {
    let pool = rx_pool::<1>(DMA_LOW);
    let rx = pool.arm_next().unwrap();
    let active = MacReady::new().request_receive_without_auto_ack(rx);
    let active = pending(
        active
            .process_batch(batch(Ieee802154Event::RxSfdDone.mask()))
            .unwrap(),
    );
    assert_eq!(active.phase(), MacActivePhase::Receive);

    let rejected = active
        .process_batch(batch(Ieee802154Event::TxDone.mask()))
        .expect_err("TX_DONE cannot complete RX");
    assert_eq!(
        rejected.reason(),
        MacBatchRejectReason::UnexpectedEvents(Ieee802154Event::TxDone.mask())
    );
    let active = rejected.into_active();
    let completion = deferred(
        active
            .process_batch(batch(Ieee802154Event::RxDone.mask()))
            .unwrap(),
    );
    assert_eq!(completion.completion(), MacCompletion::ReceiveFrame);
    let terminal = DmaTerminalEvidence::for_native_model();
    let resolved = completion
        .resolve_with_terminal_evidence(MacDeferredNext::ReceiveWhenIdle, &terminal)
        .unwrap();
    let (_ready, reclaimed, result, next) = resolved.into_parts();
    assert_eq!(reclaimed.outcome(), MacResolvedRxOutcome::Received);
    assert!(matches!(
        reclaimed.frame(),
        Some(Err(RxFrameError::PhrLengthOutOfRange { length: 0 }))
    ));
    reclaimed.recycle().unwrap();
    assert_eq!(result, MacCompletion::ReceiveFrame);
    assert_eq!(next, MacDeferredNext::ReceiveWhenIdle);
}

#[test]
fn no_ack_transmit_has_one_terminal_edge() {
    for access in [
        MacTransmitAccess::Direct,
        MacTransmitAccess::ClearChannelAssessment,
    ] {
        let mut owner = tx_owner(DMA_LOW);
        let armed = arm_no_ack(&mut owner, &[1]);
        let active = MacReady::new().request_transmit_without_ack(armed, access);
        let completion = deferred(
            active
                .process_batch(batch(
                    Ieee802154Event::TxSfdDone
                        .mask()
                        .union(Ieee802154Event::TxDone.mask()),
                ))
                .unwrap(),
        );
        assert_eq!(completion.completion(), MacCompletion::TransmitComplete);
        let terminal = DmaTerminalEvidence::for_native_model();
        let resolved =
            completion.resolve_with_terminal_evidence(MacDeferredNext::IdlePolicy, &terminal);
        let (_ready, completed, _, _) = resolved.into_parts();
        completed.release();
    }
}

#[test]
fn ack_transmit_advances_then_defers_once_for_terminal_batch() {
    let mut owner = tx_owner(DMA_LOW);
    let tx = arm_with_ack(&mut owner, &[0x21]);
    let pool = rx_pool::<1>(DMA_LOW + 128);
    let rx = pool.arm_next().unwrap();
    let active = MacReady::new().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
    let active = pending(
        active
            .process_batch(batch(Ieee802154Event::TxDone.mask()))
            .unwrap(),
    );
    assert_eq!(
        active.phase(),
        MacActivePhase::AwaitingAcknowledgement {
            access: MacTransmitAccess::Direct
        }
    );
    assert!(
        active.start_plan().is_none(),
        "RX_ACK must not expose a second transmit start intent"
    );
    let completion = deferred(
        active
            .process_batch(batch(
                Ieee802154Event::RxSfdDone
                    .mask()
                    .union(Ieee802154Event::AckRxDone.mask()),
            ))
            .unwrap(),
    );
    assert_eq!(completion.completion(), MacCompletion::TransmitAcknowledged);
    let terminal = DmaTerminalEvidence::for_native_model();
    let resolved = completion
        .resolve_with_terminal_evidence(MacDeferredNext::IdlePolicy, &terminal)
        .unwrap();
    let (_ready, reclaimed, _, _) = resolved.into_parts();
    let (tx, rx) = reclaimed.into_parts();
    tx.release();
    assert_eq!(rx.outcome(), MacResolvedAcknowledgementOutcome::Received);
    rx.recycle().unwrap();
}

#[test]
fn tx_done_and_ack_done_can_share_one_reviewed_order_batch() {
    let mut owner = tx_owner(DMA_LOW);
    let tx = arm_with_ack(&mut owner, &[0x21]);
    let pool = rx_pool::<1>(DMA_LOW + 128);
    let rx = pool.arm_next().unwrap();
    let active = MacReady::new().request_transmit_with_ack(
        tx,
        rx,
        MacTransmitAccess::ClearChannelAssessment,
    );
    let events = Ieee802154Event::TxDone
        .mask()
        .union(Ieee802154Event::AckRxDone.mask());
    let completion = deferred(active.process_batch(batch(events)).unwrap());
    assert_eq!(completion.completion(), MacCompletion::TransmitAcknowledged);
}

#[test]
fn delayed_ack_is_legal_in_tx_but_multiple_terminals_fail_closed() {
    let mut owner = tx_owner(DMA_LOW);
    let tx = arm_with_ack(&mut owner, &[0x21]);
    let pool = rx_pool::<1>(DMA_LOW + 128);
    let rx = pool.arm_next().unwrap();
    let active = MacReady::new().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
    let completion = deferred(
        active
            .process_batch(batch(Ieee802154Event::AckRxDone.mask()))
            .unwrap(),
    );
    assert_eq!(completion.completion(), MacCompletion::TransmitAcknowledged);

    let mut owner = tx_owner(DMA_LOW + 256);
    let tx = arm_with_ack(&mut owner, &[0x21]);
    let pool = rx_pool::<1>(DMA_LOW + 384);
    let active = MacReady::new().request_transmit_with_ack(
        tx,
        pool.arm_next().unwrap(),
        MacTransmitAccess::Direct,
    );
    let conflicting = MacEventBatch::new(
        Ieee802154Event::AckRxDone
            .mask()
            .union(Ieee802154Event::TxAbort.mask()),
        None,
        Some(Ieee802154TxAbortReason::TxSecurityError),
        None,
    )
    .unwrap();
    let rejected = active
        .process_batch(conflicting)
        .expect_err("success plus abort is ambiguous");
    assert!(matches!(
        rejected.reason(),
        MacBatchRejectReason::ConflictingTerminalEvents(_)
    ));
}

#[test]
fn ack_timeout_sources_are_terminal_only_after_tx_done() {
    for by_timer in [false, true] {
        let mut owner = tx_owner(DMA_LOW);
        let tx = arm_with_ack(&mut owner, &[0x21]);
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let rx = pool.arm_next().unwrap();
        let active = MacReady::new().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
        let active = pending(
            active
                .process_batch(batch(Ieee802154Event::TxDone.mask()))
                .unwrap(),
        );
        let terminal = if by_timer {
            batch(Ieee802154Event::Timer0Overflow.mask())
        } else {
            tx_abort(Ieee802154TxAbortReason::RxAckTimeout)
        };
        let completion = deferred(active.process_batch(terminal).unwrap());
        assert_eq!(
            completion.completion(),
            if by_timer {
                MacCompletion::AcknowledgementTimedOutByTimer
            } else {
                MacCompletion::AcknowledgementFailed(Ieee802154TxAbortReason::RxAckTimeout)
            }
        );
    }
}

#[test]
fn every_public_ack_failure_is_terminal_and_never_exposes_a_frame() {
    const REASONS: [Ieee802154TxAbortReason; 9] = [
        Ieee802154TxAbortReason::RxAckSfdTimeout,
        Ieee802154TxAbortReason::RxAckCrcError,
        Ieee802154TxAbortReason::RxAckInvalidLength,
        Ieee802154TxAbortReason::RxAckFilterFail,
        Ieee802154TxAbortReason::RxAckNoRss,
        Ieee802154TxAbortReason::RxAckCoexistenceBreak,
        Ieee802154TxAbortReason::RxAckTypeNotAck,
        Ieee802154TxAbortReason::RxAckRestart,
        Ieee802154TxAbortReason::RxAckTimeout,
    ];

    for reason in REASONS {
        let mut owner = tx_owner(DMA_LOW);
        let tx = arm_with_ack(&mut owner, &[0x21]);
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let active = MacReady::new().request_transmit_with_ack(
            tx,
            pool.arm_next().unwrap(),
            MacTransmitAccess::Direct,
        );
        let terminal = MacEventBatch::new(
            Ieee802154Event::TxDone
                .mask()
                .union(Ieee802154Event::TxAbort.mask()),
            None,
            Some(reason),
            None,
        )
        .unwrap();
        let deferred = deferred(active.process_batch(terminal).unwrap());
        assert_eq!(
            deferred.completion(),
            MacCompletion::AcknowledgementFailed(reason)
        );

        let evidence = DmaTerminalEvidence::for_native_model();
        let resolved = deferred
            .resolve_with_terminal_evidence(MacDeferredNext::IdlePolicy, &evidence)
            .unwrap();
        let (_ready, reclaimed, _, _) = resolved.into_parts();
        let (transmit, acknowledgement) = reclaimed.into_parts();
        assert_eq!(
            acknowledgement.outcome(),
            MacResolvedAcknowledgementOutcome::NotReceived
        );
        assert!(acknowledgement.frame().is_none());
        acknowledgement.recycle().unwrap();
        transmit.release();
    }
}

#[test]
fn transmit_abort_reasons_are_access_specific() {
    for reason in [
        Ieee802154TxAbortReason::TxCoexistenceBreak,
        Ieee802154TxAbortReason::TxSecurityError,
    ] {
        let mut owner = tx_owner(DMA_LOW);
        let armed = arm_no_ack(&mut owner, &[1]);
        let completion = deferred(
            MacReady::new()
                .request_transmit_without_ack(armed, MacTransmitAccess::Direct)
                .process_batch(tx_abort(reason))
                .unwrap(),
        );
        assert_eq!(
            completion.completion(),
            MacCompletion::TransmitAborted(reason)
        );
    }

    let mut owner = tx_owner(DMA_LOW);
    let armed = arm_no_ack(&mut owner, &[1]);
    let active = MacReady::new().request_transmit_without_ack(armed, MacTransmitAccess::Direct);
    let rejected = active
        .process_batch(tx_abort(Ieee802154TxAbortReason::CcaBusy))
        .expect_err("CCA_BUSY requires the combined TX_CCA state");
    assert_eq!(
        rejected.reason(),
        MacBatchRejectReason::UnexpectedTxAbortReason(Ieee802154TxAbortReason::CcaBusy)
    );
    let active = rejected.into_active();
    assert_eq!(
        active.phase(),
        MacActivePhase::Transmit {
            access: MacTransmitAccess::Direct,
            acknowledgement: MacTransmitAcknowledgement::None
        }
    );
}

#[test]
fn standalone_cca_and_ed_require_their_own_sample_kind() {
    for sample in [MacCcaSample::Clear, MacCcaSample::Busy] {
        let completion = deferred(
            MacReady::new()
                .request_clear_channel_assessment()
                .process_batch(cca_done(sample))
                .unwrap(),
        );
        assert_eq!(
            completion.completion(),
            MacCompletion::ClearChannelAssessment(sample)
        );
        let resolved = completion.resolve(MacDeferredNext::IdlePolicy);
        let (_ready, no_dma, result, next) = resolved.into_parts();
        assert_eq!(no_dma, MacNoDmaResources { _private: () });
        assert_eq!(result, MacCompletion::ClearChannelAssessment(sample));
        assert_eq!(next, MacDeferredNext::IdlePolicy);
    }

    for sample in [i8::MIN, -1, 0, i8::MAX] {
        let completion = deferred(
            MacReady::new()
                .request_energy_detection(MacEnergyDetectionDuration::from_hardware_units(99))
                .process_batch(ed_done(sample))
                .unwrap(),
        );
        assert_eq!(
            completion.completion(),
            MacCompletion::EnergyDetection(MacEnergySample::from_raw_code(sample))
        );
    }

    let rejected = MacReady::new()
        .request_clear_channel_assessment()
        .process_batch(ed_done(-42))
        .expect_err("energy sample cannot complete CCA");
    assert!(matches!(
        rejected.reason(),
        MacBatchRejectReason::UnexpectedMeasurement(MacMeasurementSample::Energy(_))
    ));
}

#[test]
fn only_source_terminal_measurement_aborts_are_accepted() {
    for reason in [
        Ieee802154RxAbortReason::EdAbort,
        Ieee802154RxAbortReason::EdCoexistenceReject,
    ] {
        let cca = deferred(
            MacReady::new()
                .request_clear_channel_assessment()
                .process_batch(rx_abort(reason))
                .unwrap(),
        );
        assert_eq!(
            cca.completion(),
            MacCompletion::ClearChannelAssessmentAborted(reason)
        );
        let ed = deferred(
            MacReady::new()
                .request_energy_detection(MacEnergyDetectionDuration::from_hardware_units(1))
                .process_batch(rx_abort(reason))
                .unwrap(),
        );
        assert_eq!(
            ed.completion(),
            MacCompletion::EnergyDetectionAborted(reason)
        );
    }
    let rejected = MacReady::new()
        .request_clear_channel_assessment()
        .process_batch(rx_abort(Ieee802154RxAbortReason::EdStop))
        .expect_err("vendor EdStop does not request deferred next operation");
    assert_eq!(
        rejected.reason(),
        MacBatchRejectReason::UnexpectedRxAbortReason(Ieee802154RxAbortReason::EdStop)
    );
}

#[test]
fn receive_abort_reason_domain_is_exhaustive() {
    const REASONS: [Ieee802154RxAbortReason; 16] = [
        Ieee802154RxAbortReason::RxStop,
        Ieee802154RxAbortReason::SfdTimeout,
        Ieee802154RxAbortReason::CrcError,
        Ieee802154RxAbortReason::InvalidLength,
        Ieee802154RxAbortReason::FilterFail,
        Ieee802154RxAbortReason::NoRss,
        Ieee802154RxAbortReason::CoexistenceBreak,
        Ieee802154RxAbortReason::UnexpectedAck,
        Ieee802154RxAbortReason::RxRestart,
        Ieee802154RxAbortReason::TxAckTimeout,
        Ieee802154RxAbortReason::TxAckStop,
        Ieee802154RxAbortReason::TxAckCoexistenceBreak,
        Ieee802154RxAbortReason::EnhancedAckSecurityError,
        Ieee802154RxAbortReason::EdAbort,
        Ieee802154RxAbortReason::EdStop,
        Ieee802154RxAbortReason::EdCoexistenceReject,
    ];
    for reason in REASONS {
        let pool = rx_pool::<1>(DMA_LOW);
        let active = MacReady::new().request_receive_without_auto_ack(pool.arm_next().unwrap());
        let result = active.process_batch(rx_abort(reason));
        assert_eq!(result.is_ok(), is_terminal_receive_abort(reason));
    }
}
