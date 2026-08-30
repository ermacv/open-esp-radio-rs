//! Controller-output and runtime-timer activation after BLE PHY initialization.

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptOutputPreparedOwner, BluetoothInterruptRegistersOwner,
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCounterStartedOwner,
    BluetoothModemLpTimerInterruptReadyOwner, BluetoothModemLpTimerSoftwarePendingOwner,
    BluetoothSchedulerRunInterruptsPrepared,
};

#[cfg(target_arch = "riscv32")]
use crate::controller_time::{
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError,
    BluetoothPostEnableTimeOrphanStep, BluetoothPostEnableTimeOwner,
    BluetoothPostEnableTimeOwnerStep, BluetoothPostEnableTimePendingCore,
    BluetoothPostEnableTimePendingCoreStep, drain_post_enable_time_orphan,
};
#[cfg(target_arch = "riscv32")]
use crate::{
    BluetoothControllerBlePhyEngineInitialized, BluetoothModemLpTimerEventCell,
    BluetoothModemLpTimerEventPublication, BluetoothModemLpTimerExpiration,
    BluetoothModemLpTimerExpirationPending, BluetoothModemLpTimerPublishedInterruptStep,
    BluetoothModemLpTimerSoftwareStep, BluetoothModemLpTimerSoftwareWork,
    BluetoothModemLpTimerStableInterruptStep, BluetoothNrtDefaultInterruptEpoch,
    BluetoothPrimaryInterruptStep, BluetoothPrimaryPublishedInterruptStep,
};

/// Powered Controller after IRQ-output preparation and runtime-timer start.
///
/// This state retains the complete BLE PHY epoch, the prepared-but-unrouted
/// interrupt partition and the uniquely started low-power timer. It does not
/// claim stable ISR storage, a CPU route, scheduler activation or operational
/// Link-Layer work.
#[must_use = "the started Bluetooth Controller retains every hardware owner"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerOutputTimerStarted<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    pub(crate) initialized: BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _interrupt_output: BluetoothInterruptOutputPreparedOwner,
    pub(crate) timer: BluetoothModemLpTimerCounterStartedOwner,
}

/// Powered Controller with both register owners ready for ISR publication.
///
/// The controller interrupt partition and source-127 timer partition have
/// crossed their final no-MMIO ownership transitions. They remain movable and
/// no CPU route is active; the next platform composition must publish both in
/// stable ISR storage before it enables any of the three routes.
#[must_use = "the prepared Bluetooth interrupt owners must be published before routing"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerInterruptOwnersReady<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    initialized: BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _interrupts: BluetoothInterruptRegistersOwner,
    _timer: BluetoothModemLpTimerInterruptReadyOwner,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

/// Platform boundary that publishes both disjoint owners in stable ISR slots.
///
/// Implementations must either publish both owners atomically and return one
/// affine lease, or return the storage value and both unchanged owners. This
/// transition must not enable a CPU route; routing is a later lifecycle edge.
#[cfg(target_arch = "riscv32")]
pub trait BluetoothInterruptOwnerStorage: Sized {
    /// Affine proof that both owners remain in the implementation's storage.
    type Published;
    /// Exact pre-publication rejection reason.
    type Error;

    /// Publish both owners without enabling any interrupt source.
    fn publish(
        self,
        interrupts: BluetoothInterruptRegistersOwner,
        timer: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<
        Self::Published,
        (
            Self::Error,
            Self,
            BluetoothInterruptRegistersOwner,
            BluetoothModemLpTimerInterruptReadyOwner,
        ),
    >;
}

/// Stable-storage boundary for source-127 task ownership.
///
/// The interrupt platform implements this trait for the same affine lease
/// returned by [`BluetoothInterruptOwnerStorage`]. Taking an owner leaves the
/// stable ISR slot empty, so repeated interrupt entry cannot touch MMIO while
/// task work owns the timer. Only a fully rearmed owner may be restored.
#[cfg(target_arch = "riscv32")]
pub trait BluetoothModemLpTimerSoftwareOwnerStorage {
    /// Exact reason task context could not acquire pending work.
    type TakeError;
    /// Exact reason the fully rearmed owner could not return to ISR storage.
    type RestoreError;

    /// Move software-pending ownership out of stable ISR storage.
    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<BluetoothModemLpTimerSoftwarePendingOwner, Self::TakeError>;

    /// Restore only a fully rearmed owner, retaining it on rejection.
    fn restore_modem_lp_timer_ready(
        &self,
        owner: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, BluetoothModemLpTimerInterruptReadyOwner)>;
}

/// Stable platform dispatch over the published source-127 interrupt owner.
///
/// Implementations retain either the ready or software-pending affine owner
/// in process-wide storage and perform no executor notification themselves.
#[cfg(target_arch = "riscv32")]
pub trait BluetoothModemLpTimerInterruptDispatchStorage {
    /// Exact reason the stable owner could not service this entry.
    type Error;

    /// Execute one finite register entry and return its semantic disposition.
    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<BluetoothModemLpTimerStableInterruptStep, Self::Error>;
}

/// Stable platform dispatch over the published shared interrupt owner.
///
/// Implementations must retain the unique primary/NRT register owner in
/// stable storage across every call. Both methods execute exactly one finite
/// Controller disposition and enable no CPU route themselves.
#[cfg(target_arch = "riscv32")]
pub trait BluetoothSharedInterruptDispatchStorage {
    /// Exact reason the shared owner could not service an entry.
    type Error;

    /// Capture, acknowledge and classify one primary source-124 epoch.
    fn service_primary_interrupt(&self) -> Result<BluetoothPrimaryInterruptStep, Self::Error>;

    /// Capture and acknowledge one default-profile NRT source-133 epoch.
    fn service_nrt_default_interrupt(
        &self,
    ) -> Result<BluetoothNrtDefaultInterruptEpoch, Self::Error>;
}

/// Stable task-side access to the shared interrupt owner for scheduler start.
///
/// This is deliberately separate from hard-handler dispatch. Implementations
/// must execute the finite dynamic interrupt preparation synchronously while
/// retaining the owner in the same stable slot. The Controller state that
/// calls it guarantees that CPU routes are still inactive.
#[cfg(target_arch = "riscv32")]
pub trait BluetoothSchedulerRunInterruptStorage {
    /// Exact reason the stable owner could not prepare scheduler interrupts.
    type Error;

