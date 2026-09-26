//! Affine executor-neutral owner for one ESP32-S31 IEEE 802.15.4 MAC operation.
//!
//! This crate composes the existing DMA ownership tokens, IRQ event vocabulary,
//! and pure MAC actor. It executes the actor's [`MacStartPlan`] through one
//! closed command executor and retains both that command capability and the
//! active DMA resources until an ISR-sampled event batch is accepted.
//!
//! The production executor owns the dedicated HAL task capability and exposes
//! only the public-LL command sequence. Interrupt status remains in a disjoint
//! hard-IRQ owner; PHY acquisition and the platform CPU route are composed by
//! higher layers before they mint their ready state.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

use core::fmt;

use oer_esp32s31_ieee802154_dma::{
    DmaTerminalEvidence, RxArm, RxDmaAddress, TxAckNotRequested, TxArmed, TxCompleted, TxDmaAddress,
};

use oer_esp32s31_ieee802154_irq::{
    Ieee802154AcknowledgedInterrupt, Ieee802154Event, Ieee802154EventMask,
    Ieee802154EventObservationError, Ieee802154RxAbortReasonObservation,
    Ieee802154TxAbortReasonObservation,
};

use oer_esp32s31_ieee802154_mac::{
    MacActive, MacActivePhase, MacBatchConstructionError, MacBatchOutcome, MacBatchRejectReason,
    MacBatchRejected, MacCcaSample, MacCommandIntent, MacCompletion, MacDeferred, MacDeferredNext,
    MacEnergySample, MacEventBatch, MacIntentStep, MacMeasurementSample, MacNoDmaResources,
    MacReady, MacResolved, MacResolvedRx, MacResolvedTxWithAck, MacRxResolutionFailure,
    MacStartPlan, MacTxWithAckResolutionFailure, MacTxWithAckResources,
};

mod command_executor;

pub mod engine;

pub use command_executor::{
    IEEE802154_ACK_WATCHDOG_MICROSECONDS, Ieee802154CommandError, Ieee802154CommandExecutor,
    Ieee802154MonotonicMicrosecondClock, ieee802154_ack_watchdog_threshold,
};

mod sealed {
    pub trait CommandExecutor {}
    pub trait OperationResources {}
}

/// Closed task-side command executor for the finite MAC operation boundary.
///
/// The private supertrait prevents downstream implementations from claiming
/// command-register execution. Interrupt sampling and acknowledgement are
/// deliberately absent: the hard-IRQ capability owns those operations. A
/// target adapter must be implemented inside this crate. Each method denotes
/// one semantic obligation and exposes neither a raw address nor a complete
/// register image.
pub trait MacCommandExecutor: sealed::CommandExecutor {
    /// Executor-specific failure retained with the affine operation owner.
    type Error;

    /// Prove that the retained automatic-ACK policy gives the requested actor
    /// phase the same terminal semantics used by the pure state machine.
    fn validate_operation_policy(
        &self,
        phase: MacActivePhase,
    ) -> Result<(), MacOperationPolicyError>;

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

    /// Arm the source-defined TIMER0 watchdog after an ACK-requesting transmit
    /// crosses from `TX_DONE` into acknowledgement reception.
    fn arm_acknowledgement_watchdog(&mut self);

    /// Stop TIMER0 and remove its event from the active interrupt baseline.
    fn disarm_acknowledgement_watchdog(&mut self);

    /// Close the internal active-command epoch after the pure actor accepted
    /// one terminal, already-acknowledged IRQ batch.
    fn finish_terminal_operation(&mut self);
}

/// Static automatic-ACK policy is incompatible with one bounded operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOperationPolicyError {
    /// A receive advertised as terminal at `RX_DONE` would continue into
    /// automatic or enhanced ACK transmission.
    ReceiveWouldTransmitAcknowledgement {
        /// Hardware automatic ACK transmission is enabled.
        tx_auto_ack: bool,
        /// Hardware enhanced ACK transmission is enabled.
        enhanced_ack_tx: bool,
    },
    /// An ACK-requesting frame cannot enter `RX_ACK` because automatic ACK
    /// reception is disabled in the retained hardware policy.
    AcknowledgementReceptionDisabled,
}

