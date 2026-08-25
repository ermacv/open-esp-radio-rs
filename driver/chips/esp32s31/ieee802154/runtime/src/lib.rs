//! Affine executor-neutral owner for one ESP32-S31 IEEE 802.15.4 MAC operation.
//!
//! This crate composes the existing DMA ownership tokens, IRQ event vocabulary,
//! and pure MAC actor. It executes the actor's [`MacStartPlan`] through one
//! closed semantic executor and retains both that executor capability and the
//! active DMA resources until the event batch is accepted.
//!
//! There is deliberately no production executor, PAC dependency, target
//! constructor, interrupt route, status-register accessor, PHY acquisition, or
//! live-MMIO claim here. A future target integration must add a reviewed sealed
//! adapter in this crate before it can construct [`MacHardwareCapability`].

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

use core::fmt;

use open_esp_radio_esp32s31_ieee802154_dma::{RxArm, RxDmaAddress, TxArmed, TxDmaAddress};
use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154EventMask;
use open_esp_radio_esp32s31_ieee802154_mac::{
    MacActive, MacActivePhase, MacBatchOutcome, MacBatchRejectReason, MacBatchRejected,
    MacCommandIntent, MacDeferred, MacEventBatch, MacIntentStep, MacNoDmaResources, MacStartPlan,
    MacTxWithAckResources,
};

mod sealed {
    pub trait HardwareExecutor {}
    pub trait RuntimeResources {}
}

/// Closed semantic executor for the finite MAC runtime boundary.
///
/// The private supertrait prevents downstream implementations from claiming
/// hardware execution. A future HAL adapter must be reviewed and implemented
/// inside this crate. Each method denotes one semantic obligation; it exposes
/// neither a raw register address nor an arbitrary register image.
pub trait MacHardwareExecutor: sealed::HardwareExecutor {
    /// Executor-specific failure retained with the affine runtime owner.
    type Error;

    /// Establish state-specific quiescence and reconcile prior events.
    fn require_state_specific_quiescence(&mut self) -> Result<(), Self::Error>;

    /// Refresh the already reviewed static MAC policy.
    fn refresh_static_policy(&mut self) -> Result<(), Self::Error>;

