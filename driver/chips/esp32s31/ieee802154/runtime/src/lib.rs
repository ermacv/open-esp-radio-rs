//! Affine executor-neutral owner for one ESP32-S31 IEEE 802.15.4 MAC operation.
//!
//! This crate composes the existing DMA ownership tokens, IRQ event vocabulary,
//! and pure MAC actor. It executes the actor's [`MacStartPlan`] through one
//! closed command executor and retains both that command capability and the
//! active DMA resources until an ISR-sampled event batch is accepted.
//!
//! The production executor owns the dedicated PAC task capability and exposes
//! only the public-LL command sequence. Interrupt status remains in a disjoint
//! hard-IRQ owner; PHY acquisition and the platform CPU route are composed by
//! higher layers before they mint their ready state.

#![no_std]
#![deny(unsafe_code)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

use core::fmt;

use open_esp_radio_esp32s31_ieee802154_dma::{
    DmaTerminalEvidence, RxArm, RxDmaAddress, TxAckNotRequested, TxArmed, TxCompleted, TxDmaAddress,
};
use open_esp_radio_esp32s31_ieee802154_irq::{
    Ieee802154AcknowledgedInterrupt, Ieee802154Event, Ieee802154EventMask,
    Ieee802154EventObservationError, Ieee802154RxAbortReasonObservation,
    Ieee802154TxAbortReasonObservation,
};
use open_esp_radio_esp32s31_ieee802154_mac::{
    MacActive, MacActivePhase, MacBatchConstructionError, MacBatchOutcome, MacBatchRejectReason,
    MacBatchRejected, MacCcaSample, MacCommandIntent, MacCompletion, MacDeferred, MacDeferredNext,
    MacEnergySample, MacEventBatch, MacIntentStep, MacMeasurementSample, MacNoDmaResources,
    MacReady, MacResolved, MacResolvedRx, MacResolvedTxWithAck, MacRxResolutionFailure,
    MacStartPlan, MacTxWithAckResolutionFailure, MacTxWithAckResources,
};

mod pac_command_executor;

pub use pac_command_executor::{
    Esp32s31Ieee802154CommandError, Esp32s31Ieee802154CommandExecutor,
    IEEE802154_ACK_WATCHDOG_MICROSECONDS, Ieee802154MonotonicMicrosecondClock,
    ieee802154_ack_watchdog_threshold,
};

mod sealed {
    pub trait CommandExecutor {}
    pub trait RuntimeResources {}
}

/// Closed task-side command executor for the finite MAC runtime boundary.
///
/// The private supertrait prevents downstream implementations from claiming
/// command-register execution. Interrupt sampling and acknowledgement are
/// deliberately absent: the hard-IRQ capability owns those operations. A
/// target adapter must be implemented inside this crate. Each method denotes
/// one semantic obligation and exposes neither a raw address nor a complete
/// register image.
pub trait MacCommandExecutor: sealed::CommandExecutor {
    /// Executor-specific failure retained with the affine runtime owner.
    type Error;