    /// Clear stale dynamic sources and enable the scheduler-run groups.
    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error>;
}

/// Failed DTM scheduler start before the synchronous run suffix began.
///
/// The published head is returned unchanged. Once interrupt preparation
/// succeeds, every remaining operation is infallible and ownership advances
/// directly to [`crate::BluetoothDtmSchedulerRunning`].
#[must_use = "a failed DTM scheduler start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmSchedulerStartFailure<Role, E> {
    error: E,
    head: crate::BluetoothDtmSchedulerHeadPublished<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role, E> BluetoothDtmSchedulerStartFailure<Role, E> {
    /// Inspect the exact stable-storage rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged published DTM head.
    pub fn into_parts(self) -> (E, crate::BluetoothDtmSchedulerHeadPublished<Role>) {
        (self.error, self.head)
    }
}

/// Stable-dispatch failure retaining the already-unlinked DTM graph.
#[must_use = "a dispatch failure must retain the unlinked graph for a later event"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmSoftwareListRemovalObservationFailure<Role, E> {
    error: E,
    unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role, E> BluetoothDtmSoftwareListRemovalObservationFailure<Role, E> {
    /// Inspect the exact stable-dispatch rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged unlinked DTM graph.
    pub fn into_parts(self) -> (E, crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>) {
        (self.error, self.unlinked)
    }
}

/// Controller result of one fresh post-unlink primary-event observation.
#[must_use = "every outcome retains the exact unlinked or removal-ready graph"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmSoftwareListRemovalObservationStep<Role> {
    /// The primary epoch reported a baseline or unclassified fault.
    Fault {
        /// Already-unlinked graph retained for fail-stop handling.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact primary controller fault.
        fault: crate::BluetoothPrimaryControllerFault,
    },
    /// The acknowledged epoch contained no reviewed scheduler work.
    NoSchedulerWork {
        /// Already-unlinked graph retained for a later event.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact acknowledged empty primary epoch.
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    /// Selector-6 recovery blocked the later fresh scheduler-state read.
    ReferenceRecoveryRequired {
        /// Already-unlinked graph retained while recovery remains blocked.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact affine selector-6 recovery requirement.
        required: crate::BluetoothPrimaryReferenceRecoveryRequired,
    },
    /// The fresh scheduler sample or task command statuses were not ready.
    Pending {
        /// Already-unlinked graph retained for another external event.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Durable scheduler-wake publication from this exact event.
        scheduler: crate::BluetoothSchedulerWakePublication,
        /// Durable lock/modify publication from this exact event.
        lock_modify: crate::BluetoothSchedulerLockModifyEventPublication,
    },
    /// The graph belongs to another Controller scheduler epoch.
    SchedulerIdentityMismatch {
        /// Unchanged already-unlinked graph.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Durable scheduler-wake publication from this exact event.
        scheduler: crate::BluetoothSchedulerWakePublication,
        /// Durable lock/modify publication from this exact event.
        lock_modify: crate::BluetoothSchedulerLockModifyEventPublication,
    },
    /// The complete post-unlink return predicate became ready.
    Ready {
        /// Exact removal-ready graph; CPU ownership is still not returned.
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
        /// Durable scheduler-wake publication from this exact event.
        scheduler: crate::BluetoothSchedulerWakePublication,
        /// Durable lock/modify publication from this exact event.
        lock_modify: crate::BluetoothSchedulerLockModifyEventPublication,
    },
}

#[cfg(target_arch = "riscv32")]
enum BluetoothControllerModemLpTimerSoftwarePhase<'runtime, const CAPACITY: usize> {
    Work(BluetoothModemLpTimerSoftwareWork<'runtime, CAPACITY>),
    Expiration(BluetoothModemLpTimerExpirationPending<'runtime, CAPACITY>),
}

/// One borrowed source-127 task epoch tied to Controller runtime and ISR storage.
///
/// This value retains the unique HAL owner, mutable queue/epoch borrows and the
/// stable-storage lease. It can advance only one bounded queue or publication
/// edge per [`step`](Self::step) call.
#[must_use = "source-127 task ownership must reach publication and rearm"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerModemLpTimerSoftwareWork<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothModemLpTimerSoftwareOwnerStorage,
{
    phase: BluetoothControllerModemLpTimerSoftwarePhase<'runtime, CAPACITY>,
    events: &'runtime BluetoothModemLpTimerEventCell,
    storage: &'runtime S,
}

/// Result of one Controller-level source-127 task step.
#[must_use = "retain the returned task work or the failed ready owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerModemLpTimerSoftwareStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothModemLpTimerSoftwareOwnerStorage,
{
    /// One due expiration now owns the publication edge.
    ExpirationPending(BluetoothControllerModemLpTimerSoftwareWork<'runtime, S, CAPACITY>),
    /// The durable cell accepted one expiration and task work may continue.
    Published {
        /// Retained timer work after successful publication.
        work: BluetoothControllerModemLpTimerSoftwareWork<'runtime, S, CAPACITY>,
        /// Wake disposition for the event consumer.
        publication: BluetoothModemLpTimerEventPublication,
    },
    /// The event cell is occupied; no expiration or owner was lost.
    Backpressured(BluetoothControllerModemLpTimerSoftwareWork<'runtime, S, CAPACITY>),
    /// Immediate compare programming requires one later fresh recheck.
    Recheck(BluetoothControllerModemLpTimerSoftwareWork<'runtime, S, CAPACITY>),
    /// Queue work completed and the ready owner returned to stable ISR storage.
    Rearmed,
    /// Stable storage rejected the fully rearmed owner and it remains retained.
    RestoreFailed(BluetoothControllerModemLpTimerRestoreFailure<S::RestoreError>),
}

/// Failed source-127 stable restore retaining the unique ready owner.
#[must_use = "a failed source-127 restore still owns the timer registers"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerModemLpTimerRestoreFailure<E> {
    error: E,
    owner: BluetoothModemLpTimerInterruptReadyOwner,
}

#[cfg(target_arch = "riscv32")]
impl<E> BluetoothControllerModemLpTimerRestoreFailure<E> {
    /// Inspect the exact platform rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the platform error and unchanged ready owner.
    pub fn into_parts(self) -> (E, BluetoothModemLpTimerInterruptReadyOwner) {
        (self.error, self.owner)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize>
    BluetoothControllerModemLpTimerSoftwareWork<'runtime, S, CAPACITY>
where
    S: BluetoothModemLpTimerSoftwareOwnerStorage,
{
    /// Inspect an expiration blocked on durable publication, if any.
    pub const fn pending_expiration(&self) -> Option<BluetoothModemLpTimerExpiration> {
        match &self.phase {
            BluetoothControllerModemLpTimerSoftwarePhase::Work(_) => None,
            BluetoothControllerModemLpTimerSoftwarePhase::Expiration(pending) => {
                Some(pending.event())
            }
        }
    }

    /// Execute one bounded queue, publication or rearm edge.
    pub fn step(self) -> BluetoothControllerModemLpTimerSoftwareStep<'runtime, S, CAPACITY> {
        let Self {
            phase,
            events,
            storage,
        } = self;
        match phase {
            BluetoothControllerModemLpTimerSoftwarePhase::Work(work) => match work.step() {
                BluetoothModemLpTimerSoftwareStep::Expiration(pending) => {
                    BluetoothControllerModemLpTimerSoftwareStep::ExpirationPending(Self {
                        phase: BluetoothControllerModemLpTimerSoftwarePhase::Expiration(pending),
                        events,
                        storage,
                    })
                }
                BluetoothModemLpTimerSoftwareStep::Recheck(work) => {
                    BluetoothControllerModemLpTimerSoftwareStep::Recheck(Self {
                        phase: BluetoothControllerModemLpTimerSoftwarePhase::Work(work),
                        events,
                        storage,
                    })
                }
                BluetoothModemLpTimerSoftwareStep::Rearmed(owner) => {
                    match storage.restore_modem_lp_timer_ready(owner) {
                        Ok(()) => BluetoothControllerModemLpTimerSoftwareStep::Rearmed,
                        Err((error, owner)) => {
                            BluetoothControllerModemLpTimerSoftwareStep::RestoreFailed(
                                BluetoothControllerModemLpTimerRestoreFailure { error, owner },
                            )
                        }
                    }
                }
            },
            BluetoothControllerModemLpTimerSoftwarePhase::Expiration(pending) => {
                match pending.publish(events) {
                    Ok((work, publication)) => {
                        BluetoothControllerModemLpTimerSoftwareStep::Published {
                            work: Self {
                                phase: BluetoothControllerModemLpTimerSoftwarePhase::Work(work),
                                events,
                                storage,
                            },
                            publication,
                        }
                    }
                    Err(pending) => {
                        BluetoothControllerModemLpTimerSoftwareStep::Backpressured(Self {
                            phase: BluetoothControllerModemLpTimerSoftwarePhase::Expiration(
                                pending,
                            ),
                            events,
                            storage,
                        })
                    }
                }
            }
        }
    }
}

/// Powered Controller after atomic stable publication of both ISR owners.
///
/// The platform lease retains stable placement, but no CPU route is active and
/// no hard-handler entry is possible from this state. The nested Controller
/// also retains the affine standalone always-awake profile selection; that
/// marker performs no RF MMIO and supplies neither RF-ready nor time-pending
/// authority.
#[must_use = "published Bluetooth interrupt owners must remain retained through route setup"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerInterruptOwnersPublished<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    initialized: BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _storage: S,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
    scheduler_epoch: Option<crate::BluetoothControllerSchedulerEpoch>,
}

/// Why an affine post-enable controller-time acquisition did not start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothAlwaysAwakePostEnableTimeBeginError {
    /// The first live sample has already initialized this Controller's epoch.
    AlreadyInitialized,
    /// Another acquisition or abandoned hardware request is still active.
    Busy,
    /// The private worker and lower sticky owner disagreed at publication.
    OwnershipCollision,
    /// The private non-repeating request identity space was exhausted.
    GenerationExhausted,
    /// An earlier ownership mismatch already stopped this worker.
    Faulted,
}

#[cfg(target_arch = "riscv32")]
impl From<BluetoothControllerTimeRequestError> for BluetoothAlwaysAwakePostEnableTimeBeginError {
    fn from(error: BluetoothControllerTimeRequestError) -> Self {
        match error {
            BluetoothControllerTimeRequestError::Busy => Self::Busy,
            BluetoothControllerTimeRequestError::OwnershipCollision => Self::OwnershipCollision,
            BluetoothControllerTimeRequestError::GenerationExhausted => Self::GenerationExhausted,
            BluetoothControllerTimeRequestError::Faulted => Self::Faulted,
        }
    }
}

/// Fail-stop result of rechecking one affine post-enable time acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothAlwaysAwakePostEnableTimeError {
    /// The affine identity no longer matched the private worker owner.
    RequestMismatch,
    /// The lower sticky owner disappeared while the request was active.
    OwnershipLost,
    /// An earlier ownership mismatch already stopped this worker.
    Faulted,
}

#[cfg(target_arch = "riscv32")]
impl From<BluetoothControllerTimeEventError> for BluetoothAlwaysAwakePostEnableTimeError {
    fn from(error: BluetoothControllerTimeEventError) -> Self {
        match error {
            BluetoothControllerTimeEventError::RequestMismatch => Self::RequestMismatch,
            BluetoothControllerTimeEventError::OwnershipLost => Self::OwnershipLost,
            BluetoothControllerTimeEventError::Faulted => Self::Faulted,
        }
    }
}

/// Result of one bounded abandoned-request drain observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a waiting orphan requires one later bounded drain observation"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep {
    /// No abandoned request existed; no hardware was touched.
    Idle,
    /// Hardware still owns the abandoned request; arrange one later recheck.
    Waiting,
    /// The abandoned result was discarded and the worker is idle again.
    Drained,
}

/// One exact in-flight post-enable controller-time request.
///
/// This affine value borrows the complete published Controller, so no second
/// operation can use that Controller while the request is pending. Dropping or
/// cancelling it abandons the exact identity into the private orphan drain.
#[must_use = "recheck, cancel, or drop the exact post-enable time request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothAlwaysAwakePostEnableTimePending<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    core: BluetoothPostEnableTimePendingCore<
        &'controller mut BluetoothControllerInterruptOwnersPublished<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >,
}