    /// Publish the address of the exact hardware-owned TX token.
    fn publish_transmit_address(&mut self, address: TxDmaAddress<'_>) -> Result<(), Self::Error>;

    /// Publish the address of the exact hardware-owned RX token.
    fn publish_receive_address(&mut self, address: RxDmaAddress<'_>) -> Result<(), Self::Error>;

    /// Configure the bounded energy-detection duration carried by the plan.
    fn configure_energy_detection_duration(&mut self, units: u16) -> Result<(), Self::Error>;

    /// Request the final typed MAC command carried by the plan.
    fn request_command(&mut self, command: MacCommandIntent) -> Result<(), Self::Error>;

    /// Sample one complete event batch and acknowledge that exact snapshot.
    ///
    /// Success is the executor's semantic assertion that all sidebands were
    /// sampled before the corresponding event snapshot was acknowledged. The
    /// returned [`MacEventBatch`] is already structurally validated by the MAC
    /// crate. This runtime does not read or acknowledge an event register.
    fn sample_and_acknowledge_event_batch(&mut self) -> Result<MacEventBatch, Self::Error>;
}

/// Explicit authority to execute one MAC runtime chain.
///
/// The type has no public constructor. Merely implementing a similarly shaped
/// trait or possessing a PAC peripheral cannot mint this capability.
pub struct MacHardwareCapability<E: MacHardwareExecutor> {
    executor: E,
}

impl<E: MacHardwareExecutor> MacHardwareCapability<E> {
    #[cfg(test)]
    const fn from_model_executor(executor: E) -> Self {
        Self { executor }
    }
}

/// One supported affine resource set retained by [`MacActive`].
///
/// This trait is closed so a downstream type cannot manufacture a start-plan
/// association for unrelated resources.
pub trait MacRuntimeResources: sealed::RuntimeResources + Sized {
    #[doc(hidden)]
    /// Borrow the exact start plan for these resources.
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>>;
}

impl<'pool, const COUNT: usize> sealed::RuntimeResources for RxArm<'pool, COUNT> {}

impl<'pool, const COUNT: usize> MacRuntimeResources for RxArm<'pool, COUNT> {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

impl sealed::RuntimeResources for TxArmed<'_> {}

impl MacRuntimeResources for TxArmed<'_> {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

impl<'tx, 'rx, const COUNT: usize> sealed::RuntimeResources
    for MacTxWithAckResources<'tx, 'rx, COUNT>
{
}

impl<'tx, 'rx, const COUNT: usize> MacRuntimeResources for MacTxWithAckResources<'tx, 'rx, COUNT> {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

impl sealed::RuntimeResources for MacNoDmaResources {}

impl MacRuntimeResources for MacNoDmaResources {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

/// Idle executor owner before a logical MAC request has been started.
pub struct MacRuntime<E: MacHardwareExecutor> {
    hardware: MacHardwareCapability<E>,
}

impl<E: MacHardwareExecutor> MacRuntime<E> {
    /// Bind an explicit sealed hardware capability without touching hardware.
    pub const fn from_hardware(hardware: MacHardwareCapability<E>) -> Self {
        Self { hardware }
    }

    /// Execute an active actor's complete [`MacStartPlan`] in exact index order.
    ///
    /// Success retains the active actor and executor together. Failure also
    /// retains both in [`MacRuntimeStartFailure`], because an earlier step may
    /// already have changed external state and retry is not automatically safe.
    pub fn start<R: MacRuntimeResources>(
        mut self,
        active: MacActive<R>,
    ) -> Result<MacRuntimeActive<R, E>, MacRuntimeStartFailure<R, E>> {
        let execution = execute_start_plan(&mut self.hardware.executor, &active);
        match execution {
            Ok(()) => Ok(MacRuntimeActive {
                hardware: self.hardware,
                active,
            }),
            Err(error) => Err(MacRuntimeStartFailure {
                hardware: self.hardware,
                active,
                error,
            }),
        }
    }
}

/// Failure to obtain or completely execute one start plan.
pub enum MacRuntimeStartError<Error> {
    /// The actor phase does not expose a start plan.
    StartPlanUnavailable,
    /// `step_count` named an index for which the plan returned no step.
    InconsistentStartPlan {
        /// First missing zero-based step index.
        step_index: usize,
    },
    /// The executor rejected one indexed step after every earlier step ran.
    Executor {
        /// Zero-based position in [`MacStartPlan`].
        step_index: usize,
        /// Executor-specific failure.
        error: Error,
    },
}

impl<Error: fmt::Debug> fmt::Debug for MacRuntimeStartError<Error> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StartPlanUnavailable => formatter.write_str("StartPlanUnavailable"),
            Self::InconsistentStartPlan { step_index } => formatter
                .debug_struct("InconsistentStartPlan")
                .field("step_index", step_index)
                .finish(),
            Self::Executor { step_index, error } => formatter
                .debug_struct("Executor")
                .field("step_index", step_index)
                .field("error", error)
                .finish(),
        }
    }
}

/// Quarantined failed start retaining the executor and all active resources.
#[must_use = "a partial start retains affine hardware and DMA ownership"]
pub struct MacRuntimeStartFailure<R, E: MacHardwareExecutor> {
    #[allow(
        dead_code,
        reason = "the capability is intentionally quarantined even when production code cannot recover it yet"
    )]
    hardware: MacHardwareCapability<E>,
    active: MacActive<R>,
    error: MacRuntimeStartError<E::Error>,
}

impl<R, E: MacHardwareExecutor> MacRuntimeStartFailure<R, E> {
    /// Return the phase whose start failed.
    pub const fn phase(&self) -> MacActivePhase {
        self.active.phase()
    }

    /// Borrow the failure without releasing the retained owners.
    pub const fn error(&self) -> &MacRuntimeStartError<E::Error> {
        &self.error
    }
}

impl<R, E> fmt::Debug for MacRuntimeStartFailure<R, E>
where
    E: MacHardwareExecutor,
    E::Error: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeStartFailure")
            .field("phase", &self.active.phase())
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// One non-replayable event batch sampled and acknowledged by the executor.
///
/// The constructor is private, and the value is neither `Clone` nor `Copy`.
/// [`MacRuntimeActive::process_batch`] consumes it exactly once.
#[derive(Debug)]
pub struct SampledAcknowledgedMacEventBatch {
    batch: MacEventBatch,
}

