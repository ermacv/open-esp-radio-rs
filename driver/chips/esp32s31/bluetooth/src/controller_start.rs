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
    BluetoothControllerTimePendingCore, BluetoothControllerTimePendingCoreStep,
    BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimePendingOwner,
    BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeRequest,
    BluetoothControllerTimeRequestError, drain_controller_time_orphan,
};
#[cfg(target_arch = "riscv32")]
use crate::dtm_post_unlink::{
    BluetoothDtmPostUnlinkArmError, BluetoothDtmPostUnlinkMailbox, BluetoothDtmPostUnlinkRearm,
    BluetoothDtmPostUnlinkTake,
};
#[cfg(target_arch = "riscv32")]
use crate::{
    BluetoothControllerBlePhyEngineInitialized, BluetoothControllerInterruptRuntime,
    BluetoothControllerPoweredTaskRuntime, BluetoothControllerRuntimeEndpoints,
    BluetoothModemLpTimerEventCell, BluetoothModemLpTimerEventPublication,
    BluetoothModemLpTimerExpiration, BluetoothModemLpTimerExpirationPending,
    BluetoothModemLpTimerPublishedInterruptStep, BluetoothModemLpTimerSoftwareStep,
    BluetoothModemLpTimerSoftwareWork, BluetoothModemLpTimerStableInterruptStep,
    BluetoothNrtDefaultInterruptEpoch, BluetoothPrimaryInterruptStep,
    BluetoothPrimaryPublishedInterruptStep,
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

/// Controller result of consuming one opaque post-unlink event pair.
#[must_use = "every outcome retains the exact unlinked or removal-ready graph"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmSoftwareListRemovalPublishedStep<Role> {
    /// No eligible primary event is stored yet.
    Waiting(crate::BluetoothDtmPostUnlinkAwaiting<Role>),
    /// The supplied owner belongs to another mailbox identity or generation.
    MailboxAffinityMismatch(crate::BluetoothDtmPostUnlinkAwaiting<Role>),
    /// The primary epoch reported a baseline or unclassified fault.
    Fault {
        /// Already-unlinked graph retained for fail-stop handling.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact primary controller fault.
        fault: crate::BluetoothPrimaryControllerFault,
    },
    /// The acknowledged epoch contained no reviewed scheduler work.
    NoSchedulerWork {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
        /// Exact acknowledged empty primary epoch.
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    /// The fresh scheduler sample or task command statuses were not ready.
    Pending {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    },
    /// An internal mailbox invariant rejected re-arm after an empty primary epoch.
    NoSchedulerWorkRearmMismatch {
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    /// An internal mailbox invariant rejected re-arm after a pending scheduler gate.
    PendingRearmMismatch {
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    },
    /// The graph belongs to another Controller scheduler epoch.
    SchedulerIdentityMismatch {
        /// Unchanged already-unlinked graph.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact classified event which was not consumed by the mismatched graph.
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    /// The complete post-unlink return predicate became ready.
    Ready {
        /// Exact removal-ready graph; CPU ownership is still not returned.
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    },
}

/// Result of atomically unlinking one completed DTM item and arming the
/// Controller-owned post-unlink mailbox.
#[must_use = "retain the empty-head graph or the armed post-unlink owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmPostUnlinkArmStep<Role> {
    MailboxBusy(crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxIdentityExhausted(crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>),
    GenerationExhausted(crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>),
    SchedulerIdentityMismatch(crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxCommitMismatch(crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>),
    Armed(crate::BluetoothDtmPostUnlinkAwaiting<Role>),
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
    post_unlink_mailbox: BluetoothDtmPostUnlinkMailbox,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
    scheduler_epoch: Option<crate::BluetoothControllerSchedulerEpoch>,
    dtm_resources: crate::BluetoothDtmRuntimeResources,
}

/// Disjoint runtime endpoints borrowed from one statically placed final
/// Controller owner.
///
/// The task endpoint owns all mutable scheduler workers, the interrupt service
/// owns only stable platform dispatch plus shared publication cells, and HCI
/// exposes raw Controller/bootstrap endpoints. Keeping the backing final owner
/// in caller-owned stable storage prevents a self-referential runtime object.
#[must_use = "the final Controller endpoints must remain in one live runtime epoch"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerPublishedRuntimeEndpoints<
    'runtime,
    M,
    S,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Finite hard-handler service over the stable PAC/HAL owners.
    pub interrupt: BluetoothControllerPublishedInterruptService<'runtime, S>,
    /// Sole powered scheduler and DTM task service.
    pub task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    /// Disjoint Host transport, raw Controller endpoint and bootstrap state.
    pub hci: open_esp_radio_bluetooth_hci::LeControllerHciEndpoints<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Stable interrupt service for a materialized final Controller epoch.
///
/// This value cannot prepare DTM descriptors or mutate task-owned scheduler
/// state. It can only execute the three bounded hardware dispositions and
/// publish their durable events into the matching runtime resources.
#[must_use = "interrupt service must remain paired with its task runtime"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerPublishedInterruptService<'runtime, S> {
    storage: &'runtime S,
    runtime: BluetoothControllerInterruptRuntime<'runtime>,
    mailbox: &'runtime BluetoothDtmPostUnlinkMailbox,
}

/// Task-side hardware service for one published Controller epoch.
///
/// The service owns the mutable scheduler workers, the task-side HAL owner and
/// the exclusive scheduler-list identity. It also holds the unique mutable
/// borrow of the composition-owned DTM graph and physical default-power
/// profile. Stable interrupt storage is borrowed only for finite task-context
/// preparations; hard-handler dispatch remains in the disjoint interrupt
/// service.
#[must_use = "the DTM task service owns the powered scheduler epoch"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerPublishedTaskService<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    storage: &'runtime S,
    runtime: BluetoothControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    mailbox: &'runtime BluetoothDtmPostUnlinkMailbox,
    hci_epoch: open_esp_radio_bluetooth_hci::HciEpochIdentity<'runtime>,
    dtm_resources: &'runtime mut crate::BluetoothDtmRuntimeResources,
    rf_ready: crate::ble_phy::BluetoothDtmRfReadyAuthority,
    scheduler_epoch: &'runtime mut Option<crate::BluetoothControllerSchedulerEpoch>,
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

/// Why a fresh scheduler-current acquisition did not start.
///
/// The already-initialized scheduler epoch and exact Controller remain owned
/// by the corresponding [`BluetoothControllerSchedulerCurrentBeginFailure`]
/// on every variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerSchedulerCurrentBeginError {
    /// The exact published Controller no longer retained an initialized epoch.
    EpochUnavailable,
    /// Another acquisition or abandoned hardware request is still active.
    Busy,
    /// The private worker and lower sticky owner disagreed at publication.
    OwnershipCollision,
    /// The private non-repeating request identity space was exhausted.
    GenerationExhausted,
    /// An earlier ownership mismatch already stopped this worker.
    Faulted,
}

/// The published Controller has not initialized its scheduler epoch yet.
///
/// This failure performs no MMIO and retains the complete task service for a
/// later cold scheduler-time acquisition.
#[must_use = "the unchanged task service must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerEpochUnavailable<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerEpochUnavailable<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Recover the unchanged task service.
    pub fn into_task_service(
        self,
    ) -> BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY> {
        self.controller
    }
}

#[cfg(target_arch = "riscv32")]
impl From<BluetoothControllerTimeRequestError> for BluetoothControllerSchedulerCurrentBeginError {
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

/// Fail-stop result of rechecking or cancelling a fresh scheduler current.
///
/// Every public failure retains the exact owned scheduler epoch. Faulted
/// ownership never yields a sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerSchedulerCurrentError {
    /// The affine identity no longer matched the private worker owner.
    RequestMismatch,
    /// The lower sticky owner disappeared while the request was active.
    OwnershipLost,
    /// An earlier ownership mismatch already stopped this worker.
    Faulted,
}

#[cfg(target_arch = "riscv32")]
impl From<BluetoothControllerTimeEventError> for BluetoothControllerSchedulerCurrentError {
    fn from(error: BluetoothControllerTimeEventError) -> Self {
        match error {
            BluetoothControllerTimeEventError::RequestMismatch => Self::RequestMismatch,
            BluetoothControllerTimeEventError::OwnershipLost => Self::OwnershipLost,
            BluetoothControllerTimeEventError::Faulted => Self::Faulted,
        }
    }
}

/// Rejected cold post-enable acquisition retaining the complete task service.
#[must_use = "the task service retained by a rejected acquisition must be handled"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothAlwaysAwakePostEnableTimeBeginFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothAlwaysAwakePostEnableTimeBeginError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact reason the cold acquisition did not start.
    pub const fn error(&self) -> BluetoothAlwaysAwakePostEnableTimeBeginError {
        self.error
    }

    /// Recover the unchanged task service and exact rejection.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothAlwaysAwakePostEnableTimeBeginError,
    ) {
        (self.controller, self.error)
    }
}