/// Explicit authority to execute one MAC operation chain.
///
/// The type has no public constructor. Merely implementing a similarly shaped
/// trait or possessing a register peripheral cannot mint this capability.
pub struct MacCommandCapability<E: MacCommandExecutor> {
    executor: E,
}

impl<E: MacCommandExecutor> MacCommandCapability<E> {
    #[cfg(test)]
    const fn from_model_executor(executor: E) -> Self {
        Self { executor }
    }
}

/// MMIO-free executor for dependent-crate ownership and cancellation tests.
#[cfg(all(feature = "validation-probes", not(target_arch = "riscv32")))]
#[doc(hidden)]
pub struct ValidationMacCommandExecutor;

#[cfg(all(feature = "validation-probes", not(target_arch = "riscv32")))]
impl sealed::CommandExecutor for ValidationMacCommandExecutor {}

#[cfg(all(feature = "validation-probes", not(target_arch = "riscv32")))]
impl MacCommandExecutor for ValidationMacCommandExecutor {
    type Error = core::convert::Infallible;

    fn validate_operation_policy(
        &self,
        _phase: MacActivePhase,
    ) -> Result<(), MacOperationPolicyError> {
        Ok(())
    }

    fn require_state_specific_quiescence(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn refresh_static_policy(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn publish_transmit_address(&mut self, _address: TxDmaAddress<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn publish_receive_address(&mut self, _address: RxDmaAddress<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn configure_energy_detection_duration(&mut self, _units: u16) -> Result<(), Self::Error> {
        Ok(())
    }

    fn request_command(&mut self, _command: MacCommandIntent) -> Result<(), Self::Error> {
        Ok(())
    }

    fn arm_acknowledgement_watchdog(&mut self) {}

    fn disarm_acknowledgement_watchdog(&mut self) {}

    fn finish_terminal_operation(&mut self) {}
}

#[cfg(all(feature = "validation-probes", not(target_arch = "riscv32")))]
impl MacOperation<ValidationMacCommandExecutor> {
    /// Construct an MMIO-free validation owner for dependent-crate tests.
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self::from_commands(MacCommandCapability {
            executor: ValidationMacCommandExecutor,
        })
    }
}

/// One supported affine resource set retained by [`MacActive`].
///
/// This trait is closed so a downstream type cannot manufacture a start-plan
/// association for unrelated resources.
pub trait MacOperationResources: sealed::OperationResources + Sized {
    #[doc(hidden)]
    /// Borrow the exact start plan for these resources.
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>>;
}

impl<'pool, const COUNT: usize> sealed::OperationResources for RxArm<'pool, COUNT> {}

impl<'pool, const COUNT: usize> MacOperationResources for RxArm<'pool, COUNT> {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

impl sealed::OperationResources for TxArmed<'_, TxAckNotRequested> {}

impl MacOperationResources for TxArmed<'_, TxAckNotRequested> {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

impl<'tx, 'rx, const COUNT: usize> sealed::OperationResources
    for MacTxWithAckResources<'tx, 'rx, COUNT>
{
}

impl<'tx, 'rx, const COUNT: usize> MacOperationResources
    for MacTxWithAckResources<'tx, 'rx, COUNT>
{
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

impl sealed::OperationResources for MacNoDmaResources {}

impl MacOperationResources for MacNoDmaResources {
    fn start_plan(active: &MacActive<Self>) -> Option<MacStartPlan<'_>> {
        active.start_plan()
    }
}

/// Idle executor owner before a logical MAC request has been started.
pub struct MacOperation<E: MacCommandExecutor> {
    hardware: MacCommandCapability<E>,
}

impl<E: MacCommandExecutor> MacOperation<E> {
    /// Bind an explicit sealed command capability without touching hardware.
    pub const fn from_commands(hardware: MacCommandCapability<E>) -> Self {
        Self { hardware }
    }

    /// Execute an active actor's complete [`MacStartPlan`] in exact index order.
    ///
    /// Success retains the active actor and executor together. Failure also
    /// retains both in [`MacOperationStartFailure`], because an earlier step may
    /// already have changed external state and retry is not automatically safe.
    pub fn start<R: MacOperationResources>(
        mut self,
        active: MacActive<R>,
    ) -> Result<MacOperationActive<R, E>, MacOperationStartFailure<R, E>> {
        if let Err(error) = self
            .hardware
            .executor
            .validate_operation_policy(active.phase())
        {
            return Err(MacOperationStartFailure {
                hardware: self.hardware,
                active,
                error: MacOperationStartError::IncompatiblePolicy(error),
            });
        }
        let execution = execute_start_plan(&mut self.hardware.executor, &active);
        match execution {
            Ok(()) => Ok(MacOperationActive {
                hardware: self.hardware,
                active,
            }),
            Err(error) => Err(MacOperationStartFailure {
                hardware: self.hardware,
                active,
                error,
            }),
        }
    }
}

/// Failure to obtain or completely execute one start plan.
pub enum MacOperationStartError<Error> {
    /// The static automatic-ACK policy would change the actor's terminal edge.
    IncompatiblePolicy(MacOperationPolicyError),
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

impl<Error: fmt::Debug> fmt::Debug for MacOperationStartError<Error> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompatiblePolicy(error) => formatter
                .debug_tuple("IncompatiblePolicy")
                .field(error)
                .finish(),
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
pub struct MacOperationStartFailure<R, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "the capability is intentionally quarantined even when production code cannot recover it yet"
    )]
    hardware: MacCommandCapability<E>,
    active: MacActive<R>,
    error: MacOperationStartError<E::Error>,
}

impl<R, E: MacCommandExecutor> MacOperationStartFailure<R, E> {
    /// Return the phase whose start failed.
    pub const fn phase(&self) -> MacActivePhase {
        self.active.phase()
    }

    /// Borrow the failure without releasing the retained owners.
    pub const fn error(&self) -> &MacOperationStartError<E::Error> {
        &self.error
    }
}

impl<R, E> fmt::Debug for MacOperationStartFailure<R, E>
where
    E: MacCommandExecutor,
    E::Error: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacOperationStartFailure")
            .field("phase", &self.active.phase())
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// One non-replayable event batch sampled and acknowledged by the hard IRQ.
///
/// The constructor is private, and the value is neither `Clone` nor `Copy`.
/// [`MacOperationActive::process_batch`] consumes it exactly once.
#[derive(Debug)]
pub struct AcknowledgedMacEventBatch {
    batch: MacEventBatch,
}

/// An acknowledged hard-IRQ value could not become a valid MAC event batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptBatchError {
    /// The register-level classifier could not classify every sampled event.
    UnclassifiedEvents(Ieee802154EventObservationError),
    /// `RX_ABORT` was asserted without a source-confirmed reason.
    UnknownRxAbortReason,
    /// `TX_ABORT` was asserted without a source-confirmed reason.
    UnknownTxAbortReason,
    /// `ED_DONE` arrived outside a CCA or energy-detection phase.
    UnexpectedMeasurementPhase {
        /// Active affine MAC phase at interrupt delivery.
        phase: MacActivePhase,
    },
    /// The decoded sidebands did not satisfy the MAC batch invariants.
    Batch(MacBatchConstructionError),
}

impl AcknowledgedMacEventBatch {
    /// Decode one non-replayable hard-IRQ value for the active MAC phase.
    ///
    /// The input can only be minted after the interrupt port acknowledged its
    /// exact opaque snapshot. Unknown event bits and abort reasons fail closed;
    /// no task-side code reads or acknowledges hardware status here.
    pub fn from_interrupt(
        interrupt: Ieee802154AcknowledgedInterrupt,
        phase: MacActivePhase,
    ) -> Result<Self, MacInterruptBatchError> {
        let events = interrupt
            .event_classification()
            .map_err(MacInterruptBatchError::UnclassifiedEvents)?;

        let rx_abort_reason = if events.contains(Ieee802154Event::RxAbort) {
            Some(match interrupt.rx_abort_reason() {
                Some(Ieee802154RxAbortReasonObservation::Named(reason)) => reason,
                Some(Ieee802154RxAbortReasonObservation::Unclassified) | None => {
                    return Err(MacInterruptBatchError::UnknownRxAbortReason);
                }
            })
        } else {
            None
        };
        let tx_abort_reason = if events.contains(Ieee802154Event::TxAbort) {
            Some(match interrupt.tx_abort_reason() {
                Some(Ieee802154TxAbortReasonObservation::Named(reason)) => reason,
                Some(Ieee802154TxAbortReasonObservation::Unclassified) | None => {
                    return Err(MacInterruptBatchError::UnknownTxAbortReason);
                }
            })
        } else {
            None
        };
        let measurement = if events.contains(Ieee802154Event::EdDone) {
            Some(match phase {
                MacActivePhase::ClearChannelAssessment => {
                    MacMeasurementSample::ClearChannel(if interrupt.cca_busy() {
                        MacCcaSample::Busy
                    } else {
                        MacCcaSample::Clear
                    })
                }
                MacActivePhase::EnergyDetection { .. } => MacMeasurementSample::Energy(
                    MacEnergySample::from_raw_code(interrupt.ed_rss_code()),
                ),
                phase => {
                    return Err(MacInterruptBatchError::UnexpectedMeasurementPhase { phase });
                }
            })
        } else {
            None
        };

        let batch = MacEventBatch::new(events, rx_abort_reason, tx_abort_reason, measurement)
            .map_err(MacInterruptBatchError::Batch)?;
        Ok(Self { batch })
    }