impl SampledAcknowledgedMacEventBatch {
    /// Return the closed IRQ event subset without exposing a replayable batch.
    pub const fn events(&self) -> Ieee802154EventMask {
        self.batch.events()
    }
}

/// Started runtime retaining the executor and exact active MAC resources.
pub struct MacRuntimeActive<R, E: MacHardwareExecutor> {
    hardware: MacHardwareCapability<E>,
    active: MacActive<R>,
}

impl<R, E: MacHardwareExecutor> fmt::Debug for MacRuntimeActive<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeActive")
            .field("phase", &self.active.phase())
            .finish_non_exhaustive()
    }
}

impl<R, E: MacHardwareExecutor> MacRuntimeActive<R, E> {
    /// Return the current logical MAC phase.
    pub const fn phase(&self) -> MacActivePhase {
        self.active.phase()
    }

    /// Ask the sealed executor for one sampled-and-acknowledged event batch.
    ///
    /// This consumes the owner. On failure [`MacRuntimeEventFailure`] retains
    /// it in quarantine because sampling or acknowledgement may have partially
    /// changed external state. On success the returned owner must accept the
    /// paired non-replayable batch through [`Self::process_batch`].
    pub fn sample_and_acknowledge(
        mut self,
    ) -> Result<(Self, SampledAcknowledgedMacEventBatch), MacRuntimeEventFailure<R, E>> {
        match self.hardware.executor.sample_and_acknowledge_event_batch() {
            Ok(batch) => Ok((self, SampledAcknowledgedMacEventBatch { batch })),
            Err(error) => Err(MacRuntimeEventFailure {
                hardware: self.hardware,
                active: self.active,
                error,
            }),
        }
    }

    /// Accept one sampled-and-acknowledged batch and advance transactionally.
    ///
    /// A pending result retains the same resources. A terminal result returns
    /// [`MacRuntimeCompletion`], which continues to retain the deferred actor
    /// and hardware capability. Rejection returns the exact active actor.
    pub fn process_batch(
        self,
        batch: SampledAcknowledgedMacEventBatch,
    ) -> Result<MacRuntimeBatchOutcome<R, E>, MacRuntimeBatchRejected<R, E>> {
        match self.active.process_batch(batch.batch) {
            Ok(MacBatchOutcome::Pending(active)) => {
                Ok(MacRuntimeBatchOutcome::Pending(MacRuntimeActive {
                    hardware: self.hardware,
                    active,
                }))
            }
            Ok(MacBatchOutcome::Deferred(deferred)) => {
                Ok(MacRuntimeBatchOutcome::Completed(MacRuntimeCompletion {
                    hardware: self.hardware,
                    deferred,
                }))
            }
            Err(rejected) => Err(MacRuntimeBatchRejected {
                hardware: self.hardware,
                rejected,
            }),
        }
    }
}

/// Quarantined sample/acknowledgement failure retaining every owner.
#[must_use = "an event-path failure retains affine hardware and DMA ownership"]
pub struct MacRuntimeEventFailure<R, E: MacHardwareExecutor> {
    #[allow(
        dead_code,
        reason = "the capability is intentionally quarantined even when production code cannot recover it yet"
    )]
    hardware: MacHardwareCapability<E>,
    active: MacActive<R>,
    error: E::Error,
}

impl<R, E: MacHardwareExecutor> MacRuntimeEventFailure<R, E> {
    /// Return the phase in which event sampling or acknowledgement failed.
    pub const fn phase(&self) -> MacActivePhase {
        self.active.phase()
    }

    /// Borrow the executor-specific failure.
    pub const fn error(&self) -> &E::Error {
        &self.error
    }
}

impl<R, E> fmt::Debug for MacRuntimeEventFailure<R, E>
where
    E: MacHardwareExecutor,
    E::Error: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeEventFailure")
            .field("phase", &self.active.phase())
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Accepted result of one non-empty sampled-and-acknowledged batch.
pub enum MacRuntimeBatchOutcome<R, E: MacHardwareExecutor> {
    /// No terminal event ran and all owners remain active.
    Pending(MacRuntimeActive<R, E>),
    /// A terminal event ran and all owners remain deferred until reclamation.
    Completed(MacRuntimeCompletion<R, E>),
}