/// Controller-bound proof that the post-enable latch request completed.
///
/// The sample remains private and inseparable from the exact borrowed
/// Controller. This is not an RF-ready instant: this path neither performs nor
/// proves a sleep/wake transition, and it applies no recovered RF-settling
/// interval.
#[must_use = "initialize the persistent scheduler epoch from the bound first-live sample"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothAlwaysAwakeTimeObservedAfterEnable<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    controller: &'controller mut BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    sample: crate::BluetoothControllerTimeSample,
}

/// Exact published Controller with one live sample bound to its retained epoch.
///
/// This affine owner is the only public entry into DTM preparation. It grants
/// neither RF-ready nor deadline-ready authority: role-specific RF readiness
/// and the later admission/sequence samples remain separate inputs. Consuming
/// any preparation attempt returns the same Controller borrow in the retained
/// epoch state, regardless of whether preparation succeeds.
#[must_use = "consume the epoch-bound live sample through one DTM preparation attempt"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerNowReady<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    controller: &'controller mut BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    epoch: crate::BluetoothControllerSchedulerEpoch,
    sample: crate::BluetoothControllerTimeSample,
}

/// Exact published Controller after its scheduler epoch has been initialized.
///
/// The epoch remains stored inside the same Controller and cannot detach or be
/// paired with another owner. This state carries no fresh current-time sample;
/// another source-owned observation is required before another preparation.
#[must_use = "the retained scheduler epoch owns the published Controller borrow"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerEpochRetained<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    controller: &'controller mut BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Result of rechecking one exact post-enable controller-time request.
#[must_use = "retain Waiting or consume the Controller-bound Ready proof"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothAlwaysAwakePostEnableTimeStep<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Hardware still owns the same request and complete Controller borrow.
    Waiting(
        BluetoothAlwaysAwakePostEnableTimePending<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ),
    /// The exact request completed with a Controller-bound private sample.
    Ready(
        BluetoothAlwaysAwakeTimeObservedAfterEnable<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ),
}