    /// Return the closed IRQ event subset without exposing a replayable batch.
    pub const fn events(&self) -> Ieee802154EventMask {
        self.batch.events()
    }
}

/// Started operation retaining the executor and exact active MAC resources.
pub struct MacOperationActive<R, E: MacCommandExecutor> {
    hardware: MacCommandCapability<E>,
    active: MacActive<R>,
}

/// Permanently contained operation after an acknowledged IRQ handoff was lost
/// or could not be decoded.
///
/// The value intentionally exposes no recovery or processing API. If the
/// operation was waiting for an acknowledgement, construction first stops
/// TIMER0 and removes its interrupt from the operation baseline.
#[must_use = "a contained operation permanently retains its command and DMA owners"]
pub struct MacOperationQuarantined<R, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "contained hardware ownership is deliberately unrecoverable"
    )]
    hardware: MacCommandCapability<E>,
    #[allow(
        dead_code,
        reason = "contained actor and DMA ownership are deliberately unrecoverable"
    )]
    active: MacActive<R>,
}

impl<R, E: MacCommandExecutor> fmt::Debug for MacOperationActive<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacOperationActive")
            .field("phase", &self.active.phase())
            .finish_non_exhaustive()
    }
}

impl<R, E: MacCommandExecutor> MacOperationActive<R, E> {
    /// Return the current logical MAC phase.
    pub const fn phase(&self) -> MacActivePhase {
        self.active.phase()
    }