impl<R, E: MacHardwareExecutor> fmt::Debug for MacRuntimeBatchOutcome<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending(active) => formatter.debug_tuple("Pending").field(active).finish(),
            Self::Completed(completed) => formatter
                .debug_tuple("Completed")
                .field(&completed.completion())
                .finish(),
        }
    }
}

/// Rejected event batch retaining the exact active actor and executor.
pub struct MacRuntimeBatchRejected<R, E: MacHardwareExecutor> {
    hardware: MacHardwareCapability<E>,
    rejected: MacBatchRejected<R>,
}

impl<R, E: MacHardwareExecutor> MacRuntimeBatchRejected<R, E> {
    /// Return the pure MAC rejection reason.
    pub const fn reason(&self) -> MacBatchRejectReason {
        self.rejected.reason()
    }

    /// Recover the exact runtime owner for a later fresh event batch.
    pub fn into_active(self) -> MacRuntimeActive<R, E> {
        MacRuntimeActive {
            hardware: self.hardware,
            active: self.rejected.into_active(),
        }
    }
}

impl<R, E: MacHardwareExecutor> fmt::Debug for MacRuntimeBatchRejected<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeBatchRejected")
            .field("reason", &self.rejected.reason())
            .finish_non_exhaustive()
    }
}

/// Terminal logical completion retaining deferred DMA and executor ownership.
#[must_use = "terminal DMA resources remain deferred until an explicit reclaim path"]
pub struct MacRuntimeCompletion<R, E: MacHardwareExecutor> {
    #[allow(
        dead_code,
        reason = "the executor stays affine with deferred resources until a reviewed reclaim API exists"
    )]
    hardware: MacHardwareCapability<E>,
    deferred: MacDeferred<R>,
}

impl<R, E: MacHardwareExecutor> MacRuntimeCompletion<R, E> {
    /// Return the terminal logical completion without releasing resources.
    pub const fn completion(&self) -> open_esp_radio_esp32s31_ieee802154_mac::MacCompletion {
        self.deferred.completion()
    }
}

