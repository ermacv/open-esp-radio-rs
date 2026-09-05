use super::*;
use open_esp_radio_esp32s31_ieee802154_dma::{
    DMA_LOW, DmaFrameAddress, DmaTerminalEvidence, PinnedRxPool, PinnedTxBuffer, PreparedTx, RxArm,
    RxCompletionKind, RxPoolStorage, RxSlotState, TxAckRequested, TxState, TxStorage,
};
use open_esp_radio_esp32s31_ieee802154_irq::{
    Ieee802154AcknowledgedInterrupt, Ieee802154RxAbortReason, acknowledged_interrupt_for_validation,
};
use open_esp_radio_esp32s31_ieee802154_mac::{MacMeasurementSample, MacTransmitAccess};
use std::{boxed::Box, vec::Vec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FakeError {
    Injected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LogEntry {
    Quiesce,
    RefreshPolicy,
    PublishTx(u32),
    PublishRx(u32),
    ConfigureDuration(u16),
    Command(MacCommandIntent),
    ArmAcknowledgementWatchdog,
    DisarmAcknowledgementWatchdog,
    FinishTerminal,
}

struct FakeExecutor {
    log: Vec<LogEntry>,
    fail_step: Option<usize>,
    policy_error: Option<MacRuntimePolicyError>,
    calls: usize,
}

impl FakeExecutor {
    fn new() -> Self {
        Self {
            log: Vec::new(),
            fail_step: None,
            policy_error: None,
            calls: 0,
        }
    }

    fn record(&mut self, entry: LogEntry) -> Result<(), FakeError> {
        let index = self.calls;
        self.calls += 1;
        if self.fail_step == Some(index) {
            return Err(FakeError::Injected);
        }
        self.log.push(entry);
        Ok(())
    }
}

impl sealed::CommandExecutor for FakeExecutor {}

impl MacCommandExecutor for FakeExecutor {
    type Error = FakeError;

    fn validate_operation_policy(
        &self,
        _phase: MacActivePhase,
    ) -> Result<(), MacRuntimePolicyError> {
        self.policy_error.map_or(Ok(()), Err)
    }

    fn require_state_specific_quiescence(&mut self) -> Result<(), Self::Error> {
        self.record(LogEntry::Quiesce)
    }

    fn refresh_static_policy(&mut self) -> Result<(), Self::Error> {
        self.record(LogEntry::RefreshPolicy)
    }

    fn publish_transmit_address(&mut self, address: TxDmaAddress<'_>) -> Result<(), Self::Error> {
        self.record(LogEntry::PublishTx(address.as_u32()))
    }

    fn publish_receive_address(&mut self, address: RxDmaAddress<'_>) -> Result<(), Self::Error> {
        self.record(LogEntry::PublishRx(address.as_u32()))
    }

    fn configure_energy_detection_duration(&mut self, units: u16) -> Result<(), Self::Error> {
        self.record(LogEntry::ConfigureDuration(units))
    }

    fn request_command(&mut self, command: MacCommandIntent) -> Result<(), Self::Error> {
        self.record(LogEntry::Command(command))
    }

    fn arm_acknowledgement_watchdog(&mut self) {
        self.log.push(LogEntry::ArmAcknowledgementWatchdog);
    }

    fn disarm_acknowledgement_watchdog(&mut self) {
        self.log.push(LogEntry::DisarmAcknowledgementWatchdog);
    }

    fn finish_terminal_operation(&mut self) {
        self.calls += 1;
        self.log.push(LogEntry::FinishTerminal);
    }
}

fn runtime(executor: FakeExecutor) -> MacRuntime<FakeExecutor> {
    MacRuntime::from_commands(MacCommandCapability::from_model_executor(executor))
}

fn tx_owner(address: u32) -> PinnedTxBuffer {
    let storage = Box::leak(Box::new(TxStorage::new()));
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
    let storage = Box::leak(Box::new(RxPoolStorage::new()));
    RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap()).unwrap()
}

fn batch(events: Ieee802154EventMask) -> MacEventBatch {
    MacEventBatch::new(events, None, None, None).unwrap()
}

fn acknowledged(batch: MacEventBatch) -> AcknowledgedMacEventBatch {
    AcknowledgedMacEventBatch { batch }
}

fn interrupt(
    event_classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
    rx_abort: Option<Ieee802154RxAbortReasonObservation>,
    tx_abort: Option<Ieee802154TxAbortReasonObservation>,
    ed_rss: i8,
    cca_busy: bool,
) -> Ieee802154AcknowledgedInterrupt {
    acknowledged_interrupt_for_validation(
        event_classification,
        rx_abort,
        tx_abort,
        ed_rss,
        cca_busy,
    )
}

#[test]
fn acknowledged_irq_decodes_measurement_for_the_active_phase() {
    let energy = AcknowledgedMacEventBatch::from_interrupt(
            interrupt(
                Ok(Ieee802154Event::EdDone.mask()),
                None,
                None,
                -42,
                true,
            ),
            MacActivePhase::EnergyDetection {
                duration: open_esp_radio_esp32s31_ieee802154_mac::MacEnergyDetectionDuration::from_hardware_units(8),
            },
        )
        .unwrap();
    assert_eq!(energy.events(), Ieee802154Event::EdDone.mask());
    assert_eq!(
        energy.batch.measurement(),
        Some(MacMeasurementSample::Energy(
            MacEnergySample::from_raw_code(-42)
        ))
    );

    let cca = AcknowledgedMacEventBatch::from_interrupt(
        interrupt(Ok(Ieee802154Event::EdDone.mask()), None, None, -1, true),
        MacActivePhase::ClearChannelAssessment,
    )
    .unwrap();
    assert_eq!(
        cca.batch.measurement(),
        Some(MacMeasurementSample::ClearChannel(MacCcaSample::Busy))
    );
}

#[test]
fn acknowledged_irq_rejects_unclassified_events_and_abort_reasons() {
    assert!(matches!(
        AcknowledgedMacEventBatch::from_interrupt(
            interrupt(Err(Ieee802154EventObservationError), None, None, 0, false),
            MacActivePhase::ClearChannelAssessment,
        ),
        Err(MacInterruptBatchError::UnclassifiedEvents(
            Ieee802154EventObservationError
        ))
    ));
    assert!(matches!(
        AcknowledgedMacEventBatch::from_interrupt(
            interrupt(
                Ok(Ieee802154Event::RxAbort.mask()),
                Some(Ieee802154RxAbortReasonObservation::Unclassified),
                None,
                0,
                false,
            ),
            MacActivePhase::Receive,
        ),
        Err(MacInterruptBatchError::UnknownRxAbortReason)
    ));
}

#[test]
fn transmit_with_ack_executes_the_complete_plan_in_exact_order() {
    let mut tx = tx_owner(DMA_LOW);
    let armed_tx = arm_with_ack(&mut tx, &[0x21]);
    let rx = rx_pool::<1>(DMA_LOW + 128);
    let armed_rx = rx.arm_next().unwrap();
    let actor =
        MacReady::new().request_transmit_with_ack(armed_tx, armed_rx, MacTransmitAccess::Direct);

    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    assert_eq!(
        active.hardware.executor.log,
        [
            LogEntry::Quiesce,
            LogEntry::RefreshPolicy,
            LogEntry::PublishTx(DMA_LOW),
            LogEntry::PublishRx(DMA_LOW + 128),
            LogEntry::Command(MacCommandIntent::Transmit),
        ]
    );
}

#[test]
fn energy_detection_keeps_duration_before_the_final_command() {
    let actor = MacReady::new().request_energy_detection(
        open_esp_radio_esp32s31_ieee802154_mac::MacEnergyDetectionDuration::from_hardware_units(37),
    );

    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    assert_eq!(
        active.hardware.executor.log,
        [
            LogEntry::Quiesce,
            LogEntry::RefreshPolicy,
            LogEntry::ConfigureDuration(37),
            LogEntry::Command(MacCommandIntent::EnergyDetection),
        ]
    );
}

#[test]
fn partial_start_failure_quarantines_the_affine_actor() {
    let mut fake = FakeExecutor::new();
    fake.fail_step = Some(2);
    let actor = MacReady::new().request_clear_channel_assessment();

    let failure = runtime(fake).start(actor).unwrap_err();
    assert_eq!(failure.phase(), MacActivePhase::ClearChannelAssessment);
    assert!(matches!(
        failure.error(),
        MacRuntimeStartError::Executor {
            step_index: 2,
            error: FakeError::Injected,
        }
    ));
    assert_eq!(
        failure.hardware.executor.log,
        [LogEntry::Quiesce, LogEntry::RefreshPolicy]
    );
}

#[test]
fn incompatible_policy_is_quarantined_before_any_executor_step() {
    let mut fake = FakeExecutor::new();
    let policy_error = MacRuntimePolicyError::AcknowledgementReceptionDisabled;
    fake.policy_error = Some(policy_error);
    let actor = MacReady::new().request_clear_channel_assessment();

    let failure = runtime(fake).start(actor).unwrap_err();
    assert!(matches!(
        failure.error(),
        MacRuntimeStartError::IncompatiblePolicy(error) if *error == policy_error
    ));
    assert!(failure.hardware.executor.log.is_empty());
    assert_eq!(failure.hardware.executor.calls, 0);
}

#[test]
fn sampled_acknowledged_batches_advance_without_reexecuting_start() {
    let first = batch(Ieee802154Event::TxDone.mask());
    let second = batch(Ieee802154Event::AckRxDone.mask());
    let mut tx = tx_owner(DMA_LOW);
    let armed_tx = arm_with_ack(&mut tx, &[0x21]);
    let rx = rx_pool::<1>(DMA_LOW + 128);
    let armed_rx = rx.arm_next().unwrap();
    let actor =
        MacReady::new().request_transmit_with_ack(armed_tx, armed_rx, MacTransmitAccess::Direct);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let start_calls = active.hardware.executor.calls;

    let sampled = acknowledged(first);
    assert_eq!(sampled.events(), Ieee802154Event::TxDone.mask());
    let pending = match active.process_batch(sampled).unwrap() {
        MacRuntimeBatchOutcome::Pending(active) => active,
        MacRuntimeBatchOutcome::Completed(_) => panic!("TX_DONE must await ACK"),
    };
    assert_eq!(
        pending.phase(),
        MacActivePhase::AwaitingAcknowledgement {
            access: MacTransmitAccess::Direct,
        }
    );
    assert_eq!(pending.hardware.executor.calls, start_calls);
    assert_eq!(
        pending.hardware.executor.log.last(),
        Some(&LogEntry::ArmAcknowledgementWatchdog)
    );

    let sampled = acknowledged(second);
    let completed = match pending.process_batch(sampled).unwrap() {
        MacRuntimeBatchOutcome::Completed(completed) => completed,
        MacRuntimeBatchOutcome::Pending(_) => panic!("ACK_RX_DONE must be terminal"),
    };
    assert_eq!(completed.completion(), MacCompletion::TransmitAcknowledged);
    let resolved = completed
        .resolve(open_esp_radio_esp32s31_ieee802154_mac::MacDeferredNext::IdlePolicy)
        .unwrap();
    let (runtime, _ready, reclaimed, _, _) = resolved.into_parts();
    assert_eq!(
        runtime.hardware.executor.log,
        [
            LogEntry::Quiesce,
            LogEntry::RefreshPolicy,
            LogEntry::PublishTx(DMA_LOW),
            LogEntry::PublishRx(DMA_LOW + 128),
            LogEntry::Command(MacCommandIntent::Transmit),
            LogEntry::ArmAcknowledgementWatchdog,
            LogEntry::DisarmAcknowledgementWatchdog,
            LogEntry::FinishTerminal,
        ]
    );
    let (tx, rx) = reclaimed.into_parts();
    tx.release();
    assert_eq!(
        rx.outcome(),
        open_esp_radio_esp32s31_ieee802154_mac::MacResolvedAcknowledgementOutcome::Received
    );
    rx.recycle().unwrap();
}

#[test]
fn accepted_tx_done_returns_the_exact_transmit_buffer() {
    let mut tx = tx_owner(DMA_LOW);
    let armed = arm_no_ack(&mut tx, &[0x41, 0x88, 0x01]);
    let actor = MacReady::new().request_transmit_without_ack(armed, MacTransmitAccess::Direct);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let sampled = AcknowledgedMacEventBatch::from_interrupt(
        interrupt(Ok(Ieee802154Event::TxDone.mask()), None, None, 0, false),
        active.phase(),
    )
    .unwrap();
    let completed = match active.process_batch(sampled).unwrap() {
        MacRuntimeBatchOutcome::Completed(completed) => completed,
        MacRuntimeBatchOutcome::Pending(_) => panic!("TX_DONE must be terminal"),
    };

    let resolved = completed.resolve(MacDeferredNext::IdlePolicy);
    let (_runtime, _ready, completed, result, _) = resolved.into_parts();
    assert_eq!(result, MacCompletion::TransmitComplete);
    assert_eq!(completed.frame().mac_bytes(), &[0x41, 0x88, 0x01]);
    completed.release();
    assert_eq!(tx.state(), TxState::Free);
}

#[test]
fn accepted_rx_done_delivers_validated_frame_owner_and_allows_rearm() {
    let rx = rx_pool::<2>(DMA_LOW);
    let mut armed = rx.arm_next().unwrap();
    let RxArm::Buffer(slot) = &mut armed else {
        panic!("the first destination must be an ordinary slot");
    };
    let mut image = [0_u8; open_esp_radio_esp32s31_ieee802154_dma::FRAME_BUFFER_SIZE];
    image[0] = 5;
    image[1..4].copy_from_slice(&[0x61, 0x88, 0x01]);
    image[4] = (-37_i8) as u8;
    image[5] = 201;
    slot.write_model(&image);

    let actor = MacReady::new().request_receive_without_auto_ack(armed);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let sampled = AcknowledgedMacEventBatch::from_interrupt(
        interrupt(Ok(Ieee802154Event::RxDone.mask()), None, None, 0, false),
        active.phase(),
    )
    .unwrap();
    let completed = match active.process_batch(sampled).unwrap() {
        MacRuntimeBatchOutcome::Completed(completed) => completed,
        MacRuntimeBatchOutcome::Pending(_) => panic!("RX_DONE must be terminal"),
    };
    let resolved = completed.resolve(MacDeferredNext::ReceiveWhenIdle).unwrap();
    let (_runtime, _ready, received, result, _) = resolved.into_parts();

    assert_eq!(result, MacCompletion::ReceiveFrame);
    assert_eq!(
        received.outcome(),
        open_esp_radio_esp32s31_ieee802154_mac::MacResolvedRxOutcome::Received
    );
    assert_eq!(received.kind(), RxCompletionKind::Frame { index: 0 });
    let frame = received.frame().unwrap().unwrap();
    assert_eq!(frame.phr_length(), 5);
    assert_eq!(frame.mac_bytes(), &[0x61, 0x88, 0x01]);
    assert_eq!(frame.rssi(), -37);
    assert_eq!(frame.lqi(), 201);
    assert_eq!(rx.slot_state(0), Some(RxSlotState::Delivered));

    let second = rx.arm_next().unwrap();
    assert!(matches!(&second, RxArm::Buffer(slot) if slot.index() == 1));
    let model_terminal = DmaTerminalEvidence::for_native_model();
    second.complete(&model_terminal).unwrap().recycle().unwrap();
    received.recycle().unwrap();
    assert_eq!(rx.slot_state(0), Some(RxSlotState::Free));
    assert_eq!(rx.slot_state(1), Some(RxSlotState::Free));
}

#[test]
fn accepted_rx_abort_reclaims_without_exposing_partial_frame() {
    let rx = rx_pool::<1>(DMA_LOW);
    let armed = rx.arm_next().unwrap();
    let actor = MacReady::new().request_receive_without_auto_ack(armed);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let sampled = AcknowledgedMacEventBatch::from_interrupt(
        interrupt(
            Ok(Ieee802154Event::RxAbort.mask()),
            Some(Ieee802154RxAbortReason::SfdTimeout.into()),
            None,
            0,
            false,
        ),
        active.phase(),
    )
    .unwrap();
    let completed = match active.process_batch(sampled).unwrap() {
        MacRuntimeBatchOutcome::Completed(completed) => completed,
        MacRuntimeBatchOutcome::Pending(_) => panic!("SFD timeout must be terminal"),
    };
    let resolved = completed.resolve(MacDeferredNext::ReceiveWhenIdle).unwrap();
    let (_runtime, _ready, reclaimed, result, _) = resolved.into_parts();

    assert_eq!(
        result,
        MacCompletion::ReceiveAborted(
            open_esp_radio_esp32s31_ieee802154_irq::Ieee802154RxAbortReason::SfdTimeout
        )
    );
    assert_eq!(
        reclaimed.outcome(),
        open_esp_radio_esp32s31_ieee802154_mac::MacResolvedRxOutcome::Aborted(
            open_esp_radio_esp32s31_ieee802154_irq::Ieee802154RxAbortReason::SfdTimeout
        )
    );
    assert!(reclaimed.frame().is_none());
    reclaimed.recycle().unwrap();
    assert!(matches!(rx.arm_next(), Ok(RxArm::Buffer(_))));
}

#[test]
fn acknowledgement_timeout_returns_non_frame_rx_ownership() {
    let mut tx = tx_owner(DMA_LOW);
    let armed_tx = arm_with_ack(&mut tx, &[0x21]);
    let rx = rx_pool::<1>(DMA_LOW + 128);
    let armed_rx = rx.arm_next().unwrap();
    let actor =
        MacReady::new().request_transmit_with_ack(armed_tx, armed_rx, MacTransmitAccess::Direct);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let tx_done = AcknowledgedMacEventBatch::from_interrupt(
        interrupt(Ok(Ieee802154Event::TxDone.mask()), None, None, 0, false),
        active.phase(),
    )
    .unwrap();
    let pending = match active.process_batch(tx_done).unwrap() {
        MacRuntimeBatchOutcome::Pending(active) => active,
        MacRuntimeBatchOutcome::Completed(_) => panic!("TX_DONE must await ACK"),
    };
    let timeout = AcknowledgedMacEventBatch::from_interrupt(
        interrupt(
            Ok(Ieee802154Event::Timer0Overflow.mask()),
            None,
            None,
            0,
            false,
        ),
        pending.phase(),
    )
    .unwrap();
    let completed = match pending.process_batch(timeout).unwrap() {
        MacRuntimeBatchOutcome::Completed(completed) => completed,
        MacRuntimeBatchOutcome::Pending(_) => panic!("timer zero must terminate ACK wait"),
    };
    let resolved = completed.resolve(MacDeferredNext::IdlePolicy).unwrap();
    let (_runtime, _ready, reclaimed, result, _) = resolved.into_parts();
    let (completed_tx, acknowledgement) = reclaimed.into_parts();

    assert_eq!(result, MacCompletion::AcknowledgementTimedOutByTimer);
    assert_eq!(
        acknowledgement.outcome(),
        open_esp_radio_esp32s31_ieee802154_mac::MacResolvedAcknowledgementOutcome::NotReceived
    );
    assert!(acknowledgement.frame().is_none());
    acknowledgement.recycle().unwrap();
    completed_tx.release();
    assert_eq!(tx.state(), TxState::Free);
}

#[test]
fn rejected_acknowledged_batch_quarantines_the_runtime_owner() {
    let wrong = batch(Ieee802154Event::TxDone.mask());
    let actor = MacReady::new().request_clear_channel_assessment();
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();

    let sampled = acknowledged(wrong);
    let rejected = active.process_batch(sampled).unwrap_err();
    assert!(matches!(
        rejected.reason(),
        MacBatchRejectReason::UnexpectedEvents(_)
    ));
}

#[test]
fn rejected_acknowledgement_batch_disarms_the_watchdog_before_quarantine() {
    let mut tx = tx_owner(DMA_LOW);
    let armed_tx = arm_with_ack(&mut tx, &[0x21]);
    let rx = rx_pool::<1>(DMA_LOW + 128);
    let armed_rx = rx.arm_next().unwrap();
    let actor =
        MacReady::new().request_transmit_with_ack(armed_tx, armed_rx, MacTransmitAccess::Direct);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let pending = match active
        .process_batch(acknowledged(batch(Ieee802154Event::TxDone.mask())))
        .unwrap()
    {
        MacRuntimeBatchOutcome::Pending(active) => active,
        MacRuntimeBatchOutcome::Completed(_) => panic!("TX_DONE must await ACK"),
    };

    let rejected = pending
        .process_batch(acknowledged(batch(Ieee802154Event::RxDone.mask())))
        .unwrap_err();
    assert!(matches!(
        rejected.reason(),
        MacBatchRejectReason::UnexpectedEvents(_)
    ));
    assert_eq!(
        rejected._hardware.executor.log.last(),
        Some(&LogEntry::DisarmAcknowledgementWatchdog)
    );
}

#[test]
fn lost_acknowledgement_handoff_disarms_the_watchdog_before_containment() {
    let mut tx = tx_owner(DMA_LOW);
    let armed_tx = arm_with_ack(&mut tx, &[0x21]);
    let rx = rx_pool::<1>(DMA_LOW + 128);
    let armed_rx = rx.arm_next().unwrap();
    let actor =
        MacReady::new().request_transmit_with_ack(armed_tx, armed_rx, MacTransmitAccess::Direct);
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let pending = match active
        .process_batch(acknowledged(batch(Ieee802154Event::TxDone.mask())))
        .unwrap()
    {
        MacRuntimeBatchOutcome::Pending(active) => active,
        MacRuntimeBatchOutcome::Completed(_) => panic!("TX_DONE must await ACK"),
    };

    let quarantined = pending.quarantine_after_handoff_failure();
    assert_eq!(
        quarantined.hardware.executor.log.last(),
        Some(&LogEntry::DisarmAcknowledgementWatchdog)
    );
}

#[test]
fn no_dma_completion_returns_a_reusable_runtime_and_ready_state() {
    let actor = MacReady::new().request_clear_channel_assessment();
    let active = runtime(FakeExecutor::new()).start(actor).unwrap();
    let completed = match active
        .process_batch(acknowledged(
            MacEventBatch::new(
                Ieee802154Event::EdDone.mask(),
                None,
                None,
                Some(MacMeasurementSample::ClearChannel(MacCcaSample::Clear)),
            )
            .unwrap(),
        ))
        .unwrap()
    {
        MacRuntimeBatchOutcome::Completed(completed) => completed,
        MacRuntimeBatchOutcome::Pending(_) => panic!("ED_DONE must complete CCA"),
    };

    let resolved = completed.resolve(MacDeferredNext::IdlePolicy);
    let (runtime, ready, _no_dma, completion, next) = resolved.into_parts();
    assert_eq!(
        completion,
        MacCompletion::ClearChannelAssessment(MacCcaSample::Clear)
    );
    assert_eq!(next, MacDeferredNext::IdlePolicy);

    let active = runtime
        .start(ready.request_clear_channel_assessment())
        .unwrap();
    assert_eq!(
        active.hardware.executor.log,
        [
            LogEntry::Quiesce,
            LogEntry::RefreshPolicy,
            LogEntry::ConfigureDuration(8),
            LogEntry::Command(MacCommandIntent::ClearChannelAssessment),
            LogEntry::FinishTerminal,
            LogEntry::Quiesce,
            LogEntry::RefreshPolicy,
            LogEntry::ConfigureDuration(8),
            LogEntry::Command(MacCommandIntent::ClearChannelAssessment),
        ]
    );
}
