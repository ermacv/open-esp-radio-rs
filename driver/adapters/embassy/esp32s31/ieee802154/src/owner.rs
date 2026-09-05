//! Affine Embassy task and hard-IRQ owners.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_ieee802154_dma::{
    RxArm, TxAckNotRequested, TxAckRequested, TxArmed, TxCompleted,
};
use open_esp_radio_esp32s31_ieee802154_mac::{
    MacActive, MacCompletion, MacDeferredNext, MacEnergyDetectionDuration, MacNoDmaResources,
    MacReady, MacResolvedRx, MacResolvedTxWithAck, MacRxResolutionFailure, MacTransmitAccess,
    MacTxWithAckResolutionFailure, MacTxWithAckResources,
};
use open_esp_radio_esp32s31_ieee802154_runtime::{
    MacCommandExecutor, MacRuntime, MacRuntimeActive, MacRuntimeCompletion,
    MacRuntimeDmaResolutionFailure, MacRuntimeResolved, MacRuntimeResources,
    MacRuntimeStartFailure,
};

use crate::{
    EmbassyIeee802154IrqRuntime, EmbassyIeee802154Operation, EmbassyIeee802154OperationError,
    EmbassyIeee802154OperationProgress,
};

/// Ready owner for one genuine ESP32-S31 MAC command epoch.
///
/// This is the Embassy equivalent of the neighboring Wi-Fi composition's
/// owner boundary: the executor-neutral [`MacReady`] token and a sealed
/// task-side command runtime move together, while the hard-IRQ PAC port stays
/// in the ESP-HAL route owner. [`MacReady`] is a pure logical token, but
/// application code cannot manufacture the sealed executor. The
/// whole-radio transition must return that runtime only after PHY, BTBB,
/// coexistence, interrupt masks, and the CPU route are ready.
pub struct EmbassyIeee802154Ready<
    'irq,
    M: RawMutex,
    const DEPTH: usize,
    Executor: MacCommandExecutor,
> {
    runtime: MacRuntime<Executor>,
    ready: MacReady,
    irq: &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
}

impl<'irq, M: RawMutex, const DEPTH: usize, Executor: MacCommandExecutor>
    EmbassyIeee802154Ready<'irq, M, DEPTH, Executor>
{
    /// Join already-proved MAC readiness, its sealed executor, and one IRQ
    /// handoff without touching hardware.
    ///
    /// The target whole-radio transition is responsible for minting `runtime`;
    /// this adapter deliberately has no PAC/raw-address constructor and does
    /// not infer hardware readiness from reset state.
    pub const fn from_runtime(
        runtime: MacRuntime<Executor>,
        ready: MacReady,
        irq: &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
    ) -> Self {
        Self {
            runtime,
            ready,
            irq,
        }
    }

    /// Recover the untouched runtime, MAC-ready token, and IRQ handoff.
    pub fn into_parts(
        self,
    ) -> (
        MacRuntime<Executor>,
        MacReady,
        &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
    ) {
        (self.runtime, self.ready, self.irq)
    }

    /// Start receive with automatic acknowledgement disabled.
    pub fn receive_without_auto_ack<'pool, const COUNT: usize>(
        self,
        armed: RxArm<'pool, COUNT>,
    ) -> Result<
        EmbassyIeee802154Active<'irq, M, DEPTH, RxArm<'pool, COUNT>, Executor>,
        MacRuntimeStartFailure<RxArm<'pool, COUNT>, Executor>,
    > {
        let Self {
            runtime,
            ready,
            irq,
        } = self;
        let active = ready.request_receive_without_auto_ack(armed);
        Self::start(runtime, irq, active)
    }

    /// Start transmit without waiting for an acknowledgement.
    #[allow(
        clippy::type_complexity,
        reason = "the return type retains the exact FCF-derived marker and affine owners"
    )]
    pub fn transmit_without_ack<'owner>(
        self,
        armed: TxArmed<'owner, TxAckNotRequested>,
        access: MacTransmitAccess,
    ) -> Result<
        EmbassyIeee802154Active<'irq, M, DEPTH, TxArmed<'owner, TxAckNotRequested>, Executor>,
        MacRuntimeStartFailure<TxArmed<'owner, TxAckNotRequested>, Executor>,
    > {
        let Self {
            runtime,
            ready,
            irq,
        } = self;
        let active = ready.request_transmit_without_ack(armed, access);
        Self::start(runtime, irq, active)
    }

    /// Start transmit and retain one armed receive slot for the expected ACK.
    pub fn transmit_with_ack<'tx, 'rx, const COUNT: usize>(
        self,
        transmit: TxArmed<'tx, TxAckRequested>,
        acknowledgement_receive: RxArm<'rx, COUNT>,
        access: MacTransmitAccess,
    ) -> Result<
        EmbassyIeee802154Active<'irq, M, DEPTH, MacTxWithAckResources<'tx, 'rx, COUNT>, Executor>,
        MacRuntimeStartFailure<MacTxWithAckResources<'tx, 'rx, COUNT>, Executor>,
    > {
        let Self {
            runtime,
            ready,
            irq,
        } = self;
        let active = ready.request_transmit_with_ack(transmit, acknowledgement_receive, access);
        Self::start(runtime, irq, active)
    }

    /// Start one standalone clear-channel assessment.
    pub fn clear_channel_assessment(
        self,
    ) -> Result<
        EmbassyIeee802154Active<'irq, M, DEPTH, MacNoDmaResources, Executor>,
        MacRuntimeStartFailure<MacNoDmaResources, Executor>,
    > {
        let Self {
            runtime,
            ready,
            irq,
        } = self;
        let active = ready.request_clear_channel_assessment();
        Self::start(runtime, irq, active)
    }

    /// Start one standalone energy-detection measurement.
    pub fn energy_detection(
        self,
        duration: MacEnergyDetectionDuration,
    ) -> Result<
        EmbassyIeee802154Active<'irq, M, DEPTH, MacNoDmaResources, Executor>,
        MacRuntimeStartFailure<MacNoDmaResources, Executor>,
    > {
        let Self {
            runtime,
            ready,
            irq,
        } = self;
        let active = ready.request_energy_detection(duration);
        Self::start(runtime, irq, active)
    }

    fn start<Resources: MacRuntimeResources>(
        runtime: MacRuntime<Executor>,
        irq: &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
        active: MacActive<Resources>,
    ) -> Result<
        EmbassyIeee802154Active<'irq, M, DEPTH, Resources, Executor>,
        MacRuntimeStartFailure<Resources, Executor>,
    > {
        let active = runtime.start(active)?;
        Ok(EmbassyIeee802154Active::new(active, irq))
    }
}