fn execute_start_plan<R: MacRuntimeResources, E: MacHardwareExecutor>(
    executor: &mut E,
    active: &MacActive<R>,
) -> Result<(), MacRuntimeStartError<E::Error>> {
    let plan = R::start_plan(active).ok_or(MacRuntimeStartError::StartPlanUnavailable)?;
    for step_index in 0..plan.step_count() {
        let step = plan
            .step(step_index)
            .ok_or(MacRuntimeStartError::InconsistentStartPlan { step_index })?;
        let result = match step {
            MacIntentStep::RequireStateSpecificQuiescence => {
                executor.require_state_specific_quiescence()
            }
            MacIntentStep::RefreshStaticPolicy => executor.refresh_static_policy(),
            MacIntentStep::PublishTransmitAddress(address) => {
                executor.publish_transmit_address(address)
            }
            MacIntentStep::PublishReceiveAddress(address) => {
                executor.publish_receive_address(address)
            }
            MacIntentStep::ConfigureEnergyDetectionDuration(units) => {
                executor.configure_energy_detection_duration(units)
            }
            MacIntentStep::RequestCommand(command) => executor.request_command(command),
        };
        if let Err(error) = result {
            return Err(MacRuntimeStartError::Executor { step_index, error });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_ieee802154_dma::{
        DMA_LOW, DmaFrameAddress, PinnedRxPool, PinnedTxBuffer, RxPoolStorage, TxStorage,
    };
    use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154Event;
    use open_esp_radio_esp32s31_ieee802154_mac::{
        MacCompletion, MacMeasurementSample, MacReady, MacTransmitAccess,
    };
    use std::{boxed::Box, collections::VecDeque, vec::Vec};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FakeError {
        Injected,
        NoBatch,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum LogEntry {
        Quiesce,
        RefreshPolicy,
        PublishTx(u32),
        PublishRx(u32),
        ConfigureDuration(u16),
        Command(MacCommandIntent),
        SampleAndAcknowledge(u16),
    }

    struct FakeExecutor {
        log: Vec<LogEntry>,
        batches: VecDeque<MacEventBatch>,
        fail_step: Option<usize>,
        calls: usize,
        fail_sample: bool,
    }

    impl FakeExecutor {
        fn new(batches: impl IntoIterator<Item = MacEventBatch>) -> Self {
            Self {
                log: Vec::new(),
                batches: batches.into_iter().collect(),
                fail_step: None,
                calls: 0,
                fail_sample: false,
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

    impl sealed::HardwareExecutor for FakeExecutor {}

    impl MacHardwareExecutor for FakeExecutor {
        type Error = FakeError;

        fn require_state_specific_quiescence(&mut self) -> Result<(), Self::Error> {
            self.record(LogEntry::Quiesce)
        }

        fn refresh_static_policy(&mut self) -> Result<(), Self::Error> {
            self.record(LogEntry::RefreshPolicy)
        }

        fn publish_transmit_address(
            &mut self,
            address: TxDmaAddress<'_>,
        ) -> Result<(), Self::Error> {
            self.record(LogEntry::PublishTx(address.as_u32()))
        }

        fn publish_receive_address(
            &mut self,
            address: RxDmaAddress<'_>,
        ) -> Result<(), Self::Error> {
            self.record(LogEntry::PublishRx(address.as_u32()))
        }

        fn configure_energy_detection_duration(&mut self, units: u16) -> Result<(), Self::Error> {
            self.record(LogEntry::ConfigureDuration(units))
        }

        fn request_command(&mut self, command: MacCommandIntent) -> Result<(), Self::Error> {
            self.record(LogEntry::Command(command))
        }

        fn sample_and_acknowledge_event_batch(&mut self) -> Result<MacEventBatch, Self::Error> {
            if self.fail_sample {
                return Err(FakeError::Injected);
            }
            let batch = self.batches.pop_front().ok_or(FakeError::NoBatch)?;
            self.log
                .push(LogEntry::SampleAndAcknowledge(batch.events().bits()));
            Ok(batch)
        }
    }

    fn runtime(executor: FakeExecutor) -> MacRuntime<FakeExecutor> {
        MacRuntime::from_hardware(MacHardwareCapability::from_model_executor(executor))
    }

    fn tx_owner(address: u32) -> PinnedTxBuffer {
        let storage = Box::leak(Box::new(TxStorage::new()));
        TxStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
    }

    fn rx_pool<const COUNT: usize>(address: u32) -> PinnedRxPool<COUNT> {
        let storage = Box::leak(Box::new(RxPoolStorage::new()));
        RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
            .unwrap()
    }

    fn batch(events: Ieee802154EventMask) -> MacEventBatch {
        MacEventBatch::new(events, None, None, None).unwrap()
    }

    #[test]
    fn transmit_with_ack_executes_the_complete_plan_in_exact_order() {
        let mut tx = tx_owner(DMA_LOW);
        let armed_tx = tx.prepare(&[1]).unwrap().arm();
        let rx = rx_pool::<1>(DMA_LOW + 128);
        let armed_rx = rx.arm_next().unwrap();
        let actor = MacReady::new_model().request_transmit_with_ack(
            armed_tx,
            armed_rx,
            MacTransmitAccess::Direct,
        );

        let active = runtime(FakeExecutor::new([])).start(actor).unwrap();
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
        let actor = MacReady::new_model().request_energy_detection(
            open_esp_radio_esp32s31_ieee802154_mac::MacEnergyDetectionDuration::from_hardware_units(
                37,
            ),
        );

        let active = runtime(FakeExecutor::new([])).start(actor).unwrap();
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
        let mut fake = FakeExecutor::new([]);
        fake.fail_step = Some(2);
        let actor = MacReady::new_model().request_clear_channel_assessment();

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
    fn sampled_acknowledged_batches_advance_without_reexecuting_start() {
        let first = batch(Ieee802154Event::TxDone.mask());
        let second = batch(Ieee802154Event::AckRxDone.mask());
        let mut tx = tx_owner(DMA_LOW);
        let armed_tx = tx.prepare(&[1]).unwrap().arm();
        let rx = rx_pool::<1>(DMA_LOW + 128);
        let armed_rx = rx.arm_next().unwrap();
        let actor = MacReady::new_model().request_transmit_with_ack(
            armed_tx,
            armed_rx,
            MacTransmitAccess::Direct,
        );
        let active = runtime(FakeExecutor::new([first, second]))
            .start(actor)
            .unwrap();
        let start_calls = active.hardware.executor.calls;

        let (active, sampled) = active.sample_and_acknowledge().unwrap();
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

        let (pending, sampled) = pending.sample_and_acknowledge().unwrap();
        let completed = match pending.process_batch(sampled).unwrap() {
            MacRuntimeBatchOutcome::Completed(completed) => completed,
            MacRuntimeBatchOutcome::Pending(_) => panic!("ACK_RX_DONE must be terminal"),
        };
        assert_eq!(completed.completion(), MacCompletion::TransmitAcknowledged);
        let MacRuntimeCompletion { hardware, deferred } = completed;
        assert_eq!(
            hardware.executor.log,
            [
                LogEntry::Quiesce,
                LogEntry::RefreshPolicy,
                LogEntry::PublishTx(DMA_LOW),
                LogEntry::PublishRx(DMA_LOW + 128),
                LogEntry::Command(MacCommandIntent::Transmit),
                LogEntry::SampleAndAcknowledge(Ieee802154Event::TxDone.bit()),
                LogEntry::SampleAndAcknowledge(Ieee802154Event::AckRxDone.bit()),
            ]
        );

        let resolved = deferred
            .resolve_model(open_esp_radio_esp32s31_ieee802154_mac::MacDeferredNext::IdlePolicy)
            .unwrap();
        let (_ready, reclaimed, _, _) = resolved.into_parts();
        let (tx, rx) = reclaimed.into_parts();
        tx.release();
        match rx {
            open_esp_radio_esp32s31_ieee802154_mac::MacModelResolvedRx::Frame(frame) => {
                frame.release().unwrap();
            }
            open_esp_radio_esp32s31_ieee802154_mac::MacModelResolvedRx::Stub(stub) => {
                stub.discard().unwrap();
            }
        }
    }

    #[test]
    fn rejected_batch_returns_the_same_runtime_owner() {
        let wrong = batch(Ieee802154Event::TxDone.mask());
        let done = MacEventBatch::new(
            Ieee802154Event::EdDone.mask(),
            None,
            None,
            Some(MacMeasurementSample::ClearChannel(
                open_esp_radio_esp32s31_ieee802154_mac::MacCcaSample::Clear,
            )),
        )
        .unwrap();
        let actor = MacReady::new_model().request_clear_channel_assessment();
        let active = runtime(FakeExecutor::new([wrong, done]))
            .start(actor)
            .unwrap();

        let (active, sampled) = active.sample_and_acknowledge().unwrap();
        let rejected = active.process_batch(sampled).unwrap_err();
        assert!(matches!(
            rejected.reason(),
            MacBatchRejectReason::UnexpectedEventBits { .. }
        ));
        let active = rejected.into_active();
        let (active, sampled) = active.sample_and_acknowledge().unwrap();
        let completed = match active.process_batch(sampled).unwrap() {
            MacRuntimeBatchOutcome::Completed(completed) => completed,
            MacRuntimeBatchOutcome::Pending(_) => panic!("ED_DONE must complete CCA"),
        };
        assert_eq!(
            completed.completion(),
            MacCompletion::ClearChannelAssessment(
                open_esp_radio_esp32s31_ieee802154_mac::MacCcaSample::Clear
            )
        );
    }

    #[test]
    fn event_path_failure_quarantines_the_active_owner() {
        let mut fake = FakeExecutor::new([]);
        fake.fail_sample = true;
        let actor = MacReady::new_model().request_clear_channel_assessment();
        let active = runtime(fake).start(actor).unwrap();

        let failure = active.sample_and_acknowledge().unwrap_err();
        assert_eq!(failure.phase(), MacActivePhase::ClearChannelAssessment);
        assert_eq!(failure.error(), &FakeError::Injected);
    }
}