    /// Fail closed after an acknowledged value was lost or undecodable.
    #[doc(hidden)]
    pub fn quarantine_after_handoff_failure(self) -> MacOperationQuarantined<R, E> {
        let mut hardware = self.hardware;
        if matches!(
            self.active.phase(),
            MacActivePhase::AwaitingAcknowledgement { .. }
        ) {
            hardware.executor.disarm_acknowledgement_watchdog();
        }
        MacOperationQuarantined {
            hardware,
            active: self.active,
        }
    }

    /// Accept one sampled-and-acknowledged batch and advance transactionally.
    ///
    /// A pending result retains the same resources. A terminal result returns
    /// [`MacOperationCompletion`], which continues to retain the deferred actor
    /// and hardware capability. Rejection returns the exact active actor.
    pub fn process_batch(
        self,
        batch: AcknowledgedMacEventBatch,
    ) -> Result<MacOperationBatchOutcome<R, E>, MacOperationBatchRejected<R, E>> {
        let prior_phase = self.active.phase();
        match self.active.process_batch(batch.batch) {
            Ok(MacBatchOutcome::Pending(active)) => {
                let mut hardware = self.hardware;
                if enters_acknowledgement_wait(prior_phase, active.phase()) {
                    hardware.executor.arm_acknowledgement_watchdog();
                }
                Ok(MacOperationBatchOutcome::Pending(MacOperationActive {
                    hardware,
                    active,
                }))
            }
            Ok(MacBatchOutcome::Deferred(deferred)) => {
                let mut hardware = self.hardware;
                if matches!(prior_phase, MacActivePhase::AwaitingAcknowledgement { .. }) {
                    hardware.executor.disarm_acknowledgement_watchdog();
                }
                hardware.executor.finish_terminal_operation();
                #[allow(
                    unsafe_code,
                    reason = "the private acknowledged terminal batch is the DMA reclaim proof"
                )]
                // SAFETY: `batch` can only be constructed from the affine
                // acknowledged hard-IRQ value. The actor consumed that batch
                // and returned `Deferred`, proving it accepted a terminal for
                // these exact retained resources. Evidence stays private in
                // the completion until type-specific reclaim consumes it.
                let terminal = unsafe { DmaTerminalEvidence::from_accepted_terminal_batch() };
                Ok(MacOperationBatchOutcome::Completed(
                    MacOperationCompletion {
                        hardware,
                        deferred,
                        terminal,
                    },
                ))
            }
            Err(rejected) => {
                let reason = rejected.reason();
                let mut hardware = self.hardware;
                if matches!(prior_phase, MacActivePhase::AwaitingAcknowledgement { .. }) {
                    hardware.executor.disarm_acknowledgement_watchdog();
                }
                Err(MacOperationBatchRejected {
                    _hardware: hardware,
                    _rejected: rejected,
                    reason,
                })
            }
        }
    }
}