#[cfg(target_arch = "riscv32")]
impl<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothAlwaysAwakePostEnableTimePending<
        'controller,
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Perform exactly one observation of this exact latch request.
    pub fn recheck(
        self,
    ) -> Result<
        BluetoothAlwaysAwakePostEnableTimeStep<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothAlwaysAwakePostEnableTimeError,
    > {
        match self.core.recheck() {
            Ok(BluetoothPostEnableTimePendingCoreStep::Waiting(core)) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(Self {
                    core,
                }))
            }
            Ok(BluetoothPostEnableTimePendingCoreStep::Ready { owner, sample }) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(
                    BluetoothAlwaysAwakeTimeObservedAfterEnable {
                        controller: owner,
                        sample,
                    },
                ))
            }
            Err(failure) => {
                let (_controller, error) = failure.into_parts();
                Err(error.into())
            }
        }
    }

    /// Abandon this exact request and release the Controller borrow.
    ///
    /// The returned Controller cannot begin another acquisition until
    /// `drain_abandoned_always_awake_post_enable_time` reports `Drained` (or
    /// `Idle`). An ownership mismatch is returned explicitly and leaves the
    /// private worker fail-stop; the error variant retains no Controller borrow.
    pub fn cancel(
        self,
    ) -> Result<
        &'controller mut BluetoothControllerInterruptOwnersPublished<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothAlwaysAwakePostEnableTimeError,
    > {
        match self.core.cancel() {
            Ok(controller) => Ok(controller),
            Err(failure) => {
                let (_controller, error) = failure.into_parts();
                Err(error.into())
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> BluetoothPostEnableTimeOwner
    for BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    fn recheck_owned_post_enable_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothPostEnableTimeOwnerStep, BluetoothControllerTimeEventError> {
        match self
            .initialized
            .recheck_owned_post_enable_controller_time(request)
        {
            Ok(BluetoothControllerTimeEventStep::Waiting) => {
                Ok(BluetoothPostEnableTimeOwnerStep::Waiting)
            }
            Ok(BluetoothControllerTimeEventStep::Sample {
                request: completed,
                sample,
            }) if completed == request => Ok(BluetoothPostEnableTimeOwnerStep::Ready(sample)),
            Ok(
                BluetoothControllerTimeEventStep::Idle
                | BluetoothControllerTimeEventStep::OrphanDrained
                | BluetoothControllerTimeEventStep::Sample { .. },
            ) => Err(BluetoothControllerTimeEventError::RequestMismatch),
            Err(error) => Err(error),
        }
    }

    fn cancel_owned_post_enable_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        self.initialized
            .cancel_owned_post_enable_controller_time(request)
    }

    fn drain_orphan_post_enable_time(
        &mut self,
    ) -> Result<BluetoothPostEnableTimeOrphanStep, BluetoothControllerTimeEventError> {
        match self.initialized.drain_orphan_post_enable_controller_time() {
            Ok(BluetoothControllerTimeEventStep::Idle) => {
                Ok(BluetoothPostEnableTimeOrphanStep::Idle)
            }
            Ok(BluetoothControllerTimeEventStep::Waiting) => {
                Ok(BluetoothPostEnableTimeOrphanStep::Waiting)
            }
            Ok(BluetoothControllerTimeEventStep::OrphanDrained) => {
                Ok(BluetoothPostEnableTimeOrphanStep::Drained)
            }
            Ok(BluetoothControllerTimeEventStep::Sample { .. }) => {
                Err(BluetoothControllerTimeEventError::RequestMismatch)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothAlwaysAwakeTimeObservedAfterEnable<
        'controller,
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Initialize this Controller's persistent scheduler epoch from the first
    /// live post-enable sample.
    ///
    /// The same affine sample is retained as the sole current-time authority
    /// for one DTM preparation attempt. This transition proves neither RF
    /// readiness nor deadline readiness.
    pub fn initialize_scheduler_epoch(
        self,
    ) -> BluetoothControllerSchedulerNowReady<
        'controller,
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let epoch = crate::BluetoothControllerSchedulerEpoch::from_first_live_update(
            &self.sample,
            self.controller.initialized.controller_time_scale(),
        );
        self.controller.scheduler_epoch = Some(epoch);
        BluetoothControllerSchedulerNowReady {
            controller: self.controller,
            epoch,
            sample: self.sample,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerSchedulerEpochRetained<
        'controller,
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Release the exact Controller borrow for its ordinary lifecycle APIs.
    ///
    /// The scheduler epoch remains stored in the Controller. Consequently the
    /// cold initialization path remains rejected with `AlreadyInitialized`.
    pub fn into_controller(
        self,
    ) -> &'controller mut BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        self.controller
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    'controller,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerSchedulerNowReady<
        'controller,
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    fn into_parts(
        self,
    ) -> (
        &'controller mut BluetoothControllerInterruptOwnersPublished<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        crate::controller_time::BluetoothControllerSchedulerNow,
    ) {
        (
            self.controller,
            crate::controller_time::BluetoothControllerSchedulerNow::from_retained_epoch(
                self.epoch,
                self.sample,
            ),
        )
    }

    /// Prepare the sole initial transmitter item authorized by this live
    /// current-time sample.
    ///
    /// The returned owner retains the same Controller and persistent epoch on
    /// both success and failure. RF readiness, admission and sequence remain
    /// independently acquired authorities.
    #[expect(
        clippy::too_many_arguments,
        clippy::type_complexity,
        reason = "the affine return retains the exact generic Controller alongside the typed DTM result"
    )]
    pub fn prepare_dtm_transmitter_first_item(
        self,
        owner: crate::BluetoothDtmPreparedTxGraph,
        link_state: crate::BluetoothDtmLinkStateReset,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        requested_interval_micros: u16,
        margin: crate::BluetoothDtmSchedulerMargin,
        rf_ready: crate::BluetoothDtmSchedulerInstant,
        admission_sample: crate::BluetoothControllerTimeSample,
        sequence_sample: crate::BluetoothControllerTimeSample,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmTransmitterEvent,
                crate::BluetoothDtmInitialSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerTxPreparationFailure,
        >,
    ) {
        let (controller, now) = self.into_parts();
        let result = controller.initialized.prepare_dtm_transmitter_first_item(
            owner,
            link_state,
            channel,
            phy,
            requested_interval_micros,
            margin,
            now,
            rf_ready,
            admission_sample,
            sequence_sample,
        );
        (
            BluetoothControllerSchedulerEpochRetained { controller },
            result,
        )
    }

    /// Prepare the sole initial receiver item authorized by this live
    /// current-time sample.
    ///
    /// The returned owner retains the same Controller and persistent epoch on
    /// both success and failure. RF readiness, admission and sequence remain
    /// independently acquired authorities.
    #[expect(
        clippy::too_many_arguments,
        clippy::type_complexity,
        reason = "the affine return retains the exact generic Controller alongside the typed DTM result"
    )]
    pub fn prepare_dtm_receiver_first_item(
        self,
        owner: crate::BluetoothDtmReceiverCpuOwned,
        link_state: crate::BluetoothDtmLinkStateReset,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        margin: crate::BluetoothDtmSchedulerMargin,
        rf_ready: crate::BluetoothDtmSchedulerInstant,
        admission_sample: crate::BluetoothControllerTimeSample,
        sequence_sample: crate::BluetoothControllerTimeSample,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmReceiverEvent,
                crate::BluetoothDtmInitialSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerRxPreparationFailure,
        >,
    ) {
        let (controller, now) = self.into_parts();
        let result = controller.initialized.prepare_dtm_receiver_first_item(
            owner,
            link_state,
            channel,
            phy,
            margin,
            now,
            rf_ready,
            admission_sample,
            sequence_sample,
        );
        (
            BluetoothControllerSchedulerEpochRetained { controller },
            result,
        )
    }

    /// Prepare the sole recurring transmitter item authorized by this live
    /// current-time sample.
    ///
    /// The returned owner retains the same Controller and persistent epoch on
    /// both success and failure. Sequence authorization remains independent.
    #[expect(
        clippy::type_complexity,
        reason = "the affine return retains the exact generic Controller alongside the typed DTM result"
    )]
    pub fn prepare_dtm_transmitter_recurring_item(
        self,
        owner: crate::BluetoothDtmActiveTransmitterCpuOwned,
        sequence_sample: crate::BluetoothControllerTimeSample,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmTransmitterEvent,
                crate::BluetoothDtmRecurringSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerTxRecurringPreparationFailure,
        >,
    ) {
        let (controller, now) = self.into_parts();
        let result = controller
            .initialized
            .prepare_dtm_transmitter_recurring_item(owner, now, sequence_sample);
        (
            BluetoothControllerSchedulerEpochRetained { controller },
            result,
        )
    }

    /// Prepare the sole recurring receiver item authorized by this live
    /// current-time sample.
    ///
    /// The returned owner retains the same Controller and persistent epoch on
    /// both success and failure. RF readiness and sequence authorization remain
    /// independent.
    #[expect(
        clippy::type_complexity,
        reason = "the affine return retains the exact generic Controller alongside the typed DTM result"
    )]
    pub fn prepare_dtm_receiver_recurring_item(
        self,
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
        rf_ready: crate::BluetoothDtmSchedulerInstant,
        sequence_sample: crate::BluetoothControllerTimeSample,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<
            'controller,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmReceiverEvent,
                crate::BluetoothDtmRecurringSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerRxRecurringPreparationFailure,
        >,
    ) {
        let (controller, now) = self.into_parts();
        let result = controller.initialized.prepare_dtm_receiver_recurring_item(
            owner,
            now,
            rf_ready,
            sequence_sample,
        );
        (
            BluetoothControllerSchedulerEpochRetained { controller },
            result,
        )
    }
}