    /// Prove that the retained automatic-ACK policy gives the requested actor
    /// phase the same terminal semantics used by the pure state machine.
    fn validate_operation_policy(&self, phase: MacActivePhase)
    -> Result<(), MacRuntimePolicyError>;

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
pub enum MacRuntimePolicyError {
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

/// Explicit authority to execute one MAC runtime chain.
///
/// The type has no public constructor. Merely implementing a similarly shaped
/// trait or possessing a PAC peripheral cannot mint this capability.
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
    ) -> Result<(), MacRuntimePolicyError> {
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
impl MacRuntime<ValidationMacCommandExecutor> {
    /// Construct an MMIO-free validation runtime for dependent-crate tests.
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

impl sealed::RuntimeResources for TxArmed<'_, TxAckNotRequested> {}

impl MacRuntimeResources for TxArmed<'_, TxAckNotRequested> {
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
pub struct MacRuntime<E: MacCommandExecutor> {
    hardware: MacCommandCapability<E>,
}

impl<E: MacCommandExecutor> MacRuntime<E> {
    /// Bind an explicit sealed command capability without touching hardware.
    pub const fn from_commands(hardware: MacCommandCapability<E>) -> Self {
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
        if let Err(error) = self
            .hardware
            .executor
            .validate_operation_policy(active.phase())
        {
            return Err(MacRuntimeStartFailure {
                hardware: self.hardware,
                active,
                error: MacRuntimeStartError::IncompatiblePolicy(error),
            });
        }
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
    /// The static automatic-ACK policy would change the actor's terminal edge.
    IncompatiblePolicy(MacRuntimePolicyError),
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
pub struct MacRuntimeStartFailure<R, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "the capability is intentionally quarantined even when production code cannot recover it yet"
    )]
    hardware: MacCommandCapability<E>,
    active: MacActive<R>,
    error: MacRuntimeStartError<E::Error>,
}

impl<R, E: MacCommandExecutor> MacRuntimeStartFailure<R, E> {
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
    E: MacCommandExecutor,
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

/// One non-replayable event batch sampled and acknowledged by the hard IRQ.
///
/// The constructor is private, and the value is neither `Clone` nor `Copy`.
/// [`MacRuntimeActive::process_batch`] consumes it exactly once.
#[derive(Debug)]
pub struct AcknowledgedMacEventBatch {
    batch: MacEventBatch,
}

/// An acknowledged hard-IRQ value could not become a valid MAC event batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptBatchError {
    /// The PAC could not classify every sampled event.
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

/// Started runtime retaining the executor and exact active MAC resources.
pub struct MacRuntimeActive<R, E: MacCommandExecutor> {
    hardware: MacCommandCapability<E>,
    active: MacActive<R>,
}

/// Permanently contained runtime after an acknowledged IRQ handoff was lost
/// or could not be decoded.
///
/// The value intentionally exposes no recovery or processing API. If the
/// operation was waiting for an acknowledgement, construction first stops
/// TIMER0 and removes its interrupt from the runtime baseline.
#[must_use = "a contained runtime permanently retains its command and DMA owners"]
pub struct MacRuntimeQuarantined<R, E: MacCommandExecutor> {
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

impl<R, E: MacCommandExecutor> fmt::Debug for MacRuntimeActive<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeActive")
            .field("phase", &self.active.phase())
            .finish_non_exhaustive()
    }
}

impl<R, E: MacCommandExecutor> MacRuntimeActive<R, E> {
    /// Return the current logical MAC phase.
    pub const fn phase(&self) -> MacActivePhase {
        self.active.phase()
    }

    /// Fail closed after an acknowledged value was lost or undecodable.
    #[doc(hidden)]
    pub fn quarantine_after_handoff_failure(self) -> MacRuntimeQuarantined<R, E> {
        let mut hardware = self.hardware;
        if matches!(
            self.active.phase(),
            MacActivePhase::AwaitingAcknowledgement { .. }
        ) {
            hardware.executor.disarm_acknowledgement_watchdog();
        }
        MacRuntimeQuarantined {
            hardware,
            active: self.active,
        }
    }

    /// Accept one sampled-and-acknowledged batch and advance transactionally.
    ///
    /// A pending result retains the same resources. A terminal result returns
    /// [`MacRuntimeCompletion`], which continues to retain the deferred actor
    /// and hardware capability. Rejection returns the exact active actor.
    pub fn process_batch(
        self,
        batch: AcknowledgedMacEventBatch,
    ) -> Result<MacRuntimeBatchOutcome<R, E>, MacRuntimeBatchRejected<R, E>> {
        let prior_phase = self.active.phase();
        match self.active.process_batch(batch.batch) {
            Ok(MacBatchOutcome::Pending(active)) => {
                let mut hardware = self.hardware;
                if enters_acknowledgement_wait(prior_phase, active.phase()) {
                    hardware.executor.arm_acknowledgement_watchdog();
                }
                Ok(MacRuntimeBatchOutcome::Pending(MacRuntimeActive {
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
                // SAFETY: `batch` can only be constructed from the affine
                // acknowledged hard-IRQ value. The actor consumed that batch
                // and returned `Deferred`, proving it accepted a terminal for
                // these exact retained resources. Evidence stays private in
                // the completion until type-specific reclaim consumes it.
                #[allow(
                    unsafe_code,
                    reason = "the private acknowledged terminal batch is the DMA reclaim proof"
                )]
                let terminal = unsafe { DmaTerminalEvidence::from_accepted_terminal_batch() };
                Ok(MacRuntimeBatchOutcome::Completed(MacRuntimeCompletion {
                    hardware,
                    deferred,
                    terminal,
                }))
            }
            Err(rejected) => {
                let reason = rejected.reason();
                let mut hardware = self.hardware;
                if matches!(prior_phase, MacActivePhase::AwaitingAcknowledgement { .. }) {
                    hardware.executor.disarm_acknowledgement_watchdog();
                }
                Err(MacRuntimeBatchRejected {
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
            acknowledgement:
                open_esp_radio_esp32s31_ieee802154_mac::MacTransmitAcknowledgement::Expected,
            ..
        }
    ) && matches!(next, MacActivePhase::AwaitingAcknowledgement { .. })
}

/// Accepted result of one non-empty sampled-and-acknowledged batch.
pub enum MacRuntimeBatchOutcome<R, E: MacCommandExecutor> {
    /// No terminal event ran and all owners remain active.
    Pending(MacRuntimeActive<R, E>),
    /// A terminal event ran and all owners remain deferred until reclamation.
    Completed(MacRuntimeCompletion<R, E>),
}

impl<R, E: MacCommandExecutor> fmt::Debug for MacRuntimeBatchOutcome<R, E> {
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
/// type deliberately exposes no method that can recover an active runtime.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_runtime::{
///     MacCommandExecutor, MacRuntimeBatchRejected,
/// };
///
/// fn retry<R, E: MacCommandExecutor>(rejected: MacRuntimeBatchRejected<R, E>) {
///     let _active = rejected.into_active();
/// }
/// ```
#[must_use = "an acknowledged rejected batch permanently retains the runtime and resources"]
pub struct MacRuntimeBatchRejected<R, E: MacCommandExecutor> {
    _hardware: MacCommandCapability<E>,
    _rejected: MacBatchRejected<R>,
    reason: MacBatchRejectReason,
}

impl<R, E: MacCommandExecutor> MacRuntimeBatchRejected<R, E> {
    /// Return the pure MAC rejection reason.
    pub const fn reason(&self) -> MacBatchRejectReason {
        self.reason
    }
}

impl<R, E: MacCommandExecutor> fmt::Debug for MacRuntimeBatchRejected<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeBatchRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Terminal logical completion retaining deferred DMA and executor ownership.
#[must_use = "terminal DMA resources remain deferred until an explicit reclaim path"]
pub struct MacRuntimeCompletion<R, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "the executor stays affine with deferred resources until a reviewed reclaim API exists"
    )]
    hardware: MacCommandCapability<E>,
    deferred: MacDeferred<R>,
    terminal: DmaTerminalEvidence,
}

impl<R, E: MacCommandExecutor> MacRuntimeCompletion<R, E> {
    /// Return the terminal logical completion without releasing resources.
    pub const fn completion(&self) -> open_esp_radio_esp32s31_ieee802154_mac::MacCompletion {
        self.deferred.completion()
    }
}

/// Reusable runtime and logical ready state after a terminal no-DMA request.
#[must_use = "the returned command owner is required for the next MAC operation"]
pub struct MacRuntimeResolved<C, E: MacCommandExecutor> {
    runtime: MacRuntime<E>,
    ready: MacReady,
    reclaimed: C,
    completion: MacCompletion,
    next: MacDeferredNext,
}

/// Quarantined runtime after a terminal RX ownership transition failed.
///
/// The sealed command executor and actor-side failure stay paired and cannot
/// be recovered for another command.
#[must_use = "a DMA resolution failure retains the command and DMA owners"]
pub struct MacRuntimeDmaResolutionFailure<F, E: MacCommandExecutor> {
    #[allow(
        dead_code,
        reason = "the sealed executor is intentionally quarantined on DMA lifecycle failure"
    )]
    hardware: MacCommandCapability<E>,
    failure: F,
}

impl<F, E: MacCommandExecutor> MacRuntimeDmaResolutionFailure<F, E> {
    /// Inspect the actor-side lifecycle failure without releasing its owners.
    pub const fn failure(&self) -> &F {
        &self.failure
    }
}

impl<F, E: MacCommandExecutor> fmt::Debug for MacRuntimeDmaResolutionFailure<F, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacRuntimeDmaResolutionFailure")
            .finish_non_exhaustive()
    }
}