/// Cancellation-safe Embassy owner of one active MAC operation.
///
/// The IRQ reference and affine runtime are stored together, so callers do not
/// have to re-pair an active DMA/command owner with an arbitrary event queue.
/// Cancelling [`Self::run`] drops only its mutable borrow and leaves the exact
/// active operation recoverable through [`Self::into_active`].
pub struct EmbassyIeee802154Active<
    'irq,
    M: RawMutex,
    const DEPTH: usize,
    Resources,
    Executor: MacCommandExecutor,
> {
    operation: EmbassyIeee802154Operation<Resources, Executor>,
    irq: &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
}

/// Reusable Embassy owner and terminal result of one CCA/ED request.
///
/// The command executor has crossed its acknowledged terminal boundary and is
/// carried inside `ready`; no PAC or DMA owner is left in the completed task.
#[must_use = "the returned ready owner is required for the next MAC operation"]
pub struct EmbassyIeee802154NoDmaResolved<
    'irq,
    M: RawMutex,
    const DEPTH: usize,
    Executor: MacCommandExecutor,
> {
    ready: EmbassyIeee802154Ready<'irq, M, DEPTH, Executor>,
    completion: MacCompletion,
    next: MacDeferredNext,
}

/// Reusable Embassy owner plus one CPU-owned terminal DMA result.
#[must_use = "the returned ready and DMA owners are required for later operations"]
pub struct EmbassyIeee802154DmaResolved<
    'irq,
    M: RawMutex,
    const DEPTH: usize,
    Reclaimed,
    Executor: MacCommandExecutor,
> {
    ready: EmbassyIeee802154Ready<'irq, M, DEPTH, Executor>,
    reclaimed: Reclaimed,
    completion: MacCompletion,
    next: MacDeferredNext,
}