/// Failed cold post-enable recheck or cancellation retaining the task service.
#[must_use = "the fail-stop task service must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothAlwaysAwakePostEnableTimeError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact fail-stop observation.
    pub const fn error(&self) -> BluetoothAlwaysAwakePostEnableTimeError {
        self.error
    }

    /// Recover the task service and exact fail-stop observation.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothAlwaysAwakePostEnableTimeError,
    ) {
        (self.controller, self.error)
    }
}

/// Rejected fresh-current acquisition retaining the epoch owner.
#[must_use = "the retained scheduler epoch must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerCurrentBeginFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothControllerSchedulerCurrentBeginError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact reason the fresh acquisition did not start.
    pub const fn error(&self) -> BluetoothControllerSchedulerCurrentBeginError {
        self.error
    }

    /// Recover the retained epoch and exact rejection.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothControllerSchedulerCurrentBeginError,
    ) {
        (self.controller, self.error)
    }
}

/// Failed fresh-current recheck or cancellation retaining the epoch owner.
#[must_use = "the fail-stop scheduler epoch must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerCurrentFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
{
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothControllerSchedulerCurrentError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact fail-stop observation.
    pub const fn error(&self) -> BluetoothControllerSchedulerCurrentError {
        self.error
    }

    /// Recover the retained epoch and exact fail-stop observation.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothControllerSchedulerCurrentError,
    ) {
        (self.controller, self.error)
    }
}

#[cfg(target_arch = "riscv32")]
const fn dtm_time_begin_error(
    error: BluetoothControllerTimeRequestError,
) -> crate::BluetoothDtmControllerTimeAcquisitionError {
    match error {
        BluetoothControllerTimeRequestError::Busy => {
            crate::BluetoothDtmControllerTimeAcquisitionError::Busy
        }
        BluetoothControllerTimeRequestError::OwnershipCollision => {
            crate::BluetoothDtmControllerTimeAcquisitionError::OwnershipCollision
        }
        BluetoothControllerTimeRequestError::GenerationExhausted => {
            crate::BluetoothDtmControllerTimeAcquisitionError::GenerationExhausted
        }
        BluetoothControllerTimeRequestError::Faulted => {
            crate::BluetoothDtmControllerTimeAcquisitionError::Faulted
        }
    }
}