const fn enters_acknowledgement_wait(prior: MacActivePhase, next: MacActivePhase) -> bool {
    matches!(
        prior,
        MacActivePhase::Transmit {
            acknowledgement: oer_esp32s31_ieee802154_mac::MacTransmitAcknowledgement::Expected,
            ..
        }
    ) && matches!(next, MacActivePhase::AwaitingAcknowledgement { .. })
}

/// Accepted result of one non-empty sampled-and-acknowledged batch.
pub enum MacOperationBatchOutcome<R, E: MacCommandExecutor> {
    /// No terminal event ran and all owners remain active.
    Pending(MacOperationActive<R, E>),
    /// A terminal event ran and all owners remain deferred until reclamation.
    Completed(MacOperationCompletion<R, E>),
}

impl<R, E: MacCommandExecutor> fmt::Debug for MacOperationBatchOutcome<R, E> {
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

/// Quarantined acknowledged event batch retaining the exact active actor and
/// executor.
///
/// Once the hard IRQ acknowledged a snapshot, rejection is not retryable: the
/// rejected value may already have contained the hardware terminal edge. This
/// type deliberately exposes no method that can recover an active operation.
///
/// ```compile_fail
/// use oer_esp32s31_ieee802154::{
///     MacCommandExecutor, MacOperationBatchRejected,
/// };
///
/// fn retry<R, E: MacCommandExecutor>(rejected: MacOperationBatchRejected<R, E>) {
///     let _active = rejected.into_active();
/// }
/// ```
#[must_use = "an acknowledged rejected batch permanently retains the operation and resources"]
pub struct MacOperationBatchRejected<R, E: MacCommandExecutor> {
    _hardware: MacCommandCapability<E>,
    _rejected: MacBatchRejected<R>,
    reason: MacBatchRejectReason,
}

impl<R, E: MacCommandExecutor> MacOperationBatchRejected<R, E> {
    /// Return the pure MAC rejection reason.
    pub const fn reason(&self) -> MacBatchRejectReason {
        self.reason
    }
}

impl<R, E: MacCommandExecutor> fmt::Debug for MacOperationBatchRejected<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacOperationBatchRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Terminal logical completion retaining deferred DMA and executor ownership.
#[must_use = "terminal DMA resources remain deferred until an explicit reclaim path"]
pub struct MacOperationCompletion<R, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "the executor stays affine with deferred resources until a reviewed reclaim API exists"
    )]
    hardware: MacCommandCapability<E>,
    deferred: MacDeferred<R>,
    terminal: DmaTerminalEvidence,
}