impl<'irq, M: RawMutex, const DEPTH: usize, Reclaimed, Executor: MacCommandExecutor>
    EmbassyIeee802154DmaResolved<'irq, M, DEPTH, Reclaimed, Executor>
{
    /// Return the accepted terminal MAC result.
    pub const fn completion(&self) -> MacCompletion {
        self.completion
    }

    /// Return the deferred policy selected at the terminal boundary.
    pub const fn next(&self) -> MacDeferredNext {
        self.next
    }

    /// Borrow the reclaimed buffer ownership without separating it from the
    /// next-operation owner.
    pub const fn reclaimed(&self) -> &Reclaimed {
        &self.reclaimed
    }

    /// Split the next-operation owner, reclaimed buffers, and value-only
    /// terminal observations.
    pub fn into_parts(
        self,
    ) -> (
        EmbassyIeee802154Ready<'irq, M, DEPTH, Executor>,
        Reclaimed,
        MacCompletion,
        MacDeferredNext,
    ) {
        (self.ready, self.reclaimed, self.completion, self.next)
    }
}

/// Failure while driving and reclaiming one async DMA-backed operation.
pub enum EmbassyIeee802154DmaRunToReadyError<Failure, Executor: MacCommandExecutor> {
    /// IRQ handoff, decoding, or actor acceptance failed. Cancellation before
    /// an IRQ preserves the active owner; an error after consumption
    /// quarantines it inside the Embassy operation.
    Operation(EmbassyIeee802154OperationError),
    /// A terminal batch was accepted, but the DMA lifecycle transition failed
    /// and quarantined the command and buffer owners.
    Resolution(MacRuntimeDmaResolutionFailure<Failure, Executor>),
}

impl<'irq, M: RawMutex, const DEPTH: usize, Executor: MacCommandExecutor>
    EmbassyIeee802154NoDmaResolved<'irq, M, DEPTH, Executor>
{
    /// Return the terminal logical MAC result.
    pub const fn completion(&self) -> MacCompletion {
        self.completion
    }

    /// Return the deferred policy selected at the terminal boundary.
    pub const fn next(&self) -> MacDeferredNext {
        self.next
    }

    /// Split the next-operation owner from value-only completion data.
    pub fn into_parts(
        self,
    ) -> (
        EmbassyIeee802154Ready<'irq, M, DEPTH, Executor>,
        MacCompletion,
        MacDeferredNext,
    ) {
        (self.ready, self.completion, self.next)
    }
}

impl<'irq, M: RawMutex, const DEPTH: usize, Resources, Executor: MacCommandExecutor>
    EmbassyIeee802154Active<'irq, M, DEPTH, Resources, Executor>
{
    const fn new(
        active: MacRuntimeActive<Resources, Executor>,
        irq: &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
    ) -> Self {
        Self {
            operation: EmbassyIeee802154Operation::new(active),
            irq,
        }
    }

    /// Return whether the operation has not yet produced a terminal event.
    pub const fn is_active(&self) -> bool {
        self.operation.is_active()
    }

    /// Await and process exactly one acknowledged hard-IRQ snapshot.
    pub async fn advance(
        &mut self,
    ) -> Result<
        EmbassyIeee802154OperationProgress<Resources, Executor>,
        EmbassyIeee802154OperationError,
    > {
        self.operation.advance(self.irq).await
    }

    /// Drive the operation to a terminal MAC event.
    ///
    /// Cancellation preserves the active runtime inside `self`.
    pub async fn run(
        &mut self,
    ) -> Result<MacRuntimeCompletion<Resources, Executor>, EmbassyIeee802154OperationError> {
        self.operation.run(self.irq).await
    }

    /// Recover the exact active runtime only after cancellation while the IRQ
    /// await was still pending.
    ///
    /// An acknowledged/lost/invalid IRQ or actor rejection irreversibly
    /// quarantines the runtime and DMA resources, so this returns `None` after
    /// such an operation error.
    pub fn into_active(self) -> Option<MacRuntimeActive<Resources, Executor>> {
        self.operation.into_active()
    }
}

impl<'irq, 'owner, M: RawMutex, const DEPTH: usize, Executor: MacCommandExecutor>
    EmbassyIeee802154Active<'irq, M, DEPTH, TxArmed<'owner, TxAckNotRequested>, Executor>
{
    /// Drive a no-ACK transmit through its accepted terminal IRQ batch,
    /// reclaim the TX image, and return an owner ready for another command.
    ///
    /// Cancelling this future preserves the active runtime in `self`.
    pub async fn run_to_ready(
        &mut self,
        next: MacDeferredNext,
    ) -> Result<
        EmbassyIeee802154DmaResolved<'irq, M, DEPTH, TxCompleted<'owner>, Executor>,
        EmbassyIeee802154OperationError,
    > {
        let completed = self.run().await?;
        Ok(embassy_dma_resolved(completed.resolve(next), self.irq))
    }
}