#[cfg(target_arch = "riscv32")]
const fn dtm_time_event_error(
    error: BluetoothControllerTimeEventError,
) -> crate::BluetoothDtmControllerTimeAcquisitionError {
    match error {
        BluetoothControllerTimeEventError::RequestMismatch => {
            crate::BluetoothDtmControllerTimeAcquisitionError::RequestMismatch
        }
        BluetoothControllerTimeEventError::OwnershipLost => {
            crate::BluetoothDtmControllerTimeAcquisitionError::OwnershipLost
        }
        BluetoothControllerTimeEventError::Faulted => {
            crate::BluetoothDtmControllerTimeAcquisitionError::Faulted
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

/// Result of one bounded abandoned controller-time drain observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a waiting controller-time orphan requires one later bounded drain observation"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerTimeOrphanDrainStep {
    /// No abandoned request existed; no hardware was touched.
    Idle,
    /// Hardware still owns the abandoned request; arrange one later recheck.
    Waiting,
    /// The abandoned result was discarded and the worker is idle again.
    Drained,
}

/// One exact in-flight post-enable controller-time request.
///
/// This affine value owns the complete published task service, so no second
/// operation can use that Controller while the request is pending. Dropping it
/// abandons the exact identity into the private orphan drain and drops the
/// fail-stop task owner; explicit cancellation returns that owner by value.
#[must_use = "recheck, cancel, or drop the exact post-enable time request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    core: BluetoothControllerTimePendingCore<
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Controller-bound proof that the post-enable latch request completed.
///
/// The sample remains private and inseparable from the exact owned
/// Controller. This is not an RF-ready instant: this path neither performs nor
/// proves a sleep/wake transition, and it applies no recovered RF-settling
/// interval.
#[must_use = "initialize the persistent scheduler epoch from the bound first-live sample"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothAlwaysAwakeTimeObservedAfterEnable<'runtime, S, const SCHEDULER_CAPACITY: usize>
{
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    sample: crate::BluetoothControllerTimeSample,
}

/// Exact published Controller with one live sample bound to its retained epoch.
///
/// This affine owner is the cold first-live entry into DTM preparation. Later
/// fresh currents re-enter through the retained epoch without repeating cold
/// initialization. Neither path grants RF-ready authority. Admission and
/// sequence time are acquired privately after this state consumes one typed
/// role request, and every terminal outcome returns the same owned Controller
/// in the retained epoch state.
#[must_use = "consume the epoch-bound live sample through one DTM preparation attempt"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerNowReady<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    epoch: crate::BluetoothControllerSchedulerEpoch,
    sample: crate::BluetoothControllerTimeSample,
}

/// Exact published Controller after its scheduler epoch has been initialized.
///
/// The epoch remains stored inside the same Controller and cannot detach or be
/// paired with another owner. This state carries no fresh current-time sample;
/// another source-owned observation is required before another preparation.
#[must_use = "the retained scheduler epoch owns the published Controller"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerEpochRetained<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One exact in-flight fresh scheduler-current acquisition.
///
/// This value owns an already initialized scheduler-epoch owner. Waiting and
/// cancellation retain or return that complete owner; they never construct a
/// cold epoch. Only a completed fresh observation advances the retained epoch
/// to its source-owned task-run anchor. A cancelled request must be drained
/// before another acquisition can begin. Dropping is an explicit fail-stop.
#[must_use = "recheck, cancel, or drop the exact fresh scheduler-current request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerSchedulerCurrentPending<'runtime, S, const SCHEDULER_CAPACITY: usize>
{
    core: BluetoothControllerTimePendingCore<
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
    epoch: crate::BluetoothControllerSchedulerEpoch,
}

/// Result of exactly one fresh scheduler-current recheck.
#[must_use = "retain Waiting or consume Ready through one DTM preparation"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerSchedulerCurrentStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Hardware still owns the exact request and complete Controller owner.
    Waiting(BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact request completed with one private epoch-bound sample.
    Ready(BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Terminal result of one source-ordered DTM preparation transaction.
///
/// Every variant owns the exact role-specific success or lossless failure.
/// Admission and sequence samples never cross this boundary.
#[must_use = "the prepared item or exact retry owner must be handled"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmControllerPreparationOutcome {
    /// Initial transmitter preparation reached a terminal result.
    TransmitterFirst(
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmTransmitterEvent,
                crate::BluetoothDtmInitialSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerTxPreparationFailure,
        >,
    ),
    /// Initial receiver preparation reached a terminal result.
    ReceiverFirst(
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmReceiverEvent,
                crate::BluetoothDtmInitialSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerRxPreparationFailure,
        >,
    ),
    /// Recurring transmitter preparation reached a terminal result.
    TransmitterRecurring(
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmTransmitterEvent,
                crate::BluetoothDtmRecurringSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerTxRecurringPreparationFailure,
        >,
    ),
    /// Recurring receiver preparation reached a terminal result.
    ReceiverRecurring(
        Result<
            crate::BluetoothDtmEmptySchedulerMergePrepared<
                crate::BluetoothDtmReceiverEvent,
                crate::BluetoothDtmRecurringSchedulerItemPhase,
            >,
            crate::BluetoothDtmControllerRxRecurringPreparationFailure,
        >,
    ),
}

#[cfg(target_arch = "riscv32")]
enum BluetoothDtmControllerPreparationPhase {
    TransmitterFirstRfReady {
        owner: crate::BluetoothDtmPreparedTxGraph,
        link_state: crate::BluetoothDtmLinkStateReset,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        requested_interval_micros: u16,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    ReceiverFirstRfReady {
        owner: crate::BluetoothDtmReceiverCpuOwned,
        link_state: crate::BluetoothDtmLinkStateReset,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    ReceiverRecurringRfReady {
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
        epoch: crate::BluetoothControllerSchedulerEpoch,
    },
    ReceiverRecurringCurrent {
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
        epoch: crate::BluetoothControllerSchedulerEpoch,
        rf_ready: crate::BluetoothDtmRfReady,
    },
    TransmitterFirstAdmission(crate::scheduler::BluetoothDtmTransmitterFirstStaged),
    ReceiverFirstAdmission(crate::scheduler::BluetoothDtmReceiverFirstStaged),
    TransmitterFirstSequence(crate::scheduler::BluetoothDtmTransmitterFirstPreSequence),
    ReceiverFirstSequence(crate::scheduler::BluetoothDtmReceiverFirstPreSequence),
    TransmitterRecurringSequence(crate::scheduler::BluetoothDtmTransmitterRecurringPreSequence),
    ReceiverRecurringSequence(crate::scheduler::BluetoothDtmReceiverRecurringPreSequence),
}

#[cfg(target_arch = "riscv32")]
struct BluetoothDtmControllerPreparationTimeOwner<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<BluetoothDtmControllerPreparationPhase>,
    cancelled: Option<BluetoothDtmControllerPreparationOutcome>,
}

/// One exact RF-ready, current, admission or sequence-time request.
///
/// Initial operations acquire RF-ready after their current, then admission
/// before reservation and sequence only after reservation. Recurring RX
/// acquires RF-ready before current; recurring TX starts after current without
/// RF-ready. Explicit cancellation returns the task owner and releases any
/// retained reservation. Dropping cancels the exact latch request but also
/// drops the sole task owner as a deliberate fail-stop; the long-lived runner
/// must therefore retain this state and use its explicit cancellation edge.
#[must_use = "recheck, cancel, or drop the exact DTM time request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmControllerPreparationPending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    core: BluetoothControllerTimePendingCore<
        BluetoothDtmControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Terminal DTM preparation result with the exact Controller epoch retained.
#[must_use = "the Controller and role-specific preparation result must be handled"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmControllerPreparationTerminal<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: BluetoothDtmControllerPreparationOutcome,
}

/// Lossless rejection of an initial DTM preparation.
///
/// `SessionActive` retains the unused source-owned current together with the
/// task service whose sole graph is already checked out. A lower preparation
/// failure instead remains a complete terminal transaction: its role-specific
/// outcome retains the checked-out graph for the session runner.
#[must_use = "the source-owned current or lower terminal transaction must be handled"]
#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::large_enum_variant,
    reason = "both no-alloc variants retain their complete affine Controller owner"
)]
pub enum BluetoothDtmControllerInitialPreparationFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    /// Another DTM session already owns the composition graph.
    SessionActive(BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>),
    /// The checked-out graph reached a lower preparation terminal.
    PreparationTerminal(BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of one bounded DTM controller-time phase observation.
#[must_use = "retain Pending or consume the terminal Controller and DTM result"]
#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc affine variants retain the complete in-flight or terminal DTM transaction"
)]
pub enum BluetoothDtmControllerPreparationStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Hardware still owns the exact phase request.
    Pending(BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>),
    /// Preparation completed or failed with every affine owner returned.
    Terminal(BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of rechecking one exact post-enable controller-time request.
#[must_use = "retain Waiting or consume the Controller-bound Ready proof"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothAlwaysAwakePostEnableTimeStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Hardware still owns the same request and complete Controller owner.
    Waiting(BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact request completed with a Controller-bound private sample.
    Ready(BluetoothAlwaysAwakeTimeObservedAfterEnable<'runtime, S, SCHEDULER_CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform exactly one observation of this exact latch request.
    pub fn recheck(
        self,
    ) -> Result<
        BluetoothAlwaysAwakePostEnableTimeStep<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(Self {
                    core,
                }))
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(
                    BluetoothAlwaysAwakeTimeObservedAfterEnable {
                        controller: owner,
                        sample,
                    },
                ))
            }
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(BluetoothAlwaysAwakePostEnableTimeFailure {
                    controller,
                    error: error.into(),
                })
            }
        }
    }

    /// Abandon this exact request and return the complete Controller owner.
    ///
    /// The returned Controller cannot begin another acquisition until
    /// `drain_abandoned_always_awake_post_enable_time` reports `Drained` (or
    /// `Idle`). An ownership mismatch is returned explicitly and leaves the
    /// private worker fail-stop while the failure retains the complete owner.
    pub fn cancel(
        self,
    ) -> Result<
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.core.cancel() {
            Ok(controller) => Ok(controller),
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(BluetoothAlwaysAwakePostEnableTimeFailure {
                    controller,
                    error: error.into(),
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Borrow the exact role-specific terminal result.
    pub const fn outcome(&self) -> &BluetoothDtmControllerPreparationOutcome {
        &self.outcome
    }

    /// Recover the retained Controller epoch and role-specific terminal result.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothDtmControllerPreparationOutcome,
    ) -> BluetoothDtmControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothDtmControllerPreparationStep::Terminal(BluetoothDtmControllerPreparationTerminal {
            controller: BluetoothControllerSchedulerEpochRetained { controller },
            outcome,
        })
    }

    /// Perform one bounded observation of the current DTM time request.
    ///
    /// Completing an initial admission reserves the resolved window and only
    /// then publishes the sequence request. Consequently one call may advance
    /// into another `Pending` state without yet producing a terminal result.
    pub fn recheck(self) -> BluetoothDtmControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                return BluetoothDtmControllerPreparationStep::Pending(Self { core });
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let phase = owner
                    .phase
                    .take()
                    .expect("failed DTM time recheck retains its exact phase");
                let outcome = owner
                    .controller
                    .cancel_dtm_preparation_phase(phase, dtm_time_event_error(error));
                return Self::terminal(owner.controller, outcome);
            }
        };
        let phase = owner
            .phase
            .take()
            .expect("completed DTM time request retains its exact phase");
        let mut controller = owner.controller;
        match phase {
            BluetoothDtmControllerPreparationPhase::TransmitterFirstRfReady {
                owner,
                link_state,
                channel,
                phy,
                requested_interval_micros,
                now,
            } => {
                let rf_ready = controller.rf_ready.complete(now.epoch(), sample);
                let staged = match controller.runtime.stage_dtm_transmitter_first_item(
                    owner,
                    link_state,
                    channel,
                    phy,
                    requested_interval_micros,
                    now,
                    rf_ready,
                ) {
                    Ok(staged) => staged,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(
                                failure,
                            )),
                        );
                    }
                };
                match controller.begin_dtm_preparation_time(
                    BluetoothDtmControllerPreparationPhase::TransmitterFirstAdmission(staged),
                ) {
                    Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                }
            }
            BluetoothDtmControllerPreparationPhase::ReceiverFirstRfReady {
                owner,
                link_state,
                channel,
                phy,
                now,
            } => {
                let rf_ready = controller.rf_ready.complete(now.epoch(), sample);
                let staged = match controller
                    .runtime
                    .stage_dtm_receiver_first_item(owner, link_state, channel, phy, now, rf_ready)
                {
                    Ok(staged) => staged,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
                        );
                    }
                };
                match controller.begin_dtm_preparation_time(
                    BluetoothDtmControllerPreparationPhase::ReceiverFirstAdmission(staged),
                ) {
                    Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                }
            }
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringRfReady { owner, epoch } => {
                let rf_ready = controller.rf_ready.complete(epoch, sample);
                match controller.begin_dtm_preparation_time(
                    BluetoothDtmControllerPreparationPhase::ReceiverRecurringCurrent {
                        owner,
                        epoch,
                        rf_ready,
                    },
                ) {
                    Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                }
            }
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringCurrent {
                owner,
                epoch,
                rf_ready,
            } => {
                let epoch = epoch.reanchor(&sample);
                *controller.scheduler_epoch = Some(epoch);
                let now =
                    crate::controller_time::BluetoothControllerSchedulerNow::from_retained_epoch(
                        epoch, sample,
                    );
                let staged = match controller
                    .runtime
                    .stage_dtm_receiver_recurring_item(owner, now, rf_ready)
                {
                    Ok(staged) => staged,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Err(
                                failure,
                            )),
                        );
                    }
                };
                let pre_sequence = match controller
                    .runtime
                    .reserve_dtm_receiver_recurring_item(staged)
                {
                    Ok(pre_sequence) => pre_sequence,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Err(
                                failure,
                            )),
                        );
                    }
                };
                match controller.begin_dtm_preparation_time(
                    BluetoothDtmControllerPreparationPhase::ReceiverRecurringSequence(pre_sequence),
                ) {
                    Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                }
            }
            BluetoothDtmControllerPreparationPhase::TransmitterFirstAdmission(staged) => {
                match controller
                    .runtime
                    .admit_dtm_transmitter_first_item(staged, sample)
                {
                    Ok(pre_sequence) => match controller.begin_dtm_preparation_time(
                        BluetoothDtmControllerPreparationPhase::TransmitterFirstSequence(
                            pre_sequence,
                        ),
                    ) {
                        Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                        Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                    },
                    Err(failure) => Self::terminal(
                        controller,
                        BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
                    ),
                }
            }
            BluetoothDtmControllerPreparationPhase::ReceiverFirstAdmission(staged) => {
                match controller
                    .runtime
                    .admit_dtm_receiver_first_item(staged, sample)
                {
                    Ok(pre_sequence) => match controller.begin_dtm_preparation_time(
                        BluetoothDtmControllerPreparationPhase::ReceiverFirstSequence(pre_sequence),
                    ) {
                        Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                        Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                    },
                    Err(failure) => Self::terminal(
                        controller,
                        BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
                    ),
                }
            }
            BluetoothDtmControllerPreparationPhase::TransmitterFirstSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_transmitter_first_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    BluetoothDtmControllerPreparationOutcome::TransmitterFirst(result),
                )
            }
            BluetoothDtmControllerPreparationPhase::ReceiverFirstSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_receiver_first_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    BluetoothDtmControllerPreparationOutcome::ReceiverFirst(result),
                )
            }
            BluetoothDtmControllerPreparationPhase::TransmitterRecurringSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_transmitter_recurring_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(result),
                )
            }
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_receiver_recurring_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(result),
                )
            }
        }
    }

    /// Cancel the exact phase, release any reservation and recover retry ownership.
    pub fn cancel(
        self,
    ) -> BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY> {
        let mut owner = match self.core.cancel() {
            Ok(owner) => owner,
            Err(failure) => failure.into_parts().0,
        };
        let outcome = owner
            .cancelled
            .take()
            .expect("explicit DTM time cancellation records its lossless outcome");
        BluetoothDtmControllerPreparationTerminal {
            controller: BluetoothControllerSchedulerEpochRetained {
                controller: owner.controller,
            },
            outcome,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothDtmControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        BluetoothControllerTimePendingOwner::recheck_owned_controller_time(
            &mut self.controller,
            request,
        )
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        let result = BluetoothControllerTimePendingOwner::cancel_owned_controller_time(
            &mut self.controller,
            request,
        );
        let error = match result {
            Ok(()) => crate::BluetoothDtmControllerTimeAcquisitionError::Cancelled,
            Err(error) => dtm_time_event_error(error),
        };
        let phase = self
            .phase
            .take()
            .expect("private DTM time owner retains one exact preparation phase");
        self.cancelled = Some(self.controller.cancel_dtm_preparation_phase(phase, error));
        result
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        BluetoothControllerTimePendingOwner::drain_orphan_controller_time(&mut self.controller)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform exactly one observation of this fresh-current request.
    ///
    /// `Waiting` retains the same request, prior epoch and Controller owner.
    /// `Ready` applies the reference-update arithmetic recovered from the
    /// vendor task-run path and binds the same private sample to the resulting
    /// current. This does not prove that a vendor task-run event occurred. On
    /// error the exact epoch-retained owner is returned in the failure.
    pub fn recheck(
        self,
    ) -> Result<
        BluetoothControllerSchedulerCurrentStep<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let epoch = self.epoch;
        match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                Ok(BluetoothControllerSchedulerCurrentStep::Waiting(Self {
                    core,
                    epoch,
                }))
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => {
                let epoch = epoch.reanchor(&sample);
                *owner.scheduler_epoch = Some(epoch);
                Ok(BluetoothControllerSchedulerCurrentStep::Ready(
                    BluetoothControllerSchedulerNowReady {
                        controller: owner,
                        epoch,
                        sample,
                    },
                ))
            }
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(BluetoothControllerSchedulerCurrentFailure {
                    controller: BluetoothControllerSchedulerEpochRetained { controller },
                    error: error.into(),
                })
            }
        }
    }

    /// Abandon this exact request and return the complete retained epoch.
    ///
    /// Success leaves the worker in orphan-drain state. The caller must drive
    /// `drain_abandoned_controller_time` before beginning another fresh
    /// acquisition. An error preserves the same retained Controller but leaves
    /// the private worker fail-stop.
    pub fn cancel(
        self,
    ) -> Result<
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.core.cancel() {
            Ok(controller) => Ok(BluetoothControllerSchedulerEpochRetained { controller }),
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(BluetoothControllerSchedulerCurrentFailure {
                    controller: BluetoothControllerSchedulerEpochRetained { controller },
                    error: error.into(),
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Return the exact cancelled or stopped session graph to this Controller.
    ///
    /// The embedded runtime rejects an occupied slot or a graph minted from
    /// another pinned storage object and returns that owner unchanged. This is
    /// the only public graph-return edge after final Controller publication;
    /// initial checkout remains private to the typed TX/RX start operations.
    pub fn restore_dtm_session_idle(
        &mut self,
        idle: crate::BluetoothDtmSessionIdle,
    ) -> Result<(), crate::BluetoothDtmSessionIdle> {
        self.dtm_resources.restore_idle(idle)
    }

    fn new_dtm_link_state_reset(
        &self,
        role: crate::BluetoothDtmRole,
    ) -> crate::BluetoothDtmLinkStateReset {
        crate::BluetoothDtmLinkStateReset::new(
            None,
            None,
            self.dtm_resources.default_tx_power_dbm(),
            role,
        )
    }

    fn cancel_dtm_preparation_phase(
        &mut self,
        phase: BluetoothDtmControllerPreparationPhase,
        error: crate::BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerPreparationOutcome {
        match phase {
            BluetoothDtmControllerPreparationPhase::TransmitterFirstRfReady { owner, .. } => {
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(self
                    .runtime
                    .reject_dtm_transmitter_first_before_stage(owner, error)))
            }
            BluetoothDtmControllerPreparationPhase::ReceiverFirstRfReady { owner, .. } => {
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(self
                    .runtime
                    .reject_dtm_receiver_first_before_stage(owner, error)))
            }
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringRfReady { owner, .. }
            | BluetoothDtmControllerPreparationPhase::ReceiverRecurringCurrent { owner, .. } => {
                BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Err(self
                    .runtime
                    .reject_dtm_receiver_recurring_before_stage(owner, error)))
            }
            BluetoothDtmControllerPreparationPhase::TransmitterFirstAdmission(staged) => {
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(self
                    .runtime
                    .cancel_dtm_transmitter_first_staged(staged, error)))
            }
            BluetoothDtmControllerPreparationPhase::ReceiverFirstAdmission(staged) => {
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(self
                    .runtime
                    .cancel_dtm_receiver_first_staged(staged, error)))
            }
            BluetoothDtmControllerPreparationPhase::TransmitterFirstSequence(pre_sequence) => {
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(self
                    .runtime
                    .cancel_dtm_transmitter_first_pre_sequence(pre_sequence, error)))
            }
            BluetoothDtmControllerPreparationPhase::ReceiverFirstSequence(pre_sequence) => {
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(self
                    .runtime
                    .cancel_dtm_receiver_first_pre_sequence(pre_sequence, error)))
            }
            BluetoothDtmControllerPreparationPhase::TransmitterRecurringSequence(pre_sequence) => {
                BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(Err(self
                    .runtime
                    .cancel_dtm_transmitter_recurring_pre_sequence(pre_sequence, error)))
            }
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringSequence(pre_sequence) => {
                BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Err(self
                    .runtime
                    .cancel_dtm_receiver_recurring_pre_sequence(pre_sequence, error)))
            }
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete role retry owner"
    )]
    fn begin_dtm_rf_ready_time(
        mut self,
        phase: BluetoothDtmControllerPreparationPhase,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome = self.cancel_dtm_preparation_phase(phase, dtm_time_begin_error(error));
                return Err(BluetoothDtmControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(BluetoothDtmControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothDtmControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                    cancelled: None,
                },
                request,
            ),
        })
    }

    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete role retry owner"
    )]
    fn begin_dtm_preparation_time(
        mut self,
        phase: BluetoothDtmControllerPreparationPhase,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome = self.cancel_dtm_preparation_phase(phase, dtm_time_begin_error(error));
                return Err(BluetoothDtmControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(BluetoothDtmControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothDtmControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                    cancelled: None,
                },
                request,
            ),
        })
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        match self.runtime.recheck_owned_controller_time(request) {
            Ok(BluetoothControllerTimeEventStep::Waiting) => {
                Ok(BluetoothControllerTimePendingOwnerStep::Waiting)
            }
            Ok(BluetoothControllerTimeEventStep::Sample {
                request: completed,
                sample,
            }) if completed == request => {
                Ok(BluetoothControllerTimePendingOwnerStep::Ready(sample))
            }
            Ok(
                BluetoothControllerTimeEventStep::Idle
                | BluetoothControllerTimeEventStep::OrphanDrained
                | BluetoothControllerTimeEventStep::Sample { .. },
            ) => Err(BluetoothControllerTimeEventError::RequestMismatch),
            Err(error) => Err(error),
        }
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        self.runtime.cancel_owned_controller_time(request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        match self.runtime.drain_orphan_controller_time() {
            Ok(BluetoothControllerTimeEventStep::Idle) => {
                Ok(BluetoothControllerTimePendingOrphanStep::Idle)
            }
            Ok(BluetoothControllerTimeEventStep::Waiting) => {
                Ok(BluetoothControllerTimePendingOrphanStep::Waiting)
            }
            Ok(BluetoothControllerTimeEventStep::OrphanDrained) => {
                Ok(BluetoothControllerTimePendingOrphanStep::Drained)
            }
            Ok(BluetoothControllerTimeEventStep::Sample { .. }) => {
                Err(BluetoothControllerTimeEventError::RequestMismatch)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothAlwaysAwakeTimeObservedAfterEnable<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Initialize this Controller's persistent scheduler epoch from the first
    /// live post-enable sample.
    ///
    /// The same affine sample is retained as the sole current-time authority
    /// for one DTM preparation attempt. This transition proves neither RF
    /// readiness nor deadline readiness.
    pub fn initialize_scheduler_epoch(
        self,
    ) -> BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY> {
        let epoch = crate::BluetoothControllerSchedulerEpoch::from_first_live_update(
            &self.sample,
            self.controller.runtime.controller_time_scale(),
        );
        *self.controller.scheduler_epoch = Some(epoch);
        BluetoothControllerSchedulerNowReady {
            controller: self.controller,
            epoch,
            sample: self.sample,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Begin one affine fresh scheduler-current acquisition.
    ///
    /// The request consumes and retains the complete epoch owner. Begin failure
    /// returns that owner unchanged. Completion advances the persistent epoch
    /// with the arithmetic recovered from the task-run reference update before
    /// the private sample can enter one DTM preparation; it does not itself
    /// prove that a vendor task-run event occurred.
    pub fn begin_fresh_scheduler_current(
        mut self,
    ) -> Result<
        BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let Some(epoch) = *self.controller.scheduler_epoch else {
            return Err(BluetoothControllerSchedulerCurrentBeginFailure {
                controller: self,
                error: BluetoothControllerSchedulerCurrentBeginError::EpochUnavailable,
            });
        };
        let request = match self.controller.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                return Err(BluetoothControllerSchedulerCurrentBeginFailure {
                    controller: self,
                    error: error.into(),
                });
            }
        };
        let controller = self.controller;
        Ok(BluetoothControllerSchedulerCurrentPending {
            core: BluetoothControllerTimePendingCore::new(controller, request),
            epoch,
        })
    }

    /// Begin recurring receiver preparation in vendor RF-before-current order.
    ///
    /// The retained always-awake BLE-PHY owner first publishes a private
    /// RF-ready time request. Only its completed scheduler-domain result can
    /// advance to a second fresh-current request and reanchor this epoch.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete active RX owner"
    )]
    pub fn begin_dtm_receiver_recurring_item(
        self,
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let controller = self.controller;
        let epoch = (*controller.scheduler_epoch)
            .expect("the retained scheduler epoch cannot lose its stored epoch");
        controller.begin_dtm_rf_ready_time(
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringRfReady { owner, epoch },
        )
    }

    /// Perform one bounded observation of an abandoned fresh-current request.
    ///
    /// `Waiting` requires one later call. A completed orphan is discarded and
    /// never advances the retained epoch or becomes a sample for a later DTM
    /// preparation. Every outcome preserves this exact retained owner.
    pub fn drain_abandoned_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimeOrphanDrainStep, BluetoothControllerSchedulerCurrentError>
    {
        match drain_controller_time_orphan(&mut self.controller) {
            Ok(BluetoothControllerTimePendingOrphanStep::Idle) => {
                Ok(BluetoothControllerTimeOrphanDrainStep::Idle)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Waiting) => {
                Ok(BluetoothControllerTimeOrphanDrainStep::Waiting)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Drained) => {
                Ok(BluetoothControllerTimeOrphanDrainStep::Drained)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Release the exact task service for its ordinary lifecycle APIs.
    ///
    /// The scheduler epoch remains stored in the Controller. Consequently the
    /// cold initialization path remains rejected with `AlreadyInitialized`.
    pub fn into_task_service(
        self,
    ) -> BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY> {
        self.controller
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Discard this unused current sample while retaining the initialized
    /// scheduler epoch and the exact published task service.
    ///
    /// This is the lossless abort edge before a DTM preparation starts. The
    /// next attempt must acquire a fresh scheduler current; it cannot reuse
    /// the discarded sample.
    pub fn into_retained_epoch(
        self,
    ) -> BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothControllerSchedulerEpochRetained {
            controller: self.controller,
        }
    }

    fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
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

    /// Begin source-ordered initial transmitter preparation.
    ///
    /// The private current is retained while the always-awake BLE-PHY owner
    /// obtains a later source-owned RF-ready instant. Only that ordered pair
    /// can form the candidate before admission and sequence requests.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete TX retry owner"
    )]
    pub(crate) fn begin_dtm_transmitter_first_item(
        self,
        pattern: crate::BluetoothDtmPayloadPattern,
        length: crate::BluetoothDtmPayloadLength,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        requested_interval_micros: u16,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let graph = match self.controller.dtm_resources.begin_session_epoch() {
            Ok(graph) => graph,
            Err(crate::BluetoothDtmRuntimeSessionBeginError::SessionActive) => {
                return Err(BluetoothDtmControllerInitialPreparationFailure::SessionActive(self));
            }
        };
        let owner = crate::BluetoothDtmTxGraphPrepare::prepare_dtm_tx_packet(
            graph.into_graph(),
            pattern,
            length,
        );
        let link_state = self
            .controller
            .new_dtm_link_state_reset(crate::BluetoothDtmRole::Transmitter);
        let (controller, now) = self.into_parts();
        controller
            .begin_dtm_rf_ready_time(
                BluetoothDtmControllerPreparationPhase::TransmitterFirstRfReady {
                    owner,
                    link_state,
                    channel,
                    phy,
                    requested_interval_micros,
                    now,
                },
            )
            .map_err(BluetoothDtmControllerInitialPreparationFailure::PreparationTerminal)
    }

    /// Begin source-ordered initial receiver preparation.
    ///
    /// The current retained by this state precedes a private source-owned
    /// RF-ready request. Admission and sequence then remain private affine
    /// requests on the same Controller.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete RX retry owner"
    )]
    pub(crate) fn begin_dtm_receiver_first_item(
        self,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let graph = match self.controller.dtm_resources.begin_session_epoch() {
            Ok(graph) => graph,
            Err(crate::BluetoothDtmRuntimeSessionBeginError::SessionActive) => {
                return Err(BluetoothDtmControllerInitialPreparationFailure::SessionActive(self));
            }
        };
        let owner = crate::BluetoothDtmReceiverCpuOwned::new(graph.into_graph());
        let link_state = self
            .controller
            .new_dtm_link_state_reset(crate::BluetoothDtmRole::Receiver);
        let (controller, now) = self.into_parts();
        controller
            .begin_dtm_rf_ready_time(
                BluetoothDtmControllerPreparationPhase::ReceiverFirstRfReady {
                    owner,
                    link_state,
                    channel,
                    phy,
                    now,
                },
            )
            .map_err(BluetoothDtmControllerInitialPreparationFailure::PreparationTerminal)
    }

    /// Reserve and begin source-ordered recurring transmitter preparation.
    ///
    /// The recurring window is reserved before its private sequence request.
    /// This path has no RF-ready phase in the reviewed vendor flow.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete active TX owner"
    )]
    pub fn begin_dtm_transmitter_recurring_item(
        self,
        owner: crate::BluetoothDtmActiveTransmitterCpuOwned,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let (mut controller, now) = self.into_parts();
        let staged = match controller
            .runtime
            .stage_dtm_transmitter_recurring_item(owner, now)
        {
            Ok(staged) => staged,
            Err(failure) => {
                return Err(BluetoothDtmControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller },
                    outcome: BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(Err(
                        failure,
                    )),
                });
            }
        };
        let pre_sequence = match controller
            .runtime
            .reserve_dtm_transmitter_recurring_item(staged)
        {
            Ok(pre_sequence) => pre_sequence,
            Err(failure) => {
                return Err(BluetoothDtmControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller },
                    outcome: BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(Err(
                        failure,
                    )),
                });
            }
        };
        controller.begin_dtm_preparation_time(
            BluetoothDtmControllerPreparationPhase::TransmitterRecurringSequence(pre_sequence),
        )
    }
}