/// Failed stable publication retaining the complete Controller and storage.
#[must_use = "failed ISR publication returns every affine owner for inspection or retry"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerInterruptOwnerPublicationFailure<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
    S: BluetoothInterruptOwnerStorage,
{
    controller: BluetoothControllerInterruptOwnersReady<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    storage: S,
    error: S::Error,
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerOutputTimerStarted<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(&self) -> crate::BluetoothBlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::BluetoothBasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::BluetoothPhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Conditional runtime-control branch retained across the timer start.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.timer.runtime_control_observation()
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(&self) -> crate::BluetoothBlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::BluetoothBasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::BluetoothPhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Conditional runtime-control branch retained across publication.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Acquire source-127 task ownership from stable ISR storage.
    ///
    /// Success borrows this complete Controller state until queue work has
    /// durably published every expiration and restored the fully rearmed owner.
    /// The CPU routes remain inactive in this lifecycle state.
    pub fn begin_modem_lp_timer_software_work(
        &mut self,
    ) -> Result<
        BluetoothControllerModemLpTimerSoftwareWork<'_, S, MODEM_TIMER_CAPACITY>,
        S::TakeError,
    >
    where
        S: BluetoothModemLpTimerSoftwareOwnerStorage,
    {
        let owner = self._storage.take_modem_lp_timer_software_pending()?;
        self.initialized.modem_lp_timer_worker_wake().take();
        let (queue, epoch, events) = self.initialized.modem_lp_timer_software_parts_mut();
        Ok(BluetoothControllerModemLpTimerSoftwareWork {
            phase: BluetoothControllerModemLpTimerSoftwarePhase::Work(
                BluetoothModemLpTimerSoftwareWork::begin(queue, epoch, owner),
            ),
            events,
            storage: &self._storage,
        })
    }

    /// Begin one affine post-enable controller-time acquisition.
    ///
    /// The nested standalone always-awake selection gates publication through
    /// the complete BLE-PHY chain. The returned pending value borrows this exact
    /// Controller until it is rechecked, cancelled or dropped. Completion proves
    /// only that the latch request completed after enable. This path neither
    /// performs nor proves a sleep/wake transition, and no RF-settling interval
    /// is recovered here; the proof therefore is not RF-ready authority. Once
    /// its first live sample initializes the persistent scheduler epoch, this
    /// cold acquisition path rejects every later attempt as `AlreadyInitialized`.
    pub fn begin_always_awake_post_enable_time(
        &mut self,
    ) -> Result<
        BluetoothAlwaysAwakePostEnableTimePending<
            '_,
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothAlwaysAwakePostEnableTimeBeginError,
    > {
        if self.scheduler_epoch.is_some() {
            return Err(BluetoothAlwaysAwakePostEnableTimeBeginError::AlreadyInitialized);
        }
        let request = self
            .initialized
            .request_post_enable_controller_time()
            .map_err(BluetoothAlwaysAwakePostEnableTimeBeginError::from)?;
        Ok(BluetoothAlwaysAwakePostEnableTimePending {
            core: BluetoothPostEnableTimePendingCore::new(self, request),
        })
    }

    /// Perform one bounded observation of an abandoned post-enable request.
    ///
    /// `Waiting` requires one later call. A completed orphan is discarded and
    /// never becomes a sample or readiness instant for a subsequent operation.
    pub fn drain_abandoned_always_awake_post_enable_time(
        &mut self,
    ) -> Result<
        BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep,
        BluetoothAlwaysAwakePostEnableTimeError,
    > {
        match drain_post_enable_time_orphan(self) {
            Ok(BluetoothPostEnableTimeOrphanStep::Idle) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Idle)
            }
            Ok(BluetoothPostEnableTimeOrphanStep::Waiting) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Waiting)
            }
            Ok(BluetoothPostEnableTimeOrphanStep::Drained) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Drained)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Cancel one not-yet-published TX item through the same Controller.
    ///
    /// Success releases both the exclusive list and private timeline slot,
    /// returning ordinary graph ownership plus the complete TX program.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub fn cancel_dtm_transmitter_first_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        (
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphCpuOwned,
            crate::BluetoothDtmPayloadPattern,
            crate::BluetoothDtmPayloadLength,
        ),
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    > {
        self.initialized.cancel_dtm_transmitter_first_item(merged)
    }

    /// Cancel one not-yet-published RX item through the same Controller.
    ///
    /// Success releases both scheduling owners and returns the non-copyable
    /// graph/session aggregate unchanged.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub fn cancel_dtm_receiver_first_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        crate::BluetoothDtmReceiverCpuOwned,
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    > {
        self.initialized.cancel_dtm_receiver_first_item(merged)
    }

    /// Cancel one not-yet-published recurring TX item and recover its exact
    /// active command owner.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub fn cancel_dtm_transmitter_recurring_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        crate::BluetoothDtmActiveTransmitterCpuOwned,
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    > {
        self.initialized
            .cancel_dtm_transmitter_recurring_item(merged)
    }

    /// Cancel one not-yet-published recurring RX item and recover its exact
    /// active command owner.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub fn cancel_dtm_receiver_recurring_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        crate::BluetoothDtmActiveReceiverCpuOwned,
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    > {
        self.initialized.cancel_dtm_receiver_recurring_item(merged)
    }

    /// Publish the exact merge-selected DTM scheduler head while every CPU
    /// route is still inactive and both register owners reside in stable
    /// storage.
    ///
    /// Success advances descriptor ownership irreversibly: the returned graph
    /// can no longer be cancelled or mutated by CPU code. It does not yet
    /// prepare dynamic interrupts, publish the scheduler event or issue RUN.
    /// Both initial and recurring events cross this same list-head edge.
    #[expect(
        clippy::result_large_err,
        reason = "pre-MMIO rejection returns the complete affine DTM graph"
    )]
    pub fn publish_dtm_scheduler_head<Role, Phase>(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        crate::BluetoothDtmSchedulerHeadPublished<Role>,
        crate::BluetoothDtmSchedulerHeadPublicationFailure<Role, Phase>,
    >
    where
        Phase: crate::BluetoothDtmSchedulerItemPhase<Role>,
    {
        self.initialized.publish_dtm_scheduler_head(merged)
    }

    /// Admit one published DTM graph through the complete scheduler-run suffix.
    ///
    /// The exact order is dynamic interrupt preparation, synchronous BTMAC
    /// scheduler-event publication and the final RUN command. The returned
    /// state retains the graph and grants no CPU-side completion access.
    #[expect(
        clippy::result_large_err,
        reason = "a start rejection must return the complete affine published graph"
    )]
    pub fn start_dtm_scheduler<Role>(
        &mut self,
        head: crate::BluetoothDtmSchedulerHeadPublished<Role>,
    ) -> Result<
        crate::BluetoothDtmSchedulerRunning<Role>,
        BluetoothDtmSchedulerStartFailure<Role, S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let interrupts = match self._storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => return Err(BluetoothDtmSchedulerStartFailure { error, head }),
        };
        let address = head.scheduler_item_address();
        let (item, publication) = head.into_parts();
        let task = self.initialized.task_mut();
        let event = task.publish_scheduler_run_event(publication, interrupts);
        let run = task.publish_scheduler_hardware_run_command(event);
        self.initialized.retain_running_dtm_first_item(address);
        Ok(crate::BluetoothDtmSchedulerRunning::new(item, run))
    }

    /// Perform one fresh fenced completion-list transfer and immediately join
    /// its affine result to this exact running DTM graph.
    ///
    /// A non-sentinel item status advances only to completion-observed. The
    /// descriptor, packet and scheduler reservation remain hardware-owned
    /// until the later unlink and recycle transaction is complete.
    pub fn observe_dtm_completion<Role>(
        &mut self,
        running: crate::BluetoothDtmSchedulerRunning<Role>,
    ) -> crate::BluetoothDtmSchedulerCompletionStep<Role> {
        self.initialized.observe_dtm_completion(running)
    }

    /// Observe the post-picker hardware-list retirement barrier.
    ///
    /// This operation never clears or republishes the head. It performs one
    /// fresh typed read with a trailing device fence. Any nonempty result is a
    /// fail-stop invariant violation; the affine owner is retained only for
    /// diagnostic shutdown handling, never for a polling retry.
    pub fn observe_dtm_hardware_head_retirement<Role>(
        &mut self,
        completed: crate::BluetoothDtmSchedulerCompletionObserved<Role>,
    ) -> crate::BluetoothDtmSchedulerHardwareHeadRetirementStep<Role> {
        self.initialized
            .observe_dtm_hardware_head_retirement(completed)
    }

    /// Remove the sole empty-head DTM item from the source-owned software
    /// list exactly once.
    ///
    /// This is an ownership-only transition for the open scheduler; it does
    /// not recreate the vendor intrusive list or return descriptor access.
    pub fn unlink_dtm_software_list<Role>(
        &mut self,
        observed: crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> crate::BluetoothDtmSchedulerSoftwareListUnlinkStep<Role> {
        self.initialized.unlink_dtm_software_list(observed)
    }

    /// Service one fresh primary epoch after software-list unlink and join its
    /// exact scheduler-state sample to the finite task-side return gate.
    ///
    /// Busy and command-pending outcomes consume the event and return the
    /// already-unlinked graph. Retry therefore requires another external
    /// event; this method never polls. Selector-6 recovery remains fail-closed.
    #[expect(
        clippy::result_large_err,
        reason = "dispatch failure retains the complete unlinked graph and stable-owner error"
    )]
    pub fn observe_dtm_software_list_removal<Role>(
        &mut self,
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    ) -> Result<
        BluetoothDtmSoftwareListRemovalObservationStep<Role>,
        BluetoothDtmSoftwareListRemovalObservationFailure<Role, S::Error>,
    >
    where
        S: BluetoothSharedInterruptDispatchStorage,
    {
        let step = match self._storage.service_primary_interrupt() {
            Ok(step) => step,
            Err(error) => {
                return Err(BluetoothDtmSoftwareListRemovalObservationFailure { error, unlinked });
            }
        };
        let (scheduler_wake, lock_modify_events) =
            self.initialized.primary_interrupt_publications();
        match step.publish(scheduler_wake, lock_modify_events) {
            BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                Ok(BluetoothDtmSoftwareListRemovalObservationStep::Fault {
                    unlinked,
                    fault,
                })
            }
            BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                Ok(
                    BluetoothDtmSoftwareListRemovalObservationStep::NoSchedulerWork {
                        unlinked,
                        epoch,
                    },
                )
            }
            BluetoothPrimaryPublishedInterruptStep::ReferenceRecoveryRequired(required) => {
                Ok(
                    BluetoothDtmSoftwareListRemovalObservationStep::ReferenceRecoveryRequired {
                        unlinked,
                        required,
                    },
                )
            }
            BluetoothPrimaryPublishedInterruptStep::Scheduler {
                event,
                scheduler,
                lock_modify,
            } => match self
                .initialized
                .join_dtm_software_list_removal(unlinked, event)
            {
                crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch(
                    unlinked,
                ) => Ok(
                    BluetoothDtmSoftwareListRemovalObservationStep::SchedulerIdentityMismatch {
                        unlinked,
                        scheduler,
                        lock_modify,
                    },
                ),
                crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::Pending(
                    unlinked,
                ) => Ok(BluetoothDtmSoftwareListRemovalObservationStep::Pending {
                    unlinked,
                    scheduler,
                    lock_modify,
                }),
                crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::Ready(ready) => {
                    Ok(BluetoothDtmSoftwareListRemovalObservationStep::Ready {
                        ready,
                        scheduler,
                        lock_modify,
                    })
                }
            },
        }
    }

    /// Return TX or RX-non-success completion ownership to source-owned CPU
    /// state after the exact removal-ready transition.
    ///
    /// RX success is rejected into its separate drain/account/re-arm method.
    pub fn recycle_dtm_completed<Role>(
        &mut self,
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> crate::BluetoothDtmSchedulerRecycleStep<Role> {
        self.initialized.recycle_dtm_completed(ready)
    }

    /// Drain, account and re-arm one successful removal-ready receiver event.
    ///
    /// The returned chain is validated before mutation. Every rejection keeps
    /// the exact graph/session owner; success releases memory, timeline and
    /// source-list ownership before exposing the re-armed session.
    pub fn recycle_dtm_receiver_success(
        &mut self,
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<
            crate::BluetoothDtmReceiverEvent,
        >,
    ) -> crate::BluetoothDtmSchedulerRxSuccessRecycleStep {
        self.initialized.recycle_dtm_receiver_success(ready)
    }

    /// Service and durably publish one primary source-124 epoch.
    ///
    /// Both Controller cells are selected from this exact powered runtime.
    /// The returned wake dispositions can notify an executor later; pending
    /// state is already durable before this method returns.
    pub fn service_primary_interrupt(
        &self,
    ) -> Result<BluetoothPrimaryPublishedInterruptStep, S::Error>
    where
        S: BluetoothSharedInterruptDispatchStorage,
    {
        let step = self._storage.service_primary_interrupt()?;
        let (scheduler_wake, lock_modify_events) =
            self.initialized.primary_interrupt_publications();
        Ok(step.publish(scheduler_wake, lock_modify_events))
    }

    /// Service and durably publish one modem-timer source-127 epoch.
    ///
    /// A software-pending owner remains affine in stable platform storage.
    /// Its matching Controller wake cell is published before this method
    /// returns, so later task registration cannot lose the work request.
    pub fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<BluetoothModemLpTimerPublishedInterruptStep, S::Error>
    where
        S: BluetoothModemLpTimerInterruptDispatchStorage,
    {
        let step = self._storage.service_modem_lp_timer_interrupt()?;
        Ok(step.publish(self.initialized.modem_lp_timer_worker_wake()))
    }

    /// Service one opaque default-profile NRT source-133 epoch.
    ///
    /// The reviewed default path intentionally publishes no scheduler or
    /// Link-Layer work and keeps the shared owner in stable platform storage.
    pub fn service_nrt_default_interrupt(
        &self,
    ) -> Result<BluetoothNrtDefaultInterruptEpoch, S::Error>
    where
        S: BluetoothSharedInterruptDispatchStorage,
    {
        self._storage.service_nrt_default_interrupt()
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerInterruptOwnerPublicationFailure<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
    S: BluetoothInterruptOwnerStorage,
{
    /// Inspect the exact platform rejection.
    pub const fn error(&self) -> &S::Error {
        &self.error
    }

    /// Recover the complete pre-publication Controller and storage value.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerInterruptOwnersReady<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        S,
        S::Error,
    ) {
        (self.controller, self.storage, self.error)
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerInterruptOwnersReady<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(&self) -> crate::BluetoothBlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::BluetoothBasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::BluetoothPhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Conditional runtime-control branch retained by the ISR-ready timer.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Atomically publish both owners in caller-selected stable ISR storage.
    ///
    /// Rejection occurs before publication and returns this exact state plus
    /// the storage capability. Success still leaves every CPU route inactive.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return every affine powered owner"
    )]
    #[expect(
        clippy::type_complexity,
        reason = "the return type preserves the exact Controller and platform-storage states"
    )]
    pub fn publish_interrupt_owners<S>(
        self,
        storage: S,
    ) -> Result<
        BluetoothControllerInterruptOwnersPublished<
            P,
            M,
            S::Published,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerInterruptOwnerPublicationFailure<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >
    where
        S: BluetoothInterruptOwnerStorage,
    {
        let Self {
            initialized,
            _interrupts: interrupts,
            _timer: timer,
            runtime_control,
        } = self;
        match storage.publish(interrupts, timer) {
            Ok(published) => Ok(BluetoothControllerInterruptOwnersPublished {
                initialized,
                _storage: published,
                runtime_control,
                scheduler_epoch: None,
            }),
            Err((error, storage, interrupts, timer)) => {
                Err(BluetoothControllerInterruptOwnerPublicationFailure {
                    controller: BluetoothControllerInterruptOwnersReady {
                        initialized,
                        _interrupts: interrupts,
                        _timer: timer,
                        runtime_control,
                    },
                    storage,
                    error,
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerOutputTimerStarted<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Transfer both disjoint register owners into their pre-route states.
    ///
    /// This is an ownership-only transition. It performs no MMIO and does not
    /// claim stable placement or a live interrupt epoch.
    pub fn stage_interrupt_owners(
        self,
    ) -> BluetoothControllerInterruptOwnersReady<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let Self {
            initialized,
            _interrupt_output: interrupt_output,
            timer,
        } = self;
        let runtime_control = timer.runtime_control_observation();
        BluetoothControllerInterruptOwnersReady {
            initialized,
            _interrupts: interrupt_output.stage_for_cpu_routes(),
            _timer: timer.stage_for_interrupt(),
            runtime_control,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Prepare Controller IRQ output and then start the runtime timer once.
    ///
    /// The consuming BLE-PHY state proves that controller HAL, scheduler,
    /// HCI, low-power hardware, common PHY, BTBB and BLE PHY initialization
    /// all belong to this epoch. CPU routes remain inaccessible, so the lower
    /// unsafe interrupt prerequisite is discharged here and never exported.
    #[allow(
        unsafe_code,
        reason = "the complete Controller typestate proves the HAL interrupt prerequisites"
    )]
    pub fn prepare_controller_output_and_start_runtime_timer(
        mut self,
    ) -> BluetoothControllerOutputTimerStarted<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let (interrupts, timer) = self.take_activation_owners();
        let (interrupt_output, timer) = prepare_output_then_start_timer(
            interrupts,
            timer,
            |interrupts| {
                // SAFETY: `self` retains the matching complete powered
                // Controller epoch and no CPU-route owner has been exposed.
                unsafe { interrupts.prepare_controller_output() }
            },
            |timer| timer.start_runtime_timer(),
        );

        BluetoothControllerOutputTimerStarted {
            initialized: self,
            _interrupt_output: interrupt_output,
            timer,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn prepare_output_then_start_timer<Interrupt, Timer, Output, Started>(
    interrupt: Interrupt,
    timer: Timer,
    prepare_output: impl FnOnce(Interrupt) -> Output,
    start_timer: impl FnOnce(Timer) -> Started,
) -> (Output, Started) {
    let output = prepare_output(interrupt);
    let timer = start_timer(timer);
    (output, timer)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use super::prepare_output_then_start_timer;

    #[test]
    fn controller_output_precedes_the_single_runtime_timer_start() {
        let operations = Rc::new(RefCell::new(Vec::new()));
        let output_operations = Rc::clone(&operations);
        let timer_operations = Rc::clone(&operations);

        let (output, timer) = prepare_output_then_start_timer(
            "interrupt-owner",
            "timer-owner",
            |owner| {
                output_operations.borrow_mut().push("prepare-output");
                owner
            },
            |owner| {
                timer_operations.borrow_mut().push("start-timer");
                owner
            },
        );

        assert_eq!(output, "interrupt-owner");
        assert_eq!(timer, "timer-owner");
        assert_eq!(
            operations.borrow().as_slice(),
            ["prepare-output", "start-timer"]
        );
    }
}