impl<R, E: MacCommandExecutor> MacOperationCompletion<R, E> {
    /// Return the terminal logical completion without releasing resources.
    pub const fn completion(&self) -> oer_esp32s31_ieee802154_mac::MacCompletion {
        self.deferred.completion()
    }
}

/// Reusable operation owner and logical ready state after a terminal no-DMA request.
#[must_use = "the returned command owner is required for the next MAC operation"]
pub struct MacOperationResolved<C, E: MacCommandExecutor> {
    operation: MacOperation<E>,
    ready: MacReady,
    reclaimed: C,
    completion: MacCompletion,
    next: MacDeferredNext,
}

/// Quarantined operation after a terminal RX ownership transition failed.
///
/// The sealed command executor and actor-side failure stay paired and cannot
/// be recovered for another command.
#[must_use = "a DMA resolution failure retains the command and DMA owners"]
pub struct MacOperationDmaResolutionFailure<F, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "the sealed executor is intentionally quarantined on DMA lifecycle failure"
    )]
    hardware: MacCommandCapability<E>,
    failure: F,
}

impl<F, E: MacCommandExecutor> MacOperationDmaResolutionFailure<F, E> {
    /// Inspect the actor-side lifecycle failure without releasing its owners.
    pub const fn failure(&self) -> &F {
        &self.failure
    }
}

impl<F, E: MacCommandExecutor> fmt::Debug for MacOperationDmaResolutionFailure<F, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacOperationDmaResolutionFailure")
            .finish_non_exhaustive()
    }
}

impl<C, E: MacCommandExecutor> MacOperationResolved<C, E> {
    /// Split the reusable command owner from the pure actor state, reclaimed
    /// resource value, terminal result, and deferred-next decision.
    pub fn into_parts(self) -> (MacOperation<E>, MacReady, C, MacCompletion, MacDeferredNext) {
        (
            self.operation,
            self.ready,
            self.reclaimed,
            self.completion,
            self.next,
        )
    }
}

impl<E: MacCommandExecutor> MacOperationCompletion<MacNoDmaResources, E> {
    /// Resolve a terminal CCA/ED request and return a operation ready for another
    /// operation.
    ///
    /// No DMA ownership transition is needed: the acknowledged terminal batch
    /// already closed the concrete command epoch before this value was minted.
    pub fn resolve(self, next: MacDeferredNext) -> MacOperationResolved<MacNoDmaResources, E> {
        let Self {
            hardware,
            deferred,
            terminal: _,
        } = self;
        let (ready, reclaimed, completion, next) = deferred.resolve(next).into_parts();
        MacOperationResolved {
            operation: MacOperation { hardware },
            ready,
            reclaimed,
            completion,
            next,
        }
    }
}