/// Failed stable publication retaining the Controller, storage and DTM runtime.
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
    dtm_resources: crate::BluetoothDtmRuntimeResources,
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
    /// Borrow the complete final Controller and DTM runtime as disjoint
    /// interrupt, task and HCI endpoints.
    ///
    /// The caller must retain this owner in stable storage for the complete
    /// routed lifetime. The task endpoint uniquely borrows its embedded DTM
    /// runtime; no endpoint can cross-wire a graph from another composition.
    pub fn split_runtime<'runtime>(
        &'runtime mut self,
    ) -> BluetoothControllerPublishedRuntimeEndpoints<
        'runtime,
        M,
        S,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let Self {
            initialized,
            _storage,
            post_unlink_mailbox,
            scheduler_epoch,
            dtm_resources,
            ..
        } = self;
        let (
            BluetoothControllerRuntimeEndpoints {
                interrupt,
                task,
                hci,
            },
            rf_ready,
        ) = initialized.split_runtime();
        let hci_epoch = hci.controller.epoch_identity();
        BluetoothControllerPublishedRuntimeEndpoints {
            interrupt: BluetoothControllerPublishedInterruptService {
                storage: _storage,
                runtime: interrupt,
                mailbox: post_unlink_mailbox,
            },
            task: BluetoothControllerPublishedTaskService {
                storage: _storage,
                runtime: task,
                mailbox: post_unlink_mailbox,
                hci_epoch,
                dtm_resources,
                rf_ready,
                scheduler_epoch,
            },
            hci,
        }
    }

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
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Move this exact task service into its initialized scheduler epoch.
    ///
    /// This transition performs no MMIO, exposes no epoch image and cannot
    /// initialize missing state.
    pub fn retain_scheduler_epoch(
        self,
    ) -> Result<
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothControllerSchedulerEpochUnavailable<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        if self.scheduler_epoch.is_none() {
            return Err(BluetoothControllerSchedulerEpochUnavailable { controller: self });
        }
        Ok(BluetoothControllerSchedulerEpochRetained { controller: self })
    }

    /// Begin one affine post-enable controller-time acquisition.
    ///
    /// The nested standalone always-awake selection gates publication through
    /// the complete BLE-PHY chain. The returned pending value owns this exact
    /// Controller until it is rechecked, cancelled or dropped. Completion proves
    /// only that the latch request completed after enable. This path neither
    /// performs nor proves a sleep/wake transition, and no RF-settling interval
    /// is recovered here; the proof therefore is not RF-ready authority. Once
    /// its first live sample initializes the persistent scheduler epoch, this
    /// cold acquisition path rejects every later attempt as `AlreadyInitialized`.
    pub fn begin_always_awake_post_enable_time(
        mut self,
    ) -> Result<
        BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        if self.scheduler_epoch.is_some() {
            return Err(BluetoothAlwaysAwakePostEnableTimeBeginFailure {
                controller: self,
                error: BluetoothAlwaysAwakePostEnableTimeBeginError::AlreadyInitialized,
            });
        }
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                return Err(BluetoothAlwaysAwakePostEnableTimeBeginFailure {
                    controller: self,
                    error: error.into(),
                });
            }
        };
        Ok(BluetoothAlwaysAwakePostEnableTimePending {
            core: BluetoothControllerTimePendingCore::new(self, request),
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
        match drain_controller_time_orphan(self) {
            Ok(BluetoothControllerTimePendingOrphanStep::Idle) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Idle)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Waiting) => {
                Ok(BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Waiting)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Drained) => {
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
        self.runtime.cancel_dtm_transmitter_first_item(merged)
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
        self.runtime.cancel_dtm_receiver_first_item(merged)
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
        self.runtime.cancel_dtm_transmitter_recurring_item(merged)
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
        self.runtime.cancel_dtm_receiver_recurring_item(merged)
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
    pub(crate) fn publish_dtm_scheduler_head<Role, Phase>(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        crate::BluetoothDtmSchedulerHeadPublished<Role>,
        crate::BluetoothDtmSchedulerHeadPublicationFailure<Role, Phase>,
    >
    where
        Phase: crate::BluetoothDtmSchedulerItemPhase<Role>,
    {
        self.runtime.publish_dtm_scheduler_head(merged)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn hci_epoch_identity(
        &self,
    ) -> open_esp_radio_bluetooth_hci::HciEpochIdentity<'runtime> {
        self.hci_epoch
    }

    /// Durable general scheduler handoff for this powered epoch.
    pub const fn scheduler_wake(&self) -> &crate::BluetoothSchedulerWakeCell {
        self.runtime.scheduler_wake()
    }

    /// Durable scheduler lock/modify handoff for this powered epoch.
    pub const fn scheduler_lock_modify_events(
        &self,
    ) -> &crate::BluetoothSchedulerLockModifyEventCell {
        self.runtime.scheduler_lock_modify_events()
    }

    /// Sole bounded finished-list worker for task-side draining.
    pub fn scheduler_finished_lists(&mut self) -> &mut crate::BluetoothSchedulerFinishedListWorker {
        self.runtime.scheduler_finished_lists()
    }

    /// Durable source-127 task-readiness handoff.
    pub const fn modem_lp_timer_worker_wake(&self) -> &crate::BluetoothModemLpTimerWorkerWakeCell {
        self.runtime.modem_lp_timer_worker_wake()
    }

    /// Durable modem low-power timer expiration handoff.
    pub const fn modem_lp_timer_events(&self) -> &crate::BluetoothModemLpTimerEventCell {
        self.runtime.modem_lp_timer_events()
    }

    /// Durable ready notification for the Controller-owned post-unlink mailbox.
    ///
    /// Executor integrations register their waker before rechecking this cell.
    /// The mailbox itself closes the epoch only when it consumes the ready
    /// event, so cancellation of a waiter cannot discard notification state.
    pub const fn post_unlink_wake(&self) -> &crate::BluetoothDtmPostUnlinkWakeCell {
        self.mailbox.wake()
    }

    /// Advance one finite scheduler lock/modify transaction.
    pub fn step_scheduler_lock_modify(
        &mut self,
        event: crate::BluetoothSchedulerLockModifyEvent,
    ) -> crate::BluetoothSchedulerLockModifyWorkerStep {
        self.runtime.step_scheduler_lock_modify(event)
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
    pub(crate) fn start_dtm_scheduler<Role>(
        &mut self,
        head: crate::BluetoothDtmSchedulerHeadPublished<Role>,
    ) -> Result<
        crate::BluetoothDtmSchedulerRunning<Role>,
        BluetoothDtmSchedulerStartFailure<Role, S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => return Err(BluetoothDtmSchedulerStartFailure { error, head }),
        };
        let address = head.scheduler_item_address();
        let (item, publication) = head.into_parts();
        let event = self
            .runtime
            .publish_scheduler_run_event(publication, interrupts);
        let run = self.runtime.publish_scheduler_hardware_run_command(event);
        self.runtime.retain_running_dtm_first_item(address);
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
        self.runtime.observe_dtm_completion(running)
    }

    /// Continue an already captured finished-list drain while the DTM graph
    /// remains running.
    ///
    /// The opaque input proves that the same capture retains another list. No
    /// new hardware transfer occurs, and one retained list is returned per
    /// call together with the unchanged running or newly completed graph.
    pub fn continue_dtm_running_finished_list_drain<Role>(
        &mut self,
        pending: crate::BluetoothDtmSchedulerFinishedListDrainPending<
            crate::BluetoothDtmSchedulerRunning<Role>,
        >,
    ) -> crate::BluetoothDtmSchedulerRunningDrainStep<Role> {
        self.runtime
            .continue_dtm_running_finished_list_drain(pending)
    }

    /// Continue the captured finished-list drain after DTM completion was
    /// observed while unrelated list tokens remained.
    ///
    /// The opaque input proves affinity to that exact capture. This consumes no
    /// new hardware observation and returns every unrelated affine list token
    /// losslessly.
    pub fn continue_dtm_completed_finished_list_drain<Role>(
        &mut self,
        pending: crate::BluetoothDtmSchedulerFinishedListDrainPending<
            crate::BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
    ) -> crate::BluetoothDtmSchedulerCompletionObservedDrainStep<Role> {
        self.runtime
            .continue_dtm_completed_finished_list_drain(pending)
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
        self.runtime.observe_dtm_hardware_head_retirement(completed)
    }

    /// Remove the sole empty-head DTM item and arm its post-unlink mailbox in
    /// one serialization boundary.
    ///
    /// A primary service cannot run between the ownership-only unlink and the
    /// mailbox arm. A busy or exhausted mailbox rejects before unlinking.
    pub fn unlink_and_arm_dtm_software_list_removal<Role>(
        &mut self,
        observed: crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> BluetoothDtmPostUnlinkArmStep<Role> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(BluetoothDtmPostUnlinkArmError::Busy) => {
                    return BluetoothDtmPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted) => {
                    return BluetoothDtmPostUnlinkArmStep::MailboxIdentityExhausted(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted) => {
                    return BluetoothDtmPostUnlinkArmStep::GenerationExhausted(observed);
                }
            };
            match runtime.unlink_dtm_software_list(observed) {
                crate::scheduler::BluetoothDtmSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                    observed,
                ) => BluetoothDtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed),
                crate::scheduler::BluetoothDtmSchedulerSoftwareListUnlinkStep::Unlinked(
                    unlinked,
                ) => {
                    if mailbox.commit_arm(critical_section, key) {
                        BluetoothDtmPostUnlinkArmStep::Armed(
                            crate::BluetoothDtmPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        BluetoothDtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    /// Consume the exact primary event stored for one armed post-unlink owner.
    ///
    /// Mailbox take, finite command-status reads and any pending re-arm remain
    /// inside the same serialization boundary used by primary service.
    pub fn consume_published_dtm_software_list_removal<Role>(
        &mut self,
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    ) -> BluetoothDtmSoftwareListRemovalPublishedStep<Role> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take(critical_section, awaiting) {
                BluetoothDtmPostUnlinkTake::Waiting(awaiting) => {
                    return BluetoothDtmSoftwareListRemovalPublishedStep::Waiting(awaiting);
                }
                BluetoothDtmPostUnlinkTake::AffinityMismatch(awaiting) => {
                    return BluetoothDtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                        awaiting,
                    );
                }
                BluetoothDtmPostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                    BluetoothDtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault }
                }
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm(critical_section, key, unlinked) {
                        BluetoothDtmPostUnlinkRearm::Armed(awaiting) => {
                            BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWork {
                                awaiting,
                                epoch,
                            }
                        }
                        BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked) => {
                            BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                                unlinked,
                                epoch,
                            }
                        }
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_dtm_software_list_removal(unlinked, event) {
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        } => BluetoothDtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::Pending(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            BluetoothDtmPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::Pending { awaiting }
                            }
                            BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::PendingRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::Ready(
                            ready,
                        ) => BluetoothDtmSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
            }
        })
    }

    /// Cancel an armed post-unlink wait without discarding a stored event.
    pub fn cancel_dtm_software_list_removal<Role>(
        &mut self,
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    ) -> crate::BluetoothDtmPostUnlinkCancelStep<Role> {
        critical_section::with(|critical_section| self.mailbox.cancel(critical_section, awaiting))
    }

    /// Return TX or RX-non-success completion ownership to source-owned CPU
    /// state after the exact removal-ready transition.
    ///
    /// RX success is rejected into its separate drain/account/re-arm method.
    pub fn recycle_dtm_completed<Role>(
        &mut self,
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> crate::BluetoothDtmSchedulerRecycleStep<Role> {
        self.runtime.recycle_dtm_completed(ready)
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
        self.runtime.recycle_dtm_receiver_success(ready)
    }
}