impl<C, E: MacCommandExecutor> MacRuntimeResolved<C, E> {
    /// Split the reusable command runtime from the pure actor state, reclaimed
    /// resource value, terminal result, and deferred-next decision.
    pub fn into_parts(self) -> (MacRuntime<E>, MacReady, C, MacCompletion, MacDeferredNext) {
        (
            self.runtime,
            self.ready,
            self.reclaimed,
            self.completion,
            self.next,
        )
    }
}

impl<E: MacCommandExecutor> MacRuntimeCompletion<MacNoDmaResources, E> {
    /// Resolve a terminal CCA/ED request and return a runtime ready for another
    /// operation.
    ///
    /// No DMA ownership transition is needed: the acknowledged terminal batch
    /// already closed the concrete command epoch before this value was minted.
    pub fn resolve(self, next: MacDeferredNext) -> MacRuntimeResolved<MacNoDmaResources, E> {
        let Self {
            hardware,
            deferred,
            terminal: _,
        } = self;
        let (ready, reclaimed, completion, next) = deferred.resolve(next).into_parts();
        MacRuntimeResolved {
            runtime: MacRuntime { hardware },
            ready,
            reclaimed,
            completion,
            next,
        }
    }
}

impl<'owner, E: MacCommandExecutor> MacRuntimeCompletion<TxArmed<'owner, TxAckNotRequested>, E> {
    /// Reclaim a no-ACK TX buffer after the accepted terminal batch and return
    /// a runtime ready for another operation.
    pub fn resolve(self, next: MacDeferredNext) -> MacRuntimeResolved<TxCompleted<'owner>, E> {
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
    MacRuntimeCompletion<RxArm<'pool, COUNT>, E>
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
        MacRuntimeResolved<MacResolvedRx<'pool, COUNT>, E>,
        MacRuntimeDmaResolutionFailure<MacRxResolutionFailure<'pool, COUNT>, E>,
    > {
        let Self {
            hardware,
            deferred,
            terminal,
        } = self;
        match deferred.resolve_with_terminal_evidence(next, &terminal) {
            Ok(resolved) => Ok(resolved_runtime(hardware, resolved)),
            Err(failure) => Err(MacRuntimeDmaResolutionFailure { hardware, failure }),
        }
    }
}

