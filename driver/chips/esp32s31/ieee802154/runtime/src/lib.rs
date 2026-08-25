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
};
use open_esp_radio_esp32s31_ieee802154_mac::{
    MacActive, MacActivePhase, MacBatchConstructionError, MacBatchOutcome, MacBatchRejectReason,
    MacBatchRejected, MacCcaSample, MacCommandIntent, MacCompletion, MacDeferred, MacDeferredNext,
    MacEnergySample, MacEventBatch, MacIntentStep, MacMeasurementSample, MacNoDmaResources,
    MacReady, MacResolved, MacResolvedRx, MacResolvedTxWithAck, MacRxResolutionFailure,
    MacStartPlan, MacTxWithAckResolutionFailure, MacTxWithAckResources,
};

mod pac_command_executor;

pub use pac_command_executor::{Esp32s31Ieee802154CommandError, Esp32s31Ieee802154CommandExecutor};

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
    /// The raw event image contains bits absent from the public LL vocabulary.
    UnsupportedEventBits {
        /// Complete raw event image acknowledged by the ISR.
        raw_event_bits: u16,
        /// Bits not named by the public LL.
        unsupported_bits: u16,
    },
    /// `RX_ABORT` was asserted with an unknown reason code.
    UnknownRxAbortReason {
        /// Raw `RX_STATUS[8:4]` value sampled before acknowledgement.
        code: u8,
    },
    /// `TX_ABORT` was asserted with an unknown reason code.
    UnknownTxAbortReason {
        /// Raw `TX_STATUS[8:4]` value sampled before acknowledgement.
        code: u8,
    },
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
        let raw_event_bits = interrupt.raw_event_bits();
        let events = Ieee802154EventMask::from_named_bits(raw_event_bits).map_err(|error| {
            MacInterruptBatchError::UnsupportedEventBits {
                raw_event_bits,
                unsupported_bits: error.unsupported_bits(),
            }
        })?;

        let rx_abort_reason = if events.contains(Ieee802154Event::RxAbort) {
            Some(interrupt.rx_abort_reason().ok_or(
                MacInterruptBatchError::UnknownRxAbortReason {
                    code: interrupt.raw_rx_abort_reason_code(),
                },
            )?)
        } else {
            None
        };
        let tx_abort_reason = if events.contains(Ieee802154Event::TxAbort) {
            Some(interrupt.tx_abort_reason().ok_or(
                MacInterruptBatchError::UnknownTxAbortReason {
                    code: interrupt.raw_tx_abort_reason_code(),
                },
            )?)
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

    /// Accept one sampled-and-acknowledged batch and advance transactionally.
    ///
    /// A pending result retains the same resources. A terminal result returns
    /// [`MacRuntimeCompletion`], which continues to retain the deferred actor
    /// and hardware capability. Rejection returns the exact active actor.
    pub fn process_batch(
        self,
        batch: AcknowledgedMacEventBatch,
    ) -> Result<MacRuntimeBatchOutcome<R, E>, MacRuntimeBatchRejected<R, E>> {
        match self.active.process_batch(batch.batch) {
            Ok(MacBatchOutcome::Pending(active)) => {
                Ok(MacRuntimeBatchOutcome::Pending(MacRuntimeActive {
                    hardware: self.hardware,
                    active,
                }))
            }
            Ok(MacBatchOutcome::Deferred(deferred)) => {
                let mut hardware = self.hardware;
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
                Err(MacRuntimeBatchRejected {
                    _hardware: self.hardware,
                    _rejected: rejected,
                    reason,
                })
            }
        }
    }
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
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_ieee802154_dma::{
        DMA_LOW, DmaFrameAddress, DmaTerminalEvidence, PinnedRxPool, PinnedTxBuffer, PreparedTx,
        RxArm, RxCompletionKind, RxPoolStorage, RxSlotState, TxAckRequested, TxState, TxStorage,
    };
    use open_esp_radio_esp32s31_ieee802154_irq::{
        Ieee802154AcknowledgedInterrupt, acknowledged_interrupt_for_validation,
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
        RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
            .unwrap()
    }

    fn batch(events: Ieee802154EventMask) -> MacEventBatch {
        MacEventBatch::new(events, None, None, None).unwrap()
    }

    fn acknowledged(batch: MacEventBatch) -> AcknowledgedMacEventBatch {
        AcknowledgedMacEventBatch { batch }
    }

    fn interrupt(
        events: u16,
        rx_abort: u8,
        tx_abort: u8,
        ed_rss: i8,
        cca_busy: bool,
    ) -> Ieee802154AcknowledgedInterrupt {
        acknowledged_interrupt_for_validation(events, rx_abort, tx_abort, ed_rss, cca_busy)
    }

    #[test]
    fn acknowledged_irq_decodes_measurement_for_the_active_phase() {
        let energy = AcknowledgedMacEventBatch::from_interrupt(
            interrupt(Ieee802154Event::EdDone.bit(), 0, 0, -42, true),
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
            interrupt(Ieee802154Event::EdDone.bit(), 0, 0, -1, true),
            MacActivePhase::ClearChannelAssessment,
        )
        .unwrap();
        assert_eq!(
            cca.batch.measurement(),
            Some(MacMeasurementSample::ClearChannel(MacCcaSample::Busy))
        );
    }

    #[test]
    fn acknowledged_irq_rejects_unknown_bits_and_abort_reasons() {
        assert!(matches!(
            AcknowledgedMacEventBatch::from_interrupt(
                interrupt(1 << 7, 0, 0, 0, false),
                MacActivePhase::ClearChannelAssessment,
            ),
            Err(MacInterruptBatchError::UnsupportedEventBits {
                raw_event_bits: 0x80,
                unsupported_bits: 0x80,
            })
        ));
        assert!(matches!(
            AcknowledgedMacEventBatch::from_interrupt(
                interrupt(Ieee802154Event::RxAbort.bit(), 31, 0, 0, false),
                MacActivePhase::Receive,
            ),
            Err(MacInterruptBatchError::UnknownRxAbortReason { code: 31 })
        ));
    }

    #[test]
    fn transmit_with_ack_executes_the_complete_plan_in_exact_order() {
        let mut tx = tx_owner(DMA_LOW);
        let armed_tx = arm_with_ack(&mut tx, &[0x21]);
        let rx = rx_pool::<1>(DMA_LOW + 128);
        let armed_rx = rx.arm_next().unwrap();
        let actor = MacReady::new().request_transmit_with_ack(
            armed_tx,
            armed_rx,
            MacTransmitAccess::Direct,
        );

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
            open_esp_radio_esp32s31_ieee802154_mac::MacEnergyDetectionDuration::from_hardware_units(
                37,
            ),
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
        let actor = MacReady::new().request_transmit_with_ack(
            armed_tx,
            armed_rx,
            MacTransmitAccess::Direct,
        );
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
            interrupt(Ieee802154Event::TxDone.bit(), 0, 0, 0, false),
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
            interrupt(Ieee802154Event::RxDone.bit(), 0, 0, 0, false),
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
            interrupt(Ieee802154Event::RxAbort.bit(), 2, 0, 0, false),
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
        let actor = MacReady::new().request_transmit_with_ack(
            armed_tx,
            armed_rx,
            MacTransmitAccess::Direct,
        );
        let active = runtime(FakeExecutor::new()).start(actor).unwrap();
        let tx_done = AcknowledgedMacEventBatch::from_interrupt(
            interrupt(Ieee802154Event::TxDone.bit(), 0, 0, 0, false),
            active.phase(),
        )
        .unwrap();
        let pending = match active.process_batch(tx_done).unwrap() {
            MacRuntimeBatchOutcome::Pending(active) => active,
            MacRuntimeBatchOutcome::Completed(_) => panic!("TX_DONE must await ACK"),
        };
        let timeout = AcknowledgedMacEventBatch::from_interrupt(
            interrupt(Ieee802154Event::Timer0Overflow.bit(), 0, 0, 0, false),
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
            MacBatchRejectReason::UnexpectedEventBits { .. }
        ));
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
}