#[cfg(target_arch = "riscv32")]
impl<S> BluetoothControllerPublishedInterruptService<'_, S> {
    /// Service, durably publish and route one primary source-124 epoch through
    /// the Controller-owned post-unlink mailbox.
    ///
    /// Capture/acknowledge, both ordinary cell publications and the capacity-one
    /// mailbox transition are serialized by one critical section. An armed
    /// mailbox stores exactly the first eligible event; a full mailbox returns
    /// the newer event without replacing the retained one.
    pub fn service_primary_interrupt(
        &self,
    ) -> Result<crate::BluetoothPrimarySerializedServiceStep, S::Error>
    where
        S: BluetoothSharedInterruptDispatchStorage,
    {
        critical_section::with(|critical_section| {
            let step = self.storage.service_primary_interrupt()?;
            let published = step.publish(
                self.runtime.scheduler_wake(),
                self.runtime.scheduler_lock_modify_events(),
            );
            Ok(self.mailbox.publish(critical_section, published))
        })
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
        let step = self.storage.service_modem_lp_timer_interrupt()?;
        Ok(step.publish(self.runtime.modem_lp_timer_worker_wake()))
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
        self.storage.service_nrt_default_interrupt()
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

    /// Recover the complete pre-publication Controller, storage and DTM runtime.
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
        crate::BluetoothDtmRuntimeResources,
        S::Error,
    ) {
        (
            self.controller,
            self.storage,
            self.dtm_resources,
            self.error,
        )
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
    /// the storage capability and unmodified DTM runtime. Success still leaves
    /// every CPU route inactive.
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
        dtm_resources: crate::BluetoothDtmRuntimeResources,
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
                post_unlink_mailbox: BluetoothDtmPostUnlinkMailbox::new(),
                runtime_control,
                scheduler_epoch: None,
                dtm_resources,
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
                    dtm_resources,
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