impl<'tx, 'rx, const COUNT: usize, E: MacCommandExecutor>
    MacRuntimeCompletion<MacTxWithAckResources<'tx, 'rx, COUNT>, E>
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
        MacRuntimeResolved<MacResolvedTxWithAck<'tx, 'rx, COUNT>, E>,
        MacRuntimeDmaResolutionFailure<MacTxWithAckResolutionFailure<'tx, 'rx, COUNT>, E>,
    > {
        let Self {
            hardware,
            deferred,
            terminal,
        } = self;
        match deferred.resolve_with_terminal_evidence(next, &terminal) {
            Ok(resolved) => Ok(resolved_runtime(hardware, resolved)),
            Err(failure) => Err(MacRuntimeDmaResolutionFailure { hardware, failure }),
        }
    }
}

fn resolved_runtime<C, E: MacCommandExecutor>(
    hardware: MacCommandCapability<E>,
    resolved: MacResolved<C>,
) -> MacRuntimeResolved<C, E> {
    let (ready, reclaimed, completion, next) = resolved.into_parts();
    MacRuntimeResolved {
        runtime: MacRuntime { hardware },
        ready,
        reclaimed,
        completion,
        next,
    }
}

fn execute_start_plan<R: MacRuntimeResources, E: MacCommandExecutor>(
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
mod tests;