impl<'owner, E: MacCommandExecutor> MacOperationCompletion<TxArmed<'owner, TxAckNotRequested>, E> {
    /// Reclaim a no-ACK TX buffer after the accepted terminal batch and return
    /// a operation ready for another operation.
    pub fn resolve(self, next: MacDeferredNext) -> MacOperationResolved<TxCompleted<'owner>, E> {
        let Self {
            hardware,
            deferred,
            terminal,
        } = self;
        resolved_runtime(
            hardware,
            deferred.resolve_with_terminal_evidence(next, &terminal),
        )
    }
}

impl<'pool, const COUNT: usize, E: MacCommandExecutor>
    MacOperationCompletion<RxArm<'pool, COUNT>, E>
{
    /// Reclaim the RX destination after an accepted `RX_DONE` or reviewed
    /// terminal abort.
    ///
    /// A successful `RX_DONE` returns frame ownership without recycling it;
    /// the caller may inspect the PHR-validated frame and explicitly recycle
    /// the slot before re-arm. Abort outcomes never expose frame bytes.
    pub fn resolve(
        self,
        next: MacDeferredNext,
    ) -> Result<
        MacOperationResolved<MacResolvedRx<'pool, COUNT>, E>,
        MacOperationDmaResolutionFailure<MacRxResolutionFailure<'pool, COUNT>, E>,
    > {
        let Self {
            hardware,
            deferred,
            terminal,
        } = self;
        match deferred.resolve_with_terminal_evidence(next, &terminal) {
            Ok(resolved) => Ok(resolved_runtime(hardware, resolved)),
            Err(failure) => Err(MacOperationDmaResolutionFailure { hardware, failure }),
        }
    }
}

impl<'tx, 'rx, const COUNT: usize, E: MacCommandExecutor>
    MacOperationCompletion<MacTxWithAckResources<'tx, 'rx, COUNT>, E>
{
    /// Reclaim a terminal TX image and its paired ACK receive destination.
    ///
    /// Only `ACK_RX_DONE` marks the RX resource as containing an ACK frame;
    /// timeout and abort completions return it as non-frame ownership for
    /// explicit recycle.
    pub fn resolve(
        self,
        next: MacDeferredNext,
    ) -> Result<
        MacOperationResolved<MacResolvedTxWithAck<'tx, 'rx, COUNT>, E>,
        MacOperationDmaResolutionFailure<MacTxWithAckResolutionFailure<'tx, 'rx, COUNT>, E>,
    > {
        let Self {
            hardware,
            deferred,
            terminal,
        } = self;
        match deferred.resolve_with_terminal_evidence(next, &terminal) {
            Ok(resolved) => Ok(resolved_runtime(hardware, resolved)),
            Err(failure) => Err(MacOperationDmaResolutionFailure { hardware, failure }),
        }
    }
}

fn resolved_runtime<C, E: MacCommandExecutor>(
    hardware: MacCommandCapability<E>,
    resolved: MacResolved<C>,
) -> MacOperationResolved<C, E> {
    let (ready, reclaimed, completion, next) = resolved.into_parts();
    MacOperationResolved {
        operation: MacOperation { hardware },
        ready,
        reclaimed,
        completion,
        next,
    }
}

fn execute_start_plan<R: MacOperationResources, E: MacCommandExecutor>(
    executor: &mut E,
    active: &MacActive<R>,
) -> Result<(), MacOperationStartError<E::Error>> {
    let plan = R::start_plan(active).ok_or(MacOperationStartError::StartPlanUnavailable)?;
    for step_index in 0..plan.step_count() {
        let step = plan
            .step(step_index)
            .ok_or(MacOperationStartError::InconsistentStartPlan { step_index })?;
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
            return Err(MacOperationStartError::Executor { step_index, error });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