impl<'irq, 'pool, M: RawMutex, const DEPTH: usize, const COUNT: usize, Executor: MacCommandExecutor>
    EmbassyIeee802154Active<'irq, M, DEPTH, RxArm<'pool, COUNT>, Executor>
{
    /// Drive receive through `RX_DONE` or a reviewed terminal abort, then
    /// reclaim its exact DMA destination.
    ///
    /// A received frame stays owned by the result until explicitly recycled;
    /// this permits the caller to inspect its validated PHR layout and arm a
    /// different free pool slot. Abort results never expose frame bytes.
    pub async fn run_to_ready(
        &mut self,
        next: MacDeferredNext,
    ) -> Result<
        EmbassyIeee802154DmaResolved<'irq, M, DEPTH, MacResolvedRx<'pool, COUNT>, Executor>,
        EmbassyIeee802154DmaRunToReadyError<MacRxResolutionFailure<'pool, COUNT>, Executor>,
    > {
        let completed = self
            .run()
            .await
            .map_err(EmbassyIeee802154DmaRunToReadyError::Operation)?;
        let resolved = completed
            .resolve(next)
            .map_err(EmbassyIeee802154DmaRunToReadyError::Resolution)?;
        Ok(embassy_dma_resolved(resolved, self.irq))
    }
}

impl<
    'irq,
    'tx,
    'rx,
    M: RawMutex,
    const DEPTH: usize,
    const COUNT: usize,
    Executor: MacCommandExecutor,
> EmbassyIeee802154Active<'irq, M, DEPTH, MacTxWithAckResources<'tx, 'rx, COUNT>, Executor>
{
    /// Drive a transmit-with-ACK through its accepted terminal batch and
    /// reclaim both its TX image and paired ACK receive destination.
    ///
    /// Only `ACK_RX_DONE` exposes an ACK frame. Timeout and abort outcomes
    /// return the RX destination as typed non-frame ownership for recycling.
    pub async fn run_to_ready(
        &mut self,
        next: MacDeferredNext,
    ) -> Result<
        EmbassyIeee802154DmaResolved<
            'irq,
            M,
            DEPTH,
            MacResolvedTxWithAck<'tx, 'rx, COUNT>,
            Executor,
        >,
        EmbassyIeee802154DmaRunToReadyError<
            MacTxWithAckResolutionFailure<'tx, 'rx, COUNT>,
            Executor,
        >,
    > {
        let completed = self
            .run()
            .await
            .map_err(EmbassyIeee802154DmaRunToReadyError::Operation)?;
        let resolved = completed
            .resolve(next)
            .map_err(EmbassyIeee802154DmaRunToReadyError::Resolution)?;
        Ok(embassy_dma_resolved(resolved, self.irq))
    }
}

fn embassy_dma_resolved<
    'irq,
    M: RawMutex,
    const DEPTH: usize,
    Reclaimed,
    Executor: MacCommandExecutor,
>(
    resolved: MacRuntimeResolved<Reclaimed, Executor>,
    irq: &'irq EmbassyIeee802154IrqRuntime<M, DEPTH>,
) -> EmbassyIeee802154DmaResolved<'irq, M, DEPTH, Reclaimed, Executor> {
    let (runtime, ready, reclaimed, completion, next) = resolved.into_parts();
    EmbassyIeee802154DmaResolved {
        ready: EmbassyIeee802154Ready::from_runtime(runtime, ready, irq),
        reclaimed,
        completion,
        next,
    }
}

impl<'irq, M: RawMutex, const DEPTH: usize, Executor: MacCommandExecutor>
    EmbassyIeee802154Active<'irq, M, DEPTH, MacNoDmaResources, Executor>
{
    /// Run one CCA/ED request and return an owner ready for the next command.
    ///
    /// Cancelling this future preserves the active runtime in `self`. Success
    /// leaves `self` inactive and moves the sealed executor into the returned
    /// [`EmbassyIeee802154NoDmaResolved`].
    pub async fn run_to_ready(
        &mut self,
        next: MacDeferredNext,
    ) -> Result<
        EmbassyIeee802154NoDmaResolved<'irq, M, DEPTH, Executor>,
        EmbassyIeee802154OperationError,
    > {
        let completed = self.run().await?;
        let resolved = completed.resolve(next);
        let (runtime, ready, _no_dma, completion, next) = resolved.into_parts();
        Ok(EmbassyIeee802154NoDmaResolved {
            ready: EmbassyIeee802154Ready::from_runtime(runtime, ready, self.irq),
            completion,
            next,
        })
    }
}
